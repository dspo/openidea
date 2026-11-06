//! User-visible text as plain constants (M1 has no text catalog / i18n; the
//! Tauri frontend's `text/en.json` + `zh.json` discipline starts with M2+).

pub const APP_TITLE: &str = "OpenIDEA";
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
pub const RUN_LAUNCH_FAILED: &str = "Run failed";
// The Run picker's synthetic row for a one-off shell command line.
pub const RUN_SHELL_PREFIX: &str = "Run command: ";
// Git (M4).
pub const GIT_NOT_A_REPO: &str = "The project is not a git repository";
pub const GIT_BRANCH_LABEL: &str = "on";
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
// Quick fixes / format (M5).
pub const CODE_ACTION_PREFERRED: &str = "preferred";
pub const NO_CODE_ACTIONS: &str = "No code actions here";
pub const CODE_ACTIONS_UNAVAILABLE: &str = "Code actions unavailable";
pub const CODE_ACTION_FAILED: &str = "Code action failed";
pub const FORMAT_UNAVAILABLE: &str = "Format unavailable";
pub const ALREADY_FORMATTED: &str = "Already formatted";
// Organize imports (M6).
pub const ORGANIZED_IMPORTS: &str = "Organized imports";
pub const ORGANIZE_NO_ACTIONS: &str = "Nothing to organize";
// In-file find/replace (M5).
pub const FIND_PLACEHOLDER: &str = "Find (⏎ next, ⇧⏎ previous)";
pub const FIND_NO_MATCHES: &str = "No matches";
pub const FIND_HINT: &str = "Type to find";
pub const FIND_CASE: &str = "Aa";
pub const REPLACE_PLACEHOLDER: &str = "Replace";
pub const REPLACE_BUTTON: &str = "Replace";
pub const REPLACE_ALL_BUTTON: &str = "All";
// Find in Files (M5).
pub const FIND_FILES_TITLE: &str = "Find in Files";
pub const FIND_FILES_PLACEHOLDER: &str = "Search project (⏎)";
pub const FIND_SEARCHING: &str = "searching…";
// Welcome / project picker (M7).
pub const WELCOME_TITLE: &str = "OpenIDEA";
pub const WELCOME_SUBTITLE: &str = "Open a folder to start editing";
pub const OPEN_FOLDER: &str = "Open Folder…";
pub const WELCOME_RECENTS: &str = "Recent projects";
pub const OPEN_PROJECT_FAILED: &str = "Cannot open";
// Tab management (M7).
pub const TAB_PIN: &str = "Pin Tab";
pub const TAB_UNPIN: &str = "Unpin Tab";
pub const TAB_CLOSE: &str = "Close Tab";
pub const TAB_REOPEN: &str = "Reopen Closed Tab";
// Tree context menu / mutations (M7).
pub const TREE_NEW_FILE: &str = "New File…";
pub const TREE_NEW_FOLDER: &str = "New Folder…";
pub const TREE_RENAME: &str = "Rename…";
pub const TREE_DELETE: &str = "Delete…";
pub const PROMPT_CONFIRM_HINT: &str = "⏎ confirm · Esc cancel";
pub const DELETE_TITLE: &str = "Delete";
pub const DELETE_BUTTON: &str = "Delete";
pub const FS_CREATE_FAILED: &str = "Cannot create";
pub const FS_RENAME_FAILED: &str = "Cannot rename";
pub const FS_DELETE_FAILED: &str = "Delete failed";
pub const DELETE_FILE_MESSAGE: &str = "Delete";
pub const DELETE_FOLDER_MESSAGE: &str = "Delete";
pub const DELETE_AND_CONTENTS: &str = "and everything inside it? The folder moves to the Trash.";
// Settings overlay (M7).
pub const SETTINGS_TITLE: &str = "Settings";
pub const SETTINGS_FONT: &str = "Editor font size";
pub const SETTINGS_INDENT: &str = "Indent";
pub const SETTINGS_INDENT_DEFAULT: &str = "Follows the language defaults";
pub const SETTINGS_KEYMAP: &str = "Keymap";
// Stability (M8): the crash toast on the launch after an unclean exit.
pub const LAST_RUN_UNCLEAN: &str = "The previous session ended unexpectedly";

// M9 navigation habits.
pub const NO_ERRORS: &str = "No errors to navigate to";
pub const NO_HISTORY_BACK: &str = "No earlier location";
pub const NO_HISTORY_FORWARD: &str = "No later location";
pub const BOOKMARK_ADDED: &str = "Bookmark added:";
pub const BOOKMARK_REMOVED: &str = "Bookmark removed:";
pub const NO_EDIT_LOCATION: &str = "No edit location yet";
pub const NOT_YET_AVAILABLE: &str = "{} is not yet available";

// M10 tool-window stripes.
pub const TW_MOVE_TO: &str = "Move to";
pub const TW_MOVE_LEFT: &str = "Left";
pub const TW_MOVE_RIGHT: &str = "Right";
pub const TW_MOVE_BOTTOM: &str = "Bottom";
pub const TW_MAXIMIZE: &str = "Maximize";
pub const TW_HIDE: &str = "Hide";
pub const TW_NO_STRUCTURE: &str = "Open a file to see its structure";
pub const TW_NO_BOOKMARKS: &str = "No bookmarks yet — F3 bookmarks a line";
pub const TW_NO_PROBLEMS: &str = "No problems";
pub const TW_NOTHING_RUN: &str = "No run or search yet — ⌃R runs, ⇧⌘F finds";
pub const TW_NOTHING_COMMIT: &str = "No changes to commit";
pub const TW_REFRESH_CHANGES: &str = "Refresh Changes";
pub const TW_COMMIT_HISTORY: &str = "History…";
pub const TW_COMMIT_DIFF_HINT: &str = "Select a file to preview its diff";

// M10 status bar widgets.
pub const STATUS_WIDGETS_MENU: &str = "Status Bar Widgets";
pub const W_PROJECT: &str = "Project";
pub const W_BRANCH: &str = "VCS Branch";
pub const W_POSITION: &str = "Line:Column";
pub const W_PROBLEMS: &str = "Problems Count";
pub const W_ENCODING: &str = "Encoding";
pub const W_LINESEP: &str = "Line Separator";
pub const W_SERVERS: &str = "Language Servers";
pub const STATUS_ENCODING: &str = "UTF-8";
pub const STATUS_LINESEP: &str = "LF";
pub const NO_PROBLEMS_SHORT: &str = "no problems";

// M10 project trust.
pub const TRUST_TITLE: &str = "Trust and Open Project?";
pub const TRUST_HINT: &str = "Projects in the trusted list open without this prompt.";
pub const TRUST_ACCEPT: &str = "Trust Project";
pub const TRUST_DECLINE: &str = "Don't Open";
