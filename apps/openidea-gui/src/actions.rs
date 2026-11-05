//! The action layer: every user-invocable behaviour is a gpui action with a
//! stable name, bound to keys in `main.rs`, and listed here once more as
//! [`REGISTRY`] rows (title + category + default keystroke) so Search
//! Everywhere can enumerate them and the keymap has one display source.
//!
//! Printable input is deliberately *not* an action: typed characters keep
//! flowing through the IME and the raw `on_key_down` fallback in the editor.

use gpui::actions;

actions!(
    workbench,
    [OpenFile, Save, CloseTab, Undo, Redo, Quit, Cancel, GoToFile, SearchEverywhere, Run, StopRun, ShowDiff, Commit,]
);

// Palette plumbing (⇧⌘O / double-⇧ popup): deliberately not in REGISTRY —
// these are the list's own keys, not user-invocable commands.
actions!(palette, [SelectPrev, SelectNext]);

actions!(
    editor,
    [
        TogglePerfHud,
        CursorLeft,
        CursorRight,
        CursorUp,
        CursorDown,
        CursorWordLeft,
        CursorWordRight,
        CursorLineHome,
        CursorLineEnd,
        CursorDocStart,
        CursorDocEnd,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectLineHome,
        SelectLineEnd,
        SelectDocStart,
        SelectDocEnd,
        DeleteBackward,
        DeleteForward,
        DeleteWordBackward,
        DeleteWordForward,
        Newline,
        TabIndent,
        OutdentLine,
        Paste,
        GotoDefinition,
        FindUsages,
        Rename,
        CodeActions,
        FormatDocument,
    ]
);

/// One row of the app's action registry (the Search Everywhere / keymap view
/// source of truth). `name` is the gpui action name; `keys` is the display
/// form of the default binding in `main.rs` (empty = no default).
// Consumed by Search Everywhere later in M3; until that phase lands the table
// is only read by the tests below.
#[allow(dead_code)]
pub struct ActionInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub keys: &'static str,
}

macro_rules! action_info {
    ($name:literal, $title:literal, $category:literal, $keys:literal) => {
        ActionInfo { name: $name, title: $title, category: $category, keys: $keys }
    };
}

// Consumed by Search Everywhere later in M3; until that phase lands the table
// is only read by the tests below.
#[allow(dead_code)]
pub const REGISTRY: &[ActionInfo] = &[
    action_info!("workbench::OpenFile", "Open File…", "File", "⌘O"),
    action_info!("workbench::Save", "Save", "File", "⌘S"),
    action_info!("workbench::CloseTab", "Close Tab", "File", "⌘W"),
    action_info!("workbench::GoToFile", "Go to File", "Navigate", "⇧⌘O"),
    action_info!("workbench::SearchEverywhere", "Search Everywhere", "Navigate", "Double ⇧"),
    action_info!("workbench::Run", "Run…", "Run", "⌃R"),
    action_info!("workbench::StopRun", "Stop Run", "Run", "⌃⌘R"),
    action_info!("workbench::ShowDiff", "Show Diff", "Git", "⌃D"),
    action_info!("workbench::Commit", "Commit…", "Git", "⌘K"),
    action_info!("workbench::Undo", "Undo", "Edit", "⌘Z"),
    action_info!("workbench::Redo", "Redo", "Edit", "⇧⌘Z"),
    action_info!("editor::Paste", "Paste", "Edit", "⌘V"),
    action_info!("editor::TabIndent", "Indent / Expand Template", "Edit", "⇥"),
    action_info!("editor::OutdentLine", "Outdent Line", "Edit", "⇧⇥"),
    action_info!("editor::GotoDefinition", "Go to Definition", "Navigate", "⌘B"),
    action_info!("editor::FindUsages", "Find Usages", "Navigate", "⌥F7"),
    action_info!("editor::Rename", "Rename…", "Refactor", "⇧F6"),
    action_info!("editor::CodeActions", "Quick Fix…", "Refactor", "⌥⏎"),
    action_info!("editor::FormatDocument", "Format Document", "Code", "⌘⌥L"),
    action_info!("editor::TogglePerfHud", "Toggle Perf HUD", "View", "F12"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_rows_are_unique_and_labelled() {
        let mut names: Vec<&str> = REGISTRY.iter().map(|row| row.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "action names must be unique");
        for row in REGISTRY {
            assert!(!row.title.is_empty());
            assert!(!row.keys.is_empty(), "{} should show its default binding", row.name);
            assert!(row.name.starts_with("workbench::") || row.name.starts_with("editor::"));
        }
    }
}
