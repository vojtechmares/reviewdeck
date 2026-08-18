import { app, BrowserWindow, Menu, nativeTheme, shell, Tray, nativeImage } from 'electron'
import { join } from 'node:path'
import { deck } from './deck.ts'
import { registerImageScheme, serveImages } from './images.ts'
import { flushDrafts, registerIpc } from './ipc.ts'
import { getSettings } from './store.ts'
import { visibleReviews } from '@shared/types.ts'

const isDev = !app.isPackaged

let mainWindow: BrowserWindow | null = null
let tray: Tray | null = null

function createWindow(): BrowserWindow {
  const window = new BrowserWindow({
    width: 1280,
    height: 860,
    minWidth: 940,
    minHeight: 600,
    show: false,
    // Milky glass: macOS vibrancy supplies the blur, the renderer paints a
    // heavy translucent film on top. `transparent: true` is deliberately absent -
    // it disables vibrancy and the window shadow, and reads as see-through rather
    // than frosted.
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: 18, y: 20 },
    vibrancy: 'under-window',
    visualEffectState: 'active',
    backgroundColor: '#00000000',
    webPreferences: {
      preload: join(import.meta.dirname, '../preload/index.mjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
      webSecurity: true,
    },
  })

  window.once('ready-to-show', () => window.show())

  // Anything that wants a new window opens in the user's browser instead.
  window.webContents.setWindowOpenHandler(({ url }) => {
    if (url.startsWith('https://') || url.startsWith('http://')) void shell.openExternal(url)
    return { action: 'deny' }
  })

  // Block in-page navigation away from the app shell.
  window.webContents.on('will-navigate', (event, url) => {
    const devServer = process.env['ELECTRON_RENDERER_URL']
    if (devServer && url.startsWith(devServer)) return
    event.preventDefault()
    if (url.startsWith('https://') || url.startsWith('http://')) void shell.openExternal(url)
  })

  const devServer = process.env['ELECTRON_RENDERER_URL']
  if (isDev && devServer) {
    void window.loadURL(devServer)
  } else {
    void window.loadFile(join(import.meta.dirname, '../renderer/index.html'))
  }

  return window
}

function show(): void {
  if (!mainWindow || mainWindow.isDestroyed()) {
    mainWindow = createWindow()
    return
  }
  if (mainWindow.isMinimized()) mainWindow.restore()
  mainWindow.show()
  mainWindow.focus()
}

/** A 16pt template icon drawn as a monochrome SVG, so it adapts to the menu bar theme. */
function trayIcon(): Electron.NativeImage {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32">
    <g fill="black">
      <rect x="4" y="7" width="24" height="4" rx="2"/>
      <rect x="4" y="14" width="18" height="4" rx="2"/>
      <rect x="4" y="21" width="12" height="4" rx="2"/>
    </g>
  </svg>`
  const image = nativeImage.createFromDataURL(
    `data:image/svg+xml;base64,${Buffer.from(svg).toString('base64')}`,
  )
  const resized = image.resize({ width: 16, height: 16 })
  resized.setTemplateImage(true)
  return resized
}

function createTray(): void {
  tray = new Tray(trayIcon())
  tray.setToolTip('Reviewdeck')

  const render = (): void => {
    const { items, statuses } = deck.state()
    // The same reviews the window lists, not every review the deck holds: a menu
    // bar that says three while the app shows one is the menu bar being wrong.
    const settings = getSettings()
    const waiting = visibleReviews(items, settings)
    const failing = statuses.filter((status) => !status.ok).length
    // Empty rather than a space when the count is off, so the icon sits where it
    // would if nothing were ever drawn beside it. The context menu still counts.
    tray?.setTitle(settings.showMenuBarCount && waiting.length ? ` ${waiting.length}` : '')
    tray?.setContextMenu(
      Menu.buildFromTemplate([
        {
          label: waiting.length
            ? `${waiting.length} review${waiting.length === 1 ? '' : 's'} waiting`
            : 'Nothing waiting on you',
          enabled: false,
        },
        ...(failing ? [{ label: `${failing} account(s) failing to sync`, enabled: false }] : []),
        { type: 'separator' as const },
        { label: 'Open Reviewdeck', click: show },
        { label: 'Refresh now', click: () => void deck.refresh() },
        { type: 'separator' as const },
        { label: 'Quit', role: 'quit' as const },
      ]),
    )
  }

  render()
  deck.on('changed', render)
  tray.on('click', show)
}

function buildMenu(): void {
  const template: Electron.MenuItemConstructorOptions[] = [
    {
      label: app.name,
      submenu: [
        { role: 'about' },
        { type: 'separator' },
        {
          label: 'Settings…',
          accelerator: 'Cmd+,',
          click: () => mainWindow?.webContents.send('deck:focus-item', '__settings__'),
        },
        { type: 'separator' },
        { role: 'services' },
        { type: 'separator' },
        { role: 'hide' },
        { role: 'hideOthers' },
        { role: 'unhide' },
        { type: 'separator' },
        { role: 'quit' },
      ],
    },
    { label: 'Edit', submenu: [{ role: 'undo' }, { role: 'redo' }, { type: 'separator' }, { role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { role: 'selectAll' }] },
    {
      label: 'View',
      submenu: [
        {
          label: 'Refresh',
          accelerator: 'Cmd+R',
          click: () => void deck.refresh(),
        },
        { type: 'separator' },
        { role: 'resetZoom' },
        { role: 'zoomIn' },
        { role: 'zoomOut' },
        { type: 'separator' },
        { role: 'togglefullscreen' },
        ...(isDev ? [{ role: 'toggleDevTools' as const }] : []),
      ],
    },
    { label: 'Window', submenu: [{ role: 'minimize' }, { role: 'zoom' }, { role: 'front' }] },
  ]
  Menu.setApplicationMenu(Menu.buildFromTemplate(template))
}

// One instance only: a second launch just focuses the window we already have.
if (!app.requestSingleInstanceLock()) {
  app.quit()
} else {
  app.on('second-instance', show)

  // Has to happen before the app is ready, unlike everything else below.
  registerImageScheme()

  void app.whenReady().then(() => {
    nativeTheme.themeSource = getSettings().theme

    serveImages()
    registerIpc()
    buildMenu()
    mainWindow = createWindow()
    createTray()
    deck.start()

    deck.on('focus-item', (itemId: string) => {
      show()
      mainWindow?.webContents.send('deck:focus-item', itemId)
    })

    app.on('activate', () => {
      if (BrowserWindow.getAllWindows().length === 0) mainWindow = createWindow()
      else show()
    })
  })

  // Menu-bar app: closing the window leaves the tray running.
  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') app.quit()
  })

  app.on('before-quit', () => {
    deck.stop()
    // Anything the debounce was still holding, before the process goes away.
    flushDrafts()
  })
}
