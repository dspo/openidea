//! openidea-gui — the GPUI frontend host, M0 gate (go/no-go IME spike).
//!
//! One window, one hand-drawn text element, the gpui input protocol driven by
//! the macOS Pinyin IME. Everything else (workbench, IDE features, theme
//! tokens) is out of scope until M1.

mod editor;

use gpui::{App, AppContext as _, KeyBinding, WindowBounds, WindowOptions, actions, px, size};

use crate::editor::EditorPane;

actions!(spike, [Quit]);

const WINDOW_WIDTH: f32 = 960.0;
const WINDOW_HEIGHT: f32 = 320.0;
const SEED_TEXT: &str = "OpenIDEA GPUI M0 — type here";

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        // No menus in M0: the quit path is the keybinding (no platform Quit
        // menu item exists to carry it).
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            window.activate_window();
            window.set_window_title("OpenIDEA — GPUI M0 IME spike");
            cx.new(|cx| EditorPane::new(SEED_TEXT, cx))
        })
        .expect("failed to open the M0 spike window");
    });
}
