//! The IME echo deduper (M11 phase 0.5): kills the double-write a
//! pass-through input method produces on Linux (LINUX-GATE-REPORT §2).
//!
//! The mechanics (clean-room reading, `docs/m11-design-notes.md` Part 1): the
//! Wayland client delivers an IME commit of a single plain character as an
//! ordinary key event, *alongside* the real key — on GNOME+ibus the two are
//! mutually exclusive, on sway+fcitx5's pass-through both arrive, and the
//! buffer gets "nniihhaaoo". By the time the echo reaches the app it is
//! indistinguishable from a keypress except by timing, so the dedup is
//! time-and-text keyed: an insert whose text equals the raw insert that just
//! landed (within one Wayland dispatch batch) and that was not preceded by a
//! composition is an echo and is dropped.
//!
//! Pure and clock-free — timestamps are injected (`now_ms`), so the logic is
//! pinned by plain unit tests on any platform.

/// How close an identical insert must be to the previous one to count as the
/// echo. The echo trails the key by at most the compositor's IME round-trip —
/// a couple of frames (~16 ms each) at worst. Above it: the harness's 60 ms
/// synthetic-typing cadence, held-key repeats (filtered by the caller's
/// `is_held` check anyway), and the fastest intentional same-key typing
/// (≥100 ms).
pub const ECHO_WINDOW_MS: u64 = 40;

/// Milliseconds on a process-lifetime monotonic clock (call sites; tests
/// inject their own numbers).
pub fn now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

#[derive(Default)]
pub struct ImeEchoDeduper {
    /// The last raw-path insert: its text and when it landed.
    last: Option<(String, u64)>,
}

impl ImeEchoDeduper {
    /// The raw-key path wrote `text` at `now_ms` — the next identical insert
    /// inside the window is the echo.
    pub fn note_raw(&mut self, text: &str, now_ms: u64) {
        if !text.is_empty() {
            self.last = Some((text.to_owned(), now_ms));
        }
    }

    /// A composition engaged (marked text set): the IME owns the input until
    /// its commit, and nothing the composition does may be dropped — a real
    /// commit can legitimately reproduce the last raw character.
    pub fn disarm(&mut self) {
        self.last = None;
    }

    /// Whether a non-composing insert of `text` at `now_ms` may write.
    /// `false` = the echo of the raw insert that just landed; skip it.
    pub fn allows(&self, text: &str, now_ms: u64) -> bool {
        match &self.last {
            Some((last, at)) => text != last || now_ms.saturating_sub(*at) > ECHO_WINDOW_MS,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fcitx5 pass-through pair: the real key and its echo land in the
    /// same dispatch batch — the second write is dropped (the "nn" bug).
    #[test]
    fn the_echo_of_a_raw_insert_is_dropped() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("n", 1000);
        // An echo one frame (16 ms) behind the raw insert is refused…
        assert!(!d.allows("n", 1016));
        // …and so is one 1 ms behind.
        assert!(!d.allows("n", 1001));
    }

    /// A human's fastest repeat typing sits far outside the dispatch batch:
    /// the same character 100 ms later is a real keypress, not an echo.
    #[test]
    fn an_intentional_repeat_outside_the_window_writes() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("n", 1000);
        assert!(d.allows("n", 1000 + ECHO_WINDOW_MS + 1));
    }

    /// The window edge itself is still inside (the boundary is `>` window).
    #[test]
    fn the_window_edge_is_inclusive() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("n", 1000);
        assert!(!d.allows("n", 1000 + ECHO_WINDOW_MS));
    }

    /// A different character in the same batch is never an echo (the IME
    /// committed something the key did not type).
    #[test]
    fn a_different_character_in_the_batch_writes() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("n", 1000);
        assert!(d.allows("i", 1001));
    }

    /// A composition re-arms nothing: after `disarm` (marked text set), the
    /// commit that ends it may reproduce the last raw character — dropping
    /// it would eat pinyin's first letter.
    #[test]
    fn a_commit_after_a_composition_is_never_dropped() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("n", 1000);
        d.disarm();
        assert!(d.allows("n", 1001));
        assert!(d.allows("你好", 1002));
    }

    /// The commit path's defensive check: a multi-char commit identical to a
    /// multi-char raw insert inside the window drops too (a snapshot that
    /// returns echo delivery to the IME channel keeps the fix).
    #[test]
    fn a_multi_char_echo_drops_like_a_single_char_one() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("nihao", 1000);
        assert!(!d.allows("nihao", 1010));
        assert!(d.allows("nihao", 1060));
    }

    /// Empty inserts note nothing and are never refused (they carry no
    /// character to double).
    #[test]
    fn empty_inserts_are_inert() {
        let mut d = ImeEchoDeduper::default();
        d.note_raw("", 1000);
        assert!(d.allows("", 1000));
        assert!(d.allows("", 1001));
    }
}
