//! The macOS menu bar (M10): the full IDEA structure, generated from this
//! app's action registry — every enabled item dispatches the same registered
//! action Find Action lists (one source of truth); the deferred faces
//! (M11+ milestones) render grayed-out `NoAction` placeholders.
//!
//! The first menu becomes macOS's application menu (About / Settings /
//! Quit); the rest carry IDEA's names: File, Edit, View, Navigate, Code,
//! Refactor, Build, Run, Tools, VCS, Window, Help.

use gpui::{Menu, MenuItem, NoAction};

use crate::actions as acts;

/// One row of the menu table: a registered action (enabled), a deferred
/// face (disabled), a separator, or a submenu.
pub(crate) enum ItemSpec {
    /// The registry name (`actions.rs` REGISTRY); the title comes from the
    /// registry row so the menu cannot drift from Find Action.
    Row(&'static str),
    /// A face a later milestone owns; shown grayed with this title.
    Deferred(&'static str),
    Separator,
    Submenu(&'static str, &'static [ItemSpec]),
}

use ItemSpec::{Deferred, Row, Separator, Submenu};

const APP_MENU: &[ItemSpec] =
    &[Deferred("About OpenIDEA"), Separator, Row("workbench::OpenSettings"), Separator, Row("workbench::Quit")];

const FILE_MENU: &[ItemSpec] = &[
    Deferred("New File…"),
    Deferred("New Project…"),
    Deferred("Open Recent"),
    Row("workbench::OpenFile"),
    Separator,
    Row("workbench::Save"),
    Deferred("Save All"),
    Row("workbench::CloseTab"),
    Row("workbench::ReopenClosedTab"),
    Separator,
    Row("workbench::OpenSettings"),
];

const EDIT_MENU: &[ItemSpec] = &[
    Row("workbench::Undo"),
    Row("workbench::Redo"),
    Separator,
    Deferred("Cut"),
    Deferred("Copy"),
    Row("editor::Paste"),
    Separator,
    Row("editor::Find"),
    Row("editor::Replace"),
    Row("editor::FindNext"),
    Row("editor::FindPrevious"),
    Row("workbench::FindInFiles"),
    Separator,
    Row("editor::CommentLine"),
    Row("editor::CommentBlock"),
    Row("editor::ToggleCase"),
    Row("editor::JoinLines"),
    Row("editor::DuplicateLine"),
    Row("editor::DeleteLine"),
    Row("editor::CloneCaretAbove"),
    Row("editor::CloneCaretBelow"),
];

const VIEW_MENU: &[ItemSpec] = &[
    Submenu(
        "Tool Windows",
        &[
            Row("workbench::ToggleTree"),
            Row("workbench::ToggleBookmarks"),
            Row("workbench::ToggleStructure"),
            Row("workbench::ToggleCommit"),
            Row("workbench::ToggleProblems"),
            Row("workbench::ToggleFind"),
            Row("workbench::ToggleDock"),
            Row("workbench::ToggleGit"),
        ],
    ),
    Submenu("Active Tool Window", &[Row("workbench::MaximizeToolWindow"), Row("workbench::HideActiveToolWindow")]),
    Separator,
    Row("editor::ToggleInlayHints"),
    Row("editor::ScrollCenter"),
    Row("editor::TogglePerfHud"),
];

const NAVIGATE_MENU: &[ItemSpec] = &[
    Row("workbench::GoToFile"),
    Row("workbench::SearchEverywhere"),
    Row("workbench::GotoLine"),
    Row("workbench::FindAction"),
    Separator,
    Row("workbench::RecentFiles"),
    Row("workbench::RecentLocations"),
    Row("workbench::Switcher"),
    Row("workbench::LastEditLocation"),
    Separator,
    Row("workbench::NavigateBack"),
    Row("workbench::NavigateForward"),
    Separator,
    Row("workbench::NextError"),
    Row("workbench::PrevError"),
    Separator,
    Row("editor::GotoDefinition"),
    Row("editor::GotoImplementation"),
    Row("editor::GotoTypeDeclaration"),
    Row("editor::FindUsages"),
    Separator,
    Row("workbench::ToggleBookmark"),
    Row("workbench::ShowBookmarks"),
    Row("workbench::FileStructure"),
];

const CODE_MENU: &[ItemSpec] = &[
    Row("editor::Complete"),
    Row("editor::ParameterInfo"),
    Row("editor::CodeActions"),
    Separator,
    Row("editor::ExtendSelection"),
    Row("editor::ShrinkSelection"),
    Separator,
    Row("editor::FormatDocument"),
    Row("editor::OrganizeImports"),
    Separator,
    Row("editor::Fold"),
    Row("editor::Unfold"),
    Row("editor::FoldAll"),
    Row("editor::UnfoldAll"),
    Separator,
    Row("editor::StartNewLineBelow"),
    Row("editor::StartNewLineAbove"),
    Row("editor::MoveLineUp"),
    Row("editor::MoveLineDown"),
    Row("editor::MoveStatementUp"),
    Row("editor::MoveStatementDown"),
];

const REFACTOR_MENU: &[ItemSpec] = &[
    Row("editor::Rename"),
    Separator,
    Deferred("Extract Method…"),
    Deferred("Extract Variable…"),
    Deferred("Extract Field…"),
    Deferred("Extract Constant…"),
    Deferred("Inline…"),
    Deferred("Move…"),
    Deferred("Refactor This…"),
];

const BUILD_MENU: &[ItemSpec] =
    &[Deferred("Build Project"), Deferred("Rebuild Project"), Deferred("Clean Project"), Deferred("Build Artifacts…")];

const RUN_MENU: &[ItemSpec] = &[
    Row("workbench::Run"),
    Row("workbench::StopRun"),
    Separator,
    Deferred("Debug…"),
    Deferred("Run with Coverage…"),
    Deferred("Edit Configurations…"),
];

const TOOLS_MENU: &[ItemSpec] = &[
    Deferred("Open Terminal"),
    Deferred("Start SSH Session…"),
    Deferred("Tasks & Contexts"),
    Separator,
    Row("workbench::OpenSettings"),
];

const VCS_MENU: &[ItemSpec] = &[
    Row("workbench::Commit"),
    Row("workbench::ShowDiff"),
    Row("workbench::Branches"),
    Row("workbench::NewBranch"),
    Separator,
    Deferred("Update Project"),
    Deferred("Push…"),
    Deferred("Annotate"),
    Deferred("Show Git Log"),
    Deferred("Manage Remotes…"),
];

const WINDOW_MENU: &[ItemSpec] = &[
    Deferred("Minimize"),
    Deferred("Zoom"),
    Separator,
    Deferred("Store Current Layout"),
    Deferred("Restore Default Layout"),
];

const HELP_MENU: &[ItemSpec] = &[
    Deferred("Documentation"),
    Deferred("Release Notes"),
    Separator,
    Deferred("Switch Boot JDK…"),
    Deferred("Collect Diagnostic Data"),
];

/// IDEA's macOS menu structure: the app menu first, then the twelve.
pub(crate) const MENUS: &[(&str, &[ItemSpec])] = &[
    ("OpenIDEA", APP_MENU),
    ("File", FILE_MENU),
    ("Edit", EDIT_MENU),
    ("View", VIEW_MENU),
    ("Navigate", NAVIGATE_MENU),
    ("Code", CODE_MENU),
    ("Refactor", REFACTOR_MENU),
    ("Build", BUILD_MENU),
    ("Run", RUN_MENU),
    ("Tools", TOOLS_MENU),
    ("VCS", VCS_MENU),
    ("Window", WINDOW_MENU),
    ("Help", HELP_MENU),
];

/// The registry row for an action name (title + the action instance source).
fn registry_row(name: &str) -> Option<&'static acts::ActionInfo> {
    acts::REGISTRY.iter().find(|row| row.name == name)
}

/// The registry name -> the concrete unit-struct action the `actions!`
/// macro declared. Menu rows must carry the real instance: gpui dispatches
/// and validates by the action's concrete type id (a boxed-by-name wrapper
/// would break both).
fn menu_item_for(name: &str, title: &'static str) -> Option<MenuItem> {
    match name {
        "workbench::OpenFile" => Some(MenuItem::action(title, crate::actions::OpenFile)),
        "workbench::Save" => Some(MenuItem::action(title, crate::actions::Save)),
        "workbench::CloseTab" => Some(MenuItem::action(title, crate::actions::CloseTab)),
        "workbench::Undo" => Some(MenuItem::action(title, crate::actions::Undo)),
        "workbench::Redo" => Some(MenuItem::action(title, crate::actions::Redo)),
        "workbench::Quit" => Some(MenuItem::action(title, crate::actions::Quit)),
        "workbench::Cancel" => Some(MenuItem::action(title, crate::actions::Cancel)),
        "workbench::GoToFile" => Some(MenuItem::action(title, crate::actions::GoToFile)),
        "workbench::SearchEverywhere" => Some(MenuItem::action(title, crate::actions::SearchEverywhere)),
        "workbench::Run" => Some(MenuItem::action(title, crate::actions::Run)),
        "workbench::StopRun" => Some(MenuItem::action(title, crate::actions::StopRun)),
        "workbench::ShowDiff" => Some(MenuItem::action(title, crate::actions::ShowDiff)),
        "workbench::Commit" => Some(MenuItem::action(title, crate::actions::Commit)),
        "workbench::FindInFiles" => Some(MenuItem::action(title, crate::actions::FindInFiles)),
        "workbench::ToggleTree" => Some(MenuItem::action(title, crate::actions::ToggleTree)),
        "workbench::ToggleDock" => Some(MenuItem::action(title, crate::actions::ToggleDock)),
        "workbench::ReopenClosedTab" => Some(MenuItem::action(title, crate::actions::ReopenClosedTab)),
        "workbench::OpenSettings" => Some(MenuItem::action(title, crate::actions::OpenSettings)),
        "workbench::ToggleBookmarks" => Some(MenuItem::action(title, crate::actions::ToggleBookmarks)),
        "workbench::ToggleFind" => Some(MenuItem::action(title, crate::actions::ToggleFind)),
        "workbench::ToggleProblems" => Some(MenuItem::action(title, crate::actions::ToggleProblems)),
        "workbench::AnalyzeDuplicates" => Some(MenuItem::action(title, crate::actions::AnalyzeDuplicates)),
        "workbench::ToggleStructure" => Some(MenuItem::action(title, crate::actions::ToggleStructure)),
        "workbench::HideActiveToolWindow" => Some(MenuItem::action(title, crate::actions::HideActiveToolWindow)),
        "workbench::MaximizeToolWindow" => Some(MenuItem::action(title, crate::actions::MaximizeToolWindow)),
        "workbench::ToggleCommit" => Some(MenuItem::action(title, crate::actions::ToggleCommit)),
        "workbench::ToggleGit" => Some(MenuItem::action(title, crate::actions::ToggleGit)),
        "workbench::Branches" => Some(MenuItem::action(title, crate::actions::Branches)),
        "workbench::NewBranch" => Some(MenuItem::action(title, crate::actions::NewBranch)),
        "workbench::TrustProject" => Some(MenuItem::action(title, crate::actions::TrustProject)),
        "workbench::DontOpenProject" => Some(MenuItem::action(title, crate::actions::DontOpenProject)),
        "workbench::RecentFiles" => Some(MenuItem::action(title, crate::actions::RecentFiles)),
        "workbench::RecentLocations" => Some(MenuItem::action(title, crate::actions::RecentLocations)),
        "workbench::NavigateBack" => Some(MenuItem::action(title, crate::actions::NavigateBack)),
        "workbench::NavigateForward" => Some(MenuItem::action(title, crate::actions::NavigateForward)),
        "workbench::NextError" => Some(MenuItem::action(title, crate::actions::NextError)),
        "workbench::PrevError" => Some(MenuItem::action(title, crate::actions::PrevError)),
        "workbench::ToggleBookmark" => Some(MenuItem::action(title, crate::actions::ToggleBookmark)),
        "workbench::ShowBookmarks" => Some(MenuItem::action(title, crate::actions::ShowBookmarks)),
        "workbench::FileStructure" => Some(MenuItem::action(title, crate::actions::FileStructure)),
        "workbench::Switcher" => Some(MenuItem::action(title, crate::actions::Switcher)),
        "workbench::SwitcherBack" => Some(MenuItem::action(title, crate::actions::SwitcherBack)),
        "workbench::SwitcherNext" => Some(MenuItem::action(title, crate::actions::SwitcherNext)),
        "workbench::SwitcherPrev" => Some(MenuItem::action(title, crate::actions::SwitcherPrev)),
        "workbench::GotoLine" => Some(MenuItem::action(title, crate::actions::GotoLine)),
        "workbench::LastEditLocation" => Some(MenuItem::action(title, crate::actions::LastEditLocation)),
        "workbench::FindAction" => Some(MenuItem::action(title, crate::actions::FindAction)),
        "palette::SelectPrev" => Some(MenuItem::action(title, crate::actions::SelectPrev)),
        "editor::ToggleReplace" => Some(MenuItem::action(title, crate::actions::ToggleReplace)),
        "editor::TogglePerfHud" => Some(MenuItem::action(title, crate::actions::TogglePerfHud)),
        "editor::CursorLeft" => Some(MenuItem::action(title, crate::actions::CursorLeft)),
        "editor::CursorRight" => Some(MenuItem::action(title, crate::actions::CursorRight)),
        "editor::CursorUp" => Some(MenuItem::action(title, crate::actions::CursorUp)),
        "editor::CursorDown" => Some(MenuItem::action(title, crate::actions::CursorDown)),
        "editor::CursorWordLeft" => Some(MenuItem::action(title, crate::actions::CursorWordLeft)),
        "editor::CursorWordRight" => Some(MenuItem::action(title, crate::actions::CursorWordRight)),
        "editor::CursorLineHome" => Some(MenuItem::action(title, crate::actions::CursorLineHome)),
        "editor::CursorLineEnd" => Some(MenuItem::action(title, crate::actions::CursorLineEnd)),
        "editor::CursorDocStart" => Some(MenuItem::action(title, crate::actions::CursorDocStart)),
        "editor::CursorDocEnd" => Some(MenuItem::action(title, crate::actions::CursorDocEnd)),
        "editor::SelectLeft" => Some(MenuItem::action(title, crate::actions::SelectLeft)),
        "editor::SelectRight" => Some(MenuItem::action(title, crate::actions::SelectRight)),
        "editor::SelectUp" => Some(MenuItem::action(title, crate::actions::SelectUp)),
        "editor::SelectDown" => Some(MenuItem::action(title, crate::actions::SelectDown)),
        "editor::SelectWordLeft" => Some(MenuItem::action(title, crate::actions::SelectWordLeft)),
        "editor::SelectWordRight" => Some(MenuItem::action(title, crate::actions::SelectWordRight)),
        "editor::SelectLineHome" => Some(MenuItem::action(title, crate::actions::SelectLineHome)),
        "editor::SelectLineEnd" => Some(MenuItem::action(title, crate::actions::SelectLineEnd)),
        "editor::SelectDocStart" => Some(MenuItem::action(title, crate::actions::SelectDocStart)),
        "editor::SelectDocEnd" => Some(MenuItem::action(title, crate::actions::SelectDocEnd)),
        "editor::DeleteBackward" => Some(MenuItem::action(title, crate::actions::DeleteBackward)),
        "editor::DeleteForward" => Some(MenuItem::action(title, crate::actions::DeleteForward)),
        "editor::DeleteWordBackward" => Some(MenuItem::action(title, crate::actions::DeleteWordBackward)),
        "editor::DeleteWordForward" => Some(MenuItem::action(title, crate::actions::DeleteWordForward)),
        "editor::Newline" => Some(MenuItem::action(title, crate::actions::Newline)),
        "editor::TabIndent" => Some(MenuItem::action(title, crate::actions::TabIndent)),
        "editor::OutdentLine" => Some(MenuItem::action(title, crate::actions::OutdentLine)),
        "editor::Paste" => Some(MenuItem::action(title, crate::actions::Paste)),
        "editor::GotoDefinition" => Some(MenuItem::action(title, crate::actions::GotoDefinition)),
        "editor::FindUsages" => Some(MenuItem::action(title, crate::actions::FindUsages)),
        "editor::Rename" => Some(MenuItem::action(title, crate::actions::Rename)),
        "editor::CodeActions" => Some(MenuItem::action(title, crate::actions::CodeActions)),
        "editor::FormatDocument" => Some(MenuItem::action(title, crate::actions::FormatDocument)),
        "editor::OrganizeImports" => Some(MenuItem::action(title, crate::actions::OrganizeImports)),
        "editor::ToggleInlayHints" => Some(MenuItem::action(title, crate::actions::ToggleInlayHints)),
        "editor::Find" => Some(MenuItem::action(title, crate::actions::Find)),
        "editor::DuplicateLine" => Some(MenuItem::action(title, crate::actions::DuplicateLine)),
        "editor::CommentLine" => Some(MenuItem::action(title, crate::actions::CommentLine)),
        "editor::CommentBlock" => Some(MenuItem::action(title, crate::actions::CommentBlock)),
        "editor::ExtendSelection" => Some(MenuItem::action(title, crate::actions::ExtendSelection)),
        "editor::ShrinkSelection" => Some(MenuItem::action(title, crate::actions::ShrinkSelection)),
        "editor::MoveLineUp" => Some(MenuItem::action(title, crate::actions::MoveLineUp)),
        "editor::MoveLineDown" => Some(MenuItem::action(title, crate::actions::MoveLineDown)),
        "editor::MoveStatementUp" => Some(MenuItem::action(title, crate::actions::MoveStatementUp)),
        "editor::MoveStatementDown" => Some(MenuItem::action(title, crate::actions::MoveStatementDown)),
        "editor::DeleteLine" => Some(MenuItem::action(title, crate::actions::DeleteLine)),
        "editor::FindNext" => Some(MenuItem::action(title, crate::actions::FindNext)),
        "editor::FindPrevious" => Some(MenuItem::action(title, crate::actions::FindPrevious)),
        "editor::Replace" => Some(MenuItem::action(title, crate::actions::Replace)),
        "editor::ScrollCenter" => Some(MenuItem::action(title, crate::actions::ScrollCenter)),
        "editor::Fold" => Some(MenuItem::action(title, crate::actions::Fold)),
        "editor::Unfold" => Some(MenuItem::action(title, crate::actions::Unfold)),
        "editor::FoldAll" => Some(MenuItem::action(title, crate::actions::FoldAll)),
        "editor::UnfoldAll" => Some(MenuItem::action(title, crate::actions::UnfoldAll)),
        "editor::StartNewLineBelow" => Some(MenuItem::action(title, crate::actions::StartNewLineBelow)),
        "editor::StartNewLineAbove" => Some(MenuItem::action(title, crate::actions::StartNewLineAbove)),
        "editor::JoinLines" => Some(MenuItem::action(title, crate::actions::JoinLines)),
        "editor::ToggleCase" => Some(MenuItem::action(title, crate::actions::ToggleCase)),
        "editor::Complete" => Some(MenuItem::action(title, crate::actions::Complete)),
        "editor::CloneCaretAbove" => Some(MenuItem::action(title, crate::actions::CloneCaretAbove)),
        "editor::CloneCaretBelow" => Some(MenuItem::action(title, crate::actions::CloneCaretBelow)),
        "editor::ParameterInfo" => Some(MenuItem::action(title, crate::actions::ParameterInfo)),
        "editor::GotoImplementation" => Some(MenuItem::action(title, crate::actions::GotoImplementation)),
        "editor::GotoTypeDeclaration" => Some(MenuItem::action(title, crate::actions::GotoTypeDeclaration)),
        _ => None,
    }
}

fn build_items(specs: &[ItemSpec]) -> Vec<MenuItem> {
    specs
        .iter()
        .map(|spec| match spec {
            Row(name) => {
                let title = registry_row(name).map_or(*name, |row| row.title);
                menu_item_for(name, title).unwrap_or_else(|| MenuItem::action(title, NoAction).disabled(true))
            }
            Deferred(title) => MenuItem::action(*title, NoAction).disabled(true),
            Separator => MenuItem::separator(),
            Submenu(title, items) => MenuItem::submenu(Menu::new(*title).items(build_items(items))),
        })
        .collect()
}

/// The macOS menu bar: the app menu first, then IDEA's twelve.
pub fn install(cx: &mut gpui::App) {
    let menus: Vec<Menu> = MENUS.iter().map(|(name, specs)| Menu::new(*name).items(build_items(specs))).collect();
    cx.set_menus(menus);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_idea_menu_structure_is_complete() {
        let names: Vec<&str> = MENUS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            vec![
                "OpenIDEA", "File", "Edit", "View", "Navigate", "Code", "Refactor", "Build", "Run", "Tools", "VCS",
                "Window", "Help"
            ]
        );
    }

    #[test]
    fn every_enabled_row_is_a_registered_action() {
        fn walk(specs: &[ItemSpec]) {
            for spec in specs {
                match spec {
                    Row(name) => {
                        assert!(
                            registry_row(name).is_some(),
                            "menu row {name} must be in the action registry (Find Action same-source)"
                        );
                    }
                    Submenu(_, items) => walk(items),
                    _ => {}
                }
            }
        }
        for (_, specs) in MENUS {
            walk(specs);
        }
    }

    #[test]
    fn deferred_faces_stay_out_of_the_enabled_sets() {
        // Build/Refactor-beyond-Rename are M11+/M12 faces: every Build row
        // is deferred, and Refactor has exactly one enabled row (Rename).
        let (_, build) = MENUS.iter().find(|(name, _)| *name == "Build").unwrap();
        assert!(build.iter().all(|spec| matches!(spec, Deferred(_) | Separator)));
        let (_, refactor) = MENUS.iter().find(|(name, _)| *name == "Refactor").unwrap();
        assert_eq!(refactor.iter().filter(|spec| matches!(spec, Row(_))).count(), 1);
    }
}
