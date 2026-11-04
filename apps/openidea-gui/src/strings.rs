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
pub const PICKER_RUN_PLACEHOLDER: &str = "Run configuration, or a shell command line";
pub const PICKER_NO_MATCHES: &str = "No matches";
// The Run dock.
pub const RUN_TITLE: &str = "Run";
pub const RUN_STARTING: &str = "starting…";
pub const RUN_RUNNING: &str = "running · ";
pub const RUN_EXIT_CODE: &str = "exit code";
pub const RUN_EXITED: &str = "exited";
pub const RUN_STOP: &str = "Stop";
pub const RUN_CLEAR: &str = "Clear";
pub const RUN_ALREADY_RUNNING: &str = "A run is already active — stop it first";
pub const RUN_LAUNCH_FAILED: &str = "Run failed";
// The Run picker's synthetic row for a one-off shell command line.
pub const RUN_SHELL_PREFIX: &str = "Run command: ";
// Git (M4).
pub const GIT_NOT_A_REPO: &str = "The project is not a git repository";
pub const DIFF_TITLE: &str = "Diff";
pub const DIFF_NO_CHANGES: &str = "No changes against HEAD";
pub const DIFF_FAILED: &str = "Diff failed";
pub const DIFF_TARGET_HINT: &str = "Select a changed file in the tree (or open one) to diff";
pub const COMMIT_TITLE: &str = "Commit";
pub const CANCEL: &str = "Cancel";
pub const COMMIT_MESSAGE_PLACEHOLDER: &str = "Commit message (⌘Enter commits)";
pub const COMMIT_BUTTON: &str = "Commit";
pub const COMMIT_EMPTY_MESSAGE: &str = "Enter a commit message first";
pub const COMMIT_NO_CHANGES: &str = "No changes to commit";
pub const COMMIT_TOAST: &str = "Committed";
pub const COMMIT_FAILED: &str = "Commit failed";
