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
mod find_panel;
mod fuzzy;
mod highlight;
mod host;
mod keymap;
// The preset tables: the active one binds; the other three stay as data for
// the Settings keymap page (M14), re-synced by the generator.
#[allow(dead_code)]
mod keymap_data;
mod markdown;
mod menu;
mod navigation;
mod perf;
mod reconcile;
mod reconcile_data;
mod run_panel;
mod semantic;
mod settings;
mod strings;
mod templates;
mod theme;
mod toolwindow;
mod workbench;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext as _, WindowBounds, WindowOptions, px, size};
use gpui_component::{Root, Theme, ThemeMode};
use gpui_kit_assets::AllAssets;

use crate::actions::Quit;
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

/// The project to open at startup (M7 order): `OPENIDEA_GUI_PROJECT` wins
/// (the scripted-session override stays stateless); then the most recent
/// project from the settings store (when it still exists); then the nearest
/// enclosing git repository of the cwd as the stateless fallback (so
/// `cargo run` inside a repo worktree still just works). `None` starts at
/// the welcome view.
fn initial_project(settings: &settings::Settings) -> Option<PathBuf> {
    if let Some(from_env) = std::env::var_os("OPENIDEA_GUI_PROJECT") {
        return Some(PathBuf::from(from_env));
    }
    if let Some(recent) = settings.recent_projects.first() {
        return Some(recent.clone());
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

/// The panic face of "crashes are never silent" (M8): append a crash.log
/// entry, mark the run unclean (the next launch toasts it), echo to stderr,
/// then chain into the default hook so the OS-side report behavior is
/// unchanged. Everything here is best-effort — this runs while unwinding.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_owned());
        let location = info.location().map_or_else(|| "?".to_owned(), |l| format!("{}:{}", l.file(), l.line()));
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_owned();
        let utc = settings::format_utc(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        );
        let entry = settings::crash_entry(&utc, &thread, &payload, &location);
        // The first frames ride the entry (M9's IME crash stayed a mystery
        // because the log carried payload + location only): force_capture
        // works without RUST_BACKTRACE, and the cost is panic-path only.
        let backtrace_text = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> =
            backtrace_text.lines().skip_while(|line| !line.contains("openidea_gui")).take(14).collect();
        let mut blob = entry.clone();
        if frames.is_empty() {
            blob.push_str("  (no openidea_gui frames — run with RUST_BACKTRACE=1 for the full trace)\n");
        } else {
            for frame in frames {
                blob.push_str("  ");
                blob.push_str(frame.trim_start());
                blob.push('\n');
            }
        }
        eprintln!("[crash] {entry}crash log: <data dir>/openidea-gui/crash.log");
        settings::append_crash_log(&blob);
        settings::mark_unclean_exit(Some(&payload));
        default_hook(info);
    }));
}

fn main() {
    install_panic_hook();
    // Crash drill (M8): `OPENIDEA_GUI_TEST_PANIC=1` exercises the hook — the
    // entry lands in crash.log, the run is marked unclean, and the next
    // launch toasts it. The soak harness and the one-week guide use it.
    if std::env::var_os("OPENIDEA_GUI_TEST_PANIC").is_some() {
        panic!("crash-hook drill (OPENIDEA_GUI_TEST_PANIC)");
    }
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
                // The persisted settings (recents, layout, font size) load once;
                // the workbench owns the value and saves on change. The editor
                // font size feeds the theme before any view renders.
                let mut app_settings = settings::load();
                theme::set_font_size(app_settings.font_size);
                // The keymap (M9): the defaults are the IDEA preset rows in
                // keymap::BINDINGS, registered data-driven after
                // gpui_component::init (later registrations win keystroke
                // ties). A persisted user override replaces a row's keys.
                keymap::bind(cx, &app_settings.keymap_overrides);
                // The macOS menu bar (M10): IDEA's full structure, generated
                // from the action registry the Find Action palette reads.
                menu::install(cx);
                let (bus, rx) = Bus::channel();
                // The Project trust gate (M10): a startup root outside the
                // trusted list waits in the trust prompt instead of opening
                // (the scripted-session env names an operator-chosen project
                // and keeps its auto-trust, which the harnesses rely on).
                let startup_from_env = std::env::var_os("OPENIDEA_GUI_PROJECT").is_some();
                let startup_root = initial_project(&app_settings);
                let untrusted_startup = startup_root.as_ref().and_then(|root| {
                    let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
                    let trusted = startup_from_env
                        || app_settings
                            .trusted_projects
                            .iter()
                            .any(|seen| Path::new(seen).canonicalize().map(|p| p == canonical).unwrap_or(false));
                    (!trusted).then_some(canonical)
                });
                eprintln!(
                    "[trust] startup root={:?} from_env={trusted_env} untrusted={untrusted_startup:?})",
                    startup_root,
                    trusted_env = startup_from_env,
                );
                // The manager must be constructed inside the runtime's context so
                // it captures this handle for its background tasks.
                let host = {
                    let _runtime = tokio::runtime::Handle::current().enter();
                    startup_root.clone().filter(|_| untrusted_startup.is_none()).and_then(|root| {
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
                // The startup-opened project lands in the recents too (M8):
                // only the folder-picker flow pushed a recent, so a project
                // reached through the env or the cwd fallback never appeared
                // in ⌘O's list — and the bare relaunch (which picks from the
                // recents, then restores the session) had nothing to pick.
                if let Some(root) = &host {
                    settings::push_recent(&mut app_settings.recent_projects, &root.root);
                    settings::save(&app_settings);
                }
                let workbench =
                    cx.new(|cx| Workbench::new(host.clone(), untrusted_startup, bus.clone(), app_settings, cx));
                // The Quit action persists the session (tabs + carets) on the
                // graceful path (M8); registered here so it can reach the
                // workbench entity.
                cx.on_action({
                    let workbench = workbench.downgrade();
                    move |_: &Quit, cx| {
                        if let Some(host) = cx.try_global::<ProjectHost>() {
                            host.0.shutdown_lsp();
                        }
                        workbench.update(cx, |workbench, cx| workbench.save_session(cx)).ok();
                        settings::mark_clean_exit();
                        cx.quit();
                    }
                });
                // An unclean previous run surfaces as the first toast (M8): the
                // crash itself could not show one, so the next launch does. The
                // marker flips to "clean (running)" only after the read, so this
                // session's own crash still records.
                if let Some(exit) = settings::read_last_exit()
                    && !exit.clean
                {
                    let reason = exit.reason.as_deref().unwrap_or("unknown cause");
                    workbench.update(cx, |workbench, _| {
                        workbench.toast(
                            ide_core::event::NotifyLevel::Warning,
                            format!("{}: {reason} (crash log in the app data dir)", strings::LAST_RUN_UNCLEAN),
                        );
                    });
                }
                settings::mark_clean_exit();
                // The recorded session (tabs + carets) restores before any
                // scripted open, so OPENIDEA_GUI_OPEN still wins the active
                // tab for scripted sessions (M8).
                workbench.update(cx, |workbench, cx| workbench.restore_session(cx));
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
