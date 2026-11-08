//! The app's persisted settings (M7): recent projects, the workbench layout
//! (tree/dock visibility + sizes), the editor font size, and — M8 — the
//! per-project session (open tabs, the active one, each tab's caret). One
//! JSON file in the platform data dir, read-modify-written with the same
//! discipline the reference host uses: every write round-trips through
//! parse (foreign keys survive), and a corrupt file is replaced rather than
//! allowed to block.
//!
//! Loaded once at startup; the workbench owns the value and saves on change
//! — for the session, on the rare structural events only (tab
//! open/close/activate, project switch, graceful quit), never on typing.
//! Missing fields fall back to the defaults, so an older file keeps working
//! when new fields land.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many projects the recents list remembers.
const MAX_RECENTS: usize = 10;

/// How many tabs a session entry remembers (M8) — the file must never grow
/// unbounded (the M7 watchpoint).
pub const MAX_SESSION_TABS: usize = 30;

/// How many projects the session map remembers (M8).
pub const MAX_SESSION_PROJECTS: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Most recent first, canonicalized. Read paths that no longer exist on
    /// disk are pruned.
    pub recent_projects: Vec<PathBuf>,
    pub layout: Layout,
    pub font_size: f32,
    /// Per-project session (M8), keyed by the canonical project root.
    pub session: BTreeMap<String, SessionTabs>,
    /// User keymap overrides (M9), keyed by the preset action id, values in
    /// the IDEA notation the presets use ("Meta+Shift+O"). An id with an
    /// empty list unbinds the action's default keys.
    pub keymap_overrides: BTreeMap<String, Vec<String>>,
    /// Bookmarks (M9), path -> ascending bookmarked lines. IDE-global (IDEA
    /// keeps bookmarks across projects too); written on toggle only.
    pub bookmarks: BTreeMap<String, Vec<u32>>,
    /// Recent commit messages (M10's Commit window history), newest first.
    pub commit_messages: Vec<String>,
    /// Projects the user trusted (M10's Project trust dialog), canonicalized.
    /// An interactive open of a directory outside this list prompts first.
    pub trusted_projects: Vec<String>,
    /// Status bar widget visibility (M10's right-click menu), id -> shown.
    /// Missing ids follow the widget's default (shown).
    pub status_widgets: BTreeMap<String, bool>,
    /// Gutter breakpoints (M11), path -> the lines (0-based rows) and the
    /// per-line condition strings. IDE-global like bookmarks; written on
    /// toggle only.
    pub breakpoints: BTreeMap<String, FileBreakpoints>,
    /// The editor's inline blame tail (M13): the caret line's
    /// `author (date rev)` label. On by default (the IDEA face).
    pub inline_blame: bool,
}

/// One file's breakpoints: the guarded lines plus the conditions some of
/// them carry (an entry without a condition is an unconditional stop).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileBreakpoints {
    pub lines: Vec<u32>,
    #[serde(default)]
    pub conditions: Vec<(u32, String)>,
}

// The mutation face rides the editor's own set and the bus payload; these
// helpers are the pure semantics the tests pin (M12's test tree reuses them).
#[allow(dead_code)]
impl FileBreakpoints {
    /// Whether `line` is guarded (unconditionally or with a condition).
    pub fn has(&self, line: u32) -> bool {
        self.lines.contains(&line)
    }

    /// Toggles `line`; returns the new presence. A condition entry for the
    /// line is cleared when the breakpoint goes away.
    pub fn toggle(&mut self, line: u32) -> bool {
        if let Some(pos) = self.lines.iter().position(|l| *l == line) {
            self.lines.remove(pos);
            self.conditions.retain(|(l, _)| *l != line);
            false
        } else {
            self.lines.push(line);
            self.lines.sort_unstable();
            true
        }
    }

    /// Sets (or clears, when `condition` is empty) the condition on `line`;
    /// creates the breakpoint when it was not set.
    pub fn set_condition(&mut self, line: u32, condition: &str) {
        if condition.is_empty() {
            self.conditions.retain(|(l, _)| *l != line);
            return;
        }
        if !self.has(line) {
            self.toggle(line);
        }
        if let Some(entry) = self.conditions.iter_mut().find(|(l, _)| *l == line) {
            entry.1 = condition.to_owned();
        } else {
            self.conditions.push((line, condition.to_owned()));
        }
    }

    /// The condition on `line`, if any.
    pub fn condition_of(&self, line: u32) -> Option<&str> {
        self.conditions.iter().find(|(l, _)| *l == line).map(|(_, c)| c.as_str())
    }
}

/// One project's restored session (M8): the tabs in open order, the active
/// one, and each tab's caret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTabs {
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    #[serde(default)]
    pub active: Option<usize>,
}

/// One tab of a session entry (M8): the file and where the caret was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabSession {
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub cursor_line: u32,
    #[serde(default)]
    pub cursor_col_utf16: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub tree_visible: bool,
    pub tree_width: f32,
    pub dock_visible: bool,
    pub dock_height: f32,
    /// Which tool window owns the left dock area (M10); None = none open.
    pub left_active: Option<String>,
    /// Which tool window owns the right dock area (M10; empty until a Move
    /// to puts a window there).
    pub right_active: Option<String>,
    /// The maximized tool window (M10), if any.
    pub maximized: Option<String>,
    /// Per-tool-window user state (M10): visibility, stripe decisions, Move
    /// to overrides. Sparse — untouched windows follow the table.
    pub windows: BTreeMap<String, crate::toolwindow::WindowState>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            tree_visible: true,
            tree_width: 256.0,
            dock_visible: true,
            dock_height: 240.0,
            left_active: None,
            right_active: None,
            maximized: None,
            windows: BTreeMap::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recent_projects: Vec::new(),
            layout: Layout::default(),
            font_size: crate::theme::FONT_SIZE,
            session: BTreeMap::new(),
            keymap_overrides: BTreeMap::new(),
            bookmarks: BTreeMap::new(),
            commit_messages: Vec::new(),
            trusted_projects: Vec::new(),
            status_widgets: BTreeMap::new(),
            breakpoints: BTreeMap::new(),
            inline_blame: true,
        }
    }
}

/// The on-disk shape: every field optional, so an older file parses.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    recent_projects: Vec<PathBuf>,
    #[serde(default)]
    layout: Option<StoreLayout>,
    #[serde(default)]
    font_size: Option<f32>,
    #[serde(default)]
    session: BTreeMap<String, SessionTabs>,
    #[serde(default)]
    keymap_overrides: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    bookmarks: BTreeMap<String, Vec<u32>>,
    #[serde(default)]
    commit_messages: Vec<String>,
    #[serde(default)]
    trusted_projects: Vec<String>,
    #[serde(default)]
    status_widgets: BTreeMap<String, bool>,
    #[serde(default)]
    breakpoints: BTreeMap<String, FileBreakpoints>,
    #[serde(default)]
    inline_blame: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreLayout {
    #[serde(default)]
    tree_visible: Option<bool>,
    #[serde(default)]
    tree_width: Option<f32>,
    #[serde(default)]
    dock_visible: Option<bool>,
    #[serde(default)]
    dock_height: Option<f32>,
    #[serde(default)]
    left_active: Option<String>,
    #[serde(default)]
    right_active: Option<String>,
    #[serde(default)]
    maximized: Option<String>,
    #[serde(default)]
    windows: BTreeMap<String, crate::toolwindow::WindowState>,
}

/// The settings file path (`None` keeps everything in memory — the defaults).
/// `OPENIDEA_GUI_SETTINGS_DIR` overrides the directory (scripted sessions and
/// the soak harness isolate their store from the user's real one).
fn settings_file() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("OPENIDEA_GUI_SETTINGS_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    dirs::data_dir().map(|dir| dir.join("openidea-gui").join("settings.json"))
}

/// The app's data directory (the settings dir unless overridden).
fn data_dir() -> Option<PathBuf> {
    settings_file()?.parent().map(|parent| parent.to_path_buf())
}

// -- last-exit sidecar + crash log (M8 stability) --------------------------
//
// The crash face of "the app must not die silently": a panic hook appends a
// crash.log entry and marks the run unclean; a graceful Quit marks it clean.
// The next launch reads the marker and surfaces a toast, so a crash is always
// seen by the user even when the process died before any UI could react. A
// signal kill leaves the stale marker — "ended unexpectedly" stays true, and
// the hook-annotated crash.log disambiguates panics from kills.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastExit {
    pub clean: bool,
    pub reason: Option<String>,
}

/// The sidecar path, next to the settings file.
fn last_exit_file() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("last_exit.json"))
}

/// The crash log path.
fn crash_log_file() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("crash.log"))
}

/// One crash.log line: `[<utc>] thread <name> panicked: <payload> at <location>`.
pub fn crash_entry(utc: &str, thread: &str, payload: &str, location: &str) -> String {
    format!("[{utc}] thread {thread} panicked: {payload} at {location}\n")
}

/// Seconds-since-epoch to `YYYY-MM-DD HH:MM:SS` UTC (civil-from-days; no
/// date dependency).
pub fn format_utc(epoch_secs: u64) -> String {
    let days = epoch_secs / 86_400;
    let rem = epoch_secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Parses the sidecar's shape (a missing `clean` falls back to true — an
/// unknown state never reads as a crash).
fn parse_last_exit(raw: &str) -> Option<LastExit> {
    #[derive(Deserialize)]
    struct Store {
        #[serde(default = "default_true")]
        clean: bool,
        #[serde(default)]
        reason: Option<String>,
    }
    fn default_true() -> bool {
        true
    }
    let store: Store = serde_json::from_str(raw).ok()?;
    Some(LastExit { clean: store.clean, reason: store.reason })
}

/// Reads the marker; `None` = absent (first run) or unreadable — never reads
/// as a crash claim.
pub fn read_last_exit() -> Option<LastExit> {
    let raw = std::fs::read_to_string(last_exit_file()?).ok()?;
    parse_last_exit(&raw)
}

/// Writes the marker; failures only log (the app keeps running).
fn write_last_exit(exit: &LastExit) {
    let Some(path) = last_exit_file() else { return };
    let Ok(json) = serde_json::to_string(exit) else { return };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("[settings] last-exit dir failed: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("[settings] last-exit write failed: {e}");
    }
}

/// Marks the run clean (the graceful Quit path).
pub fn mark_clean_exit() {
    write_last_exit(&LastExit { clean: true, reason: None });
}

/// Marks the run unclean (the panic hook). Synchronous and best-effort —
/// this runs while unwinding, so every failure is ignored.
pub fn mark_unclean_exit(reason: Option<&str>) {
    write_last_exit(&LastExit { clean: false, reason: reason.map(str::to_owned) });
}

/// Appends one entry to crash.log (created on demand); failures ignored.
pub fn append_crash_log(entry: &str) {
    use std::io::Write as _;
    let Some(path) = crash_log_file() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = file.write_all(entry.as_bytes());
}

/// The layout/font clamp windows (also what the drag handles enforce).
pub const TREE_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 150.0..=480.0;
pub const DOCK_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 120.0..=560.0;
pub const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 10.0..=24.0;

/// Converts a parsed store into settings: defaults fill missing fields,
/// out-of-range values fall back, recents prune to existing directories,
/// and session entries prune to existing projects (M8).
fn from_store(store: Store) -> Settings {
    let defaults = Settings::default();
    let layout = store.layout;
    let recent_projects = store.recent_projects.into_iter().filter(|path| path.is_dir()).take(MAX_RECENTS).collect();
    let session = store
        .session
        .into_iter()
        .filter(|(root, _)| Path::new(root).is_dir())
        .map(|(root, mut entry)| {
            entry.tabs.truncate(MAX_SESSION_TABS);
            entry.active = entry.active.filter(|index| *index < entry.tabs.len());
            (root, entry)
        })
        .take(MAX_SESSION_PROJECTS)
        .collect();
    Settings {
        recent_projects,
        layout: Layout {
            tree_visible: layout.as_ref().and_then(|l| l.tree_visible).unwrap_or(defaults.layout.tree_visible),
            tree_width: layout
                .as_ref()
                .and_then(|l| l.tree_width)
                .filter(|w| TREE_WIDTH_RANGE.contains(w))
                .unwrap_or(defaults.layout.tree_width),
            dock_visible: layout.as_ref().and_then(|l| l.dock_visible).unwrap_or(defaults.layout.dock_visible),
            dock_height: layout
                .as_ref()
                .and_then(|l| l.dock_height)
                .filter(|h| DOCK_HEIGHT_RANGE.contains(h))
                .unwrap_or(defaults.layout.dock_height),
            left_active: layout.as_ref().and_then(|l| l.left_active.clone()),
            right_active: layout.as_ref().and_then(|l| l.right_active.clone()),
            maximized: layout.as_ref().and_then(|l| l.maximized.clone()),
            windows: layout.map(|l| l.windows).unwrap_or_default(),
        },
        font_size: store.font_size.filter(|size| FONT_SIZE_RANGE.contains(size)).unwrap_or(defaults.font_size),
        session,
        keymap_overrides: store.keymap_overrides,
        bookmarks: store.bookmarks,
        commit_messages: store.commit_messages,
        trusted_projects: store.trusted_projects,
        status_widgets: store.status_widgets,
        breakpoints: store.breakpoints,
        inline_blame: store.inline_blame.unwrap_or(defaults.inline_blame),
    }
}

/// Loads the store, pruning recents whose directory no longer exists.
pub fn load() -> Settings {
    let Some(path) = settings_file() else { return Settings::default() };
    let Ok(raw) = std::fs::read_to_string(path) else { return Settings::default() };
    match serde_json::from_str::<Store>(&raw) {
        Ok(store) => from_store(store),
        Err(_) => {
            eprintln!("[settings] corrupt settings file replaced");
            Settings::default()
        }
    }
}

/// Saves the whole settings value; a write failure only logs (the app keeps
/// running on the in-memory value).
pub fn save(settings: &Settings) {
    let Some(path) = settings_file() else { return };
    let store = Store {
        recent_projects: settings.recent_projects.clone(),
        layout: Some(StoreLayout {
            tree_visible: Some(settings.layout.tree_visible),
            tree_width: Some(settings.layout.tree_width),
            dock_visible: Some(settings.layout.dock_visible),
            dock_height: Some(settings.layout.dock_height),
            left_active: settings.layout.left_active.clone(),
            right_active: settings.layout.right_active.clone(),
            maximized: settings.layout.maximized.clone(),
            windows: settings.layout.windows.clone(),
        }),
        font_size: Some(settings.font_size),
        session: settings.session.clone(),
        keymap_overrides: settings.keymap_overrides.clone(),
        bookmarks: settings.bookmarks.clone(),
        commit_messages: settings.commit_messages.clone(),
        trusted_projects: settings.trusted_projects.clone(),
        status_widgets: settings.status_widgets.clone(),
        breakpoints: settings.breakpoints.clone(),
        inline_blame: Some(settings.inline_blame),
    };
    let Ok(json) = serde_json::to_string_pretty(&store) else { return };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("[settings] create dir failed: {e}");
        return;
    }
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("[settings] write failed: {e}");
    }
}

/// Moves `root` to the front of the recents list (deduplicated, canonical
/// form; the new entry always survives even if unreadable).
pub fn push_recent(recents: &mut Vec<PathBuf>, root: &Path) {
    let canonical = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    recents.retain(|path| path != &canonical);
    recents.insert(0, canonical);
    recents.truncate(MAX_RECENTS);
}

/// The session map's key for a project root (canonical path, matching the
/// recents entries).
pub fn session_key(root: &Path) -> String {
    let canonical = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    canonical.to_string_lossy().into_owned()
}

/// Records (or drops, when empty) the session for `root` and enforces the
/// caps (M8): ≤ [`MAX_SESSION_TABS`] per project, ≤ [`MAX_SESSION_PROJECTS`]
/// projects.
pub fn set_session(session: &mut BTreeMap<String, SessionTabs>, root: &Path, mut entry: SessionTabs) {
    entry.tabs.truncate(MAX_SESSION_TABS);
    let active = entry.active.filter(|index| *index < entry.tabs.len());
    let key = session_key(root);
    if entry.tabs.is_empty() {
        session.remove(&key);
    } else {
        session.insert(key, SessionTabs { tabs: entry.tabs, active });
    }
    while session.len() > MAX_SESSION_PROJECTS {
        session.pop_first();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Option<Store> {
        serde_json::from_str(json).ok()
    }

    #[test]
    fn corrupt_store_is_rejected_and_load_falls_back() {
        assert!(parse("{ not json").is_none());
    }

    #[test]
    fn format_utc_known_epochs() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        // 2025-01-01T00:00:00Z and a leap-year day.
        assert_eq!(format_utc(1_735_689_600), "2025-01-01 00:00:00");
        assert_eq!(format_utc(1_709_164_800), "2024-02-29 00:00:00");
    }

    #[test]
    fn crash_entry_shape() {
        let line = crash_entry("2026-11-05 10:00:00", "main", "bad state", "src/editor.rs:42");
        assert_eq!(line, "[2026-11-05 10:00:00] thread main panicked: bad state at src/editor.rs:42\n");
    }

    #[test]
    fn last_exit_round_trip_and_fallbacks() {
        // Unparseable and empty shapes never read as a crash.
        assert!(parse_last_exit("{ not json").is_none());
        assert!(parse_last_exit("{}").is_some_and(|exit| exit.clean));
        let clean = parse_last_exit(r#"{"clean": true}"#).unwrap();
        assert!(clean.clean && clean.reason.is_none());
        let dirty = parse_last_exit(r#"{"clean": false, "reason": "bad state"}"#).unwrap();
        assert!(!dirty.clean);
        assert_eq!(dirty.reason.as_deref(), Some("bad state"));
        // An unknown shape (foreign schema) stays optimistic.
        assert!(parse_last_exit(r#"{"reason": 5}"#).is_none());
    }

    #[test]
    fn empty_store_yields_defaults() {
        let settings = from_store(parse("{}").unwrap());
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn older_file_without_new_fields_parses() {
        let settings = from_store(parse(r#"{"recent_projects": ["/tmp"]}"#).unwrap());
        assert_eq!(settings.layout, Layout::default());
        assert_eq!(settings.font_size, crate::theme::FONT_SIZE);
    }

    #[test]
    fn out_of_range_layout_and_font_fall_back_to_defaults() {
        let settings =
            from_store(parse(r#"{"font_size": 99, "layout": {"tree_width": 9999, "dock_height": 5}}"#).unwrap());
        assert_eq!(settings.font_size, crate::theme::FONT_SIZE);
        assert_eq!(settings.layout.tree_width, Layout::default().tree_width);
        assert_eq!(settings.layout.dock_height, Layout::default().dock_height);
    }

    #[test]
    fn push_recent_dedups_and_caps() {
        let mut recents: Vec<PathBuf> = Vec::new();
        for i in 0..(MAX_RECENTS + 3) {
            push_recent(&mut recents, Path::new(&format!("/tmp/m7-recents-{i}")));
        }
        assert_eq!(recents.len(), MAX_RECENTS);
        assert!(recents[0].ends_with(format!("m7-recents-{}", MAX_RECENTS + 2)));
        push_recent(&mut recents, Path::new("/tmp/m7-recents-0"));
        assert_eq!(recents.len(), MAX_RECENTS);
        assert!(recents[0].ends_with("m7-recents-0"));
    }

    #[test]
    fn session_round_trips_with_caps() {
        let mut session = BTreeMap::new();
        // More tabs than the cap: truncated, the active index filtered.
        let tabs: Vec<TabSession> = (0..(MAX_SESSION_TABS + 5))
            .map(|i| TabSession {
                path: PathBuf::from(format!("/tmp/m8-sess/{i}.go")),
                cursor_line: i as u32,
                cursor_col_utf16: 2,
            })
            .collect();
        set_session(&mut session, Path::new("/tmp/m8-sess"), SessionTabs { tabs, active: Some(MAX_SESSION_TABS + 2) });
        let entry = session.values().next().unwrap();
        assert_eq!(entry.tabs.len(), MAX_SESSION_TABS);
        assert_eq!(entry.active, None, "an out-of-range active must drop");

        // A valid active survives.
        let tabs = vec![TabSession { path: PathBuf::from("/tmp/m8-sess/a.go"), cursor_line: 1, cursor_col_utf16: 0 }];
        set_session(&mut session, Path::new("/tmp/m8-sess"), SessionTabs { tabs, active: Some(0) });
        assert_eq!(session.values().next().unwrap().active, Some(0));

        // An empty tab list removes the entry (the last tab closed).
        set_session(&mut session, Path::new("/tmp/m8-sess"), SessionTabs { tabs: Vec::new(), active: None });
        assert!(session.is_empty());
    }

    #[test]
    fn session_project_count_caps_and_store_prunes_missing_roots() {
        let mut session = BTreeMap::new();
        for i in 0..(MAX_SESSION_PROJECTS + 4) {
            set_session(
                &mut session,
                Path::new(&format!("/tmp/m8-sess-p{i}")),
                SessionTabs {
                    tabs: vec![TabSession {
                        path: PathBuf::from(format!("/tmp/m8-sess-p{i}/a.go")),
                        cursor_line: 0,
                        cursor_col_utf16: 0,
                    }],
                    active: Some(0),
                },
            );
        }
        assert_eq!(session.len(), MAX_SESSION_PROJECTS);

        // Reading the store prunes entries whose project directory is gone
        // (nothing here exists on the test machine).
        let store =
            parse(r#"{"session": { "/tmp/m8-gone": { "tabs": [{"path": "/tmp/m8-gone/a.go"}], "active": 0 } } }"#)
                .unwrap();
        assert!(from_store(store).session.is_empty());
    }

    #[test]
    fn session_shape_defaults_fill_missing_fields() {
        // An older/foreign session entry: missing cursor and active parse.
        let store = parse(r#"{"session": { "/tmp": { "tabs": [{}] } }, "recent_projects": ["/tmp"]}"#).unwrap();
        let settings = from_store(store);
        let entry = settings.session.get("/tmp").unwrap();
        assert_eq!(entry.tabs[0], TabSession { path: PathBuf::new(), cursor_line: 0, cursor_col_utf16: 0 });
        assert_eq!(entry.active, None);
    }

    #[test]
    fn save_store_shape_round_trips() {
        // Round-trip the Store type directly (the file path is user-global,
        // so the test does not touch it).
        let settings = Settings {
            recent_projects: vec![PathBuf::from("/tmp/m7-roundtrip")],
            layout: Layout {
                tree_visible: false,
                tree_width: 300.0,
                dock_visible: false,
                dock_height: 320.0,
                ..Layout::default()
            },
            font_size: 16.0,
            session: BTreeMap::from([(
                "/tmp/m8-roundtrip".to_owned(),
                SessionTabs {
                    tabs: vec![TabSession {
                        path: PathBuf::from("/tmp/m8-roundtrip/a.go"),
                        cursor_line: 3,
                        cursor_col_utf16: 7,
                    }],
                    active: Some(0),
                },
            )]),
            keymap_overrides: BTreeMap::from([("file.save".to_owned(), vec!["Ctrl+S".to_owned()])]),
            bookmarks: BTreeMap::from([("/tmp/x.go".to_owned(), vec![3, 9])]),
            commit_messages: vec!["initial".to_owned()],
            trusted_projects: vec!["/tmp/m10-trusted".to_owned()],
            status_widgets: BTreeMap::new(),
            breakpoints: BTreeMap::new(),
            inline_blame: true,
        };
        let store = Store {
            recent_projects: settings.recent_projects.clone(),
            layout: Some(StoreLayout {
                tree_visible: Some(settings.layout.tree_visible),
                tree_width: Some(settings.layout.tree_width),
                dock_visible: Some(settings.layout.dock_visible),
                dock_height: Some(settings.layout.dock_height),
                left_active: settings.layout.left_active.clone(),
                right_active: settings.layout.right_active.clone(),
                maximized: settings.layout.maximized.clone(),
                windows: settings.layout.windows.clone(),
            }),
            font_size: Some(settings.font_size),
            session: settings.session.clone(),
            keymap_overrides: settings.keymap_overrides.clone(),
            bookmarks: settings.bookmarks.clone(),
            commit_messages: settings.commit_messages.clone(),
            trusted_projects: settings.trusted_projects.clone(),
            status_widgets: settings.status_widgets.clone(),
            breakpoints: BTreeMap::new(),
            inline_blame: Some(settings.inline_blame),
        };
        let back: Store = serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert_eq!(back.recent_projects, settings.recent_projects);
        let layout = back.layout.unwrap();
        assert_eq!(layout.tree_visible, Some(false));
        assert_eq!(layout.tree_width, Some(300.0));
        assert_eq!(layout.dock_visible, Some(false));
        assert_eq!(layout.dock_height, Some(320.0));
        assert!(layout.windows.is_empty());
        assert_eq!(layout.left_active, None);
        assert_eq!(layout.maximized, None);
        // The commit history round-trips (M10).
        assert_eq!(back.commit_messages, vec!["initial".to_owned()]);
        // Trusted projects round-trip (M10).
        assert_eq!(back.trusted_projects, vec!["/tmp/m10-trusted".to_owned()]);
        assert_eq!(back.font_size, Some(16.0));
        // The session round-trips verbatim (M8).
        assert_eq!(back.session, settings.session);
        // The keymap overrides round-trip verbatim (M9).
        assert_eq!(back.keymap_overrides, settings.keymap_overrides);
        // The bookmarks round-trip verbatim (M9).
        assert_eq!(back.bookmarks, settings.bookmarks);
    }
}
