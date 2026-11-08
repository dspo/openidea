//! The blame faces (M13): the inline tail label and the annotation
//! swimlane's data — pure formatting and age mapping over upstream's
//! `BlameLine` rows (one per line of the current text; uncommitted lines
//! carry an all-zero commit id and the "Not Committed Yet" author).

use gpui::{Hsla, rgba};
use ide_vcs::BlameLine;

/// The swimlane's width (author + short date + a slice of the summary).
pub const LANE_W: f32 = 210.0;

/// The all-zero commit id ide-vcs reports for not-yet-committed lines.
const UNCOMMITTED: &str = "0000000000000000000000000000000000000000";

/// The short form of a commit id (the 7-char git habit).
pub fn short(id: &str) -> &str {
    &id[..id.len().min(7)]
}

/// The commit age bands the swimlane colors by (IDEA's annotation face:
/// recent lines warm, old lines fade toward the muted gray).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Under a day.
    Fresh,
    /// Under a week.
    Recent,
    /// Under a month.
    ThisMonth,
    /// Under half a year.
    HalfYear,
    /// Older.
    Ancient,
}

/// The band of `time_ms` seen from `now_ms` (both epoch milliseconds).
pub fn age_of(time_ms: f64, now_ms: f64) -> Age {
    let days = (now_ms - time_ms) / 86_400_000.0;
    if days < 1.0 {
        Age::Fresh
    } else if days < 7.0 {
        Age::Recent
    } else if days < 30.0 {
        Age::ThisMonth
    } else if days < 182.0 {
        Age::HalfYear
    } else {
        Age::Ancient
    }
}

/// The band's color: the accent for fresh work, fading through the palette's
/// warm tones into the muted gray (the low-contrast dark's family).
pub fn age_color(age: Age) -> Hsla {
    let c: u32 = match age {
        Age::Fresh => 0x74a3f0,
        Age::Recent => 0x6fbf7f,
        Age::ThisMonth => 0xd6a55f,
        Age::HalfYear => 0xa6abb5,
        Age::Ancient => 0x8c8f95,
    };
    rgba((c << 8) | 0xff).into()
}

/// Whether the entry is an uncommitted line.
pub fn uncommitted(entry: &BlameLine) -> bool {
    entry.commit == UNCOMMITTED
}

/// The editor's inline tail label: `author (date rev)`, the IDEA face.
pub fn inline_label(entry: &BlameLine) -> String {
    if uncommitted(entry) {
        return entry.author.clone();
    }
    format!("{} ({} {})", entry.author, crate::git_graph::format_date_ms(entry.time_ms), short(&entry.commit))
}

/// The hover detail: the commit summary under the inline label's identity.
pub fn hover_detail(entry: &BlameLine) -> String {
    format!("{} · {}", inline_label(entry), entry.summary)
}

/// The swimlane's label for one line: `author MM-DD summary`, truncated to
/// `max_chars` (the lane's character budget; the paint cannot clip text).
pub fn lane_label(entry: &BlameLine, max_chars: usize) -> String {
    if uncommitted(entry) {
        return entry.author.chars().take(max_chars).collect();
    }
    // The date's MM-DD face (format_date_ms yields `yyyy-MM-dd HH:mm`).
    let date = &crate::git_graph::format_date_ms(entry.time_ms)[5..10];
    let base = format!("{} {} ", entry.author, date);
    let room = max_chars.saturating_sub(base.len());
    if entry.summary.chars().count() <= room {
        format!("{base}{}", entry.summary)
    } else if room > 1 {
        let cut: String = entry.summary.chars().take(room - 1).collect();
        format!("{base}{cut}…")
    } else {
        base.trim_end().to_owned()
    }
}

/// The entry for buffer `line` (0-based), or `None` past the blame's rows
/// (the text grew since the blame was taken — the lane just comes up short).
pub fn entry_for(entries: &[BlameLine], line: u32) -> Option<&BlameLine> {
    entries.get(line as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(commit: &str, author: &str, time_ms: f64, summary: &str) -> BlameLine {
        BlameLine { commit: commit.into(), author: author.into(), time_ms, summary: summary.into() }
    }

    const NOW: f64 = 1_731_086_400_000.0; // 2024-11-08 16:00 UTC

    #[test]
    fn ages_band_by_recency() {
        assert_eq!(age_of(NOW - 3600.0 * 1000.0, NOW), Age::Fresh);
        assert_eq!(age_of(NOW - 3.0 * 86_400_000.0, NOW), Age::Recent);
        assert_eq!(age_of(NOW - 14.0 * 86_400_000.0, NOW), Age::ThisMonth);
        assert_eq!(age_of(NOW - 90.0 * 86_400_000.0, NOW), Age::HalfYear);
        assert_eq!(age_of(NOW - 400.0 * 86_400_000.0, NOW), Age::Ancient);
        // A future time (clock skew) is as fresh as it gets.
        assert_eq!(age_of(NOW + 1000.0, NOW), Age::Fresh);
    }

    #[test]
    fn the_inline_label_carries_author_date_and_rev() {
        let e = entry("60022b55abc", "Ada", 1_731_000_000_000.0, "feature: farewell");
        assert_eq!(inline_label(&e), "Ada (2024-11-07 17:20 60022b5)");
        assert_eq!(short(&e.commit), "60022b5");
    }

    #[test]
    fn uncommitted_lines_show_only_the_author_face() {
        let e = entry(UNCOMMITTED, "Not Committed Yet", 0.0, "");
        assert!(uncommitted(&e));
        assert_eq!(inline_label(&e), "Not Committed Yet");
        assert_eq!(lane_label(&e, 28), "Not Committed Yet");
    }

    #[test]
    fn the_lane_label_is_author_date_summary_truncated_to_the_budget() {
        let e = entry("60022b55abc", "Ada", 1_731_000_000_000.0, "feature: farewell plus the greet rename");
        assert_eq!(lane_label(&e, 28), "Ada 11-07 feature: farewell…");
        assert_eq!(lane_label(&e, 64), "Ada 11-07 feature: farewell plus the greet rename");
        // A budget tighter than the identity keeps the identity + date.
        assert_eq!(lane_label(&e, 6), "Ada 11-07");
    }

    #[test]
    fn entry_lookup_is_indexed_by_line_and_bounds_checked() {
        let entries = [entry("a1", "A", 0.0, "one"), entry("b2", "B", 0.0, "two")];
        assert_eq!(entry_for(&entries, 0).unwrap().author, "A");
        assert_eq!(entry_for(&entries, 1).unwrap().author, "B");
        assert!(entry_for(&entries, 2).is_none(), "lines past the blame show nothing");
    }

    #[test]
    fn the_hover_detail_joins_identity_and_summary() {
        let e = entry("60022b55abc", "Ada", 1_731_000_000_000.0, "feature: farewell");
        assert_eq!(hover_detail(&e), "Ada (2024-11-07 17:20 60022b5) · feature: farewell");
    }
}
