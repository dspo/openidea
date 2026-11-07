//! Smoke tests against the real libghostty-vt: link, terminal lifecycle,
//! render-state reads, effect callbacks, key encoding.

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use terminal_ghostty::sys;

fn check(code: c_int) {
    assert_eq!(code, sys::GHOSTTY_SUCCESS, "ghostty call failed: {code}");
}

#[test]
fn terminal_lifecycle_and_render() {
    unsafe {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        check(sys::ghostty_terminal_new(std::ptr::null(), &mut term, 40, 5));
        assert!(!term.is_null());

        let content = b"Hello, \x1b[1;32mworld\x1b[0m!\r\n\x1b[4munderlined\x1b[0m text\r\n";
        sys::ghostty_terminal_vt_write(term, content.as_ptr(), content.len());

        // Resize without cell pixel metrics must still succeed.
        check(sys::ghostty_terminal_resize(term, 20, 4, 0, 0));

        let mut rs: sys::GhosttyRenderState = std::ptr::null_mut();
        check(sys::ghostty_render_state_new(std::ptr::null(), &mut rs));
        check(sys::ghostty_render_state_update(rs, term));

        let mut dirty: c_int = -1;
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_DIRTY,
            &mut dirty as *mut _ as *mut c_void,
        ));
        assert_eq!(dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FULL);

        let mut colors = sys::GhosttyRenderStateColors::default();
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
            &mut colors as *mut _ as *mut c_void,
        ));

        let mut cursor = sys::GhosttyRenderStateCursor::default();
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
            &mut cursor as *mut _ as *mut c_void,
        ));
        assert!(cursor.viewport_has_value);
        assert_eq!(cursor.viewport_y, 2);
        assert_eq!(cursor.viewport_x, 0);

        // Row 0: "Hello, world!" — "world" is bold green (palette index 2).
        let row_text = read_row(rs, colors.palette, 0);
        assert!(row_text.starts_with("Hello, world!"), "got {row_text:?}");
        let (world_text, style) = read_row_cell_styled(rs, 0, 7);
        assert_eq!(world_text, "w");
        assert!(style.bold, "cell 7 should be bold");
        assert_eq!(style.fg, colors.palette[2], "cell 7 should be palette green");

        // Row 1 first cell: underlined plain text.
        let (u_text, u_style) = read_row_cell_styled(rs, 1, 0);
        assert_eq!(u_text, "u");
        assert_eq!(u_style.underline, sys::GHOSTTY_SGR_UNDERLINE_SINGLE);
        let _ = u_text;

        check(sys::ghostty_render_state_clean(rs));
        sys::ghostty_render_state_free(rs);
        sys::ghostty_terminal_free(term);
    }
}

struct CellStyle {
    bold: bool,
    underline: c_int,
    fg: sys::GhosttyColorRgb,
}

/// Concatenate a row's cells as UTF-8 (palette colors resolved).
unsafe fn read_row(rs: sys::GhosttyRenderState, palette: [sys::GhosttyColorRgb; 256], _row: u16) -> String {
    unsafe {
        let mut iter: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut iter));
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            &mut iter as *mut _ as *mut c_void,
        ));
        let mut cells: sys::GhosttyRenderStateRowCells = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells));

        let mut out = String::new();
        let mut y: u16 = 0;
        while sys::ghostty_render_state_row_iterator_next_dirty(iter, &mut y) {
            check(sys::ghostty_render_state_row_get(
                iter,
                sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                &mut cells as *mut _ as *mut c_void,
            ));
            while sys::ghostty_render_state_row_cells_next(cells) {
                out.push_str(&cell_utf8(cells));
                let _ = palette;
            }
        }
        sys::ghostty_render_state_row_cells_free(cells);
        sys::ghostty_render_state_row_iterator_free(iter);
        out
    }
}

/// Read cell `x` of row `row` by walking the iterator to that row.
unsafe fn read_row_cell_styled(rs: sys::GhosttyRenderState, row: u16, x: u16) -> (String, CellStyle) {
    unsafe {
        let mut iter: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut iter));
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            &mut iter as *mut _ as *mut c_void,
        ));
        let mut cells: sys::GhosttyRenderStateRowCells = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells));

        let mut result = (String::new(), CellStyle { bold: false, underline: 0, fg: Default::default() });
        let mut y: u16 = 0;
        'rows: while sys::ghostty_render_state_row_iterator_next_dirty(iter, &mut y) {
            if y != row {
                continue;
            }
            check(sys::ghostty_render_state_row_get(
                iter,
                sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                &mut cells as *mut _ as *mut c_void,
            ));
            let mut cx: u16 = 0;
            while sys::ghostty_render_state_row_cells_next(cells) {
                if cx == x {
                    let mut style = sys::GhosttyStyle::default();
                    sys::ghostty_render_state_row_cells_get(
                        cells,
                        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                        &mut style as *mut _ as *mut c_void,
                    );
                    let mut fg = sys::GhosttyColorRgb::default();
                    sys::ghostty_render_state_row_cells_get(
                        cells,
                        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                        &mut fg as *mut _ as *mut c_void,
                    );
                    result = (cell_utf8(cells), CellStyle { bold: style.bold, underline: style.underline, fg });
                    break 'rows;
                }
                cx += 1;
            }
        }
        sys::ghostty_render_state_row_cells_free(cells);
        sys::ghostty_render_state_row_iterator_free(iter);
        result
    }
}

unsafe fn cell_utf8(cells: sys::GhosttyRenderStateRowCells) -> String {
    unsafe {
        let mut buf = [0u8; 64];
        let mut gb = sys::GhosttyBuffer::sized(&mut buf);
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            &mut gb as *mut _ as *mut c_void,
        );
        String::from_utf8_lossy(std::slice::from_raw_parts(gb.ptr, gb.len)).into_owned()
    }
}

#[test]
fn title_callback_and_data() {
    static FIRES: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn on_title(_term: sys::GhosttyTerminal, _ud: *mut c_void) {
        FIRES.fetch_add(1, Ordering::SeqCst);
    }

    unsafe {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        check(sys::ghostty_terminal_new(std::ptr::null(), &mut term, 40, 5));
        check(sys::ghostty_terminal_set(
            term,
            sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            on_title as sys::GhosttyTerminalEffectFn as *const c_void,
        ));

        let osc = b"\x1b]2;smoke title\x07";
        sys::ghostty_terminal_vt_write(term, osc.as_ptr(), osc.len());
        assert_eq!(FIRES.load(Ordering::SeqCst), 1);

        let mut title = sys::GhosttyString { ptr: std::ptr::null(), len: 0 };
        check(sys::ghostty_terminal_get(term, sys::GHOSTTY_TERMINAL_DATA_TITLE, &mut title as *mut _ as *mut c_void));
        let got = String::from_utf8_lossy(std::slice::from_raw_parts(title.ptr, title.len)).into_owned();
        assert_eq!(got, "smoke title");
        sys::ghostty_terminal_free(term);
    }
}

#[test]
fn write_pty_callback_and_key_encoder() {
    use std::sync::Mutex;

    let replies: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));

    unsafe extern "C" fn on_write_pty(_term: sys::GhosttyTerminal, ud: *mut c_void, data: *const u8, len: usize) {
        unsafe {
            let replies = &*(ud as *const Mutex<Vec<u8>>);
            replies.lock().unwrap().extend_from_slice(std::slice::from_raw_parts(data, len));
        }
    }

    unsafe {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        check(sys::ghostty_terminal_new(std::ptr::null(), &mut term, 40, 5));
        check(sys::ghostty_terminal_set(
            term,
            sys::GHOSTTY_TERMINAL_OPT_USERDATA,
            Arc::as_ptr(&replies) as *const c_void,
        ));
        check(sys::ghostty_terminal_set(
            term,
            sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            on_write_pty as sys::GhosttyTerminalWritePtyFn as *const c_void,
        ));

        // DA query → the terminal answers on the WRITE_PTY callback.
        let da = b"\x1b[c";
        sys::ghostty_terminal_vt_write(term, da.as_ptr(), da.len());
        let got = replies.lock().unwrap().clone();
        assert!(got.starts_with(b"\x1b[?"), "expected a DA reply, got {got:02x?}");

        // --- key encoder, legacy mode ---
        let mut enc: sys::GhosttyKeyEncoder = std::ptr::null_mut();
        check(sys::ghostty_key_encoder_new(std::ptr::null(), &mut enc));
        sys::ghostty_key_encoder_setopt_from_terminal(enc, term);

        let mut ev: sys::GhosttyKeyEvent = std::ptr::null_mut();
        check(sys::ghostty_key_event_new(std::ptr::null(), &mut ev));
        sys::ghostty_key_event_set_action(ev, sys::GHOSTTY_KEY_ACTION_PRESS);

        sys::ghostty_key_event_set_key(ev, sys::GHOSTTY_KEY_A);
        let text = CString::new("a").unwrap();
        sys::ghostty_key_event_set_utf8(ev, text.as_ptr(), 1);
        assert_eq!(encode(enc, ev), b"a");

        sys::ghostty_key_event_set_key(ev, sys::GHOSTTY_KEY_ARROW_UP);
        sys::ghostty_key_event_set_utf8(ev, std::ptr::null(), 0);
        assert_eq!(encode(enc, ev), b"\x1b[A");

        sys::ghostty_key_event_set_key(ev, sys::GHOSTTY_KEY_C);
        sys::ghostty_key_event_set_mods(ev, sys::GHOSTTY_MODS_CTRL);
        assert_eq!(encode(enc, ev), b"\x03");

        // --- kitty keyboard protocol: mode decodes from the stream ---
        let kitty_on = b"\x1b[>1u"; // disambiguate
        sys::ghostty_terminal_vt_write(term, kitty_on.as_ptr(), kitty_on.len());
        let mut flags: u8 = 0;
        check(sys::ghostty_terminal_get(
            term,
            sys::GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
            &mut flags as *mut _ as *mut c_void,
        ));
        assert_eq!(flags, sys::GHOSTTY_KITTY_KEY_DISAMBIGUATE);

        sys::ghostty_key_encoder_setopt_from_terminal(enc, term);
        sys::ghostty_key_event_set_key(ev, sys::GHOSTTY_KEY_ARROW_UP);
        sys::ghostty_key_event_set_mods(ev, 0);
        let kitty_up = encode(enc, ev);
        assert!(!kitty_up.is_empty(), "kitty encoding produced nothing");

        sys::ghostty_key_event_free(ev);
        sys::ghostty_key_encoder_free(enc);
        sys::ghostty_terminal_free(term);
    }
}

unsafe fn encode(enc: sys::GhosttyKeyEncoder, ev: sys::GhosttyKeyEvent) -> Vec<u8> {
    unsafe {
        let mut buf = [0i8; 128];
        let mut written: usize = 0;
        let rc = sys::ghostty_key_encoder_encode(enc, ev, buf.as_mut_ptr() as *mut c_char, buf.len(), &mut written);
        assert_eq!(rc, sys::GHOSTTY_SUCCESS, "encode failed: {rc}");
        std::slice::from_raw_parts(buf.as_ptr() as *const u8, written).to_vec()
    }
}

#[test]
fn default_colors_and_plain_cell_resolution() {
    unsafe {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        check(sys::ghostty_terminal_new(std::ptr::null(), &mut term, 40, 5));

        let fg = sys::GhosttyColorRgb { r: 0xd4, g: 0xd4, b: 0xd4 };
        let bg = sys::GhosttyColorRgb { r: 0x10, g: 0x13, b: 0x18 };
        check(sys::ghostty_terminal_set(
            term,
            sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            &fg as *const _ as *const c_void,
        ));
        check(sys::ghostty_terminal_set(
            term,
            sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            &bg as *const _ as *const c_void,
        ));

        sys::ghostty_terminal_vt_write(term, b"plain\r\n".as_ptr(), 7);

        let mut rs: sys::GhosttyRenderState = std::ptr::null_mut();
        check(sys::ghostty_render_state_new(std::ptr::null(), &mut rs));
        check(sys::ghostty_render_state_update(rs, term));

        let mut colors = sys::GhosttyRenderStateColors::default();
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
            &mut colors as *mut _ as *mut c_void,
        ));
        assert_eq!(colors.foreground, fg, "render state fg should mirror the option");
        assert_eq!(colors.background, bg, "render state bg should mirror the option");

        // First cell's resolved fg via ROW_CELLS_DATA_FG_COLOR.
        let mut iter: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut iter));
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            &mut iter as *mut _ as *mut c_void,
        ));
        let mut cells: sys::GhosttyRenderStateRowCells = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells));
        let mut y: u16 = 0;
        assert!(sys::ghostty_render_state_row_iterator_next_dirty(iter, &mut y));
        check(sys::ghostty_render_state_row_get(
            iter,
            sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
            &mut cells as *mut _ as *mut c_void,
        ));
        assert!(sys::ghostty_render_state_row_cells_next(cells));

        let mut cell_fg = sys::GhosttyColorRgb::default();
        let fg_rc = sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
            &mut cell_fg as *mut _ as *mut c_void,
        );
        let mut cell_bg = sys::GhosttyColorRgb::default();
        let bg_rc = sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
            &mut cell_bg as *mut _ as *mut c_void,
        );
        println!("plain cell fg rc={fg_rc} {cell_fg:?}, bg rc={bg_rc} {cell_bg:?}");
        // Pin the C contract: unstyled cells report GHOSTTY_INVALID_VALUE
        // (zeroed RGB) for their resolved colors — embedders fall back to the
        // render-state defaults themselves (see ghostty-vt read_cell).
        assert_eq!(fg_rc, sys::GHOSTTY_INVALID_VALUE, "plain cell fg is unresolved by design");
        assert_eq!(bg_rc, sys::GHOSTTY_INVALID_VALUE, "plain cell bg is unresolved by design");

        sys::ghostty_render_state_row_cells_free(cells);
        sys::ghostty_render_state_row_iterator_free(iter);
        sys::ghostty_render_state_free(rs);
        sys::ghostty_terminal_free(term);
    }
}

#[test]
fn underline_and_inverse_style_flags() {
    unsafe {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        check(sys::ghostty_terminal_new(std::ptr::null(), &mut term, 40, 5));
        let content = b"\x1b[4mU\x1b[0m\x1b[7mI\x1b[0m";
        sys::ghostty_terminal_vt_write(term, content.as_ptr(), content.len());

        let mut rs: sys::GhosttyRenderState = std::ptr::null_mut();
        check(sys::ghostty_render_state_new(std::ptr::null(), &mut rs));
        check(sys::ghostty_render_state_update(rs, term));

        let mut iter: sys::GhosttyRenderStateRowIterator = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut iter));
        check(sys::ghostty_render_state_get(
            rs,
            sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            &mut iter as *mut _ as *mut c_void,
        ));
        let mut cells: sys::GhosttyRenderStateRowCells = std::ptr::null_mut();
        check(sys::ghostty_render_state_row_cells_new(std::ptr::null(), &mut cells));
        let mut y: u16 = 0;
        assert!(sys::ghostty_render_state_row_iterator_next_dirty(iter, &mut y));
        check(sys::ghostty_render_state_row_get(
            iter,
            sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
            &mut cells as *mut _ as *mut c_void,
        ));

        for (x, expect) in [(0u16, "underline"), (1u16, "inverse")] {
            assert!(sys::ghostty_render_state_row_cells_next(cells), "cell {x}");
            let mut style = sys::GhosttyStyle::default();
            sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                &mut style as *mut _ as *mut c_void,
            );
            let mut fg = sys::GhosttyColorRgb::default();
            let fg_rc = sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                &mut fg as *mut _ as *mut c_void,
            );
            let mut bg = sys::GhosttyColorRgb::default();
            let bg_rc = sys::ghostty_render_state_row_cells_get(
                cells,
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                &mut bg as *mut _ as *mut c_void,
            );
            println!(
                "{expect}: underline={} inverse={} fg_rc={fg_rc} {fg:?} bg_rc={bg_rc} {bg:?} bold={} italic={}",
                style.underline, style.inverse, style.bold, style.italic
            );
            assert!(style.underline != 0 || style.inverse, "{expect} flag lost");
        }

        sys::ghostty_render_state_row_cells_free(cells);
        sys::ghostty_render_state_row_iterator_free(iter);
        sys::ghostty_render_state_free(rs);
        sys::ghostty_terminal_free(term);
    }
}

/// The OSC 8 hyperlink lookup (M12's ⌘-click face): a terminal fed an OSC 8
/// sequence exposes the URI at the labeled cell and nothing at the others.
#[test]
fn hyperlink_uri_at_reads_the_labeled_cell() {
    unsafe {
        let mut term: sys::GhosttyTerminal = std::ptr::null_mut();
        check(sys::ghostty_terminal_new(std::ptr::null(), &mut term, 80, 5));
        // The OSC 8 END face is the spec's empty-URI form (`]8;;` + ST); a
        // bare ST leaks the link onto the following cells.
        let content = b"\x1b]8;;https://example.com/m12\x1b\\CLICK-ME\x1b]8;;\x1b\\\r\nplain row\r\n";
        sys::ghostty_terminal_vt_write(term, content.as_ptr(), content.len());

        let point = |col: u16, row: u32| sys::GhosttyPoint {
            tag: sys::GHOSTTY_POINT_TAG_VIEWPORT,
            value: sys::GhosttyPointValue { coordinate: sys::GhosttyPointCoordinate { x: col, y: row } },
        };
        let uri_at = |col: u16, row: u32| -> Option<String> {
            let mut grid_ref = sys::GhosttyGridRef::default();
            check(sys::ghostty_terminal_grid_ref(term, point(col, row), &mut grid_ref));
            let mut len: usize = 0;
            let rc = sys::ghostty_grid_ref_hyperlink_uri(&grid_ref, std::ptr::null_mut(), 0, &mut len);
            if rc == sys::GHOSTTY_SUCCESS && len == 0 {
                return None;
            }
            assert_eq!(rc, sys::GHOSTTY_OUT_OF_SPACE, "the URI length probe");
            let mut buf = vec![0u8; len];
            let mut written: usize = 0;
            check(sys::ghostty_grid_ref_hyperlink_uri(&grid_ref, buf.as_mut_ptr(), buf.len(), &mut written));
            buf.truncate(written);
            Some(String::from_utf8(buf).expect("utf8 uri"))
        };

        // The labeled cells (C-L-I-C-K-'-'-M-E at row 0, columns 0..7) carry
        // the URI; the plain row below carries none.
        assert_eq!(uri_at(0, 0).as_deref(), Some("https://example.com/m12"));
        assert_eq!(uri_at(7, 0).as_deref(), Some("https://example.com/m12"));
        assert_eq!(uri_at(0, 1), None, "the plain row has no hyperlink");
        sys::ghostty_terminal_free(term);
    }
}
