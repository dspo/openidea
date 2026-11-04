//! User-visible text as plain constants (M1 has no text catalog / i18n; the
//! Tauri frontend's `text/en.json` + `zh.json` discipline starts with M2+).

pub const APP_TITLE: &str = "OpenIDEA — GPUI M1 workbench";
pub const TREE_HEADER: &str = "Project";
pub const NO_PROJECT: &str = "No project open";
pub const NO_FILE: &str = "Open a file to start editing (⌘O)";
pub const DIRTY_DOT: &str = "●";
pub const STALE_MARK: &str = " ⚠";
pub const SAVE_STATE_UNSAVED: &str = "unsaved changes";
pub const SAVE_STATE_SAVED: &str = "saved";
pub const SAVE_STATE_STALE: &str = "stale — changed on disk";
pub const SAVED_TOAST: &str = "Saved";
pub const LOADING_FAILED: &str = "Cannot open";
pub const SAVE_FAILED: &str = "Save failed";
// Language-server rows in the status bar.
pub const LS_READY: &str = "ready";
pub const LS_STARTING: &str = "starting…";
pub const LS_DOWN: &str = "down";
pub const LS_STOPPED: &str = "stopped";
// Go to File / Search Everywhere palette.
pub const PICKER_FILE_PLACEHOLDER: &str = "Go to file (empty lists recent files)";
pub const PICKER_EVERYWHERE_PLACEHOLDER: &str = "Search files, symbols and actions";
pub const PICKER_NO_MATCHES: &str = "No matches";
