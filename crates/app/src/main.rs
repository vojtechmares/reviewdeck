//! Reviewdeck: every code review waiting on you, across every Git host you work with.
//!
//! Application setup, the window, menus, key bindings and lifecycle - the port of
//! src/main/index.ts. Everything that is not drawing lives in `reviewdeck_core`.

mod platform;
mod state;
mod ui;

use gpui::{
    App, Application, Context, KeyBinding, Menu, MenuItem, TitlebarOptions, Window,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowOptions, actions, div, point,
    prelude::*, px, rgba, size,
};

/// The bundle identifier, which macOS groups windows and notifications by.
const APP_ID: &str = "cz.mares.reviewdeck";

actions!(reviewdeck, [Quit]);

/// Stands in for the app view until it is ported: the milky film over the window's
/// blur, and the name.
struct Placeholder;

impl Render for Placeholder {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        // index.css `--background` / `--foreground`, approximated in sRGB.
        let (film, ink) = if dark {
            (rgba(0x17181be0), rgba(0xeeeff1ff))
        } else {
            (rgba(0xf5f6f8db), rgba(0x1f2024ff))
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(film)
            .text_color(ink)
            .font_family(".SystemUIFont")
            .text_size(ui::theme::rpx(20.))
            .child("Reviewdeck")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "Reviewdeck".into(),
            items: vec![MenuItem::action("Quit Reviewdeck", Quit)],
        }]);
        // Until the menu bar item exists there is no way back to a closed window, so
        // closing it quits.
        cx.on_window_closed(|cx| cx.quit()).detach();

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
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                cx.observe_window_appearance(window, |_, window, _| window.refresh())
                    .detach();
                Placeholder
            })
        });
        if let Err(error) = opened {
            eprintln!("Reviewdeck could not open its window: {error:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
