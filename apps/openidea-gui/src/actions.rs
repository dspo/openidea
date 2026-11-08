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
    [
        OpenFile,
        Save,
        CloseTab,
        Undo,
        Redo,
        Quit,
        Cancel,
        GoToFile,
        SearchEverywhere,
        Run,
        StopRun,
        ShowDiff,
        Commit,
        FindInFiles,
        ToggleTree,
        ToggleDock,
        ReopenClosedTab,
        OpenSettings,
        // M10 tool-window stripes: the lit windows' slot keys (⌘2/3/6/7 join
        // ⌘1/⌘4), plus the window-manager verbs the header menus share.
        ToggleBookmarks,
        ToggleFind,
        ToggleProblems,
        ToggleStructure,
        HideActiveToolWindow,
        MaximizeToolWindow,
        ToggleCommit,
        // M11: the integrated terminal window (the preset's Alt+F12 row).
        ToggleTerminal,
        // A second terminal session (⌃⌘T): no preset row — IDEA reaches the
        // + button with the mouse; the keymap face exists for scripted
        // verification and discoverability (the ⌃⌘R precedent).
        NewTerminalTab,
        // M11: the debugger — the preset's run.debug row (⌃D, the M4 note's
        // deferred retarget) and the stepping trio the preset pins to F7/F8/F9.
        Debug,
        // M12: the duplicate-code inspection (Code menu; the Duplicates
        // window earns its OnUse stripe at the first run).
        AnalyzeDuplicates,
        // M13: the Git window (⌘9, the preset's toolwindow.git slot) — the
        // log graph's home; the branches popup (the preset's vcs.newBranch
        // ⌘⌥N row arms its name input).
        ToggleGit,
        Branches,
        NewBranch,
        // M13 blame: the annotation swimlane (the preset's vcs.annotate,
        // whose row carries no keys — the app-added ⌃⌘A rides) and the
        // inline tail's toggle (no preset row; ⌃⌘B app-added).
        Annotate,
        ToggleInlineBlame,
        DebugStepInto,
        DebugStepOver,
        DebugResume,
        TrustProject,
        DontOpenProject,
        // M9 navigation habits.
        RecentFiles,
        RecentLocations,
        NavigateBack,
        NavigateForward,
        NextError,
        PrevError,
        ToggleBookmark,
        ShowBookmarks,
        FileStructure,
        Switcher,
        SwitcherBack,
        SwitcherNext,
        SwitcherPrev,
        GotoLine,
        LastEditLocation,
        FindAction,
    ]
);

// Palette plumbing (⇧⌘O / double-⇧ popup): deliberately not in REGISTRY —
// these are the list's own keys, not user-invocable commands.
actions!(palette, [SelectPrev, SelectNext]);

// The find bar's own keys (⌥⌘F reveals the replace row): user-invocable only
// while the bar is up, so the bar's buttons stay the discoverable face.
actions!(editor, [ToggleReplace]);

actions!(
    editor,
    [
        // M11: gutter breakpoints (the preset's debug.toggleBreakpoint ⌘F8
        // and the condition editor on ⌥⌘F8 — IDEA has no preset row for the
        // condition face; the keymap row exists for discoverability).
        ToggleBreakpoint,
        BreakpointCondition,
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
        OrganizeImports,
        ToggleInlayHints,
        Find,
        // M9 edit habits (the IDEA 编辑段).
        DuplicateLine,
        CommentLine,
        CommentBlock,
        ExtendSelection,
        ShrinkSelection,
        MoveLineUp,
        MoveLineDown,
        MoveStatementUp,
        MoveStatementDown,
        DeleteLine,
        FindNext,
        FindPrevious,
        Replace,
        ScrollCenter,
        Fold,
        Unfold,
        FoldAll,
        UnfoldAll,
        StartNewLineBelow,
        StartNewLineAbove,
        JoinLines,
        ToggleCase,
        Complete,
        CloneCaretAbove,
        CloneCaretBelow,
        ParameterInfo,
        GotoImplementation,
        GotoTypeDeclaration,
        // M13 conflicts: the block walk over the buffer's conflict markers
        // (no preset rows; ⌘⌥↓ / ⌘⌥↑ are the app-added keys — ⌃⌥↓ is
        // Mission Control territory on macOS).
        NextConflict,
        PrevConflict,
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
    action_info!("workbench::Quit", "Quit", "File", "⌘Q"),
    action_info!("workbench::OpenFile", "Open File…", "File", "⌘O"),
    action_info!("workbench::Save", "Save", "File", "⌘S"),
    action_info!("workbench::CloseTab", "Close Tab", "File", "⌘W"),
    action_info!("workbench::ReopenClosedTab", "Reopen Closed Tab", "File", "⇧⌘T"),
    action_info!("editor::GotoDefinition", "Go to Definition", "Navigate", "⌘B"),
    action_info!("workbench::GoToFile", "Go to File", "Navigate", "⇧⌘O"),
    action_info!("workbench::GotoLine", "Go to Line:Column…", "Navigate", "⌘L"),
    action_info!("workbench::FindAction", "Find Action…", "Navigate", "⌘⇧A"),
    action_info!("workbench::RecentFiles", "Recent Files", "Navigate", "⌘E"),
    action_info!("workbench::RecentLocations", "Recent Locations", "Navigate", "⌘⇧E"),
    action_info!("workbench::NavigateBack", "Back", "Navigate", "⌘["),
    action_info!("workbench::NavigateForward", "Forward", "Navigate", "⌘]"),
    action_info!("workbench::NextError", "Next Highlighted Error", "Navigate", "F2"),
    action_info!("workbench::PrevError", "Previous Highlighted Error", "Navigate", "⇧F2"),
    action_info!("workbench::ToggleBookmark", "Toggle Bookmark", "Navigate", "F3"),
    action_info!("workbench::ShowBookmarks", "Bookmarks", "Navigate", "⌘F3"),
    action_info!("workbench::FileStructure", "File Structure", "Navigate", "⌘F12"),
    action_info!("workbench::Switcher", "Switcher", "Navigate", "⌃⇥"),
    action_info!("workbench::LastEditLocation", "Last Edit Location", "Navigate", "⌘⇧⌫"),
    action_info!("editor::GotoImplementation", "Go to Implementation(s)", "Navigate", "⌘⌥B"),
    action_info!("editor::GotoTypeDeclaration", "Go to Type Declaration", "Navigate", "⌘⇧B"),
    action_info!("editor::NextConflict", "Next Conflict", "Git", "⌘⌥↓"),
    action_info!("editor::PrevConflict", "Previous Conflict", "Git", "⌘⌥↑"),
    action_info!("workbench::SearchEverywhere", "Search Everywhere", "Navigate", "Double ⇧"),
    action_info!("workbench::FindInFiles", "Find in Files", "Edit", "⇧⌘F"),
    action_info!("workbench::Run", "Run…", "Run", "⌃R"),
    action_info!("workbench::StopRun", "Stop Run", "Run", "⌃⌘R"),
    action_info!("workbench::ShowDiff", "Show Diff", "Git", "⌃D"),
    action_info!("workbench::Commit", "Commit…", "Git", "⌘K"),
    action_info!("workbench::Undo", "Undo", "Edit", "⌘Z"),
    action_info!("workbench::Redo", "Redo", "Edit", "⇧⌘Z"),
    action_info!("workbench::ToggleTree", "Toggle Project Tree", "View", "⌘1"),
    action_info!("workbench::ToggleDock", "Toggle Dock", "View", "⌘4"),
    action_info!("workbench::OpenSettings", "Settings…", "View", "⌘,"),
    action_info!("workbench::ToggleBookmarks", "Bookmarks", "View", "⌘2"),
    action_info!("workbench::ToggleFind", "Find", "View", "⌘3"),
    action_info!("workbench::ToggleProblems", "Problems", "View", "⌘6"),
    action_info!("workbench::ToggleStructure", "Structure", "View", "⌘7"),
    action_info!("workbench::HideActiveToolWindow", "Hide Active Tool Window", "View", "⇧⎋"),
    action_info!("workbench::MaximizeToolWindow", "Maximize Tool Window", "View", "⌘⇧'"),
    action_info!("workbench::ToggleCommit", "Commit", "VCS", "⌘0"),
    action_info!("workbench::ToggleTerminal", "Terminal", "View", "⌥F12"),
    action_info!("workbench::NewTerminalTab", "New Terminal Tab", "View", "⌃⌘T"),
    action_info!("workbench::Debug", "Debug…", "Run", "⌃D"),
    action_info!("workbench::DebugStepInto", "Step Into", "Run", "F7"),
    action_info!("workbench::DebugStepOver", "Step Over", "Run", "F8"),
    action_info!("workbench::DebugResume", "Resume", "Run", "F9"),
    action_info!("editor::Paste", "Paste", "Edit", "⌘V"),
    action_info!("editor::TabIndent", "Indent / Expand Template", "Edit", "⇥"),
    action_info!("editor::OutdentLine", "Outdent Line", "Edit", "⇧⇥"),
    action_info!("editor::FindUsages", "Find Usages", "Navigate", "⌥F7"),
    action_info!("editor::Rename", "Rename…", "Refactor", "⇧F6"),
    action_info!("editor::CodeActions", "Quick Fix…", "Refactor", "⌥⏎"),
    action_info!("workbench::AnalyzeDuplicates", "Locate Duplicates…", "Code", "⌘⇧D"),
    action_info!("workbench::ToggleGit", "Git", "View", "⌘9"),
    action_info!("workbench::Branches", "Git Branches…", "VCS", "⌃⌘G"),
    action_info!("workbench::NewBranch", "New Branch…", "VCS", "⌘⌥N"),
    action_info!("workbench::Annotate", "Annotate", "VCS", "⌃⌘A"),
    action_info!("workbench::ToggleInlineBlame", "Toggle Inline Blame", "VCS", "⌃⌘B"),
    action_info!("editor::FormatDocument", "Format Document", "Code", "⌘⌥L"),
    action_info!("editor::OrganizeImports", "Organize Imports", "Code", "⌃⌥O"),
    action_info!("editor::ToggleInlayHints", "Toggle Inlay Hints", "View", "⌥⌘I"),
    action_info!("editor::Find", "Find…", "Edit", "⌘F"),
    action_info!("editor::Replace", "Replace…", "Edit", "⌘R"),
    action_info!("editor::FindNext", "Find Next", "Edit", "⌘G"),
    action_info!("editor::FindPrevious", "Find Previous", "Edit", "⌘⇧G"),
    action_info!("editor::DuplicateLine", "Duplicate Line or Selection", "Edit", "⌘D"),
    action_info!("editor::DeleteLine", "Delete Line", "Edit", "⌘⌫"),
    action_info!("editor::CommentLine", "Comment with Line Comment", "Code", "⌘/"),
    action_info!("editor::CommentBlock", "Comment with Block Comment", "Code", "⌘⌥/"),
    action_info!("editor::ExtendSelection", "Extend Selection", "Code", "⌥↑"),
    action_info!("editor::ShrinkSelection", "Shrink Selection", "Code", "⌥↓"),
    action_info!("editor::MoveLineUp", "Move Line Up", "Code", "⌥⇧↑"),
    action_info!("editor::MoveLineDown", "Move Line Down", "Code", "⌥⇧↓"),
    action_info!("editor::MoveStatementUp", "Move Statement Up", "Code", "⌘⇧↑"),
    action_info!("editor::MoveStatementDown", "Move Statement Down", "Code", "⌘⇧↓"),
    action_info!("editor::JoinLines", "Join Lines", "Code", "⌃⇧J"),
    action_info!("editor::ToggleCase", "Toggle Case", "Code", "⌘⇧U"),
    action_info!("editor::StartNewLineBelow", "Start New Line", "Code", "⇧⏎"),
    action_info!("editor::StartNewLineAbove", "Start New Line Before Current", "Code", "⌘⌥⏎"),
    action_info!("editor::Complete", "Code Completion", "Code", "⌃Space"),
    action_info!("editor::ParameterInfo", "Parameter Info", "Code", "⌘P"),
    action_info!("editor::Fold", "Fold", "Code", "⌘-"),
    action_info!("editor::Unfold", "Unfold", "Code", "⌘="),
    action_info!("editor::FoldAll", "Fold All", "Code", "⌘⇧-"),
    action_info!("editor::UnfoldAll", "Unfold All", "Code", "⌘⇧="),
    action_info!("editor::ScrollCenter", "Scroll to Center", "View", "⌃L"),
    action_info!("editor::CloneCaretAbove", "Clone Caret Above", "Edit", "⌥⌥↑"),
    action_info!("editor::CloneCaretBelow", "Clone Caret Below", "Edit", "⌥⌥↓"),
    action_info!("editor::ToggleBreakpoint", "Toggle Breakpoint", "Run", "⌘F8"),
    action_info!("editor::BreakpointCondition", "Conditional Breakpoint…", "Run", "⌥⌘F8"),
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
