//! The navigation habits (M9): the back/forward stacks behind ⌘[ / ⌘], the
//! recent-edit-locations list behind ⌘⇧E, the bookmarks behind F3 / ⌘F3, and
//! the ⌘L line-jump parsing. Pure state, no gpui handles, so the tests pin
//! the shapes headless; the workbench owns the values and feeds them at its
//! jump points.
//!
//! Entries are (path, line, UTF-16 column) — the IPC coordinate convention.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// One navigation location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: u32,
    pub col_utf16: u32,
}

impl Location {
    pub fn new(path: impl Into<PathBuf>, line: u32, col_utf16: u32) -> Self {
        Self { path: path.into(), line, col_utf16 }
    }
}

/// How many jumps the history remembers (the workbench's ⌘[ / ⌘] depth).
pub const NAV_CAP: usize = 100;

/// The classic linear history: a bounded back list, a forward list the next
/// push truncates, and `back`/`forward` swapping the current location in.
#[derive(Default)]
pub struct NavStack {
    back: Vec<Location>,
    forward: Vec<Location>,
}

impl NavStack {
    /// Records a departure point before a jump. Truncates the forward list
    /// (a new jump invalidates the redo half, like undo).
    pub fn push(&mut self, location: Location) {
        self.back.push(location);
        self.forward.clear();
        if self.back.len() > NAV_CAP {
            self.back.remove(0);
        }
    }

    /// ⌘[: the current location parks on the forward list, the newest back
    /// entry becomes the destination. `None` at the start of history.
    pub fn back(&mut self, current: Location) -> Option<Location> {
        let destination = self.back.pop()?;
        self.forward.push(current);
        Some(destination)
    }

    /// ⌘]: the mirror of [`Self::back`].
    pub fn forward(&mut self, current: Location) -> Option<Location> {
        let destination = self.forward.pop()?;
        self.back.push(current);
        Some(destination)
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

/// How many edit locations ⌘⇧E remembers.
pub const EDIT_LOCATIONS_CAP: usize = 30;

/// The recent edit locations (⌘⇧E): an MRU of where edits landed, one entry
/// per (path, line) — moving the caret inside a line refreshes it, a new line
/// pushes. Consecutive edits on one line collapse (the list stays readable).
#[derive(Default)]
pub struct EditLocations {
    entries: Vec<(Location, u64)>,
    tick: u64,
}

impl EditLocations {
    /// Records an edit location (newest first, bounded, one entry per
    /// (path, line)).
    pub fn push(&mut self, location: Location) {
        self.tick += 1;
        // A refresh moves the entry to the front — newest first, always.
        self.entries.retain(|(entry, _)| entry.path != location.path || entry.line != location.line);
        self.entries.insert(0, (location, self.tick));
        if self.entries.len() > EDIT_LOCATIONS_CAP {
            self.entries.pop();
        }
    }

    /// Newest first, deduplicated by (path, line).
    pub fn latest(&self) -> Vec<Location> {
        self.entries.iter().map(|(entry, _)| entry.clone()).collect()
    }
}

/// The bookmarks (F3 / ⌘F3): a set of (path, line), sorted per file. The
/// persistence shape is a plain map the settings store round-trips.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Bookmarks {
    by_path: BTreeMap<PathBuf, BTreeSet<u32>>,
}

impl Bookmarks {
    /// Toggles `line` in `path`; returns whether it is now bookmarked.
    pub fn toggle(&mut self, path: impl Into<PathBuf>, line: u32) -> bool {
        let entry = self.by_path.entry(path.into()).or_default();
        if entry.contains(&line) {
            entry.remove(&line);
            false
        } else {
            entry.insert(line);
            true
        }
    }

    /// All bookmarks, path by path (rows for the ⌘F3 list).
    pub fn all(&self) -> impl Iterator<Item = (&PathBuf, u32)> {
        self.by_path.iter().flat_map(|(path, lines)| lines.iter().map(move |line| (path, *line)))
    }

    /// The persistence shape: path -> ascending lines.
    pub fn to_map(&self) -> BTreeMap<String, Vec<u32>> {
        self.by_path.iter().map(|(path, lines)| (path.display().to_string(), lines.iter().copied().collect())).collect()
    }

    pub fn from_map(map: BTreeMap<String, Vec<u32>>) -> Self {
        Self {
            by_path: map.into_iter().map(|(path, lines)| (PathBuf::from(path), lines.into_iter().collect())).collect(),
        }
    }
}

/// ⌘L's input: `123` jumps to line 123, `123:7` also lands on column 7
/// (1-based line, 1-based column; line 0 or beyond the document clamps at the
/// caller). `None` for anything else.
pub fn parse_line_col(input: &str) -> Option<(u32, u32)> {
    let input = input.trim();
    let (line, col) = match input.split_once(':') {
        Some((line, col)) => (line, col),
        None => (input, ""),
    };
    let line: u32 = line.trim().parse().ok()?;
    let col: u32 = if col.is_empty() { 1 } else { col.trim().parse().ok()? };
    Some((line.saturating_sub(1), col.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(path: &str, line: u32) -> Location {
        Location::new(path, line, 0)
    }

    #[test]
    fn nav_back_forward_round_trips() {
        let mut nav = NavStack::default();
        nav.push(loc("/a", 1));
        nav.push(loc("/b", 2));
        assert!(!nav.can_forward());
        // From /c, back lands on /b and remembers /c for forward.
        let destination = nav.back(loc("/c", 3)).expect("back has history");
        assert_eq!(destination, loc("/b", 2));
        assert!(nav.can_forward());
        let destination = nav.back(loc("/b", 2)).expect("second back");
        assert_eq!(destination, loc("/a", 1));
        assert!(!nav.can_back(), "history start");
        let destination = nav.forward(loc("/a", 1)).expect("forward again");
        assert_eq!(destination, loc("/b", 2));
        let destination = nav.forward(loc("/b", 2)).expect("second forward");
        assert_eq!(destination, loc("/c", 3));
        assert!(!nav.can_forward());
    }

    #[test]
    fn a_new_push_truncates_the_forward_half() {
        let mut nav = NavStack::default();
        nav.push(loc("/a", 1));
        nav.push(loc("/b", 2));
        nav.back(loc("/c", 3)).expect("back");
        assert!(nav.can_forward());
        // A fresh jump from where back() landed: the departure records, the
        // forward half dies.
        nav.push(loc("/c", 3));
        assert!(!nav.can_forward(), "a fresh jump invalidates forward");
        assert_eq!(nav.back(loc("/d", 4)), Some(loc("/c", 3)));
        assert_eq!(nav.back(loc("/c", 3)), Some(loc("/a", 1)));
    }

    #[test]
    fn nav_history_is_bounded() {
        let mut nav = NavStack::default();
        for i in 0..(NAV_CAP + 10) {
            nav.push(loc(&format!("/{i}"), 0));
        }
        assert!(nav.back.len() <= NAV_CAP);
        // The oldest entries dropped, the newest survives.
        assert_eq!(nav.back(loc("now", 0)), Some(loc(&format!("/{}", NAV_CAP + 9), 0)));
    }

    #[test]
    fn edit_locations_are_mru_deduped_per_line() {
        let mut edits = EditLocations::default();
        edits.push(loc("/a", 1));
        edits.push(loc("/a", 2));
        edits.push(loc("/b", 5));
        // Same line again: refresh, no duplicate.
        edits.push(loc("/a", 1));
        let latest = edits.latest();
        assert_eq!(latest, vec![loc("/a", 1), loc("/b", 5), loc("/a", 2)]);
        // The cap holds.
        for i in 0..(EDIT_LOCATIONS_CAP + 5) {
            edits.push(loc(&format!("/p{i}"), i as u32));
        }
        assert!(edits.latest().len() <= EDIT_LOCATIONS_CAP);
    }

    #[test]
    fn bookmarks_toggle_and_round_trip() {
        let mut bookmarks = Bookmarks::default();
        assert!(bookmarks.toggle("/a.go", 3), "first toggle adds");
        assert_eq!(bookmarks.all().count(), 1);
        assert!(!bookmarks.toggle("/a.go", 3), "second toggle removes");
        assert_eq!(bookmarks.all().count(), 0);
        bookmarks.toggle("/a.go", 1);
        bookmarks.toggle("/b.go", 7);
        bookmarks.toggle("/a.go", 3);
        let all: Vec<String> = bookmarks.all().map(|(path, line)| format!("{}:{line}", path.display())).collect();
        assert_eq!(all, vec!["/a.go:1", "/a.go:3", "/b.go:7"]);
        let map = bookmarks.to_map();
        let back = Bookmarks::from_map(map);
        assert_eq!(back, bookmarks);
    }

    #[test]
    fn line_jump_parses_line_and_column() {
        assert_eq!(parse_line_col("12"), Some((11, 0)));
        assert_eq!(parse_line_col("12:5"), Some((11, 4)));
        assert_eq!(parse_line_col("  3 : 2 "), Some((2, 1)));
        assert_eq!(parse_line_col("0"), Some((0, 0)));
        assert_eq!(parse_line_col(""), None);
        assert_eq!(parse_line_col("x"), None);
        assert_eq!(parse_line_col("12:x"), None);
        // A trailing colon is tolerated: line 12, column 1.
        assert_eq!(parse_line_col("12:"), Some((11, 0)));
    }
}
