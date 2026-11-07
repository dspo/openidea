//! GPUI keystrokes → libghostty-vt key events.

use crate::sys::key_consts::*;
use crate::sys::{GHOSTTY_MODS_ALT, GHOSTTY_MODS_CTRL, GHOSTTY_MODS_SHIFT, GHOSTTY_MODS_SUPER};
use gpui::Modifiers;

/// Map a GPUI `Keystroke.key` name to the physical `GhosttyKey` const, plus
/// the text to hand the encoder (`key_char` — the layout-produced character).
pub fn map_key(gpui_key: &str) -> CIntKey {
    match gpui_key {
        "enter" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "space" => GHOSTTY_KEY_SPACE,
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        "backquote" | "`" => GHOSTTY_KEY_BACKQUOTE,
        "-" | "minus" => GHOSTTY_KEY_MINUS,
        "=" | "equal" => GHOSTTY_KEY_EQUAL,
        "[" | "bracketleft" => GHOSTTY_KEY_BRACKET_LEFT,
        "]" | "bracketright" => GHOSTTY_KEY_BRACKET_RIGHT,
        "\\" => GHOSTTY_KEY_BACKSLASH,
        ";" | "semicolon" => GHOSTTY_KEY_SEMICOLON,
        "'" | "quote" => GHOSTTY_KEY_QUOTE,
        "," => GHOSTTY_KEY_COMMA,
        "." => GHOSTTY_KEY_PERIOD,
        "/" => GHOSTTY_KEY_SLASH,
        "capslock" => GHOSTTY_KEY_CAPS_LOCK,
        f if f.len() >= 2 && f.starts_with('f') => {
            // f1..f25 are sequential constants in the header.
            match f[1..].parse::<u32>() {
                Ok(n @ 1..=25) => GHOSTTY_KEY_F1 + (n as i32 - 1),
                _ => GHOSTTY_KEY_UNIDENTIFIED,
            }
        }
        single => {
            let mut chars = single.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_uppercase() {
                    return GHOSTTY_KEY_A + (c as i32 - b'A' as i32);
                }
                if c.is_ascii_lowercase() {
                    return GHOSTTY_KEY_A + (c as i32 - b'a' as i32);
                }
                if c.is_ascii_digit() {
                    return GHOSTTY_KEY_DIGIT_0 + (c as i32 - b'0' as i32);
                }
            }
            GHOSTTY_KEY_UNIDENTIFIED
        }
    }
}

/// `GhosttyKey` as a plain i32 alias for readability above.
type CIntKey = std::ffi::c_int;

/// The IME echo window for keystroke-grade input (the same algorithm and
/// reasoning as the app crate's `ime::ImeEchoDeduper` — a pass-through input
/// method's commit re-arrives as a plain key event on Linux; the terminal
/// encodes both and the shell sees doubled characters). Kept crate-local so
/// terminal-ghostty stays dependency-free of the app.
pub const ECHO_WINDOW_MS: u64 = 40;

/// An echo of the previous encoded keystroke: same bytes, inside the window.
/// `now_ms` injected — pure and testable. Held repeats are filtered by the
/// caller (`is_held`) before this logic runs.
#[derive(Default)]
pub struct EchoDeduper {
    last: Option<(Vec<u8>, u64)>,
}

impl EchoDeduper {
    pub fn note(&mut self, bytes: &[u8], now_ms: u64) {
        if !bytes.is_empty() {
            self.last = Some((bytes.to_vec(), now_ms));
        }
    }

    pub fn allows(&self, bytes: &[u8], now_ms: u64) -> bool {
        match &self.last {
            Some((last, at)) => bytes != last || now_ms.saturating_sub(*at) > ECHO_WINDOW_MS,
            None => true,
        }
    }

    /// A composition engaged: its commit may reproduce the last raw
    /// character, so the next identical insert is not treated as an echo.
    pub fn disarm(&mut self) {
        self.last = None;
    }
}

pub fn ghostty_mods(m: &Modifiers) -> u16 {
    let mut out = 0;
    if m.shift {
        out |= GHOSTTY_MODS_SHIFT;
    }
    if m.control {
        out |= GHOSTTY_MODS_CTRL;
    }
    if m.alt {
        out |= GHOSTTY_MODS_ALT;
    }
    if m.platform {
        out |= GHOSTTY_MODS_SUPER;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The named keys the encoder relies on: a wrong code here types a
    /// different key than the one pressed, which no integration test catches.
    #[test]
    fn named_keys_map_to_their_ghostty_codes() {
        assert_eq!(map_key("enter"), GHOSTTY_KEY_ENTER);
        assert_eq!(map_key("tab"), GHOSTTY_KEY_TAB);
        assert_eq!(map_key("backspace"), GHOSTTY_KEY_BACKSPACE);
        assert_eq!(map_key("escape"), GHOSTTY_KEY_ESCAPE);
        assert_eq!(map_key("space"), GHOSTTY_KEY_SPACE);
        assert_eq!(map_key("up"), GHOSTTY_KEY_ARROW_UP);
        assert_eq!(map_key("pagedown"), GHOSTTY_KEY_PAGE_DOWN);
        assert_eq!(map_key("backquote"), GHOSTTY_KEY_BACKQUOTE);
        assert_eq!(map_key("bracketleft"), GHOSTTY_KEY_BRACKET_LEFT);
        assert_eq!(map_key("\\"), GHOSTTY_KEY_BACKSLASH);
        assert_eq!(map_key("quote"), GHOSTTY_KEY_QUOTE);
    }

    /// Printable keys resolve through the printable branch: the physical code
    /// follows the character, and the shifted form is the same key (the
    /// encoder applies the modifier).
    #[test]
    fn printable_keys_resolve_by_code() {
        assert_eq!(map_key("a"), GHOSTTY_KEY_A);
        assert_eq!(map_key("z"), GHOSTTY_KEY_Z);
        assert_eq!(map_key("0"), GHOSTTY_KEY_DIGIT_0);
        assert_eq!(map_key("9"), GHOSTTY_KEY_DIGIT_9);
        assert_eq!(map_key("-"), GHOSTTY_KEY_MINUS);
        assert_eq!(map_key("/"), GHOSTTY_KEY_SLASH);
    }

    /// Function keys are sequential constants in the header; the arithmetic
    /// must land exactly on the last one and give up past it.
    #[test]
    fn function_keys_span_f1_to_f25() {
        assert_eq!(map_key("f1"), GHOSTTY_KEY_F1);
        assert_eq!(map_key("f13"), GHOSTTY_KEY_F1 + 12);
        assert_eq!(map_key("f25"), GHOSTTY_KEY_F25);
        assert_eq!(map_key("f26"), GHOSTTY_KEY_UNIDENTIFIED);
        assert_eq!(map_key("fx"), GHOSTTY_KEY_UNIDENTIFIED);
    }

    /// Unmapped input reports `UNIDENTIFIED` so the encoder (not this map)
    /// decides what a dead key or a chord produces.
    #[test]
    fn unknown_and_non_ascii_keys_are_unidentified() {
        assert_eq!(map_key("f13ignore"), GHOSTTY_KEY_UNIDENTIFIED);
        assert_eq!(map_key("中"), GHOSTTY_KEY_UNIDENTIFIED);
        assert_eq!(map_key("ab"), GHOSTTY_KEY_UNIDENTIFIED);
    }

    #[test]
    fn modifiers_follow_the_ghostty_bitmask() {
        let base = Modifiers::default();
        assert_eq!(ghostty_mods(&base), 0);
        assert_eq!(ghostty_mods(&Modifiers { shift: true, ..base }), GHOSTTY_MODS_SHIFT);
        assert_eq!(ghostty_mods(&Modifiers { control: true, alt: true, ..base }), GHOSTTY_MODS_CTRL | GHOSTTY_MODS_ALT);
        assert_eq!(ghostty_mods(&Modifiers { platform: true, ..base }), GHOSTTY_MODS_SUPER);
    }
}

/// Echo-dedup tests: the pass-through IME's echo arrives as a second,
/// byte-identical keystroke in the same dispatch batch.
#[cfg(test)]
mod echo_tests {
    use super::*;

    #[test]
    fn the_echo_of_a_keystroke_is_refused() {
        let mut d = EchoDeduper::default();
        d.note(b"n", 1000);
        assert!(!d.allows(b"n", 1010));
        assert!(d.allows(b"n", 1050), "outside the window is a real repeat");
        assert!(d.allows(b"i", 1001), "a different key is never an echo");
    }

    #[test]
    fn control_sequences_dedupe_against_themselves_only() {
        let mut d = EchoDeduper::default();
        d.note(&[0x03], 1000); // Ctrl+C bytes
        assert!(!d.allows(&[0x03], 1001));
        assert!(d.allows(&[0x04], 1001));
    }
}
