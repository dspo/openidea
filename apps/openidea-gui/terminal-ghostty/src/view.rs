//! `GhosttyView` — the embeddable GPUI component: owns the shared terminal
//! state and the PTY, translates input, and renders each frame from a
//! libghostty-vt render-state snapshot.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui::{
    App, AppContext as _, BorderStyle, Bounds, Element, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, InputHandler, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, LayoutId,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle,
    Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, fill, outline, point, px, relative, rgba, size,
};
use unicode_width::UnicodeWidthStr;

use crate::input::{self, EchoDeduper};
use crate::process::{ProcessOutput, TerminalProcess};
use crate::terminal::{KeyEncoder, RenderState, Rgb, Terminal};

/// The cell advance is measured from this face and every glyph is placed on
/// that grid, so the family must be a monospace one the platform actually has:
/// a missing family resolves to a proportional fallback and columns drift apart.
const FONT_FAMILY: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 1.3;
const INITIAL_COLS: u16 = 80;
const INITIAL_ROWS: u16 = 24;

/// The colors the surface adopts. The palette lives in the app chrome, which
/// this crate cannot depend on, so the embedder supplies its own (the
/// workbench passes its theme's terminal face).
#[derive(Clone, Copy)]
pub struct Theme {
    pub fg: Rgb,
    pub bg: Rgb,
}

/// `$SHELL` when the environment carries one; otherwise the platform's own
/// interactive shell (`/bin/zsh` is a macOS fact, not a POSIX one).
fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| if cfg!(target_os = "macos") { "/bin/zsh" } else { "/bin/bash" }.to_string())
}

/// Events from the terminal world into the view's pump task.
enum TermEvent {
    /// Process output was folded into the terminal; repaint.
    Wakeup,
    /// OSC title changed; re-read it on the UI thread.
    Title,
    /// OSC 7 pwd changed; re-read it on the UI thread.
    Pwd,
    /// Child exited.
    Exited,
}

/// What an embedding surface observes. `TitleChanged` carries the OSC title
/// (`""` when the program reset it); `PwdChanged` carries the shell's
/// reported working directory (OSC 7 — the new-tab cwd); `Exited` fires once
/// per real process exit — a host-side [`GhosttyView::kill`] does not emit it.
#[derive(Clone, Debug)]
pub enum GhosttyEvent {
    /// The OSC 0/2 title claim; `None` = the program released the title
    /// (cleared it) — the embedder falls back to its launch label.
    TitleChanged(Option<String>),
    /// The OSC 7 pwd; `None` when the program reported none (or no shell
    /// integration is installed).
    PwdChanged(Option<String>),
    Exited,
}

/// The lock-protected terminal state, shared between the PTY reader thread,
/// the effect callbacks, and the render path.
pub struct TermShared {
    pub inner: Mutex<InnerState>,
    events: async_channel::Sender<TermEvent>,
}

pub struct InnerState {
    pub term: Terminal,
    pub rs: RenderState,
}

impl TermShared {
    fn new(
        cols: u16,
        rows: u16,
        theme: Theme,
        events: async_channel::Sender<TermEvent>,
    ) -> Result<Self, std::ffi::c_int> {
        let term = Terminal::new(cols, rows)?;
        term.set_default_colors(theme.fg, theme.bg);
        let rs = RenderState::new()?;
        Ok(Self { inner: Mutex::new(InnerState { term, rs }), events })
    }

    fn events_clone(&self) -> async_channel::Sender<TermEvent> {
        self.events.clone()
    }
}

/// The grid's screen geometry at the last paint (the cmd-click's cell map
/// for the OSC 8 hyperlink face).
#[derive(Clone, Copy)]
pub struct GridMetrics {
    pub cell_w: f32,
    pub line_h: f32,
    pub origin_x: f32,
    pub origin_y: f32,
}

pub struct GhosttyView {
    shared: Arc<TermShared>,
    process: Arc<dyn TerminalProcess>,
    theme: Theme,
    focus: FocusHandle,
    enc: RefCell<KeyEncoder>,
    /// Render-driven resize dedupe + the echo deduper (see `handle_key`).
    last_grid: Cell<(u16, u16)>,
    echo: RefCell<EchoDeduper>,
    /// A composition is in flight (marked text set): the IME owns the input
    /// until it commits — composing keystrokes must not reach the shell.
    composing: Cell<bool>,
    /// While focused, owns the keyboard: this runs before any workbench
    /// binding or focus traversal resolves (the integrated terminal's
    /// mechanism) — without it "tab" is eaten by focus traversal and the
    /// shell never sees \t.
    key_interceptor: Option<gpui::Subscription>,
    /// Focus on the first render only; afterwards click-to-focus, so the
    /// terminal never steals the workbench's focus on repaints.
    focused_once: Cell<bool>,
    /// The grid's screen geometry at the last paint (cell advance, row
    /// height, the grid's origin): the cmd-click's cell mapping (OSC 8).
    grid_metrics: Cell<Option<GridMetrics>>,
    /// Live OSC title, mirrored by the pump on the UI thread.
    live_title: RefCell<Option<String>>,
    /// Live working directory (OSC 7), mirrored by the pump.
    live_cwd: RefCell<Option<String>>,
    exited: Cell<bool>,
}

impl Drop for GhosttyView {
    fn drop(&mut self) {
        // Same detached sweep as `kill` — close semantics on the drop path
        // too (the external-session close relies on the drop).
        self.kill();
    }
}

impl Focusable for GhosttyView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<GhosttyEvent> for GhosttyView {}

impl GhosttyView {
    /// Spawn the default shell as a login shell (`-l`: a Finder-launched app
    /// inherits launchd's minimal PATH, and the profile scripts are how
    /// homebrew etc. reach the terminal) rooted at `cwd`. An explicit shell
    /// override is the caller's business — [`Self::spawn_with`] adopts any
    /// process.
    pub fn spawn_shell(
        cwd: &Path,
        env: &[(String, String)],
        theme: Theme,
        cx: &mut App,
    ) -> Result<Entity<Self>, String> {
        let shell = default_shell();
        // `-l` is a POSIX login convention — pwsh/nu/cmd don't take it, so
        // the flag is keyed off the shell's own name.
        let mut args: Vec<&str> = Vec::new();
        if !matches!(
            std::path::Path::new(&shell).file_name().and_then(|s| s.to_str()),
            Some("pwsh") | Some("powershell") | Some("nu") | Some("cmd") | Some("cmd.exe")
        ) {
            args.push("-l");
        }
        let process = crate::pty::Pty::spawn_shell(&shell, &args, cwd, env, INITIAL_COLS, INITIAL_ROWS)
            .map_err(|e| e.to_string())?;
        Self::spawn_with(process, theme, cx)
    }

    /// Adopt an already-running (or lazily-starting) process and return the
    /// ready view, with its output pump already running.
    pub fn spawn_with(process: Arc<dyn TerminalProcess>, theme: Theme, cx: &mut App) -> Result<Entity<Self>, String> {
        let (events, events_rx) = async_channel::unbounded::<TermEvent>();
        let shared = Arc::new(
            TermShared::new(INITIAL_COLS, INITIAL_ROWS, theme, events)
                .map_err(|e| format!("ghostty init failed: {e}"))?,
        );

        // Terminal → process replies (queries, mode reports) go straight back
        // out.
        shared.inner.lock().unwrap().term.set_write_pty({
            let process = process.clone();
            Box::new(move |bytes| process.write(bytes))
        });
        // Title/pwd changes are marked in the callbacks and read back under
        // the lock on the UI thread (no terminal calls inside effect
        // callbacks).
        shared.inner.lock().unwrap().term.set_on_title({
            let events = shared.events_clone();
            Box::new(move || {
                let _ = events.send_blocking(TermEvent::Title);
            })
        });
        shared.inner.lock().unwrap().term.set_on_pwd({
            let events = shared.events_clone();
            Box::new(move || {
                let _ = events.send_blocking(TermEvent::Pwd);
            })
        });

        // Output side: fold bytes into the emulator (thread-safe), then wake
        // the pump; a real exit marks the shared state.
        let out = ProcessOutput::new(
            {
                let shared = shared.clone();
                let events = shared.events_clone();
                Arc::new(move |bytes: &[u8]| {
                    shared.inner.lock().unwrap().term.write(bytes);
                    let _ = events.send_blocking(TermEvent::Wakeup);
                })
            },
            {
                let events = shared.events_clone();
                Arc::new(move |code: Option<u32>| {
                    let _ = code;
                    let _ = events.send_blocking(TermEvent::Exited);
                })
            },
        );
        process.clone().attach(out);

        let view = cx.new(|cx| {
            let view = GhosttyView {
                shared: shared.clone(),
                process,
                theme,
                focus: cx.focus_handle(),
                enc: RefCell::new(KeyEncoder::new().map_err(|e| format!("key encoder: {e}")).expect("key encoder")),
                last_grid: Cell::new((0, 0)),
                echo: RefCell::new(EchoDeduper::default()),
                composing: Cell::new(false),
                key_interceptor: None,
                focused_once: Cell::new(false),
                grid_metrics: Cell::new(None),
                live_title: RefCell::new(None),
                live_cwd: RefCell::new(None),
                exited: Cell::new(false),
            };
            view.start_pump(events_rx, cx);
            view
        });
        // Keyboard ownership: two channels. The interceptor consumes the
        // keys the shell owns — control chords, navigation, escape, and tab
        // (focus traversal would eat it) — before any workbench binding or
        // focus traversal resolves. Bare printable characters pass through
        // to the key-down listener: that is where `is_held` (a platform key
        // repeat is not an IME echo) and the composition gate live, and no
        // binding competes for them.
        view.update(cx, |view, cx| {
            let weak = cx.entity().downgrade();
            let interceptor = cx.intercept_keystrokes(move |ev, window, cx| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let focused = view.read_with(cx, |v, _| v.focus.clone()).is_focused(window);
                if !focused {
                    return;
                }
                let ks = &ev.keystroke;
                // The workbench's primary-modifier chords stay with the
                // window (quit, tab switching, tool-window slots); the
                // view's key-down listener still pastes the platform
                // clipboard chord.
                if ks.modifiers.platform {
                    return;
                }
                // Linux: the keymap resolves IDEA's Meta+digit slots to
                // Ctrl+digit — those stay with the window; ctrl+letters go
                // to the shell (interrupt, flow control), IDEA's own
                // terminal behavior.
                if cfg!(target_os = "linux")
                    && ks.modifiers.control
                    && !ks.key.is_empty()
                    && ks.key.chars().all(|c| c.is_ascii_digit())
                {
                    return;
                }
                // Bare printable characters are not shell-owned (see the
                // channel doc above).
                if ks.key_char.as_ref().is_some_and(|t| !t.is_empty()) && !ks.modifiers.control && !ks.modifiers.alt {
                    return;
                }
                let keystroke = ev.keystroke.clone();
                if view.update(cx, |v, cx| v.handle_key(&keystroke, cx)) {
                    cx.stop_propagation();
                }
            });
            view.key_interceptor = Some(interceptor);
        });
        Ok(view)
    }

    fn start_pump(&self, rx: async_channel::Receiver<TermEvent>, cx: &mut gpui::Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update(cx, |view, cx| view.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_event(&self, ev: TermEvent, cx: &mut gpui::Context<Self>) {
        match ev {
            TermEvent::Wakeup => cx.notify(),
            TermEvent::Title => {
                // An empty OSC 0 title is a reset, not a claim: fold it to
                // `None` so embedders fall back to their launch label.
                let title = self.shared.inner.lock().unwrap().term.title().filter(|t| !t.is_empty());
                *self.live_title.borrow_mut() = title.clone();
                cx.emit(GhosttyEvent::TitleChanged(title));
                cx.notify();
            }
            TermEvent::Pwd => {
                let pwd = self.shared.inner.lock().unwrap().term.pwd();
                *self.live_cwd.borrow_mut() = pwd.clone();
                cx.emit(GhosttyEvent::PwdChanged(pwd));
                cx.notify();
            }
            TermEvent::Exited => {
                self.exited.set(true);
                cx.emit(GhosttyEvent::Exited);
                cx.notify();
            }
        }
    }

    /// The terminal's OSC 0/2 title, when the program has set one. `None`
    /// until then — embedders show their own fallback label.
    pub fn live_title(&self) -> Option<String> {
        self.live_title.borrow().clone()
    }

    /// The shell's reported working directory (OSC 7), when it reported one.
    pub fn live_cwd(&self) -> Option<String> {
        self.live_cwd.borrow().clone()
    }

    /// Whether the process has exited (the last frame stays on screen).
    pub fn exited(&self) -> bool {
        self.exited.get()
    }

    /// Terminate the process and stop the pump. Idempotent; the tab calls
    /// this when it closes, because dropping the view alone cannot: both
    /// watcher threads hold the pty open, so the shell would outlive the tab.
    pub fn kill(&self) {
        self.exited.set(true);
        // Detached: the sweep blocks for a sysinfo snapshot plus the
        // SIGTERM grace window — never on the UI thread.
        let process = Arc::clone(&self.process);
        std::thread::spawn(move || process.kill());
    }

    /// Put the viewport back on the active area. Called on every keystroke:
    /// typing while scrolled back must show what the shell answers.
    fn scroll_to_bottom(&self) {
        self.shared.inner.lock().unwrap().term.scroll_bottom();
    }

    /// Render-driven resize: called from the element's prepaint with the grid
    /// size derived from the current bounds. Same-frame, no notify.
    fn prepare_grid(&self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        let last = self.last_grid.get();
        if last == (cols, rows) {
            return;
        }
        self.last_grid.set((cols, rows));
        {
            let inner = self.shared.inner.lock().unwrap();
            let _ = inner.term.resize(cols, rows, cell_w_px, cell_h_px);
        }
        self.process.resize(cols, rows);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let ks = &ev.keystroke;
        // Platform-modified keys: the clipboard chord pastes, the rest stays
        // with the host (quit, tab switching, …).
        if ks.modifiers.platform {
            if ks.key == "v"
                && let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
            {
                self.paste_text(&text);
            }
            return;
        }
        // The printable path (see the interceptor's channel doc): a key
        // repeat is real input (`is_held`), an identical echo within the
        // window is the pass-through IME's duplicate. While a composition is
        // in flight the IME owns the input — composing keystrokes must not
        // leak into the shell before the commit lands.
        let Some(text) = ks.key_char.as_ref().filter(|t| !t.is_empty()) else {
            return;
        };
        if self.composing.get() {
            return;
        }
        let bytes = {
            let inner = self.shared.inner.lock().unwrap();
            let mut enc = self.enc.borrow_mut();
            enc.sync_from_terminal(&inner.term);
            enc.encode_press(input::map_key(&ks.key), input::ghostty_mods(&ks.modifiers), Some(text))
        };
        if bytes.is_empty() {
            return;
        }
        if !ev.is_held && !self.echo.borrow().allows(&bytes, input_ms()) {
            return;
        }
        self.echo.borrow_mut().note(&bytes, input_ms());
        self.scroll_bottom_and_write(&bytes);
    }

    /// Paste text through libghostty (bracketed-paste mode handled inside).
    /// A rejected paste means the text could inject commands outside
    /// bracketed-paste mode; libghostty's contract is to ask the user and
    /// retry with `allow_unsafe`. There is no confirm surface for the
    /// terminal yet, so the paste is dropped rather than forced through
    /// (see the crate's known gaps).
    fn paste_text(&mut self, text: &str) {
        let pasted = {
            let inner = self.shared.inner.lock().unwrap();
            inner.term.paste(text)
        };
        if pasted.is_ok() {
            self.scroll_to_bottom();
        }
    }

    /// Write IME-committed text to the shell. Composition output is normal
    /// input, not a bracketed paste: the platform already decided it is text.
    /// The echo deduper guards the pass-through IME case: a commit that
    /// byte-matches the keystroke that just landed is the echo, not input.
    fn commit_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.composing.set(false);
        let bytes = text.as_bytes();
        if !self.echo.borrow().allows(bytes, input_ms()) {
            return;
        }
        self.echo.borrow_mut().note(bytes, input_ms());
        self.process.write(bytes);
        self.scroll_to_bottom();
    }

    /// Translate one shell-owned keystroke to the PTY (the interceptor's
    /// channel: control chords, navigation keys — nothing an IME echoes, so
    /// no dedup here). Returns whether the key was consumed (the interceptor
    /// then stops propagation so no workbench binding or focus traversal can
    /// also act on it).
    fn handle_key(&mut self, ks: &gpui::Keystroke, _cx: &mut gpui::Context<Self>) -> bool {
        let mods = &ks.modifiers;
        if mods.platform || mods.function {
            return false;
        }

        let key = input::map_key(&ks.key);
        let gmods = input::ghostty_mods(mods);
        // Text only for plain-ish keys; the encoder derives control sequences
        // from the logical key + mods itself.
        let utf8 = ks.key_char.clone().filter(|t| !t.is_empty());
        let bytes = {
            let inner = self.shared.inner.lock().unwrap();
            let mut enc = self.enc.borrow_mut();
            enc.sync_from_terminal(&inner.term);
            enc.encode_press(key, gmods, utf8.as_deref())
        };
        if bytes.is_empty() {
            return false;
        }
        // A keystroke returns the viewport to the active area, the way a
        // native terminal does: typing while scrolled back must show the
        // shell's answer.
        self.scroll_bottom_and_write(&bytes);
        true
    }

    fn scroll_bottom_and_write(&self, bytes: &[u8]) {
        {
            let inner = self.shared.inner.lock().unwrap();
            inner.term.scroll_bottom();
        }
        self.process.write(bytes);
    }

    fn on_wheel(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let lh = FONT_SIZE * LINE_HEIGHT;
        let dy = ev.delta.pixel_delta(px(lh)).y;
        let lines = (dy / px(lh)).round();
        if lines != 0. {
            let inner = self.shared.inner.lock().unwrap();
            inner.term.scroll_delta(lines as isize);
        }
        cx.notify();
    }
}

/// Milliseconds on a process-lifetime monotonic clock (the echo deduper's
/// injected time source; tests inject their own numbers directly).
fn input_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

impl Render for GhosttyView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if !self.focused_once.get() {
            self.focused_once.set(true);
            window.focus(&self.focus, cx);
        }
        gpui::div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    // cmd-click on an OSC 8 hyperlink opens it (the mouse
                    // tracking programs keep their clicks; the modifier
                    // bypasses them -- IDEA/ghostty's own convention).
                    if event.modifiers.platform
                        && let Some(metrics) = this.grid_metrics.get()
                    {
                        let col = (f32::from(event.position.x) - metrics.origin_x) / metrics.cell_w;
                        let row = (f32::from(event.position.y) - metrics.origin_y) / metrics.line_h;
                        if col >= 0.0
                            && row >= 0.0
                            && let Some(uri) =
                                this.shared.inner.lock().unwrap().term.hyperlink_uri_at(col as u16, row as u32)
                        {
                            eprintln!("[terminal] cmd-click {uri}");
                            open_uri(&uri);
                        }
                    }
                }),
            )
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll_wheel(cx.listener(Self::on_wheel))
            .bg(gpui_color(self.theme.bg))
            .child(GhosttyElement { view: cx.entity() })
    }
}

/// Opens a URI with the platform's handler (`open` / `xdg-open`); a plain
/// detached spawn (the child lives on its own; failures just log).
fn open_uri(uri: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    let result = std::process::Command::new(program).arg(uri).spawn();
    if let Err(e) = result {
        eprintln!("[terminal] open {uri}: {e}");
    }
}

/// Convenience: spawn at `cwd`, falling back to home when it does not exist
/// (the shell cannot start in a deleted directory).
pub fn spawn_ghostty_view(cwd: &Path, theme: Theme, cx: &mut App) -> Result<Entity<GhosttyView>, String> {
    let cwd = if cwd.is_dir() {
        cwd.to_path_buf()
    } else {
        std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| Path::new(".").to_path_buf())
    };
    GhosttyView::spawn_shell(&cwd, &[], theme, cx)
}

// ---------------------------------------------------------------------------
// GhosttyElement — paint-only element: snapshot → shape in prepaint, emit in paint
// ---------------------------------------------------------------------------

struct GhosttyElement {
    view: Entity<GhosttyView>,
}

/// gpui `InputHandler` for IME composition. The platform has nowhere to send
/// committed text without one, so CJK input (and every composed sequence)
/// would never reach the pty; the editor element registers the same handler
/// for the same reason. Committed text goes to the shell as plain input;
/// in-flight marked text is not painted (no composition overlay yet).
struct GhosttyInputHandler {
    view: Entity<GhosttyView>,
    /// Cursor bounds from the latest paint — where the candidate window goes.
    cursor_bounds: Option<Bounds<Pixels>>,
}

impl InputHandler for GhosttyInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // Signal "input enabled, caret at 0..0" so the platform engages IME.
        Some(UTF16Selection { range: 0..0, reversed: false })
    }

    fn marked_text_range(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, _cx| view.commit_text(text));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        // Composition engaged (a clear empties it again): until the commit,
        // raw printable keys are the composition's own keystrokes, not
        // shell input — and a later commit may reproduce the last raw
        // character, so the echo deduper stands down.
        self.view.update(cx, |view, _| {
            view.composing.set(!new_text.is_empty());
            view.echo.borrow_mut().disarm();
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, _| view.composing.set(false));
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

struct CursorPlan {
    bounds: Bounds<Pixels>,
    style: i32,
    color: gpui::Rgba,
    /// The character under a block cursor, repainted inverted on top.
    cell_text: Option<String>,
}

struct PrepaintState {
    bounds: Bounds<Pixels>,
    line_h: Pixels,
    runs: Vec<(Point<Pixels>, ShapedLine)>,
    bg_quads: Vec<(Bounds<Pixels>, gpui::Rgba)>,
    cursor: Option<CursorPlan>,
    cursor_text: Option<(Point<Pixels>, ShapedLine)>,
    /// The frame's background (the terminal's, not the theme's: a program may
    /// override it).
    bg: gpui::Rgba,
}

fn gpui_color(c: Rgb) -> gpui::Rgba {
    rgba(((c.r as u32) << 24) | ((c.g as u32) << 16) | ((c.b as u32) << 8) | 0xff)
}

fn faint(c: gpui::Rgba) -> gpui::Rgba {
    c.alpha(0.55)
}

fn font(style: crate::terminal::Cell) -> gpui::Font {
    gpui::Font {
        family: FONT_FAMILY.into(),
        features: Default::default(),
        weight: if style.bold { gpui::FontWeight::BOLD } else { gpui::FontWeight::NORMAL },
        style: if style.italic { gpui::FontStyle::Italic } else { gpui::FontStyle::Normal },
        fallbacks: None,
    }
}

impl IntoElement for GhosttyElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for GhosttyElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("ghostty-grid".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        let layout_id = window.request_layout(style, std::iter::empty(), cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let font_size = px(FONT_SIZE);
        let line_h = px(FONT_SIZE * LINE_HEIGHT);

        // Measure the cell advance from one monospace glyph.
        let probe = TextRun {
            len: 1,
            font: gpui::Font { family: FONT_FAMILY.into(), ..Default::default() },
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped_m =
            window.text_system().shape_line(SharedString::from("m"), font_size, std::slice::from_ref(&probe), None);
        let cell_w = shaped_m.width().max(px(1.));

        let cols = ((bounds.size.width / cell_w).floor() as u16).max(1);
        let rows = ((bounds.size.height / line_h).floor() as u16).max(1);

        self.view.update(cx, |v, _| {
            v.prepare_grid(cols, rows, (f32::from(cell_w) as u32).max(1), (f32::from(line_h) as u32).max(1));
            v.grid_metrics.set(Some(GridMetrics {
                cell_w: cell_w.into(),
                line_h: line_h.into(),
                origin_x: bounds.origin.x.into(),
                origin_y: bounds.origin.y.into(),
            }));
        });

        // Snapshot the grid under the lock, then release before shaping.
        let frame = self.view.update(cx, |v, _| {
            let mut inner = v.shared.inner.lock().unwrap();
            let InnerState { term, rs } = &mut *inner;
            rs.snapshot(term).ok()
        });

        let mut runs: Vec<(Point<Pixels>, ShapedLine)> = Vec::new();
        let mut bg_quads: Vec<(Bounds<Pixels>, gpui::Rgba)> = Vec::new();
        let mut cursor: Option<CursorPlan> = None;
        let mut cursor_text: Option<(Point<Pixels>, ShapedLine)> = None;
        // The surface color: the embedder's background unless the program
        // overrode it (OSC 11), in which case the frame carries the truth.
        let theme_bg = gpui_color(self.view.read(cx).theme.bg);

        let Some(frame) = frame else {
            return PrepaintState { bounds, line_h, runs, bg_quads, cursor, cursor_text, bg: theme_bg };
        };

        let default_bg = gpui_color(frame.colors.background);
        let default_fg = gpui_color(frame.colors.foreground);

        for (y, row) in frame.cells.iter().enumerate() {
            let row_y = bounds.origin.y + (y as f32) * line_h;

            for (x, cell) in row.iter().enumerate() {
                let pos = point(bounds.origin.x + (x as f32) * cell_w, row_y);

                // Cell backgrounds that differ from the default get a quad;
                // inverse cells render the foreground as their background.
                let raw_fg = gpui_color(cell.fg);
                let cell_bg = if cell.inverse { raw_fg } else { gpui_color(cell.bg) };
                if cell_bg != default_bg {
                    bg_quads.push((Bounds::new(pos, size(cell_w, line_h)), cell_bg));
                }
            }

            // Group same-style runs (split at wide chars, whose advance is
            // doubled instead of forced to the narrow cell width; and at
            // inversion boundaries — grouping by raw colors would swallow
            // inverse cells into a plain run and paint them invisible).
            let mut x = 0usize;
            while x < row.len() {
                let cell = &row[x];
                if cell.text.is_empty() || cell.invisible {
                    x += 1;
                    continue;
                }
                let wide = UnicodeWidthStr::width(cell.text.as_str()) > 1;

                let (mut fg, mut bg) = (gpui_color(cell.fg), gpui_color(cell.bg));
                if cell.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.faint {
                    fg = faint(fg);
                }
                let sig = (fg, cell.bold, cell.italic, cell.underline, cell.strikethrough, wide, cell.invisible);

                let mut text = String::new();
                let mut end = x;
                while end < row.len() {
                    let c = &row[end];
                    if c.text.is_empty() || c.invisible {
                        break;
                    }
                    let c_wide = UnicodeWidthStr::width(c.text.as_str()) > 1;
                    let mut c_fg = if c.inverse { gpui_color(c.bg) } else { gpui_color(c.fg) };
                    if c.faint {
                        c_fg = faint(c_fg);
                    }
                    let c_sig = (c_fg, c.bold, c.italic, c.underline, c.strikethrough, c_wide, c.invisible);
                    if c_sig != sig {
                        break;
                    }
                    text.push_str(&c.text);
                    end += 1;
                }

                let cell_font = font(cell.clone());
                let text_run =
                    TextRun {
                        len: text.len(),
                        font: cell_font,
                        color: fg.into(),
                        background_color: None,
                        underline: (cell.underline != crate::sys::GHOSTTY_SGR_UNDERLINE_NONE)
                            .then_some(UnderlineStyle { thickness: px(1.), color: Some(fg.into()), wavy: false }),
                        strikethrough: cell
                            .strikethrough
                            .then_some(StrikethroughStyle { thickness: px(1.), color: Some(fg.into()) }),
                    };
                // gpui's force_width is a per-glyph monospace advance
                // (apply_force_width_to_layout snaps the i-th reference
                // glyph to i * force_width), not the run's span: passing
                // the span would scatter the run's second half past its
                // end. The per-cell width snaps every glyph half a cell
                // clear of the last onto its grid slot, pinning
                // proportional fallback fonts to the grid.
                let advance = if wide { cell_w * 2. } else { cell_w };
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.as_str()),
                    px(FONT_SIZE),
                    std::slice::from_ref(&text_run),
                    Some(advance),
                );
                runs.push((point(bounds.origin.x + (x as f32) * cell_w, row_y), shaped));
                x = end;
            }
        }

        // Cursor: a block inverts its cell; other styles are quads only.
        if let Some(cur) = &frame.cursor
            && cur.visible
            && (cur.x as usize) < cols as usize
            && (cur.y as usize) < rows as usize
        {
            let pos = point(bounds.origin.x + (cur.x as f32) * cell_w, bounds.origin.y + (cur.y as f32) * line_h);
            let cell_bounds = Bounds::new(pos, size(if cur.wide_tail { cell_w * 2. } else { cell_w }, line_h));
            let color = frame.colors.cursor.map(gpui_color).unwrap_or(default_fg);

            match cur.style {
                crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
                    let cell = frame.cells.get(cur.y as usize).and_then(|r| r.get(cur.x as usize));
                    let inverted = cell.map(|c| if c.invisible { None } else { Some(c.text.clone()) });
                    cursor = Some(CursorPlan {
                        bounds: cell_bounds,
                        style: cur.style,
                        color,
                        cell_text: inverted.flatten().filter(|t| !t.is_empty()),
                    });
                    if let (Some(text), Some(c)) = (&cursor.as_ref().unwrap().cell_text, cell) {
                        let text_run = TextRun {
                            len: text.len(),
                            font: font(c.clone()),
                            color: default_bg.into(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(
                            SharedString::from(text.as_str()),
                            px(FONT_SIZE),
                            std::slice::from_ref(&text_run),
                            Some(cell_w),
                        );
                        cursor_text = Some((pos, shaped));
                    }
                }
                crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                    cursor = Some(CursorPlan {
                        bounds: Bounds::new(point(pos.x, pos.y + line_h - px(2.)), size(cell_w, px(2.))),
                        style: cur.style,
                        color,
                        cell_text: None,
                    });
                }
                crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
                    cursor = Some(CursorPlan {
                        bounds: Bounds::new(pos, size(px(2.), line_h)),
                        style: cur.style,
                        color,
                        cell_text: None,
                    });
                }
                _ => {
                    // Hollow block (focus lost / IME).
                    cursor = Some(CursorPlan { bounds: cell_bounds, style: cur.style, color, cell_text: None });
                }
            }
        }

        PrepaintState { bounds, line_h, runs, bg_quads, cursor, cursor_text, bg: default_bg }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Default background (the frame's, so a program's OSC 11 override and
        // an unstyled cell agree on what "default" means).
        window.paint_quad(fill(prepaint.bounds, prepaint.bg));

        for (bounds, color) in &prepaint.bg_quads {
            window.paint_quad(fill(*bounds, *color));
        }

        for (pos, shaped) in &prepaint.runs {
            let _ = shaped.paint(*pos, prepaint.line_h, TextAlign::Left, None, window, cx);
        }

        if let Some(cur) = &prepaint.cursor {
            if cur.style == crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK {
                window.paint_quad(fill(cur.bounds, cur.color));
            } else if cur.style == crate::sys::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW {
                window.paint_quad(outline(cur.bounds, cur.color, BorderStyle::default()));
            } else {
                window.paint_quad(fill(cur.bounds, cur.color));
            }
        }
        if let Some((pos, shaped)) = &prepaint.cursor_text {
            let _ = shaped.paint(*pos, prepaint.line_h, TextAlign::Left, None, window, cx);
        }

        // Register the IME handler for this frame so composition events land
        // here, with the candidate window anchored at the cursor.
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            GhosttyInputHandler {
                view: self.view.clone(),
                cursor_bounds: prepaint.cursor.as_ref().map(|cur| cur.bounds),
            },
            cx,
        );
    }
}
