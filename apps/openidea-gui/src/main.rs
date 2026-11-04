//! openidea-gui — the GPUI frontend host, M1 (workbench shell).
//!
//! One window: file tree dock, editor tab stack, status bar, backed
//! in-process by `ide-workspace` + `ide-vfs` through `host.rs`, with backend
//! events delivered by the `bus.rs` pump. The gpui stack stays out of the
//! main workspace (root `Cargo.toml` `exclude`); the backend crates are path
//! dependencies into `../../crates/*`.

mod actions;
mod bus;
mod editor;
mod file_tree;
mod highlight;
mod host;
mod strings;
mod theme;
mod workbench;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, KeyBinding, WindowBounds, WindowOptions, px, size};
use gpui_component::{Root, Theme, ThemeMode};
use gpui_kit_assets::AllAssets;

use crate::actions::{
    Cancel, CloseTab, CursorDocEnd, CursorDocStart, CursorDown, CursorLeft, CursorLineEnd, CursorLineHome, CursorRight,
    CursorUp, CursorWordLeft, CursorWordRight, DeleteBackward, DeleteForward, DeleteWordBackward, DeleteWordForward,
    FindUsages, GoToFile, GotoDefinition, Newline, OpenFile, Quit, Redo, Rename, Save, SearchEverywhere, SelectDocEnd,
    SelectDocStart, SelectDown, SelectLeft, SelectLineEnd, SelectLineHome, SelectRight, SelectUp, SelectWordLeft,
    SelectWordRight, TogglePerfHud, Undo,
};
use crate::bus::Bus;
use crate::host::Host;
use crate::workbench::Workbench;

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
                KeyBinding::new("cmd-z", Undo, None),
                KeyBinding::new("shift-cmd-z", Redo, None),
                // M3: navigation and the command surfaces (the handlers land
                // with their features; an unhandled action is a no-op).
                KeyBinding::new("shift-cmd-o", GoToFile, None),
                KeyBinding::new("shift shift", SearchEverywhere, None),
                KeyBinding::new("cmd-b", GotoDefinition, None),
                KeyBinding::new("shift-f6", Rename, None),
                KeyBinding::new("alt-f7", FindUsages, None),
                // Escape cancels panels/popups first (whatever has focus),
                // then collapses the editor's own state.
                KeyBinding::new("escape", Cancel, None),
                KeyBinding::new("f12", TogglePerfHud, None),
                // Editor commands live in the "Editor" key context so they
                // only fire while the editor pane holds focus (the file tree
                // keeps its own enter / arrow handling).
                KeyBinding::new("enter", Newline, Some("Editor")),
                KeyBinding::new("backspace", DeleteBackward, Some("Editor")),
                KeyBinding::new("delete", DeleteForward, Some("Editor")),
                KeyBinding::new("alt-backspace", DeleteWordBackward, Some("Editor")),
                KeyBinding::new("alt-delete", DeleteWordForward, Some("Editor")),
                KeyBinding::new("left", CursorLeft, Some("Editor")),
                KeyBinding::new("right", CursorRight, Some("Editor")),
                KeyBinding::new("up", CursorUp, Some("Editor")),
                KeyBinding::new("down", CursorDown, Some("Editor")),
                KeyBinding::new("alt-left", CursorWordLeft, Some("Editor")),
                KeyBinding::new("alt-right", CursorWordRight, Some("Editor")),
                KeyBinding::new("cmd-left", CursorLineHome, Some("Editor")),
                KeyBinding::new("cmd-right", CursorLineEnd, Some("Editor")),
                KeyBinding::new("cmd-up", CursorDocStart, Some("Editor")),
                KeyBinding::new("cmd-down", CursorDocEnd, Some("Editor")),
                KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
                KeyBinding::new("shift-right", SelectRight, Some("Editor")),
                KeyBinding::new("shift-up", SelectUp, Some("Editor")),
                KeyBinding::new("shift-down", SelectDown, Some("Editor")),
                KeyBinding::new("shift-alt-left", SelectWordLeft, Some("Editor")),
                KeyBinding::new("shift-alt-right", SelectWordRight, Some("Editor")),
                KeyBinding::new("shift-cmd-left", SelectLineHome, Some("Editor")),
                KeyBinding::new("shift-cmd-right", SelectLineEnd, Some("Editor")),
                KeyBinding::new("shift-cmd-up", SelectDocStart, Some("Editor")),
                KeyBinding::new("shift-cmd-down", SelectDocEnd, Some("Editor")),
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
