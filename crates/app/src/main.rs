//! Reviewdeck: every code review waiting on you, across every Git host you work with.
//!
//! Application setup, the window, menus, key bindings and lifecycle - the port of
//! src/main/index.ts. Everything that is not drawing lives in `reviewdeck_core`, and
//! the state the views read lives in [`state::AppState`].

// objc 0.2's `msg_send!` expands to `cfg(feature = "cargo-clippy")` checks.
#![allow(unexpected_cfgs)]

mod platform;
mod state;
mod ui;

use std::sync::Arc;

use futures::StreamExt;
use futures::future::BoxFuture;
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url, anyhow, http::HeaderValue};
use gpui::{
    App, AppContext, AsyncApp, Global, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowOptions,
    actions, point, px, size,
};
use reviewdeck_core::http::{Http, Method, RequestOptions};
use reviewdeck_core::model::ThemeMode;
use reviewdeck_core::store::{Vault, data_dir};
use reviewdeck_core::tray_icon::{TRAY_ICON_POINTS, tray_icon_png};

use crate::platform::notify::Notifier;
use crate::platform::tray::Tray;
use crate::platform::{PlatformEvent, appearance, instance};
use crate::state::{AppDeps, AppEvent, AppState, GlobalState, Notify};
use crate::ui::app_view::AppView;
use crate::ui::theme::{Theme, set_zoom};

/// The bundle identifier, which macOS groups windows and notifications by.
const APP_ID: &str = "cz.mares.reviewdeck";

actions!(
    reviewdeck,
    [
        About,
        OpenSettings,
        Refresh,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        ActualSize,
        ZoomIn,
        ZoomOut,
        ToggleFullScreen,
        Minimize,
        Zoom,
        BringAllToFront,
    ]
);

// The Edit menu's items are AppKit's own, routed to whichever text field has focus.
actions!(edit, [Undo, Redo, Cut, Copy, Paste, SelectAll]);

/// Holds the single-instance lock for the life of the app. Dropping it releases it.
struct InstanceLock(#[allow(dead_code)] Option<std::fs::File>);

impl Global for InstanceLock {}

/// The zoom level in Electron's steps: each step is 1.2 times the last, and a half
/// step is a level of 0.5 (`zoomIn` adds 0.5 to the level).
struct ZoomLevel(f32);

impl Global for ZoomLevel {}

/// The fraction of the default size a zoom level renders at.
fn zoom_factor(level: f32) -> f32 {
    1.2f32.powf(level)
}

/// Opens the main window. Closing it keeps the app running in the menu bar.
fn open_main_window(cx: &mut App) -> gpui::Result<WindowHandle<AppView>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1280.), px(860.)), cx)),
        window_min_size: Some(size(px(940.), px(600.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Reviewdeck".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(18.), px(20.))),
        }),
        // Milky glass: the window's blur, with the film painted on top.
        window_background: WindowBackgroundAppearance::Blurred,
        app_id: Some(APP_ID.into()),
        ..WindowOptions::default()
    };
    cx.open_window(options, |window, cx| cx.new(|cx| AppView::new(window, cx)))
}

/// Brings the window forward, reopening it when it has been closed. What the menu bar
/// "Open Reviewdeck" item and a notification click both do.
fn show_window(cx: &mut App) {
    match cx.windows().first().copied() {
        Some(handle) => {
            cx.update_window(handle, |_, window, _| window.activate_window())
                .ok();
        }
        None => {
            if let Err(error) = open_main_window(cx) {
                eprintln!("Reviewdeck could not open its window: {error:#}");
            }
        }
    }
    cx.activate(true);
}

/// Sets the theme the views read from, from the setting and the system appearance.
fn apply_theme(cx: &mut App) {
    let mode = settings_theme(cx);
    Theme::apply(mode, cx);
}

fn settings_theme(cx: &App) -> ThemeMode {
    cx.try_global::<GlobalState>()
        .map(|global| global.0.read(cx).settings().theme)
        .unwrap_or(ThemeMode::System)
}

/// Routes what AppKit reported - a menu item, a notification click - into the app.
fn handle_platform_event(event: PlatformEvent, cx: &mut App) {
    match event {
        PlatformEvent::OpenWindow => show_window(cx),
        PlatformEvent::Refresh => refresh_deck(cx),
        PlatformEvent::Quit => cx.quit(),
        PlatformEvent::NotificationClicked { target } => {
            show_window(cx);
            if let Some(item_id) = target {
                let state = cx.global::<GlobalState>().0.clone();
                state.update(cx, |_, cx| cx.emit(AppEvent::FocusItem(item_id)));
            }
        }
    }
}

fn refresh_deck(cx: &mut App) {
    let state = cx.global::<GlobalState>().0.clone();
    state.update(cx, |state, cx| state.refresh(cx).detach());
}

fn change_zoom(cx: &mut App, change: impl FnOnce(f32) -> f32) {
    let current = cx.global::<ZoomLevel>().0;
    let level = change(current).clamp(-4.0, 8.0);
    cx.set_global(ZoomLevel(level));
    let factor = zoom_factor(level);
    for handle in cx.windows() {
        cx.update_window(handle, |_, window, _| set_zoom(window, factor))
            .ok();
    }
}

/// The About panel: AppKit's standard one, which reads the name and version from the
/// bundle.
#[allow(unexpected_cfgs)]
fn show_about_panel() {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};
    // SAFETY: NSApplication.sharedApplication exists once the app is running, and
    // orderFrontStandardAboutPanel: takes nil. Menu actions run on the main thread.
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, orderFrontStandardAboutPanel: nil];
    }
}

/// Serves gpui's `img("https://...")` through core's HTTP client. Plain requests only:
/// no credential ever travels this way. Authenticated images go through
/// `AppState::authenticated_image`.
struct ImageHttp {
    http: Http,
    user_agent: HeaderValue,
}

impl HttpClient for ImageHttp {
    fn type_name(&self) -> &'static str {
        "ReviewdeckHttp"
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, gpui::http_client::Result<Response<AsyncBody>>> {
        let http = self.http.clone();
        let url = req.uri().to_string();
        Box::pin(async move {
            let response = http
                .send(&url, RequestOptions::new(Method::Get))
                .await
                .map_err(|error| anyhow!(error.to_string()))?;
            let mut builder = Response::builder().status(response.status);
            for (name, value) in &response.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            Ok(builder.body(AsyncBody::from(response.body))?)
        })
    }
}

/// Posts notifications through the platform notifier, the way the TypeScript's
/// `Notification` did.
struct PlatformNotify(Notifier);

impl Notify for PlatformNotify {
    fn show(&self, notification: &platform::notify::Notification) {
        self.0.show(notification);
    }
}

fn main() {
    let data_dir = match data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    // Before anything writes the vault. Another process holding the lock means it
    // is already running, which is the case the TypeScript's requestSingleInstanceLock
    // handled by quitting; LaunchServices has focused that process already.
    let lock = match instance::acquire_single_instance_lock(&data_dir) {
        Ok(Some(file)) => Some(file),
        Ok(None) => return,
        Err(error) => {
            eprintln!("Reviewdeck could not lock its data directory: {error}");
            None
        }
    };

    let vault = match Vault::open() {
        Ok(vault) => Arc::new(vault),
        Err(error) => {
            eprintln!("Reviewdeck could not open its data: {error}");
            return;
        }
    };
    let http = Http::new();

    let app = gpui::Application::new()
        .with_assets(ui::icons::Assets)
        .with_http_client(Arc::new(ImageHttp {
            http: http.clone(),
            user_agent: HeaderValue::from_static("Reviewdeck"),
        }));

    // Dock click with no visible window reopens it.
    app.on_reopen(|cx: &mut App| {
        if cx.windows().is_empty()
            && let Err(error) = open_main_window(cx)
        {
            eprintln!("Reviewdeck could not open its window: {error:#}");
        }
        cx.activate(true);
    });

    app.run(move |cx: &mut App| {
        cx.set_global(InstanceLock(lock));

        let settings = vault.settings();
        // Like nativeTheme.themeSource at launch: before the first window draws.
        appearance::set_app_appearance(settings.theme);

        let (events, mut platform_events) = futures::channel::mpsc::unbounded();

        // Installed here: gpui calls this from applicationDidFinishLaunching:, which is
        // early enough for the click that launched the app.
        let notifier = Notifier::new(events.clone());
        if let Some(notifier) = &notifier {
            notifier.request_authorization();
        }
        let tray = match Tray::new(
            events.clone(),
            &tray_icon_png(TRAY_ICON_POINTS),
            &tray_icon_png(TRAY_ICON_POINTS * 2),
        ) {
            Ok(tray) => Some(tray),
            Err(error) => {
                eprintln!("Reviewdeck could not set up its menu bar item: {error}");
                None
            }
        };

        let deps = AppDeps {
            http: http.clone(),
            vault: vault.clone(),
            remote: None,
            demo: reviewdeck_core::demo::demo_enabled(),
            clock: None,
            notify: notifier.map(|n| Box::new(PlatformNotify(n)) as Box<dyn Notify>),
            tray,
        };

        let state = cx.new(|cx| AppState::new(deps, cx));
        cx.set_global(GlobalState(state.clone()));
        cx.set_global(ZoomLevel(0.0));
        // Before the window and the tray, so both open on the deck the last sync left.
        state.update(cx, |state, cx| state.hydrate(cx));

        apply_theme(cx);
        ui::theme::load_fonts(cx);
        ui::components::bind_keys(cx);
        ui::app_view::bind_keys(cx);
        install_menus(cx);

        if let Err(error) = open_main_window(cx) {
            eprintln!("Reviewdeck could not open its window: {error:#}");
            cx.quit();
            return;
        }
        state.update(cx, |state, cx| state.start(cx));

        // Platform events arrive on the channel from AppKit callbacks, which never call
        // into gpui themselves.
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(event) = platform_events.next().await {
                if cx.update(|cx| handle_platform_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Write the drafts down and stop the timers on quit.
        let quitting = state.clone();
        cx.on_app_quit(move |cx| {
            quitting.update(cx, |state, _| state.shutdown());
            async {}
        })
        .detach();

        cx.activate(true);
    });
}

/// The menu bar, item for item what buildMenu() in index.ts builds.
fn install_menus(cx: &mut App) {
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
    cx.on_action(|_: &Hide, cx: &mut App| cx.hide());
    cx.on_action(|_: &HideOthers, cx: &mut App| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx: &mut App| cx.unhide_other_apps());
    cx.on_action(|_: &About, _cx: &mut App| show_about_panel());
    cx.on_action(|_: &OpenSettings, cx: &mut App| {
        show_window(cx);
        let state = cx.global::<GlobalState>().0.clone();
        state.update(cx, |_, cx| cx.emit(AppEvent::OpenSettings));
    });
    cx.on_action(|_: &Refresh, cx: &mut App| refresh_deck(cx));
    cx.on_action(|_: &ActualSize, cx: &mut App| change_zoom(cx, |_| 0.0));
    cx.on_action(|_: &ZoomIn, cx: &mut App| change_zoom(cx, |level| level + 0.5));
    cx.on_action(|_: &ZoomOut, cx: &mut App| change_zoom(cx, |level| level - 0.5));
    cx.on_action(|_: &ToggleFullScreen, cx: &mut App| {
        with_active_window(cx, |window| window.toggle_fullscreen());
    });
    cx.on_action(|_: &Minimize, cx: &mut App| {
        with_active_window(cx, |window| window.minimize_window());
    });
    cx.on_action(|_: &Zoom, cx: &mut App| with_active_window(cx, |window| window.zoom_window()));
    cx.on_action(|_: &BringAllToFront, cx: &mut App| cx.activate(true));

    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-r", Refresh, None),
        KeyBinding::new("cmd-0", ActualSize, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-a", SelectAll, None),
    ]);

    cx.set_menus(vec![
        Menu {
            name: "Reviewdeck".into(),
            items: vec![
                MenuItem::action("About Reviewdeck", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Reviewdeck", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Reviewdeck", Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Refresh", Refresh),
                MenuItem::separator(),
                MenuItem::action("Actual Size", ActualSize),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::separator(),
                MenuItem::action("Toggle Full Screen", ToggleFullScreen),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::action("Bring All to Front", BringAllToFront),
            ],
        },
    ]);
}

fn with_active_window(cx: &mut App, change: impl FnOnce(&Window)) {
    if let Some(handle) = cx.active_window() {
        cx.update_window(handle, |_, window, _| change(window)).ok();
    }
}
