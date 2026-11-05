//! openidea-gui — the GPUI frontend host, M1 (workbench shell).
//!
//! One window: file tree dock, editor tab stack, status bar, backed
//! in-process by `ide-workspace` + `ide-vfs` through `host.rs`, with backend
//! events delivered by the `bus.rs` pump. The gpui stack stays out of the
//! main workspace (root `Cargo.toml` `exclude`); the backend crates are path
//! dependencies into `../../crates/*`.

mod actions;
mod bus;
mod diff_view;
mod editor;
mod file_tree;
mod fuzzy;
mod highlight;
mod host;
mod run_panel;
mod semantic;
mod strings;
mod theme;
mod workbench;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext as _, KeyBinding, WindowBounds, WindowOptions, px, size};
use gpui_component::{Root, Theme, ThemeMode};
use gpui_kit_assets::AllAssets;

use crate::actions::{
    Cancel, CloseTab, CodeActions, Commit, CursorDocEnd, CursorDocStart, CursorDown, CursorLeft, CursorLineEnd,
    CursorLineHome, CursorRight, CursorUp, CursorWordLeft, CursorWordRight, DeleteBackward, DeleteForward,
    DeleteWordBackward, DeleteWordForward, FindUsages, FormatDocument, GoToFile, GotoDefinition, Newline, OpenFile,
    OutdentLine, Paste, Quit, Redo, Rename, Run, Save, SelectDocEnd, SelectDocStart, SelectDown, SelectLeft,
    SelectLineEnd, SelectLineHome, SelectRight, SelectUp, SelectWordLeft, SelectWordRight, ShowDiff, StopRun,
    TabIndent, TogglePerfHud, Undo,
};
use crate::actions::{SelectNext, SelectPrev};
use crate::bus::Bus;
use crate::host::Host;
use crate::workbench::Workbench;

const WINDOW_WIDTH: f32 = 1200.0;
const WINDOW_HEIGHT: f32 = 800.0;

/// The open project's host as a global, so window-level actions (Quit) and
/// feature panels (the run dock's Stop) reach the backend without threading
/// it through every entity.
pub(crate) struct ProjectHost(pub(crate) Arc<Host>);

impl gpui::Global for ProjectHost {}

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
    // Backend crates (ide-lsp, ide-vfs) log through tracing; surface it on
    // stderr so scripted sessions carry the servers' side of the story.
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    // The app-owned tokio runtime (the language servers' IO and request
    // substrate) wraps the gpui run loop: `block_on` parks this thread on the
    // runtime while the window runs, which keeps the runtime — and the
    // process/signal driver every spawned server child reaps through —
    // continuously driven.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("openidea-gui-runtime")
        .build()
        .expect("build the app tokio runtime");
    runtime.block_on(async {
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
                    // (Search Everywhere has no key binding: double ⇧ is a
                    // modifiers-event gesture in the workbench, since
                    // modifier-only presses never become keystrokes.)
                    KeyBinding::new("shift-cmd-o", GoToFile, None),
                    KeyBinding::new("cmd-b", GotoDefinition, None),
                    KeyBinding::new("shift-f6", Rename, None),
                    KeyBinding::new("alt-f7", FindUsages, None),
                    // M4: run and git surfaces (handlers on the workbench).
                    KeyBinding::new("ctrl-r", Run, None),
                    KeyBinding::new("ctrl-cmd-r", StopRun, None),
                    KeyBinding::new("ctrl-d", ShowDiff, None),
                    KeyBinding::new("cmd-k", Commit, None),
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
                    // M5: language-driven indentation (tab / shift-tab).
                    KeyBinding::new("tab", TabIndent, Some("Editor")),
                    KeyBinding::new("shift-tab", OutdentLine, Some("Editor")),
                    // M5: quick fixes and formatting.
                    KeyBinding::new("alt-enter", CodeActions, Some("Editor")),
                    KeyBinding::new("cmd-alt-l", FormatDocument, Some("Editor")),
                    // Paste rides the editor context (the gpui-component inputs
                    // keep their own clipboard handling).
                    KeyBinding::new("cmd-v", Paste, Some("Editor")),
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
                    // Palette list navigation (⇧⌘O / double-⇧). Registered after
                    // gpui-component init, so for the same keystroke these beat
                    // the query input's own cursor moves inside the palette.
                    KeyBinding::new("up", SelectPrev, Some("Palette")),
                    KeyBinding::new("down", SelectNext, Some("Palette")),
                ]);
                cx.on_action(|_: &Quit, cx| {
                    // Best-effort graceful server shutdown before the loop exits;
                    // peers are killed regardless when the process ends.
                    if let Some(host) = cx.try_global::<ProjectHost>() {
                        host.0.shutdown_lsp();
                    }
                    cx.quit();
                });
                let (bus, rx) = Bus::channel();
                // The manager must be constructed inside the runtime's context so
                // it captures this handle for its background tasks.
                let host = {
                    let _runtime = tokio::runtime::Handle::current().enter();
                    initial_project().and_then(|root| {
                        match Host::open(&root, bus.sink(), tokio::runtime::Handle::current()) {
                            Ok(host) => {
                                let host = Arc::new(host);
                                cx.set_global(ProjectHost(host.clone()));
                                Some(host)
                            }
                            Err(e) => {
                                eprintln!("[workbench] opening {}: {}", root.display(), e.message);
                                None
                            }
                        }
                    })
                };
                let workbench = cx.new(|cx| Workbench::new(host.clone(), bus.clone(), cx));
                // Startup document for scripted verification sessions: the env
                // names a file inside the project (opened like a tree double-click).
                if let (Some(host), Some(open)) = (&host, std::env::var_os("OPENIDEA_GUI_OPEN")) {
                    let path = {
                        let open = PathBuf::from(open);
                        if open.is_absolute() { open } else { host.root.join(open) }
                    };
                    workbench.update(cx, |wb, cx| wb.open_path(&path, cx));
                }
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
    });
}
