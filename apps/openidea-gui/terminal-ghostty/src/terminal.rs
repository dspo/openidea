//! Safe wrapper around libghostty-vt.
//!
//! The C handles are not thread-safe; callers must serialize access to
//! [`Terminal`] themselves (a `Mutex` is enough — the libghostty-vt C API is
//! designed for single-threaded embedders that lock, like Ghostty's own
//! renderer). [`Terminal`] is `Send + Sync` on that contract.

use std::ffi::{CString, c_char, c_int, c_void};

pub use crate::sys;
pub use sys::key_consts;

/// An RGB color as libghostty-vt reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<sys::GhosttyColorRgb> for Rgb {
    fn from(c: sys::GhosttyColorRgb) -> Self {
        Self { r: c.r, g: c.g, b: c.b }
    }
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

type WritePtyFn = Box<dyn Fn(&[u8]) + Send>;
type EffectFn = Box<dyn Fn() + Send>;

#[repr(C)]
struct Callbacks {
    write_pty: Option<WritePtyFn>,
    on_title: Option<EffectFn>,
    on_pwd: Option<EffectFn>,
}

pub struct Terminal {
    term: sys::GhosttyTerminal,
    // Box keeps the address stable for the lifetime of the terminal; the C
    // side only ever sees `&*self.callbacks` as userdata.
    callbacks: Box<Callbacks>,
}

// SAFETY: all calls into the C terminal happen under the caller's own lock
// (see crate docs); no interior mutable state beyond what that lock orders.
unsafe impl Send for Terminal {}
unsafe impl Sync for Terminal {}

unsafe extern "C" fn trampoline_write_pty(_term: sys::GhosttyTerminal, ud: *mut c_void, data: *const u8, len: usize) {
    unsafe {
        let cbs = &*(ud as *const Callbacks);
        if let Some(f) = cbs.write_pty.as_ref() {
            f(std::slice::from_raw_parts(data, len));
        }
    }
}

unsafe extern "C" fn trampoline_title(_term: sys::GhosttyTerminal, ud: *mut c_void) {
    unsafe {
        let cbs = &*(ud as *const Callbacks);
        if let Some(f) = cbs.on_title.as_ref() {
            f();
        }
    }
}

extern "C" fn trampoline_pwd(_term: sys::GhosttyTerminal, ud: *mut c_void) {
    unsafe {
        let cbs = &*(ud as *const Callbacks);
        if let Some(f) = cbs.on_pwd.as_ref() {
            f();
        }
    }
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, c_int> {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        let rc = unsafe { sys::ghostty_terminal_new(std::ptr::null(), &mut term, cols, rows) };
        if rc != sys::GHOSTTY_SUCCESS {
            return Err(rc);
        }
        let this = Self { term, callbacks: Box::new(Callbacks { write_pty: None, on_title: None, on_pwd: None }) };
        let ud = &*this.callbacks as *const Callbacks as *const c_void;
        unsafe {
            sys::ghostty_terminal_set(this.term, sys::GHOSTTY_TERMINAL_OPT_USERDATA, ud);
        }
        Ok(this)
    }

    /// Route VT query/mode-report replies to the PTY. Must be called before
    /// any [`Self::write`] so queries are answered.
    pub fn set_write_pty(&mut self, f: WritePtyFn) {
        self.callbacks.write_pty = Some(f);
        unsafe {
            sys::ghostty_terminal_set(
                self.term,
                sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                trampoline_write_pty as sys::GhosttyTerminalWritePtyFn as *const c_void,
            );
        }
    }

    /// Notify on OSC 0/2 title changes; read [`Self::title`] in the handler.
    pub fn set_on_title(&mut self, f: EffectFn) {
        self.callbacks.on_title = Some(f);
        unsafe {
            sys::ghostty_terminal_set(
                self.term,
                sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                trampoline_title as sys::GhosttyTerminalEffectFn as *const c_void,
            );
        }
    }

    /// Notify on OSC 7 / OSC 9 pwd changes; read [`Self::pwd`] in the handler.
    pub fn set_on_pwd(&mut self, f: EffectFn) {
        self.callbacks.on_pwd = Some(f);
        unsafe {
            sys::ghostty_terminal_set(
                self.term,
                sys::GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
                trampoline_pwd as sys::GhosttyTerminalEffectFn as *const c_void,
            );
        }
    }

    /// Feed raw PTY output into the emulation core. Fires callbacks (pty
    /// replies, title changes) synchronously on the calling thread.
    pub fn write(&self, bytes: &[u8]) {
        unsafe {
            sys::ghostty_terminal_vt_write(self.term, bytes.as_ptr(), bytes.len());
        }
    }

    pub fn resize(&self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) -> Result<(), c_int> {
        let rc = unsafe { sys::ghostty_terminal_resize(self.term, cols, rows, cell_w_px, cell_h_px) };
        (rc == sys::GHOSTTY_SUCCESS).then_some(()).ok_or(rc)
    }

    pub fn scroll_delta(&self, lines: isize) {
        unsafe {
            sys::ghostty_terminal_scroll_viewport(self.term, sys::GhosttyTerminalScrollViewport::delta(lines));
        }
    }

    pub fn scroll_bottom(&self) {
        unsafe {
            sys::ghostty_terminal_scroll_viewport(self.term, sys::GhosttyTerminalScrollViewport::bottom());
        }
    }

    pub fn title(&self) -> Option<String> {
        unsafe { self.get_string(sys::GHOSTTY_TERMINAL_DATA_TITLE) }
    }

    pub fn pwd(&self) -> Option<String> {
        unsafe { self.get_string(sys::GHOSTTY_TERMINAL_DATA_PWD) }
    }

    /// Set the default foreground/background the render state resolves
    /// against (palette overrides from programs still win per-index).
    pub fn set_default_colors(&self, fg: Rgb, bg: Rgb) {
        let f = sys::GhosttyColorRgb { r: fg.r, g: fg.g, b: fg.b };
        let b = sys::GhosttyColorRgb { r: bg.r, g: bg.g, b: bg.b };
        unsafe {
            let _ = sys::ghostty_terminal_set(
                self.term,
                sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                &f as *const _ as *const c_void,
            );
            let _ = sys::ghostty_terminal_set(
                self.term,
                sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                &b as *const _ as *const c_void,
            );
        }
    }

    pub fn kitty_keyboard_flags(&self) -> u8 {
        let mut flags: u8 = 0;
        unsafe {
            let _ = sys::ghostty_terminal_get(
                self.term,
                sys::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                &mut flags as *mut _ as *mut c_void,
            );
        }
        flags
    }

    /// Paste text: handles bracketed-paste mode and unsafe-paste rejection
    /// internally. Returns whether the bytes were written to the pty.
    pub fn paste(&self, text: &str) -> Result<bool, c_int> {
        struct PasteCtx {
            ptr: *const u8,
            len: usize,
        }

        unsafe extern "C" fn read_mime(ud: *mut c_void, _mime: sys::GhosttyString, writer: sys::GhosttyWriter) -> bool {
            unsafe {
                let ctx = &*(ud as *const PasteCtx);
                (writer.write.unwrap())(writer.userdata, ctx.ptr, ctx.len)
            }
        }

        let mime = CString::new("text/plain").unwrap();
        let mime = sys::GhosttyString { ptr: mime.as_ptr() as *const u8, len: mime.as_bytes_with_nul().len() - 1 };
        let mimes = [mime];
        let ctx = PasteCtx { ptr: text.as_ptr(), len: text.len() };
        let paste = sys::GhosttyPaste {
            size: std::mem::size_of::<sys::GhosttyPaste>(),
            location: sys::GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
            source: sys::GHOSTTY_PASTE_SOURCE_TEXT,
            mimes: mimes.as_ptr(),
            mimes_len: mimes.len(),
            reader: sys::GhosttyMimeReader { read: Some(read_mime), userdata: &ctx as *const PasteCtx as *mut c_void },
            allow_unsafe: false,
        };
        let mut written = false;
        let rc = unsafe { sys::ghostty_terminal_paste(self.term, &paste, &mut written) };
        (rc == sys::GHOSTTY_SUCCESS).then_some(written).ok_or(rc)
    }

    unsafe fn get_string(&self, data: c_int) -> Option<String> {
        let mut s = sys::GhosttyString { ptr: std::ptr::null(), len: 0 };
        let rc = unsafe { sys::ghostty_terminal_get(self.term, data, &mut s as *mut _ as *mut c_void) };
        if rc != sys::GHOSTTY_SUCCESS || s.ptr.is_null() {
            return None;
        }
        Some(unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(s.ptr, s.len)).into_owned() })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { sys::ghostty_terminal_free(self.term) }
    }
}

// ---------------------------------------------------------------------------
// Render state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dirty {
    Clean,
    Partial,
    Full,
}

pub struct CursorPos {
    pub x: u16,
    pub y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub style: c_int,
}

#[derive(Clone)]
pub struct Cell {
    /// UTF-8 grapheme cluster; empty means a blank cell.
    pub text: String,
    /// Resolved colors (palette lookups already applied).
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub invisible: bool,
    /// One of the `GHOSTTY_SGR_UNDERLINE_*` values.
    pub underline: c_int,
}

pub struct FrameColors {
    pub background: Rgb,
    pub foreground: Rgb,
    /// `Some` only when the program set an explicit cursor color.
    pub cursor: Option<Rgb>,
}

pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub colors: FrameColors,
    pub cursor: Option<CursorPos>,
    /// Exactly `rows` rows of `cols` cells.
    pub cells: Vec<Vec<Cell>>,
}

pub struct RenderState {
    rs: sys::GhosttyRenderState,
    row_iter: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
}

// Same contract as [`Terminal`]: serialize access externally.
unsafe impl Send for RenderState {}
unsafe impl Sync for RenderState {}

impl RenderState {
    pub fn new() -> Result<Self, c_int> {
        unsafe {
            let mut rs: sys::GhosttyRenderState = std::ptr::null_mut();
            check(sys::ghostty_render_state_new(std::ptr::null(), &mut rs))?;
            let mut row_iter: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
            check(sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut row_iter))?;
            let mut cells: sys::GhosttyRenderStateRowCells = std::ptr::null_mut();
            check(sys::ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells))?;
            Ok(Self { rs, row_iter, cells })
        }
    }

    /// Snapshot the whole viewport into owned data so the terminal lock can be
    /// released before shaping/painting.
    pub fn snapshot(&mut self, term: &Terminal) -> Result<Frame, c_int> {
        unsafe {
            check(sys::ghostty_render_state_update(self.rs, term.term))?;

            let mut cols: u16 = 0;
            let mut rows: u16 = 0;
            check(sys::ghostty_render_state_get(
                self.rs,
                sys::GHOSTTY_RENDER_STATE_DATA_COLS,
                &mut cols as *mut _ as *mut c_void,
            ))?;
            check(sys::ghostty_render_state_get(
                self.rs,
                sys::GHOSTTY_RENDER_STATE_DATA_ROWS,
                &mut rows as *mut _ as *mut c_void,
            ))?;

            let mut colors = sys::GhosttyRenderStateColors::default();
            check(sys::ghostty_render_state_get(
                self.rs,
                sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut colors as *mut _ as *mut c_void,
            ))?;

            let mut cursor = sys::GhosttyRenderStateCursor::default();
            let has_cursor = sys::ghostty_render_state_get(
                self.rs,
                sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut cursor as *mut _ as *mut c_void,
            ) == sys::GHOSTTY_SUCCESS;

            let mut out = Frame {
                cols,
                rows,
                colors: FrameColors {
                    background: colors.background.into(),
                    foreground: colors.foreground.into(),
                    cursor: colors.cursor_has_value.then(|| colors.cursor.into()),
                },
                cursor: None,
                cells: Vec::with_capacity(rows as usize),
            };

            check(sys::ghostty_render_state_get(
                self.rs,
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.row_iter as *mut _ as *mut c_void,
            ))?;
            while sys::ghostty_render_state_row_iterator_next(self.row_iter) {
                let mut row_cells: Vec<Cell> = Vec::with_capacity(cols as usize);
                check(sys::ghostty_render_state_row_get(
                    self.row_iter,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    &mut self.cells as *mut _ as *mut c_void,
                ))?;
                let default_fg = colors.foreground;
                let default_bg = colors.background;
                while sys::ghostty_render_state_row_cells_next(self.cells) {
                    row_cells.push(self.read_cell(default_fg, default_bg));
                }
                out.cells.push(row_cells);
            }

            if has_cursor && cursor.viewport_has_value {
                out.cursor = Some(CursorPos {
                    x: cursor.viewport_x,
                    y: cursor.viewport_y,
                    wide_tail: cursor.wide_tail,
                    visible: cursor.visible,
                    blinking: cursor.blinking,
                    style: cursor.visual_style,
                });
            }

            check(sys::ghostty_render_state_clean(self.rs))?;
            Ok(out)
        }
    }

    /// Cells without explicit styling report `GHOSTTY_INVALID_VALUE` for
    /// their resolved colors (with zeroed RGB); fall back to the frame's
    /// default colors there.
    unsafe fn read_cell(&self, default_fg: sys::GhosttyColorRgb, default_bg: sys::GhosttyColorRgb) -> Cell {
        unsafe {
            let mut buf = [0u8; 64];
            let mut gb = sys::GhosttyBuffer::sized(&mut buf);
            let mut rc = sys::ghostty_render_state_row_cells_get(
                self.cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                &mut gb as *mut _ as *mut c_void,
            );
            let mut heap: Vec<u8>;
            if rc == sys::GHOSTTY_OUT_OF_SPACE {
                heap = vec![0u8; gb.len];
                gb = sys::GhosttyBuffer::sized(&mut heap);
                rc = sys::ghostty_render_state_row_cells_get(
                    self.cells,
                    sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                    &mut gb as *mut _ as *mut c_void,
                );
            }
            let text = if rc == sys::GHOSTTY_SUCCESS && gb.len > 0 {
                String::from_utf8_lossy(std::slice::from_raw_parts(gb.ptr, gb.len)).into_owned()
            } else {
                String::new()
            };

            let mut style = sys::GhosttyStyle::default();
            sys::ghostty_render_state_row_cells_get(
                self.cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                &mut style as *mut _ as *mut c_void,
            );
            let mut fg = default_fg;
            let mut bg = default_bg;
            let fg_rc = sys::ghostty_render_state_row_cells_get(
                self.cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                &mut fg as *mut _ as *mut c_void,
            );
            if fg_rc != sys::GHOSTTY_SUCCESS {
                fg = default_fg;
            }
            let bg_rc = sys::ghostty_render_state_row_cells_get(
                self.cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                &mut bg as *mut _ as *mut c_void,
            );
            if bg_rc != sys::GHOSTTY_SUCCESS {
                bg = default_bg;
            }

            Cell {
                text,
                fg: fg.into(),
                bg: bg.into(),
                bold: style.bold,
                italic: style.italic,
                faint: style.faint,
                inverse: style.inverse,
                strikethrough: style.strikethrough,
                invisible: style.invisible,
                underline: style.underline,
            }
        }
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        unsafe {
            sys::ghostty_render_state_row_cells_free(self.cells);
            sys::ghostty_render_state_row_iterator_free(self.row_iter);
            sys::ghostty_render_state_free(self.rs);
        }
    }
}

fn check(rc: c_int) -> Result<(), c_int> {
    (rc == sys::GHOSTTY_SUCCESS).then_some(()).ok_or(rc)
}

// ---------------------------------------------------------------------------
// Key encoding
// ---------------------------------------------------------------------------

/// Wraps a `GhosttyKeyEncoder` + one reusable `GhosttyKeyEvent`.
pub struct KeyEncoder {
    enc: sys::GhosttyKeyEncoder,
    event: sys::GhosttyKeyEvent,
}

impl KeyEncoder {
    pub fn new() -> Result<Self, c_int> {
        unsafe {
            let mut enc: sys::GhosttyKeyEncoder = std::ptr::null_mut();
            check(sys::ghostty_key_encoder_new(std::ptr::null(), &mut enc))?;
            let mut event: sys::GhosttyKeyEvent = std::ptr::null_mut();
            check(sys::ghostty_key_event_new(std::ptr::null(), &mut event))?;
            Ok(Self { enc, event })
        }
    }

    /// Pull cursor-key/keypad modes and kitty keyboard flags from the
    /// terminal's current state. Call before every encode — programs flip
    /// these modes at runtime.
    pub fn sync_from_terminal(&mut self, term: &Terminal) {
        unsafe { sys::ghostty_key_encoder_setopt_from_terminal(self.enc, term.term) }
    }

    /// Encode a key press. `key` is one of the `GHOSTTY_KEY_*` consts,
    /// `mods` a `GHOSTTY_MODS_*` bitmask, `utf8` the layout text for the key
    /// (printable keys only). Empty result = the key produces no bytes (e.g.
    /// a bare modifier).
    pub fn encode_press(&mut self, key: c_int, mods: u16, utf8: Option<&str>) -> Vec<u8> {
        unsafe {
            sys::ghostty_key_event_set_action(self.event, sys::GHOSTTY_KEY_ACTION_PRESS);
            sys::ghostty_key_event_set_key(self.event, key);
            sys::ghostty_key_event_set_mods(self.event, mods);
            match utf8 {
                Some(text) => {
                    sys::ghostty_key_event_set_utf8(self.event, text.as_ptr() as *const c_char, text.len());
                }
                None => sys::ghostty_key_event_set_utf8(self.event, std::ptr::null(), 0),
            }
            let mut buf = [0 as c_char; 128];
            let mut written: usize = 0;
            let rc = sys::ghostty_key_encoder_encode(self.enc, self.event, buf.as_mut_ptr(), buf.len(), &mut written);
            if rc != sys::GHOSTTY_SUCCESS {
                return Vec::new();
            }
            std::slice::from_raw_parts(buf.as_ptr() as *const u8, written).to_vec()
        }
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        unsafe {
            sys::ghostty_key_event_free(self.event);
            sys::ghostty_key_encoder_free(self.enc);
        }
    }
}
