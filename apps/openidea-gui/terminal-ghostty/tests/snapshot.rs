//! Wrapper-level snapshot semantics: style flags and default-color fallback.

use terminal_ghostty::terminal::{RenderState, Terminal};

#[test]
fn snapshot_carries_style_flags_and_defaults() {
    let term = Terminal::new(40, 5).expect("terminal");
    term.set_default_colors(
        terminal_ghostty::terminal::Rgb::new(0xd4, 0xd4, 0xd4),
        terminal_ghostty::terminal::Rgb::new(0x10, 0x13, 0x18),
    );
    term.write(b"\x1b[4munder\x1b[0m plain \x1b[7mzv\x1b[0m \x1b[1;32mgrn\x1b[0m");

    let mut rs = RenderState::new().expect("render state");
    let frame = rs.snapshot(&term).expect("snapshot");

    let row = &frame.cells[0];
    let find = |probe: &str| {
        row.iter().find(|c| c.text == probe).unwrap_or_else(|| panic!("cell {probe:?} not found")).clone()
    };

    let under = find("u");
    assert_eq!(under.underline, terminal_ghostty::sys::GHOSTTY_SGR_UNDERLINE_SINGLE);

    let plain = find("p");
    assert!(!plain.inverse && !plain.bold);
    assert_eq!(plain.fg, terminal_ghostty::terminal::Rgb::new(0xd4, 0xd4, 0xd4), "plain fg falls back to default");
    assert_eq!(plain.bg, terminal_ghostty::terminal::Rgb::new(0x10, 0x13, 0x18), "plain bg falls back to default");

    let inv = find("z");
    assert!(inv.inverse, "inverse flag must survive the wrapper");

    let grn = find("g");
    assert!(grn.bold);
    assert_ne!(grn.fg, plain.fg, "palette green resolves for the bold cell");
}
