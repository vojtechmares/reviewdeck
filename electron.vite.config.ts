import { resolve } from 'node:path'
import { defineConfig, externalizeDepsPlugin } from 'electron-vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin()],
    resolve: {
      alias: { '@shared': resolve('src/shared') },
    },
    build: {
      rollupOptions: { input: { index: resolve('src/main/index.ts') } },
    },
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    resolve: {
      alias: { '@shared': resolve('src/shared') },
    },
    build: {
      rollupOptions: { input: { index: resolve('src/preload/index.ts') } },
    },
  },
  renderer: {
    root: 'src/renderer',
    resolve: {
      alias: {
        '@': resolve('src/renderer/src'),
        '@shared': resolve('src/shared'),
      },
    },
    plugins: [react(), tailwindcss()],
    build: {
      // The renderer only ever runs in the Chromium that Electron ships, so tell
      // the CSS minifier that. Left to its defaults it downlevels for old Safari
      // and collapses `backdrop-filter` to the `-webkit-` alias alone - which
      // current Chromium no longer honours, silently killing every glass blur.
      target: 'chrome150',
      cssTarget: 'chrome150',
      rollupOptions: { input: { index: resolve('src/renderer/index.html') } },
    },
  },
})
