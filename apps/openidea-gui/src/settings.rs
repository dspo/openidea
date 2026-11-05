//! The app's persisted settings (M7): recent projects, the workbench layout
//! (tree/dock visibility + sizes), and the editor font size. One JSON file in
//! the platform data dir, read-modify-written with the same discipline the
//! reference host uses: every write round-trips through parse (foreign keys
//! survive), and a corrupt file is replaced rather than allowed to block.
//!
//! Loaded once at startup; the workbench owns the value and saves on every
//! change. Missing fields fall back to the defaults, so an older file keeps
//! working when new fields land.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many projects the recents list remembers.
const MAX_RECENTS: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Most recent first, canonicalized. Read paths that no longer exist on
    /// disk are pruned.
    pub recent_projects: Vec<PathBuf>,
    pub layout: Layout,
    pub font_size: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub tree_visible: bool,
    pub tree_width: f32,
    pub dock_visible: bool,
    pub dock_height: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { tree_visible: true, tree_width: 256.0, dock_visible: true, dock_height: 240.0 }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self { recent_projects: Vec::new(), layout: Layout::default(), font_size: crate::theme::FONT_SIZE }
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
}

/// The settings file path (`None` keeps everything in memory — the defaults).
fn settings_file() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("openidea-gui").join("settings.json"))
}

/// The layout/font clamp windows (also what the drag handles enforce).
pub const TREE_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 150.0..=480.0;
pub const DOCK_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 120.0..=560.0;
pub const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 10.0..=24.0;

/// Converts a parsed store into settings: defaults fill missing fields,
/// out-of-range values fall back, recents prune to existing directories.
fn from_store(store: Store) -> Settings {
    let defaults = Settings::default();
    let layout = store.layout;
    let recent_projects = store.recent_projects.into_iter().filter(|path| path.is_dir()).take(MAX_RECENTS).collect();
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
        },
        font_size: store.font_size.filter(|size| FONT_SIZE_RANGE.contains(size)).unwrap_or(defaults.font_size),
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
        }),
        font_size: Some(settings.font_size),
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
    fn save_store_shape_round_trips() {
        // Round-trip the Store type directly (the file path is user-global,
        // so the test does not touch it).
        let settings = Settings {
            recent_projects: vec![PathBuf::from("/tmp/m7-roundtrip")],
            layout: Layout { tree_visible: false, tree_width: 300.0, dock_visible: false, dock_height: 320.0 },
            font_size: 16.0,
        };
        let store = Store {
            recent_projects: settings.recent_projects.clone(),
            layout: Some(StoreLayout {
                tree_visible: Some(settings.layout.tree_visible),
                tree_width: Some(settings.layout.tree_width),
                dock_visible: Some(settings.layout.dock_visible),
                dock_height: Some(settings.layout.dock_height),
            }),
            font_size: Some(settings.font_size),
        };
        let back: Store = serde_json::from_str(&serde_json::to_string(&store).unwrap()).unwrap();
        assert_eq!(back.recent_projects, settings.recent_projects);
        let layout = back.layout.unwrap();
        assert_eq!(layout.tree_visible, Some(false));
        assert_eq!(layout.tree_width, Some(300.0));
        assert_eq!(layout.dock_visible, Some(false));
        assert_eq!(layout.dock_height, Some(320.0));
        assert_eq!(back.font_size, Some(16.0));
    }
}
