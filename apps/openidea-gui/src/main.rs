//! openidea-gui — the GPUI frontend host, M1 (workbench shell).
//!
//! One window: file tree dock, editor tab stack, status bar, backed
//! in-process by `ide-workspace` + `ide-vfs` through `host.rs`, with backend
//! events delivered by the `bus.rs` pump. The gpui stack stays out of the
//! main workspace (root `Cargo.toml` `exclude`); the backend crates are path
//! dependencies into `../../crates/*`.

mod bus;
mod editor;
mod file_tree;
mod host;
mod strings;
mod theme;
mod workbench;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, KeyBinding, WindowBounds, WindowOptions, px, size};
use gpui_component::{Root, Theme, ThemeMode};
use gpui_kit_assets::AllAssets;

use crate::bus::Bus;
use crate::host::Host;
use crate::workbench::{CloseTab, OpenFile, Quit, Save, Workbench};

const WINDOW_WIDTH: f32 = 1200.0;
const WINDOW_HEIGHT: f32 = 800.0;

/// The project to open at startup: `OPENIDEA_GUI_PROJECT` wins; otherwise the
/// nearest enclosing git repository of the cwd (so `cargo run` inside a repo
/// worktree just works), else the shell starts empty.
fn initial_project() -> Option<PathBuf> {
    if let Some(from_env) = std::env::var_os("OPENIDEA_GUI_PROJECT") {
        return Some(PathBuf::from(from_env));
    }
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn main() {
    gpui_platform::application()
        // The SVG icon set `IconName` resolves to (gpui-component's icons).
        .with_assets(AllAssets::new(""))
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            // The main app's chrome is dark; gpui-component defaults to light.
            Theme::change(ThemeMode::Dark, None, cx);
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-o", OpenFile, None),
                KeyBinding::new("cmd-s", Save, None),
                KeyBinding::new("cmd-w", CloseTab, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            let (bus, rx) = Bus::channel();
            let host = initial_project().and_then(|root| match Host::open(&root, bus.sink()) {
                Ok(host) => Some(Arc::new(host)),
                Err(e) => {
                    eprintln!("[workbench] opening {}: {}", root.display(), e.message);
                    None
                }
            });
            let workbench = cx.new(|cx| Workbench::new(host, bus.clone(), cx));
            bus.spawn_pump(rx, workbench.downgrade(), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                window.activate_window();
                window.set_window_title(strings::APP_TITLE);
                // Root provides gpui-component's window layers (tooltips,
                // notifications) above the workbench view.
                let view: gpui::AnyView = workbench.into();
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open the workbench window");
        });
}
