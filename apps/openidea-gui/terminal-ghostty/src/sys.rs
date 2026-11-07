//! Raw FFI bindings to libghostty-vt.
//!
//! Hand-transcribed from `include/ghostty/vt/*.h` at the Ghostty rev pinned
//! in `build.rs`. The upstream API is pre-1.0 and unstable: bump the pinned
//! rev by diffing the headers and updating this file (Ghostty's own ABI
//! manifest test is the upstream cross-check).
//!
//! Sized structs follow the `GHOSTTY_INIT_SIZED` convention: the first field
//! is `size` and must be set to `size_of::<T>()` before the call.
//!
//! The lint escapes below are inherent to a C-API transcription: C naming
//! (non_camel_case_types/non_snake_case), and the bindings carry the full
//! surface of the consumed headers while the view uses a subset (dead_code).

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_int, c_void};

#[path = "key_consts.rs"]
pub mod key_consts;
pub use key_consts::*;

// ---------------------------------------------------------------------------
// Common types (types.h, color.h)
// ---------------------------------------------------------------------------

pub type GhosttyTerminal = *mut c_void;
pub type GhosttyRenderState = *mut c_void;
pub type GhosttyRenderStateRowIterator = *mut c_void;
pub type GhosttyRenderStateRowCells = *mut c_void;
pub type GhosttyKeyEncoder = *mut c_void;
pub type GhosttyKeyEvent = *mut c_void;

pub const GHOSTTY_SUCCESS: c_int = 0;
pub const GHOSTTY_OUT_OF_MEMORY: c_int = -1;
pub const GHOSTTY_INVALID_VALUE: c_int = -2;
pub const GHOSTTY_OUT_OF_SPACE: c_int = -3;
pub const GHOSTTY_NO_VALUE: c_int = -4;
pub const GHOSTTY_IO_ERROR: c_int = -5;
pub const GHOSTTY_LIMIT_EXCEEDED: c_int = -6;
pub const GHOSTTY_REJECTED: c_int = -7;

/// Borrowed byte string produced by the library.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}

/// Caller-provided byte buffer. `len` receives bytes written on success or
/// the required capacity on `GHOSTTY_OUT_OF_SPACE`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyBuffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}

impl GhosttyBuffer {
    pub fn sized(buf: &mut [u8]) -> Self {
        Self { ptr: buf.as_mut_ptr(), cap: buf.len(), len: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A NULL allocator selects the library default everywhere we call it.
type GhosttyAllocator = c_void;

// ---------------------------------------------------------------------------
// Terminal (terminal.h)
// ---------------------------------------------------------------------------

/// `GHOSTTY_TERMINAL_OPT_*`
pub const GHOSTTY_TERMINAL_OPT_USERDATA: c_int = 0;
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: c_int = 1;
pub const GHOSTTY_TERMINAL_OPT_BELL: c_int = 2;
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: c_int = 5;
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: c_int = 11;
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: c_int = 12;
pub const GHOSTTY_TERMINAL_OPT_SELECTION: c_int = 21;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE: c_int = 22;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK: c_int = 23;
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: c_int = 25;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: c_int = 27;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: c_int = 28;
pub const GHOSTTY_TERMINAL_OPT_TERMINFO_NAME: c_int = 37;

/// `GHOSTTY_TERMINAL_DATA_*`
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: c_int = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: c_int = 4;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: c_int = 6;
pub const GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS: c_int = 8;
pub const GHOSTTY_TERMINAL_DATA_TITLE: c_int = 12;
pub const GHOSTTY_TERMINAL_DATA_PWD: c_int = 13;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: c_int = 15;

/// `GHOSTTY_SCROLL_VIEWPORT_*`
pub const GHOSTTY_SCROLL_VIEWPORT_TOP: c_int = 0;
pub const GHOSTTY_SCROLL_VIEWPORT_BOTTOM: c_int = 1;
pub const GHOSTTY_SCROLL_VIEWPORT_DELTA: c_int = 2;
pub const GHOSTTY_SCROLL_VIEWPORT_ROW: c_int = 3;
pub const GHOSTTY_SCROLL_VIEWPORT_DELTA_PROMPT: c_int = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    pub delta: isize,
    pub row: usize,
    pub _padding: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: c_int,
    pub value: GhosttyTerminalScrollViewportValue,
}

impl GhosttyTerminalScrollViewport {
    pub fn delta(lines: isize) -> Self {
        Self { tag: GHOSTTY_SCROLL_VIEWPORT_DELTA, value: GhosttyTerminalScrollViewportValue { delta: lines } }
    }

    pub fn bottom() -> Self {
        Self { tag: GHOSTTY_SCROLL_VIEWPORT_BOTTOM, value: GhosttyTerminalScrollViewportValue { delta: 0 } }
    }
}

/// `GHOSTTY_TERMINAL_CURSOR_STYLE_*` (terminal.h) — mirrors the render-state
/// cursor visual style values.
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK: c_int = 1;

/// `GhosttyTerminalWritePtyFn`
pub type GhosttyTerminalWritePtyFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize);

/// Effect callbacks that only report an occurrence (bell, title, pwd).
pub type GhosttyTerminalEffectFn = unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);

unsafe extern "C" {
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> c_int;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_reset(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> c_int;
    pub fn ghostty_terminal_set(terminal: GhosttyTerminal, option: c_int, value: *const c_void) -> c_int;
    pub fn ghostty_terminal_get(terminal: GhosttyTerminal, data: c_int, out: *mut c_void) -> c_int;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_scroll_viewport(terminal: GhosttyTerminal, behavior: GhosttyTerminalScrollViewport);
}

// --- paste (paste.h) ---

pub const GHOSTTY_CLIPBOARD_LOCATION_STANDARD: c_int = 0;
pub const GHOSTTY_PASTE_SOURCE_TEXT: c_int = 1;

/// `GhosttyMimeReaderFn` — stream one MIME representation into `writer`.
pub type GhosttyMimeReaderFn =
    unsafe extern "C" fn(userdata: *mut c_void, mime: GhosttyString, writer: GhosttyWriter) -> bool;

/// `GhosttyWriterFn` — must accept the whole slice; return false is fatal.
pub type GhosttyWriterFn = unsafe extern "C" fn(userdata: *mut c_void, data: *const u8, len: usize) -> bool;

/// Passed by value into `GhosttyMimeReaderFn`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyWriter {
    pub write: Option<GhosttyWriterFn>,
    pub userdata: *mut c_void,
}

#[repr(C)]
pub struct GhosttyMimeReader {
    pub read: Option<GhosttyMimeReaderFn>,
    pub userdata: *mut c_void,
}

/// Sized struct (`GHOSTTY_INIT_SIZED`).
#[repr(C)]
pub struct GhosttyPaste {
    pub size: usize,
    pub location: c_int,
    pub source: c_int,
    pub mimes: *const GhosttyString,
    pub mimes_len: usize,
    pub reader: GhosttyMimeReader,
    pub allow_unsafe: bool,
}

unsafe extern "C" {
    pub fn ghostty_terminal_paste(
        terminal: GhosttyTerminal,
        paste: *const GhosttyPaste,
        out_written: *mut bool,
    ) -> c_int;
    pub fn ghostty_paste_is_safe(data: *const c_char, len: usize) -> bool;
}

// ---------------------------------------------------------------------------
// Render state (render.h, style.h)
// ---------------------------------------------------------------------------

/// `GHOSTTY_RENDER_STATE_DIRTY_*`
pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: c_int = 0;
pub const GHOSTTY_RENDER_STATE_DIRTY_PARTIAL: c_int = 1;
pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: c_int = 2;

/// `GHOSTTY_RENDER_STATE_DATA_*`
pub const GHOSTTY_RENDER_STATE_DATA_COLS: c_int = 1;
pub const GHOSTTY_RENDER_STATE_DATA_ROWS: c_int = 2;
pub const GHOSTTY_RENDER_STATE_DATA_DIRTY: c_int = 3;
pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: c_int = 4;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_BACKGROUND: c_int = 5;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_FOREGROUND: c_int = 6;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_CURSOR: c_int = 7;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_CURSOR_HAS_VALUE: c_int = 8;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_PALETTE: c_int = 9;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: c_int = 18;
pub const GHOSTTY_RENDER_STATE_DATA_COLORS: c_int = 19;

/// `GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_*`
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR: c_int = 0;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK: c_int = 1;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE: c_int = 2;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW: c_int = 3;

/// `GHOSTTY_RENDER_STATE_ROW_DATA_*`
pub const GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY: c_int = 1;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: c_int = 3;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION: c_int = 4;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y: c_int = 6;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_ID: c_int = 7;

/// `GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_*`
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: c_int = 2;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: c_int = 3;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: c_int = 4;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR: c_int = 5;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR: c_int = 6;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8: c_int = 9;

/// Sized struct (`GHOSTTY_INIT_SIZED`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: c_int,
}

impl Default for GhosttyRenderStateCursor {
    fn default() -> Self {
        let mut v: Self = unsafe { std::mem::zeroed() };
        v.size = std::mem::size_of::<Self>();
        v
    }
}

/// Sized struct (`GHOSTTY_INIT_SIZED`).
#[repr(C)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

impl Default for GhosttyRenderStateColors {
    fn default() -> Self {
        let mut v: Self = unsafe { std::mem::zeroed() };
        v.size = std::mem::size_of::<Self>();
        v
    }
}

/// Sized struct (`GHOSTTY_INIT_SIZED`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateRowSelection {
    pub size: usize,
    pub start_x: u16,
    pub end_x: u16,
}

impl Default for GhosttyRenderStateRowSelection {
    fn default() -> Self {
        let mut v: Self = unsafe { std::mem::zeroed() };
        v.size = std::mem::size_of::<Self>();
        v
    }
}

unsafe extern "C" {
    pub fn ghostty_render_state_new(allocator: *const GhosttyAllocator, state: *mut GhosttyRenderState) -> c_int;
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    pub fn ghostty_render_state_update(state: GhosttyRenderState, terminal: GhosttyTerminal) -> c_int;
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> c_int;
    pub fn ghostty_render_state_get(state: GhosttyRenderState, data: c_int, out: *mut c_void) -> c_int;
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> c_int;
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    pub fn ghostty_render_state_row_iterator_next_dirty(
        iterator: GhosttyRenderStateRowIterator,
        out_y: *mut u16,
    ) -> bool;
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: c_int,
        out: *mut c_void,
    ) -> c_int;
    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> c_int;
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: c_int,
        out: *mut c_void,
    ) -> c_int;
}

// --- style (style.h, sgr.h) ---

/// `GHOSTTY_STYLE_COLOR_*`
pub const GHOSTTY_STYLE_COLOR_NONE: c_int = 0;
pub const GHOSTTY_STYLE_COLOR_PALETTE: c_int = 1;
pub const GHOSTTY_STYLE_COLOR_RGB: c_int = 2;

/// `GHOSTTY_SGR_UNDERLINE_*`
pub const GHOSTTY_SGR_UNDERLINE_NONE: c_int = 0;
pub const GHOSTTY_SGR_UNDERLINE_SINGLE: c_int = 1;
pub const GHOSTTY_SGR_UNDERLINE_DOUBLE: c_int = 2;
pub const GHOSTTY_SGR_UNDERLINE_CURLY: c_int = 3;
pub const GHOSTTY_SGR_UNDERLINE_DOTTED: c_int = 4;
pub const GHOSTTY_SGR_UNDERLINE_DASHED: c_int = 5;

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: u8,
    pub rgb: GhosttyColorRgb,
    pub _padding: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    pub tag: c_int,
    pub value: GhosttyStyleColorValue,
}

/// Sized struct (`GHOSTTY_INIT_SIZED`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    pub size: usize,
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: c_int,
}

impl Default for GhosttyStyle {
    fn default() -> Self {
        let mut v: Self = unsafe { std::mem::zeroed() };
        v.size = std::mem::size_of::<Self>();
        v
    }
}

// ---------------------------------------------------------------------------
// Key encoding (key.h, key/event.h, key/encoder.h)
// ---------------------------------------------------------------------------

/// `GHOSTTY_KEY_ACTION_*`
pub const GHOSTTY_KEY_ACTION_RELEASE: c_int = 0;
pub const GHOSTTY_KEY_ACTION_PRESS: c_int = 1;
pub const GHOSTTY_KEY_ACTION_REPEAT: c_int = 2;

/// `GHOSTTY_MODS_*`
pub const GHOSTTY_MODS_SHIFT: u16 = 1 << 0;
pub const GHOSTTY_MODS_CTRL: u16 = 1 << 1;
pub const GHOSTTY_MODS_ALT: u16 = 1 << 2;
pub const GHOSTTY_MODS_SUPER: u16 = 1 << 3;
pub const GHOSTTY_MODS_CAPS_LOCK: u16 = 1 << 4;
pub const GHOSTTY_MODS_NUM_LOCK: u16 = 1 << 5;

/// `GHOSTTY_KITTY_KEY_*`
pub const GHOSTTY_KITTY_KEY_DISABLED: u8 = 0;
pub const GHOSTTY_KITTY_KEY_DISAMBIGUATE: u8 = 1 << 0;
pub const GHOSTTY_KITTY_KEY_REPORT_EVENTS: u8 = 1 << 1;
pub const GHOSTTY_KITTY_KEY_REPORT_ALTERNATES: u8 = 1 << 2;
pub const GHOSTTY_KITTY_KEY_REPORT_ALL: u8 = 1 << 3;
pub const GHOSTTY_KITTY_KEY_REPORT_ASSOCIATED: u8 = 1 << 4;

/// `GHOSTTY_KEY_ENCODER_OPT_*`
pub const GHOSTTY_KEY_ENCODER_OPT_CURSOR_KEY_APPLICATION: c_int = 0;
pub const GHOSTTY_KEY_ENCODER_OPT_KEYPAD_KEY_APPLICATION: c_int = 1;
pub const GHOSTTY_KEY_ENCODER_OPT_IGNORE_KEYPAD_WITH_NUMLOCK: c_int = 2;
pub const GHOSTTY_KEY_ENCODER_OPT_ALT_ESC_PREFIX: c_int = 3;
pub const GHOSTTY_KEY_ENCODER_OPT_MODIFY_OTHER_KEYS_STATE_2: c_int = 4;
pub const GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS: c_int = 5;
pub const GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT: c_int = 6;
pub const GHOSTTY_KEY_ENCODER_OPT_BACKARROW_KEY_MODE: c_int = 7;

/// `GHOSTTY_OPTION_AS_ALT_*`
pub const GHOSTTY_OPTION_AS_ALT_FALSE: c_int = 0;
pub const GHOSTTY_OPTION_AS_ALT_TRUE: c_int = 1;

unsafe extern "C" {
    pub fn ghostty_key_event_new(allocator: *const GhosttyAllocator, event: *mut GhosttyKeyEvent) -> c_int;
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: c_int);
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: c_int);
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: u16);
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);

    pub fn ghostty_key_encoder_new(allocator: *const GhosttyAllocator, encoder: *mut GhosttyKeyEncoder) -> c_int;
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    pub fn ghostty_key_encoder_setopt(encoder: GhosttyKeyEncoder, option: c_int, value: *const c_void);
    pub fn ghostty_key_encoder_setopt_from_terminal(encoder: GhosttyKeyEncoder, terminal: GhosttyTerminal);
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> c_int;
}
