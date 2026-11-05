//! The workbench shell: left dock file tree, central editor tab stack, thin
//! status bar — assembled from gpui-component primitives over the in-process
//! backend (`Host`). Owns the tabs (one `EditorPane` each, their dirty /
//! stale flags) and consumes the bus: `FsChanged` refreshes the tree and
//! marks stale tabs, `DocPatched` reloads clean buffers, `DocDirty` and
//! `Notify` feed the status line.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable as _, InteractiveElement, IntoElement,
    KeyDownEvent, ModifiersChangedEvent, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::status_bar::StatusBar;
use gpui_component::tab::{Tab as ComponentTab, TabBar};
use gpui_component::{Icon, IconName, Selectable as _, Sizable as _};
use ide_core::event::NotifyLevel;
use ide_core::event::ServerStatus;
use ide_core::{Diagnostic, DocId, IdeError, IdeEvent, ServerId};
use ide_index::{FileMatch, SymbolFilter, SymbolMatch};
use ide_run::RunConfig;
use ide_vcs::ChangeKind;
use ide_vfs::EntryKind;

use crate::actions as acts;
use crate::actions::{
    Cancel, CloseTab, FindInFiles, GoToFile, OpenFile, Redo, Run, Save, SearchEverywhere, ShowDiff, StopRun, Undo,
};
use crate::bus::{Bus, BusMessage};
use crate::diff_view::{self, DiffLine};
use crate::editor::EditorPane;
use crate::file_tree::{FileTree, Row};
use crate::find_panel::FindPanel;
use crate::fuzzy;
use crate::host::Host;
use crate::run_panel::{RunPanel, stream_of};
use crate::{ProjectHost, settings, strings, theme};

/// The bottom dock's active page when both panels exist (M5).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DockPage {
    Run,
    Find,
}

/// One open file: the editor pane, its document in the backend, and the two
/// flags the chrome shows. The pane pushes its edits to the backend live, so
/// `dirty` simply mirrors the backend's `DocDirty` events; `stale` means the
/// file changed on disk while the buffer had unsaved edits.
struct Tab {
    path: PathBuf,
    doc: DocId,
    pane: Entity<EditorPane>,
    dirty: bool,
    stale: bool,
}

/// What the status bar shows on the right, until the next message.
type Message = (NotifyLevel, String);

/// One language server's live status (from `LanguageServer` events), keyed by
/// server id so restarted instances replace their row.
struct ServerRow {
    id: ServerId,
    name: String,
    status: ServerStatus,
    message: Option<String>,
}

/// An action row of the palette: the registry entry plus its local fuzzy hit.
#[derive(Clone)]
struct RankedAction {
    name: &'static str,
    title: &'static str,
    keys: &'static str,
    score: i32,
    indices: Vec<usize>,
}

/// Which surface the ⇧⌘O / double-⇧ / ⌃R palette shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickerMode {
    /// Go to File (files only).
    Files,
    /// Search Everywhere: files + symbols + actions.
    Everywhere,
    /// Run configurations (⌃R): pick one, or type a one-off shell command.
    Run,
}

/// The ⇧⌘O / double-⇧ / ⌃R palette: one list, grouped per mode. Files and
/// symbols come from the project index; actions are the app's registry table
/// filtered by the in-app fuzzy matcher; run configs are detected + user.
struct Picker {
    mode: PickerMode,
    input: Entity<InputState>,
    query: String,
    files: Vec<FileMatch>,
    symbols: Vec<SymbolMatch>,
    /// The language server's own `workspace/symbol` answer (M6), kept apart
    /// from the index rows so neither landing overwrites the other.
    lsp_symbols: Vec<SymbolMatch>,
    actions: Vec<RankedAction>,
    /// Run mode: the configurations offered, cached at open.
    configs: Vec<RunConfig>,
    selected: usize,
}

/// One selectable palette row (the flat form across the sections).
#[derive(Clone)]
enum PickerRow {
    File(FileMatch),
    Symbol(SymbolMatch),
    Action(RankedAction),
    /// Boxed: `RunConfig` dwarfs the other variants (clippy::large_enum_variant).
    Run(Box<RunConfig>),
    /// A one-off shell command line typed into the Run palette.
    Shell(String),
}

/// The open diff overlay (M4): one file's unified changes against HEAD.
struct DiffState {
    title: String,
    lines: Vec<DiffLine>,
}

/// One checkbox row of the commit overlay.
struct CommitFile {
    path: PathBuf,
    name: String,
    kind: ChangeKind,
    selected: bool,
}

/// The commit overlay (M4): the changed files (checked = staged for this
/// commit) and a multi-line message.
struct CommitFlow {
    input: Entity<TextareaState>,
    files: Vec<CommitFile>,
}

pub struct Workbench {
    host: Option<Arc<Host>>,
    /// The persisted settings (recents, layout, font size) — saved on change.
    settings: settings::Settings,
    tabs: Vec<Tab>,
    active: Option<usize>,
    tree: FileTree,
    tree_focus: FocusHandle,
    /// Startup focus seed: with no tab open the tree takes focus so the
    /// window-level actions (⌘O) and the tree keys work immediately.
    tree_focused_once: Cell<bool>,
    /// A bus-side project open has no window; the next render hands the tree
    /// the focus.
    tree_focus_pending: Cell<bool>,
    message: Option<Message>,
    servers: Vec<ServerRow>,
    /// Diagnostics per file, per producer (`Diagnostics` events replace one
    /// producer's list; the merged view reaches the pane). Sorted by position
    /// for the pane's line lookups.
    diagnostics: HashMap<PathBuf, HashMap<String, Vec<Diagnostic>>>,
    bus: Bus,
    /// The open Go to File / Search Everywhere / Run palette, if any.
    picker: Option<Picker>,
    /// The input's Change / PressEnter subscription, alive while the picker is.
    picker_sub: Option<gpui::Subscription>,
    /// The palette's index-query generation (M5 phase 7): every query change
    /// bumps it on the UI thread; answers from older generations drop, so a
    /// burst of keystrokes never lands stale rows.
    picker_gen: u32,
    /// The Run dock (M4), from the first run until the project closes: one
    /// run at a time, output streaming through the bus.
    run_panel: Option<Entity<RunPanel>>,
    /// The Find-in-Files dock page (M5), from the first ⇧⌘F until the
    /// project closes; searches run through the bus (the workbench owns the
    /// dirty-buffer snapshot).
    find_panel: Option<Entity<FindPanel>>,
    /// The dock's active page when both panels exist.
    dock_page: DockPage,
    /// Git working-tree kind per absolute path (working-tree kind wins, the
    /// staged one when the tree is clean), and the current branch. Refreshed
    /// on the background executor (M4).
    git_status: HashMap<PathBuf, ChangeKind>,
    git_branch: Option<String>,
    /// Coalescing for the git refreshes a burst of `FsChanged` batches would
    /// otherwise serialize: one in flight, re-run when it lands if more came.
    git_refreshing: Cell<bool>,
    git_refresh_queued: Cell<bool>,
    /// Seeded from the first render (a view render cannot act earlier).
    git_initialized: Cell<bool>,
    /// The open diff overlay, its focus handle, and the deferred focus seed
    /// (the overlay opens from the bus, which has no window).
    diff: Option<DiffState>,
    diff_focus: FocusHandle,
    diff_focus_pending: Cell<bool>,
    /// The open commit overlay (⌘K).
    commit: Option<CommitFlow>,
    /// The message input's PressEnter subscription, alive while the overlay is.
    commit_sub: Option<gpui::Subscription>,
    /// Set when a bus message closed an overlay: the next render hands the
    /// focus back to the editor (or the tree).
    restore_focus_pending: Cell<bool>,
    /// The last bare ⇧ press, for the double-⇧ Search Everywhere gesture.
    last_shift: Cell<Option<Instant>>,
    /// Whether ⇧ was held at the last modifiers event (rising-edge detection).
    shift_held: Cell<bool>,
    /// ⌘S's organize-imports-then-save flow in flight (M6), if any: the doc
    /// it organizes. A second ⌘S on it saves plainly instead of queueing.
    save_flow: Cell<Option<DocId>>,
    /// Live `$/progress` tasks (M6): the servers' indexing / load bursts, so
    /// a big first index reads as work in progress, not a hang.
    tasks: Vec<(ide_core::TaskId, String, Option<String>, Option<f32>)>,
}

impl Workbench {
    pub fn new(host: Option<Arc<Host>>, bus: Bus, settings: settings::Settings, cx: &mut Context<Self>) -> Self {
        let mut tree = FileTree::new();
        if let Some(host) = &host
            && let Err(e) = tree.open(host)
        {
            eprintln!("[workbench] project open: {}", e.message);
        }
        Self {
            host,
            settings,
            tabs: Vec::new(),
            active: None,
            tree,
            tree_focus: cx.focus_handle(),
            tree_focused_once: Cell::new(false),
            tree_focus_pending: Cell::new(false),
            message: None,
            servers: Vec::new(),
            diagnostics: HashMap::new(),
            bus,
            picker: None,
            picker_sub: None,
            picker_gen: 0,
            run_panel: None,
            find_panel: None,
            dock_page: DockPage::Run,
            git_status: HashMap::new(),
            git_branch: None,
            git_refreshing: Cell::new(false),
            git_refresh_queued: Cell::new(false),
            git_initialized: Cell::new(false),
            diff: None,
            diff_focus: cx.focus_handle(),
            diff_focus_pending: Cell::new(false),
            commit: None,
            commit_sub: None,
            restore_focus_pending: Cell::new(false),
            last_shift: Cell::new(None),
            shift_held: Cell::new(false),
            save_flow: Cell::new(None),
            tasks: Vec::new(),
        }
    }

    /// Kicks a git status + branch refresh (coalesced; no-op without a repo
    /// or while one is already in flight).
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.as_ref().filter(|host| host.repo.is_some()) else { return };
        if self.git_refreshing.replace(true) {
            self.git_refresh_queued.set(true);
            return;
        }
        let bus = self.bus.clone();
        let host = host.clone();
        cx.background_executor()
            .spawn(async move {
                let result = host.git_status();
                eprintln!(
                    "[vcs] status refresh: {} files, branch {:?} ({})",
                    result.as_ref().map_or(0, |(s, _)| s.len()),
                    result.as_ref().map(|(_, b)| b.as_deref()),
                    result.as_ref().err().map_or("ok", |e| e.message.as_str())
                );
                let (statuses, branch) = result.unwrap_or((Vec::new(), None));
                bus.send(BusMessage::GitStatus { statuses, branch });
            })
            .detach();
    }

    // -- actions ----------------------------------------------------------

    /// ⌘O: the native dialog. Context-sensitive (M7): with a project open it
    /// picks a file to edit; with none it picks a project folder (the one
    /// decision the welcome view needs). rfd's dialog future must be *created*
    /// on the main thread (NSOpenPanel is main-thread-only; the future's own
    /// constructor asserts it) but may be awaited from the background
    /// executor; the pick comes back through the bus.
    fn on_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        let bus = self.bus.clone();
        let projectless = self.host.is_none();
        let initial =
            self.host.as_ref().map(|host| host.root.clone()).or_else(|| self.settings.recent_projects.first().cloned());
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = initial {
            dialog = dialog.set_directory(&dir);
        }
        cx.background_executor()
            .spawn(async move {
                let picked = if projectless {
                    dialog.pick_folder().await.map(|handle| handle.path().to_path_buf())
                } else {
                    dialog.pick_file().await.map(|handle| handle.path().to_path_buf())
                };
                if projectless {
                    bus.send(BusMessage::PickedFolder(picked));
                } else {
                    bus.send(BusMessage::Picked(picked));
                }
            })
            .detach();
        cx.notify();
    }

    /// Opens `root` as the project: constructs the Host inside the app
    /// runtime's enter context (the bus pump runs on the main thread, which
    /// `block_on` keeps in the runtime), replaces the `ProjectHost` global,
    /// reseeds the tree and git, and records the recent. The welcome view's
    /// open-folder answer and future multi-project faces all land here.
    fn open_project(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let host = {
            let _runtime = tokio::runtime::Handle::current().enter();
            Host::open(&root, self.bus.sink(), tokio::runtime::Handle::current())
        };
        let host = match host {
            Ok(host) => Arc::new(host),
            Err(e) => {
                eprintln!("[workbench] opening {}: {}", root.display(), e.message);
                self.message = Some((
                    NotifyLevel::Error,
                    format!("{} {}: {}", strings::OPEN_PROJECT_FAILED, file_name(&root), e.message),
                ));
                cx.notify();
                return;
            }
        };
        cx.set_global(ProjectHost(host.clone()));
        self.host = Some(host.clone());
        self.tabs.clear();
        self.active = None;
        self.diagnostics.clear();
        self.git_status.clear();
        self.git_branch = None;
        self.run_panel = None;
        self.find_panel = None;
        self.dock_page = DockPage::Run;
        if let Err(e) = self.tree.open(&host) {
            eprintln!("[workbench] tree open: {}", e.message);
        }
        // The next render kicks the git refresh and hands the tree the focus
        // (the bus pump has no window).
        self.git_initialized.set(false);
        self.tree_focus_pending.set(true);
        settings::push_recent(&mut self.settings.recent_projects, &host.root);
        settings::save(&self.settings);
        eprintln!("[workbench] project opened {}", host.root.display());
        cx.notify();
    }

    /// ⌘S: writes the document to disk through the VFS. The pane's edits
    /// already reached the backend document model (every keystroke goes
    /// through `Documents::edit`), so there is nothing to push.
    /// M6: a dirty Go document gets the Go DX save first —
    /// `source.organizeImports` (unused imports dropped, the block sorted —
    /// gopls's own behavior) applied through Documents, then the write. Any
    /// failure falls back to the plain save, so saving is never blocked by
    /// the organizer; one flow at a time, a second ⌘S on the same doc while
    /// one runs saves plainly.
    fn on_save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else { return };
        let Some(host) = self.host.clone() else { return };
        let (doc, path, name) = {
            let tab = &self.tabs[index];
            (tab.doc, tab.path.clone(), file_name(&tab.path))
        };
        let organizable = self.tabs[index].dirty
            && host.languages.detect(&path, None).as_str() == "go"
            && self.save_flow.get().is_none_or(|busy| busy != doc);
        if organizable {
            self.save_flow.set(Some(doc));
            let bus = self.bus.clone();
            let runtime = host.runtime.clone();
            runtime.spawn(async move {
                let organized = async {
                    let info = host.snapshot(doc)?;
                    let last = info.text.lines().count().saturating_sub(1) as u32;
                    let range = ide_core::Range::new(ide_core::Pos::new(0, 0), ide_core::Pos::new(last, 0));
                    match crate::editor::organize_imports_action(&host.lsp, doc, range).await {
                        Ok(Some(action)) => {
                            eprintln!("[lsp] organize imports on save");
                            if let Some(edit) = &action.edit {
                                host.apply_workspace_edit(edit)?;
                            }
                            Ok::<bool, IdeError>(true)
                        }
                        Ok(None) => Ok(false),
                        Err(e) => {
                            eprintln!("[lsp] organize imports on save skipped: {}", e.message);
                            Ok(false)
                        }
                    }
                }
                .await
                .unwrap_or(false);
                let error = host.save_document(doc).err().map(|e| e.message);
                bus.send(BusMessage::SaveFinished { doc, name, organized, error });
            });
            cx.notify();
            return;
        }
        let tab = &mut self.tabs[index];
        match host.save_document(doc) {
            Ok(()) => {
                tab.dirty = false;
                tab.stale = false;
                self.message = Some((NotifyLevel::Info, format!("{} {name}", strings::SAVED_TOAST)));
            }
            Err(e) => {
                self.message = Some((NotifyLevel::Error, format!("{} {name}: {}", strings::SAVE_FAILED, e.message)));
            }
        }
        cx.notify();
    }

    /// ⌘W: closes the active tab and detaches its document.
    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else { return };
        self.close_tab(index, cx);
        self.focus_active(window, cx);
    }

    /// ⌘Z / ⌘⇧Z on the active editor pane (one undo timeline per pane).
    fn on_undo(&mut self, _: &Undo, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.active {
            self.tabs[index].pane.update(cx, |pane, cx| pane.undo(cx));
        }
    }

    fn on_redo(&mut self, _: &Redo, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.active {
            self.tabs[index].pane.update(cx, |pane, cx| pane.redo(cx));
        }
    }

    // -- Go to File / Search Everywhere (M3 phase 6) -----------------------
    //
    // ⇧⌘O opens the palette in files-only mode; double ⇧ opens it over
    // files + symbols + actions. The input keeps the focus; ↑/↓ move the
    // selection through the flat row list (bindings in the "Palette" context
    // beat the input's own cursor moves, being registered later), Enter
    // accepts, Esc closes and focus returns to the editor.

    fn on_go_to_file(&mut self, _: &GoToFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerMode::Files, window, cx);
    }

    fn on_search_everywhere(&mut self, _: &SearchEverywhere, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerMode::Everywhere, window, cx);
    }

    /// ⌃R: the Run palette — pick a detected / user configuration, or type a
    /// one-off shell command line.
    fn on_run(&mut self, _: &Run, window: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(PickerMode::Run, window, cx);
    }

    /// ⌃⌘R: stops the live run (the dock's Stop button in keymap form).
    fn on_stop_run(&mut self, _: &StopRun, _window: &mut Window, cx: &mut Context<Self>) {
        let run_id = self.run_panel.as_ref().and_then(|panel| panel.read(cx).run_id);
        let live = self.run_panel.as_ref().is_some_and(|panel| panel.read(cx).running());
        if let (Some(run_id), true) = (run_id, live)
            && let Some(host) = &self.host
        {
            if let Err(e) = host.stop_run(run_id) {
                eprintln!("[run] stop: {}", e.message);
            }
            cx.notify();
        }
    }

    /// ⇧⌘F: opens (or just surfaces) the Find-in-Files dock page. The panel
    /// is created once per project; every Enter re-runs the search through
    /// the bus with a fresh dirty-buffer snapshot.
    /// ⇧⌘F: opens (or just surfaces) the Find-in-Files dock page and hands
    /// the query input the focus — the gesture means "type a search now".
    /// The panel is created once per project; every Enter re-runs the search
    /// through the bus with a fresh dirty-buffer snapshot.
    fn on_find_in_files(&mut self, _: &FindInFiles, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_panel.is_none() {
            self.find_panel = Some(cx.new(|cx| FindPanel::new(self.bus.clone(), window, cx)));
        }
        self.dock_page = DockPage::Find;
        if let Some(panel) = &self.find_panel {
            window.focus(&panel.read(cx).query_focus(cx), cx);
        }
        cx.notify();
    }

    /// ⌃D: the diff overlay for the changed file at hand — the tree's
    /// selected row when it carries a git change, else the active tab's file.
    fn on_show_diff(&mut self, _: &ShowDiff, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.message = Some((NotifyLevel::Warning, strings::NO_PROJECT.to_owned()));
            cx.notify();
            return;
        };
        if host.repo.is_none() {
            self.message = Some((NotifyLevel::Warning, strings::GIT_NOT_A_REPO.to_owned()));
            cx.notify();
            return;
        }
        let target = self
            .tree
            .selected
            .clone()
            .filter(|path| self.git_status.contains_key(path))
            .or_else(|| self.active.and_then(|index| self.tabs.get(index)).map(|tab| tab.path.clone()));
        let Some(path) = target else {
            self.message = Some((NotifyLevel::Warning, strings::DIFF_TARGET_HINT.to_owned()));
            cx.notify();
            return;
        };
        // The dirty buffer wins over the disk (its changes are the diff).
        let current = match self.tabs.iter().find(|tab| tab.path == path) {
            Some(tab) if tab.dirty => Some(tab.pane.read(cx).buffer_text()),
            _ => None,
        };
        let current = current.or_else(|| std::fs::read_to_string(&path).ok()).unwrap_or_default();
        let title = file_name(&path);
        let bus = self.bus.clone();
        cx.background_executor()
            .spawn(async move {
                let result = host.git_line_hunks(&path, &current).map(|hunks| diff_view::from_hunks(&hunks, &current));
                bus.send(BusMessage::DiffReady { path, title, result });
            })
            .detach();
    }

    fn close_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diff.take().is_none() {
            return;
        }
        if self.active.is_some() {
            self.focus_active(window, cx);
        } else {
            window.focus(&self.tree_focus, cx);
        }
        cx.notify();
    }

    /// ⌘K: the commit overlay — every changed file checked, an empty
    /// multi-line message.
    fn on_commit(&mut self, _: &acts::Commit, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit.is_some() {
            return;
        }
        let Some(host) = &self.host else {
            self.message = Some((NotifyLevel::Warning, strings::NO_PROJECT.to_owned()));
            cx.notify();
            return;
        };
        if host.repo.is_none() {
            self.message = Some((NotifyLevel::Warning, strings::GIT_NOT_A_REPO.to_owned()));
            cx.notify();
            return;
        }
        if self.git_status.is_empty() {
            self.message = Some((NotifyLevel::Info, strings::COMMIT_NO_CHANGES.to_owned()));
            cx.notify();
            return;
        }
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder(strings::COMMIT_MESSAGE_PLACEHOLDER));
        window.focus(&input.read(cx).focus_handle(cx), cx);
        eprintln!("[commit] dialog open, focused={:?}", input.read(cx).focus_handle(cx).is_focused(window));
        // Sorted by name, checked by default (the IDEA commit dialog's shape).
        let mut files: Vec<CommitFile> = self
            .git_status
            .iter()
            .map(|(path, kind)| CommitFile { path: path.clone(), name: file_name(path), kind: *kind, selected: true })
            .collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        self.commit = Some(CommitFlow { input: input.clone(), files });
        // ⌘Enter (the input's secondary-Enter) commits; plain Enter inserts
        // the newline (the input's own keymap is nearer the focus than any
        // "Commit" context of ours, so a binding would lose there).
        self.commit_sub = Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.commit_confirm(cx);
            }
        }));
        cx.notify();
    }

    /// ⌘Enter / the Commit button: stage the checked files (unchecked ones
    /// leave the index, so the commit is exactly the dialog's checkmarks),
    /// then commit the index.
    fn commit_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = &self.commit else {
            eprintln!("[commit] confirm without an open dialog");
            return;
        };
        let message = flow.input.read(cx).value().to_string();
        eprintln!(
            "[commit] confirm: message {:?} ({} chars), {} files",
            message.trim(),
            message.len(),
            flow.files.len()
        );
        if message.trim().is_empty() {
            self.message = Some((NotifyLevel::Warning, strings::COMMIT_EMPTY_MESSAGE.to_owned()));
            cx.notify();
            return;
        }
        let staged: Vec<PathBuf> =
            flow.files.iter().filter(|file| file.selected).map(|file| file.path.clone()).collect();
        let unstaged: Vec<PathBuf> =
            flow.files.iter().filter(|file| !file.selected).map(|file| file.path.clone()).collect();
        if staged.is_empty() {
            self.message = Some((NotifyLevel::Warning, strings::COMMIT_NO_CHANGES.to_owned()));
            cx.notify();
            return;
        }
        let Some(host) = self.host.clone() else { return };
        let bus = self.bus.clone();
        cx.background_executor()
            .spawn(async move {
                // Unstage first so a previously staged-then-unchecked file
                // does not ride along.
                let result = host
                    .git_unstage(&unstaged)
                    .and_then(|()| host.git_stage(&staged))
                    .and_then(|()| host.git_commit(&message));
                bus.send(BusMessage::CommitDone { result });
            })
            .detach();
    }

    fn close_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit.take().is_none() {
            return;
        }
        self.commit_sub.take();
        if self.active.is_some() {
            self.focus_active(window, cx);
        } else {
            window.focus(&self.tree_focus, cx);
        }
        cx.notify();
    }

    /// Esc outside the pane's own overlays: the focus returns to where
    /// editing happens. This is the way out of the dock's query input after
    /// ⇧⌘F (the dock is not inside the pane, so the pane's own Esc chain
    /// never sees it, and ⌘F — an Editor-context action — is out of reach).
    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.is_some() {
            self.focus_active(window, cx);
        } else {
            window.focus(&self.tree_focus, cx);
        }
        cx.notify();
    }

    /// Bare-⇧ taps, two within a short window, open Search Everywhere.
    /// Modifier-only presses never become key events (macOS turns them into
    /// `ModifiersChanged`), so the gesture lives here on the rising edge of a
    /// bare shift instead of in the keymap.
    fn on_modifiers(&mut self, ev: &ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mods = ev.modifiers;
        let shift = mods.shift;
        let was_held = self.shift_held.replace(shift);
        let bare = shift && !mods.control && !mods.alt && !mods.platform && !mods.function;
        if bare && !was_held {
            let now = Instant::now();
            let again = self
                .last_shift
                .replace(Some(now))
                .is_some_and(|at| now.duration_since(at) < Duration::from_millis(600));
            if again {
                self.last_shift.set(None);
                self.open_picker(PickerMode::Everywhere, window, cx);
            }
        }
    }

    fn open_picker(&mut self, mode: PickerMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        let placeholder = match mode {
            PickerMode::Files => strings::PICKER_FILE_PLACEHOLDER,
            PickerMode::Everywhere => strings::PICKER_EVERYWHERE_PLACEHOLDER,
            PickerMode::Run => strings::PICKER_RUN_PLACEHOLDER,
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        window.focus(&input.read(cx).presentation().focus_handle().clone(), cx);
        // Run mode caches the configuration list once at open (the detector
        // reads package.json / Cargo.toml / …; fixture-scale work).
        let configs = if mode == PickerMode::Run {
            self.host.as_ref().map(|host| host.run_configs()).unwrap_or_else(|| Ok(Vec::new())).unwrap_or_default()
        } else {
            Vec::new()
        };
        self.picker = Some(Picker {
            mode,
            input: input.clone(),
            query: String::new(),
            files: Vec::new(),
            symbols: Vec::new(),
            lsp_symbols: Vec::new(),
            actions: Vec::new(),
            configs,
            selected: 0,
        });
        self.picker_sub =
            Some(cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.picker_query_changed(input, window, cx),
                InputEvent::PressEnter { .. } => this.picker_accept(window, cx),
                _ => {}
            }));
        self.refresh_picker(cx);
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_none() {
            return;
        }
        self.picker_sub.take();
        // Focus goes back to where editing happens (the tree when none).
        if self.active.is_some() {
            self.focus_active(window, cx);
        } else {
            window.focus(&self.tree_focus, cx);
        }
        cx.notify();
    }

    fn picker_query_changed(&mut self, input: &Entity<InputState>, _window: &mut Window, cx: &mut Context<Self>) {
        let query = input.read(cx).value().to_string();
        if let Some(picker) = &mut self.picker {
            picker.query = query;
        }
        self.refresh_picker(cx);
    }

    /// Re-runs the palette's queries against the index and the action
    /// registry. Action filtering (the tiny registry scan) stays synchronous;
    /// the index calls (find_files / find_symbols / recent_files) run on the
    /// background executor behind a generation counter — a newer query bumps
    /// it and the stale answer drops, so typing never blocks on the index
    /// (the M3 debt, paid in M5 phase 7). The previous answer stays visible
    /// until the fresh one lands.
    fn refresh_picker(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = &mut self.picker else { return };
        let query = picker.query.trim().to_string();
        let mode = picker.mode;
        if mode == PickerMode::Everywhere {
            picker.actions = ranked_actions(&query);
        }
        picker.selected = 0;
        if mode == PickerMode::Run {
            cx.notify();
            return;
        }
        self.picker_gen += 1;
        let generation = self.picker_gen;
        let Some(host) = self.host.clone() else { return };
        let everywhere = mode == PickerMode::Everywhere;
        // The language server's symbol index answers alongside the text
        // index (M6): same generation guard, its own landing slot.
        if everywhere && !query.is_empty() {
            let lsp_host = host.clone();
            let runtime = lsp_host.runtime.clone();
            let lsp_query = query.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            runtime.spawn(async move {
                let result = lsp_host.lsp.workspace_symbols(&lsp_query, ide_core::CancellationToken::new()).await;
                let _ = tx.send(result);
            });
            cx.spawn(async move |this, cx| {
                if let Ok(result) = rx.await {
                    this.update(cx, |workbench, cx| {
                        if workbench.picker_gen != generation {
                            return;
                        }
                        let Ok(symbols) = result else { return };
                        let Some(picker) = workbench.picker.as_mut() else { return };
                        let Some(host) = workbench.host.as_ref() else { return };
                        let root = host.root.clone();
                        picker.lsp_symbols = symbols
                            .into_iter()
                            .map(|symbol| SymbolMatch {
                                rel_path: symbol
                                    .location
                                    .path
                                    .0
                                    .strip_prefix(&root)
                                    .unwrap_or(&symbol.location.path.0)
                                    .display()
                                    .to_string(),
                                name: symbol.name,
                                kind: symbol.kind,
                                container: symbol.container,
                                path: symbol.location.path,
                                range: symbol.location.range,
                                score: 0,
                                indices: Vec::new(),
                            })
                            .filter(|match_| {
                                !picker
                                    .symbols
                                    .iter()
                                    .any(|known| known.path == match_.path && known.range.start == match_.range.start)
                            })
                            .take(8)
                            .collect();
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move {
                    let files = if query.is_empty() {
                        host.index
                            .recent_files()
                            .into_iter()
                            .take(8)
                            .map(|path| FileMatch {
                                rel_path: path.strip_prefix(&host.root).unwrap_or(&path).display().to_string(),
                                path: ide_core::IpcPath(path),
                                score: 0,
                                indices: Vec::new(),
                            })
                            .collect()
                    } else {
                        host.index.find_files(&query, 12)
                    };
                    let symbols = if everywhere && !query.is_empty() {
                        host.index.find_symbols(&query, SymbolFilter::All, 8)
                    } else {
                        Vec::new()
                    };
                    (files, symbols)
                })
                .await;
            this.update(cx, |workbench, cx| {
                // A newer query superseded this answer: drop it.
                if workbench.picker_gen == generation
                    && let Some(picker) = &mut workbench.picker
                {
                    picker.files = answer.0;
                    picker.symbols = answer.1;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The run palette's rows: configs matching the query (name or command),
    /// plus a trailing one-off shell row for a non-empty query.
    fn run_rows(&self) -> (Vec<PickerRow>, usize) {
        let Some(picker) = &self.picker else { return (Vec::new(), 0) };
        let query = picker.query.trim();
        let mut rows: Vec<PickerRow> = picker
            .configs
            .iter()
            .filter(|config| {
                query.is_empty()
                    || fuzzy::match_text(&config.name, query).is_some()
                    || fuzzy::match_text(&config.command, query).is_some()
            })
            .cloned()
            .map(|config| PickerRow::Run(Box::new(config)))
            .collect();
        // The typed query is always runnable as a shell line.
        if !query.is_empty() {
            rows.push(PickerRow::Shell(query.to_owned()));
        }
        let shell_index = rows.len().saturating_sub(1);
        (rows, shell_index)
    }

    /// The flat selectable rows per mode: files, then symbols, then actions;
    /// or the run list.
    fn picker_rows(&self) -> Vec<PickerRow> {
        let Some(picker) = &self.picker else { return Vec::new() };
        match picker.mode {
            PickerMode::Run => self.run_rows().0,
            _ => picker
                .files
                .iter()
                .cloned()
                .map(PickerRow::File)
                .chain(picker.symbols.iter().cloned().map(PickerRow::Symbol))
                .chain(picker.lsp_symbols.iter().cloned().map(PickerRow::Symbol))
                .chain(picker.actions.iter().cloned().map(PickerRow::Action))
                .collect(),
        }
    }

    fn picker_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.picker_rows().len();
        if count == 0 {
            return;
        }
        if let Some(picker) = &mut self.picker {
            picker.selected = (picker.selected as isize + delta).clamp(0, count as isize - 1) as usize;
            cx.notify();
        }
    }

    fn picker_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.picker.as_ref().map_or(0, |picker| picker.selected);
        let Some(row) = self.picker_rows().get(selected).cloned() else { return };
        match row {
            PickerRow::File(file) => {
                let path = file.path.0;
                self.close_picker(window, cx);
                self.open_path(&path, cx);
                self.focus_active(window, cx);
            }
            PickerRow::Symbol(symbol) => {
                let (path, pos) = (symbol.path.0, symbol.range.start);
                self.close_picker(window, cx);
                self.open_path(&path, cx);
                if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                    tab.pane.update(cx, |pane, cx| pane.jump_to_offset(pos, cx));
                }
                self.focus_active(window, cx);
            }
            PickerRow::Action(action) => {
                // Close first so the dispatch lands on the restored focus
                // (dispatch_action is deferred to after this update).
                self.close_picker(window, cx);
                match cx.build_action(action.name, None) {
                    Ok(action) => window.dispatch_action(action, cx),
                    Err(e) => {
                        self.message = Some((NotifyLevel::Error, format!("{} cannot run: {}", action.title, e)));
                        cx.notify();
                    }
                }
            }
            PickerRow::Run(config) => {
                self.close_picker(window, cx);
                self.start_run(*config, cx);
            }
            PickerRow::Shell(line) => {
                self.close_picker(window, cx);
                self.start_run(shell_config(line), cx);
            }
        }
    }

    // -- run (M4) ----------------------------------------------------------
    //
    // ⌃R opens the Run palette; a picked (or typed shell) configuration
    // starts through the host on the background executor. Output streams
    // through the bus into the Run dock; the exit lands as an `IdeEvent` on
    // the same bus. One run at a time.

    fn start_run(&mut self, config: RunConfig, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.message = Some((NotifyLevel::Warning, strings::NO_PROJECT.to_owned()));
            cx.notify();
            return;
        };
        if self.run_panel.as_ref().is_some_and(|panel| panel.read(cx).running()) {
            self.message = Some((NotifyLevel::Warning, strings::RUN_ALREADY_RUNNING.to_owned()));
            cx.notify();
            return;
        }
        let name = config.name.clone();
        self.run_panel = Some(cx.new(|cx| RunPanel::new(host.clone(), name.clone(), cx)));
        self.dock_page = DockPage::Run;
        let bus = self.bus.clone();
        let on_output = {
            let bus = bus.clone();
            move |chunk: ide_run::OutputChunk| bus.send(BusMessage::RunOutput { chunk })
        };
        // start_run forks the process and spawns its own reader threads, so
        // it stays off the UI thread; the answer comes back through the bus.
        let task_name = name.clone();
        cx.background_executor()
            .spawn(async move {
                let result = host.start_run(&config, Box::new(on_output));
                bus.send(BusMessage::RunStarted { name: task_name, result });
            })
            .detach();
        // Elapsed-time ticks for the header / status bar while the run is
        // quiet (output chunks notify anyway).
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let live = this
                    .update(cx, |this, cx| {
                        let live = this.run_panel.as_ref().is_some_and(|panel| panel.read(cx).running());
                        if live {
                            cx.notify();
                        }
                        live
                    })
                    .ok();
                if live != Some(true) {
                    return;
                }
            }
        })
        .detach();
        eprintln!("[run] starting {name}");
        cx.notify();
    }

    // -- tab plumbing -----------------------------------------------------

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = self.tabs.remove(index);
        if let Some(host) = &self.host {
            host.close_document(tab.doc);
        }
        self.active = match self.tabs.len() {
            0 => None,
            len => Some(self.active.map_or(len - 1, |active| active.min(len - 1))),
        };
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = Some(index);
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Focus the active tab's editor (tab switches must not wait for a click).
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.active
            && let Some(pane) = self.tabs.get(index).map(|tab| tab.pane.clone())
        {
            window.focus(&pane.read(cx).focus_handle(cx), cx);
        }
    }

    /// Opens `path` in a tab (activating the existing one when already open).
    pub fn open_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.message = Some((NotifyLevel::Warning, strings::NO_PROJECT.to_owned()));
            cx.notify();
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.active = Some(index);
            self.tree.select(path.to_path_buf());
            cx.notify();
            return;
        }
        let info = match host.open_document(path) {
            Ok(info) => info,
            Err(e) => {
                self.message = Some((
                    NotifyLevel::Error,
                    format!("{} {}: {}", strings::LOADING_FAILED, file_name(path), e.message),
                ));
                cx.notify();
                return;
            }
        };
        let doc_id = info.id;
        let pane = cx.new(|cx| EditorPane::new(host, self.bus.clone(), info, cx));
        // Diagnostics may have landed before the tab existed (the servers
        // publish for every open file in the project).
        let existing = self.diagnostics.get(&path.to_path_buf());
        if let Some(merged) = existing.map(Self::merge_diagnostics) {
            pane.update(cx, |pane, cx| pane.set_diagnostics(merged, cx));
        }
        let path = path.to_path_buf();
        self.tabs.push(Tab { path: path.clone(), doc: doc_id, pane: pane.clone(), dirty: false, stale: false });
        self.active = Some(self.tabs.len() - 1);
        // Every open feeds the index's recency (the empty palette query lists
        // recent files).
        if let Some(host) = &self.host {
            host.index.mark_recent(&path);
        }
        self.tree.select(path);
        cx.notify();
    }

    /// All producers' diagnostics for one file, sorted by position.
    fn merge_diagnostics(by_source: &HashMap<String, Vec<Diagnostic>>) -> Vec<Diagnostic> {
        let mut merged: Vec<Diagnostic> = by_source.values().flatten().cloned().collect();
        merged.sort_by_key(|d| (d.range.start.line, d.range.start.col, d.range.end.line, d.range.end.col));
        merged
    }

    // -- bus --------------------------------------------------------------

    /// The bus pump's landing zone: backend events and the file-dialog result.
    pub fn handle_bus(&mut self, message: BusMessage, cx: &mut Context<Self>) {
        match message {
            BusMessage::Picked(path) => {
                if let Some(path) = path {
                    self.open_path(&path, cx);
                }
            }
            BusMessage::PickedFolder(root) => {
                if let Some(root) = root {
                    self.open_project(root, cx);
                }
            }
            BusMessage::Ide(IdeEvent::FsChanged { created, modified, removed, .. }) => {
                if let Some(host) = &self.host {
                    self.tree.refresh(host);
                }
                for path in created.iter().chain(&modified).chain(&removed) {
                    for tab in &mut self.tabs {
                        if tab.path == path.0 && tab.dirty {
                            tab.stale = true;
                        }
                    }
                }
                // The tree just re-listed; the git coloring follows.
                self.refresh_git(cx);
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::DocPatched { doc, base_version, changes, .. }) => {
                // A backend-originated patch reached an open tab (external
                // reload of a clean buffer). The pane reconciles by version:
                // exact base match applies incrementally, anything else
                // resyncs from the backend snapshot.
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.doc == doc) {
                    tab.pane.update(cx, |pane, cx| pane.backend_patched(base_version, changes, cx));
                }
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::DocDirty { doc, dirty }) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.doc == doc) {
                    tab.dirty = dirty;
                    if !dirty {
                        tab.stale = false;
                    }
                }
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::Notify { level, title, .. }) => {
                self.message = Some((level, title));
                cx.notify();
            }
            BusMessage::OpenLocation { path, pos } => {
                self.open_path(&path, cx);
                if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                    tab.pane.update(cx, |pane, cx| pane.jump_to_offset(pos, cx));
                }
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::Diagnostics { path, source, items }) => {
                // One producer's full replacement for one file; the pane
                // squiggles what it shows, so hand it the merged list.
                let mut by_source = self.diagnostics.remove(&path.0).unwrap_or_default();
                if items.is_empty() {
                    by_source.remove(&source);
                } else {
                    by_source.insert(source, items);
                }
                let merged = (!by_source.is_empty()).then(|| Self::merge_diagnostics(&by_source));
                if merged.is_some() {
                    self.diagnostics.insert(path.0.clone(), by_source);
                }
                if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path.0) {
                    let merged = merged.clone().unwrap_or_default();
                    tab.pane.update(cx, |pane, cx| pane.set_diagnostics(merged, cx));
                }
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::LanguageServer { server, name, status, message }) => {
                eprintln!("[lsp] {name} -> {status:?} {message:?}");
                // The status line shows one row per server; a missing binary
                // also surfaces as a message (its text carries the install
                // hint; downloading is out of M3 scope).
                if let Some(row) = self.servers.iter_mut().find(|row| row.id == server) {
                    row.status = status;
                    row.message = message.clone();
                } else {
                    self.servers.push(ServerRow { id: server, name: name.clone(), status, message: message.clone() });
                }
                if matches!(status, ServerStatus::Missing | ServerStatus::Crashed | ServerStatus::Misconfigured) {
                    self.message = Some((
                        NotifyLevel::Warning,
                        message.clone().unwrap_or_else(|| format!("{name} is not running")),
                    ));
                }
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::LanguageServerRemoved { server }) => {
                self.servers.retain(|row| row.id != server);
                cx.notify();
            }
            BusMessage::RunStarted { name, result } => {
                match result {
                    Ok(info) => {
                        if let Some(panel) = &self.run_panel {
                            panel.update(cx, |panel, cx| {
                                panel.started(&info);
                                cx.notify();
                            });
                        }
                        eprintln!("[run] started {name} (run {})", info.id.0);
                    }
                    Err(e) => {
                        if let Some(panel) = &self.run_panel {
                            panel.update(cx, |panel, cx| {
                                panel.start_failed(&e.message);
                                cx.notify();
                            });
                        }
                        self.message =
                            Some((NotifyLevel::Error, format!("{} {name}: {}", strings::RUN_LAUNCH_FAILED, e.message)));
                    }
                }
                cx.notify();
            }
            BusMessage::RunOutput { chunk } => {
                if let Some(panel) = &self.run_panel {
                    let (stream, text) = stream_of(&chunk);
                    panel.update(cx, |panel, cx| {
                        panel.append(stream, text);
                        cx.notify();
                    });
                }
            }
            BusMessage::GitStatus { statuses, branch } => {
                self.git_refreshing.set(false);
                self.git_branch = branch;
                self.git_status = statuses
                    .into_iter()
                    .map(|file| {
                        // Working-tree state is what the tree shows; a file
                        // with only a staged change still colors by it.
                        let kind = file.unstaged.or(file.staged).unwrap_or(ChangeKind::Modified);
                        (file.path.0, kind)
                    })
                    .collect();
                // Changes landed while the read was out: one more pass.
                if self.git_refresh_queued.replace(false) {
                    self.refresh_git(cx);
                }
                cx.notify();
            }
            BusMessage::DiffReady { path, title, result } => {
                match result {
                    Ok(lines) if !lines.is_empty() => {
                        eprintln!("[diff] {} — {} lines", path.display(), lines.len());
                        self.diff = Some(DiffState { title, lines });
                        self.diff_focus_pending.set(true);
                    }
                    Ok(_) => {
                        self.message = Some((NotifyLevel::Info, strings::DIFF_NO_CHANGES.to_owned()));
                    }
                    Err(e) => {
                        self.message =
                            Some((NotifyLevel::Error, format!("{} {}: {}", strings::DIFF_FAILED, title, e.message)));
                    }
                }
                cx.notify();
            }
            BusMessage::CommitDone { result } => {
                match result {
                    Ok(id) => {
                        let short: String = id.chars().take(7).collect();
                        self.commit = None;
                        self.commit_sub.take();
                        self.restore_focus_pending.set(true);
                        self.message = Some((NotifyLevel::Info, format!("{} {short}", strings::COMMIT_TOAST)));
                        eprintln!("[vcs] committed {id}");
                        self.refresh_git(cx);
                    }
                    Err(e) => {
                        self.message = Some((NotifyLevel::Error, format!("{}: {}", strings::COMMIT_FAILED, e.message)));
                    }
                }
                cx.notify();
            }
            BusMessage::FindInFilesStart { request, query } => {
                // Snapshot the dirty open buffers once (their text wins over
                // the disk for this run), then search the project on the
                // background executor. The answer lands through the bus.
                let Some(host) = self.host.clone() else {
                    if let Some(panel) = &self.find_panel {
                        panel.update(cx, |panel, cx| {
                            panel.finished(request, Vec::new(), ide_search::SearchSummary::default(), cx)
                        });
                    }
                    self.message = Some((NotifyLevel::Warning, strings::NO_PROJECT.to_owned()));
                    cx.notify();
                    return;
                };
                let overlays: std::collections::HashMap<PathBuf, String> = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.dirty)
                    .map(|tab| (tab.path.clone(), tab.pane.read(cx).buffer_text()))
                    .collect();
                let bus = self.bus.clone();
                cx.background_executor()
                    .spawn(async move {
                        let files = std::sync::Mutex::new(Vec::new());
                        let cancel = ide_core::CancellationToken::new();
                        let overlay = |path: &std::path::Path| overlays.get(path).cloned();
                        let search = ide_search::SearchQuery {
                            pattern: query.clone(),
                            is_regex: false,
                            case_sensitive: false,
                            whole_word: false,
                            include: Vec::new(),
                            exclude: Vec::new(),
                            scope: ide_search::SearchScope::Project,
                            max_matches: 10_000,
                            context: Default::default(),
                        };
                        let summary = ide_search::search(&host.root, &search, &overlay, &cancel, &|file| {
                            files.lock().expect("find files lock").push(file)
                        });
                        let summary = summary.unwrap_or_default();
                        eprintln!(
                            "[find] {query:?}: {} files / {} matches in {} ms{}",
                            summary.files,
                            summary.matches,
                            summary.elapsed_ms,
                            if summary.limit_hit { " (limit)" } else { "" }
                        );
                        let files = files.into_inner().expect("find files lock");
                        bus.send(BusMessage::FindInFilesDone { request, files, summary });
                    })
                    .detach();
            }
            BusMessage::FindInFilesDone { request, files, summary } => {
                if let Some(panel) = &self.find_panel {
                    panel.update(cx, |panel, cx| panel.finished(request, files, summary, cx));
                }
                cx.notify();
            }
            BusMessage::FindHit { path, pos, query } => {
                self.open_path(&path, cx);
                if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
                    tab.pane.update(cx, |pane, cx| {
                        pane.jump_to_offset(pos, cx);
                        pane.show_find_hits(query, cx);
                    });
                }
                // The bus pump has no window; the next render restores focus.
                self.restore_focus_pending.set(true);
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::RunExited { run, code }) => {
                eprintln!("[run] exited {run} code {code:?}");
                let mine = self.run_panel.as_ref().is_some_and(|panel| panel.read(cx).run_id == Some(run));
                if mine && let Some(panel) = &self.run_panel {
                    panel.update(cx, |panel, cx| {
                        panel.exited(code);
                        cx.notify();
                    });
                    self.message = Some((NotifyLevel::Info, format!("{} {code:?}", strings::RUN_EXITED)));
                }
                cx.notify();
            }
            BusMessage::SaveFinished { doc, name, organized, error } => {
                // ⌘S's async organize-then-write round trip landed. Clear the
                // flow guard, mirror the flags the synchronous path used to
                // set, and surface what happened (an organizer change is part
                // of the toast so the import cleanup is visible).
                if self.save_flow.get() == Some(doc) {
                    self.save_flow.set(None);
                }
                if error.is_none()
                    && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.doc == doc)
                {
                    tab.dirty = false;
                    tab.stale = false;
                }
                match &error {
                    None => {
                        let note = if organized {
                            format!("{} · {}", strings::SAVED_TOAST, strings::ORGANIZED_IMPORTS)
                        } else {
                            strings::SAVED_TOAST.to_owned()
                        };
                        self.message = Some((NotifyLevel::Info, format!("{note} {name}")));
                    }
                    Some(message) => {
                        self.message =
                            Some((NotifyLevel::Error, format!("{} {name}: {}", strings::SAVE_FAILED, message)));
                    }
                }
                eprintln!("[save] {name} organized={organized} error={error:?}");
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::Task { id, title, message, fraction, done, .. }) => {
                // gopls announces its load/index bursts here; the status bar
                // shows the live ones until their `end`.
                if done {
                    self.tasks.retain(|(live, ..)| *live != id);
                } else {
                    match self.tasks.iter_mut().find(|(live, ..)| *live == id) {
                        Some(entry) => {
                            entry.1 = title;
                            entry.2 = message;
                            entry.3 = fraction;
                        }
                        None => self.tasks.push((id, title, message, fraction)),
                    }
                }
                cx.notify();
            }
            BusMessage::Ide(_) => {}
        }
    }

    // -- file tree events -------------------------------------------------

    fn on_tree_row(
        &mut self,
        path: PathBuf,
        kind: EntryKind,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.host.clone() else { return };
        window.focus(&self.tree_focus, cx);
        self.tree.select(path.clone());
        match kind {
            // One click toggles a directory; the second click of a double
            // click is ignored so it does not toggle twice.
            EntryKind::Dir => {
                if event.click_count() == 1 {
                    self.tree.toggle(&host, &path);
                }
            }
            _ => {
                if event.click_count() >= 2 {
                    self.open_path(&path, cx);
                    self.focus_active(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn on_tree_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else { return };
        match event.keystroke.key.as_str() {
            "up" => {
                self.tree.move_selection(-1);
                cx.notify();
            }
            "down" => {
                self.tree.move_selection(1);
                cx.notify();
            }
            "enter" => {
                if let Some(selected) = self.tree.selected.clone() {
                    let is_dir = self.tree.rows().iter().any(|row| row.path == selected && row.kind == EntryKind::Dir);
                    if is_dir {
                        self.tree.toggle(&host, &selected);
                    } else {
                        self.open_path(&selected, cx);
                        self.focus_active(window, cx);
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// A one-off shell command line from the Run palette, as a configuration
/// (ide-run runs a command with shell characters through the platform shell).
fn shell_config(line: String) -> RunConfig {
    RunConfig {
        id: format!("shell:{}", ide_core::RunId::next().0),
        name: format!("{}{line}", strings::RUN_SHELL_PREFIX),
        kind: "shell".into(),
        command: line,
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
        detected: false,
        is_test: false,
        debug_adapter: None,
        allow_parallel: false,
        extras: Default::default(),
    }
}

/// The registry filtered and ranked by the local fuzzy matcher (empty query =
/// every action, registry order).
fn ranked_actions(query: &str) -> Vec<RankedAction> {
    let mut out: Vec<RankedAction> = crate::actions::REGISTRY
        .iter()
        .filter_map(|row| {
            let hit = fuzzy::match_text(row.title, query)?;
            Some(RankedAction {
                name: row.name,
                title: row.title,
                keys: row.keys,
                score: hit.score,
                indices: hit.indices,
            })
        })
        .collect();
    // Stable: equal scores keep registry order.
    out.sort_by_key(|action| std::cmp::Reverse(action.score));
    out
}

/// Text with the matched chars accented (char-index runs).
fn highlighted(text: &str, indices: &[usize]) -> gpui::AnyElement {
    let mut runs: Vec<(String, bool)> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let matched = indices.contains(&i);
        match runs.last_mut() {
            Some((run, run_matched)) if *run_matched == matched => run.push(c),
            _ => runs.push((c.to_string(), matched)),
        }
    }
    let mut row = div().flex().min_w_0();
    for (run, matched) in runs {
        let span =
            if matched { div().text_color(theme::accent()).font_weight(gpui::FontWeight::MEDIUM) } else { div() };
        row = row.child(span.child(SharedString::from(run)));
    }
    row.into_any_element()
}

fn section_header(title: &str) -> gpui::Div {
    div().px_2().pt_1().text_size(px(10.)).text_color(theme::fg_muted()).child(SharedString::from(title.to_owned()))
}

fn row_shell(content: gpui::AnyElement, secondary: Option<String>) -> gpui::Div {
    let mut row = div().h(px(24.0)).px_2().flex().items_center().gap_2().text_size(px(12.)).child(content);
    if let Some(secondary) = secondary {
        row = row.child(
            div()
                .flex_1()
                .truncate()
                .text_color(theme::fg_muted())
                .text_size(px(11.))
                .child(SharedString::from(secondary)),
        );
    } else {
        row = row.child(div().flex_1());
    }
    row
}

fn symbol_kind_label(kind: ide_core::SymbolKind) -> String {
    format!("{kind:?}").to_lowercase()
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Seed the initial focus once (a view render cannot focus earlier):
        // the tree when no tab is active, so ⌘O and the tree keys work
        // immediately after launch. The first render also kicks the git
        // status refresh.
        if !self.tree_focused_once.get() {
            self.tree_focused_once.set(true);
            if self.active.is_none() {
                window.focus(&self.tree_focus, cx);
            }
        }
        if !self.git_initialized.replace(true) {
            self.refresh_git(cx);
        }
        // A bus-side project open has no window: hand the tree the focus on
        // the next render (⌘O and the tree keys work right after opening).
        if self.tree_focus_pending.replace(false) {
            window.focus(&self.tree_focus, cx);
        }
        // The diff overlay opens from the bus (no window there); take the
        // focus on the next render so Esc lands on its Cancel handler.
        if self.diff_focus_pending.replace(false) {
            window.focus(&self.diff_focus, cx);
        }
        // A bus message closed an overlay (the commit landed): focus returns
        // to where editing happens.
        if self.restore_focus_pending.replace(false) {
            if self.active.is_some() {
                self.focus_active(window, cx);
            } else {
                window.focus(&self.tree_focus, cx);
            }
        }
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .text_color(theme::fg())
            .on_action(cx.listener(Self::on_open_file))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_go_to_file))
            .on_action(cx.listener(Self::on_search_everywhere))
            .on_action(cx.listener(Self::on_run))
            .on_action(cx.listener(Self::on_stop_run))
            .on_action(cx.listener(Self::on_find_in_files))
            .on_action(cx.listener(Self::on_show_diff))
            .on_action(cx.listener(Self::on_commit))
            .on_action(cx.listener(Self::on_cancel))
            // Double ⇧ gesture (modifier presses arrive as modifiers events).
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .children(self.host.is_none().then(|| self.render_welcome(cx)))
            .when(self.host.is_some(), |shell| {
                shell
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_row()
                            .child(self.render_tree(cx))
                            .child(self.render_editor_area(cx)),
                    )
                    // The bottom dock: Run (M4) and Find in Files (M5) as pages —
                    // the M4 single-panel look when only one exists.
                    .children(self.render_dock(cx))
            })
            .child(self.render_status(cx))
            .children(self.render_picker(cx))
            .children(self.render_diff(cx))
            .children(self.render_commit(cx))
    }
}

// -- left dock -----------------------------------------------------------------

impl Workbench {
    /// The no-project view (M7): title, the open-folder button, and the
    /// recent-projects list — the whole "open a project" flow without docs.
    fn render_welcome(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut welcome = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(96.0))
            .gap_2()
            .child(
                div()
                    .text_size(px(28.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme::fg())
                    .child(SharedString::from(strings::WELCOME_TITLE)),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::fg_muted())
                    .child(SharedString::from(strings::WELCOME_SUBTITLE)),
            )
            .child(Button::new("welcome-open").label(strings::OPEN_FOLDER).secondary().on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.open_folder_dialog(cx);
                },
            )));
        let recents: Vec<PathBuf> = self.settings.recent_projects.clone();
        if !recents.is_empty() {
            let mut list = div().mt(px(24.0)).w(px(420.0)).flex().flex_col();
            list = list.child(section_header(strings::WELCOME_RECENTS));
            for (index, path) in recents.iter().enumerate() {
                list = list.child(
                    row_shell(
                        div()
                            .truncate()
                            .text_color(theme::fg())
                            .child(SharedString::from(path.display().to_string()))
                            .into_any_element(),
                        None,
                    )
                    .id(("welcome-recent", index))
                    .hover(|row| row.bg(theme::raised_bg()))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _: &ClickEvent, _, cx| this.open_project(path.clone(), cx)
                    })),
                );
            }
            welcome = welcome.child(list);
        }
        welcome.into_any_element()
    }

    /// The open-folder dialog (the welcome button; ⌘O routes here too while
    /// no project is open). Same background-executor shape as the file dialog.
    fn open_folder_dialog(&mut self, cx: &mut Context<Self>) {
        let bus = self.bus.clone();
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(recent) = self.settings.recent_projects.first() {
            dialog = dialog.set_directory(recent);
        }
        cx.background_executor()
            .spawn(async move {
                let picked = dialog.pick_folder().await.map(|handle| handle.path().to_path_buf());
                bus.send(BusMessage::PickedFolder(picked));
            })
            .detach();
        cx.notify();
    }
}

impl Workbench {
    fn render_tree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let project =
            self.host.as_ref().map(|host| host.name.clone()).unwrap_or_else(|| strings::NO_PROJECT.to_owned());
        let rows = self.tree.rows();
        let selected = self.tree.selected.clone();

        div()
            .w_64()
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::border())
            .track_focus(&self.tree_focus)
            .on_key_down(cx.listener(Self::on_tree_key))
            .child(
                div()
                    .h_8()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(theme::fg_muted())
                    .child(SharedString::from(strings::TREE_HEADER))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).text_color(theme::fg()).child(project)),
            )
            .child(div().id("tree-scroll").flex_1().min_h_0().overflow_y_scroll().py_1().text_size(px(13.)).children(
                rows.into_iter().enumerate().map(|(index, row)| {
                    let is_selected = selected.as_ref() == Some(&row.path);
                    let indent = px(8.0 + 14.0 * row.depth as f32);
                    // A changed file's name carries its git kind's color.
                    let git_color = self.git_status.get(&row.path).and_then(|kind| theme::git_status_color(*kind));
                    div()
                        .id(("tree-row", index))
                        .h(px(24.0))
                        .pr_2()
                        .pl(indent)
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(is_selected, |this| this.bg(theme::selected_bg()))
                        .hover(|this| this.bg(theme::raised_bg()))
                        .on_click(cx.listener({
                            let path = row.path.clone();
                            let kind = row.kind;
                            move |this, event: &ClickEvent, window, cx| {
                                this.on_tree_row(path.clone(), kind, event, window, cx)
                            }
                        }))
                        .child(row_icon(&row))
                        .child(
                            div()
                                .when_some(git_color, |name, color| name.text_color(color))
                                .child(SharedString::from(row.name)),
                        )
                }),
            ))
    }

    // -- central tab stack ---------------------------------------------------------

    fn render_editor_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let area = div().flex_1().min_w_0().flex().flex_col().bg(theme::app_bg());
        match self.active {
            Some(index) => {
                let pane = self.tabs[index].pane.clone();
                let tab_bar = self.render_tab_bar(index, cx);
                area.child(tab_bar)
                    .child(div().flex_1().min_h_0().border_t_1().border_color(theme::border()).child(pane))
            }
            None => {
                let hint = if self.host.is_some() { strings::NO_FILE } else { strings::NO_PROJECT };
                area.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme::fg_muted())
                        .text_size(px(13.))
                        .child(SharedString::from(hint)),
                )
            }
        }
    }

    fn render_tab_bar(&mut self, active: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let workbench = cx.entity().downgrade();
        TabBar::new("editor-tabs")
            .selected_index(active)
            .on_click(cx.listener(|this, index: &usize, window, cx| this.activate(*index, window, cx)))
            .children(self.tabs.iter().enumerate().map(|(index, tab)| {
                let mut label = file_name(&tab.path);
                if tab.dirty {
                    label.push(' ');
                    label.push_str(strings::DIRTY_DOT);
                }
                if tab.stale {
                    label.push_str(strings::STALE_MARK);
                }
                ComponentTab::new().label(label).selected(index == active).suffix(
                    Button::new(("tab-close", index)).icon(IconName::Close).xsmall().ghost().on_click({
                        let workbench = workbench.clone();
                        move |_, window, cx| {
                            cx.stop_propagation();
                            let focused = workbench.update(cx, |this, cx| {
                                this.close_tab(index, cx);
                                this.active.and_then(|active| this.tabs.get(active).map(|tab| tab.pane.clone()))
                            });
                            if let Ok(Some(pane)) = focused {
                                window.focus(&pane.read(cx).focus_handle(cx), cx);
                            }
                        }
                    }),
                )
            }))
    }

    // -- bottom dock ---------------------------------------------------------------

    /// The bottom dock: the M4 Run panel alone, the M5 Find panel alone, or
    /// — when both exist — a two-tab page switch above the active one. The
    /// M4 look (single panel, no header, 240 px) is preserved exactly while
    /// Find has never opened.
    fn render_dock(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        match (&self.run_panel, &self.find_panel) {
            (None, None) => None,
            (Some(run), None) => Some(self.dock_shell(240.0, None, run.clone())),
            (None, Some(find)) => Some(self.dock_shell(240.0, None, find.clone())),
            (Some(run), Some(find)) => {
                let tabs = TabBar::new("dock-tabs")
                    .selected_index(if self.dock_page == DockPage::Run { 0 } else { 1 })
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.dock_page = if *index == 0 { DockPage::Run } else { DockPage::Find };
                        cx.notify();
                    }))
                    .child(ComponentTab::new().label(strings::RUN_TITLE))
                    .child(ComponentTab::new().label(strings::FIND_FILES_TITLE));
                if self.dock_page == DockPage::Run {
                    Some(self.dock_shell(276.0, Some(tabs), run.clone()))
                } else {
                    Some(self.dock_shell(276.0, Some(tabs), find.clone()))
                }
            }
        }
    }

    fn dock_shell(&self, height: f32, header: Option<TabBar>, body: impl IntoElement) -> gpui::AnyElement {
        let mut shell =
            div().h(px(height)).flex_none().min_h_0().flex().flex_col().border_t_1().border_color(theme::border());
        if let Some(header) = header {
            shell = shell.child(header);
        }
        shell.child(div().flex_1().min_h_0().child(body)).into_any_element()
    }

    // -- status bar ----------------------------------------------------------------

    fn render_status(&self, cx: &Context<Self>) -> impl IntoElement {
        let (project, file, state) = match (&self.host, self.active) {
            (Some(host), Some(index)) => {
                let tab = &self.tabs[index];
                let state = if tab.stale {
                    (strings::SAVE_STATE_STALE, theme::warning())
                } else if tab.dirty {
                    (strings::SAVE_STATE_UNSAVED, theme::accent())
                } else {
                    (strings::SAVE_STATE_SAVED, theme::fg_muted())
                };
                (host.name.clone(), Some(file_name(&tab.path)), state)
            }
            (Some(host), None) => (host.name.clone(), None, (strings::SAVE_STATE_SAVED, theme::fg_muted())),
            (None, _) => (strings::NO_PROJECT.to_owned(), None, (strings::SAVE_STATE_SAVED, theme::fg_muted())),
        };
        let message = self.message.clone().map(|(level, text)| {
            let color = match level {
                NotifyLevel::Error => theme::error(),
                NotifyLevel::Warning => theme::warning(),
                _ => theme::fg_muted(),
            };
            (text, color)
        });

        let mut left = div().flex().items_center().gap_2().text_size(px(12.));
        left = left.child(div().text_color(theme::fg_muted()).child(SharedString::from(project)));
        if let Some(branch) = &self.git_branch {
            left = left
                .child(
                    div().text_color(theme::fg_muted()).child(SharedString::from(strings::GIT_BRANCH_LABEL.to_owned())),
                )
                .child(div().text_color(theme::fg()).child(SharedString::from(branch.clone())));
        }
        if let Some(file) = file {
            left = left.child(div().text_color(theme::fg()).child(SharedString::from(file)));
        }
        let mut right = div().flex().items_center().gap_3().text_size(px(12.));
        // The run indicator (running · elapsed, or the exit code), from the
        // dock's own status.
        if let Some(panel) = &self.run_panel {
            let (text, color) = panel.read(cx).status();
            right = right.child(div().truncate().text_color(color).child(SharedString::from(text)));
        }
        // Language-server rows: name + status, colored by health.
        for row in &self.servers {
            let (text, color) = match row.status {
                ServerStatus::Ready | ServerStatus::Busy => (strings::LS_READY.to_owned(), theme::fg_muted()),
                ServerStatus::Starting => (strings::LS_STARTING.to_owned(), theme::warning()),
                ServerStatus::Missing | ServerStatus::Crashed | ServerStatus::Misconfigured => {
                    (strings::LS_DOWN.to_owned(), theme::error())
                }
                ServerStatus::Stopped => (strings::LS_STOPPED.to_owned(), theme::fg_muted()),
            };
            right = right.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_color(theme::fg()).child(SharedString::from(row.name.clone())))
                    .child(div().text_color(color).child(SharedString::from(text))),
            );
        }
        // Live server tasks (indexing, package loads): the anti-"hang" face.
        for (_, title, message, fraction) in &self.tasks {
            let mut text = message.clone().unwrap_or_else(|| title.clone());
            if let Some(fraction) = fraction {
                text = format!("{text} {}%", (fraction * 100.0).round() as u32);
            }
            right = right
                .child(div().text_color(theme::fg()).child(SharedString::from(title.clone())))
                .child(div().max_w(px(160.0)).truncate().text_color(theme::warning()).child(SharedString::from(text)));
        }
        right = right.child(div().text_color(state.1).child(SharedString::from(state.0)));
        if let Some((text, color)) = message {
            right = right.child(div().max_w_96().truncate().text_color(color).child(SharedString::from(text)));
        }
        StatusBar::new().left(left).right(right)
    }
}

// -- Go to File / Search Everywhere palette -------------------------------------

impl Workbench {
    /// The floating palette over the workbench: query input on top, grouped
    /// results below. Clicking outside closes it; clicks inside don't.
    fn render_picker(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let picker = self.picker.as_ref()?;
        let selected = picker.selected;
        let rows = self.picker_rows();
        let bounds: Vec<(&str, std::ops::Range<usize>)> = match picker.mode {
            PickerMode::Run => vec![("Run", 0..rows.len())],
            PickerMode::Files => vec![("Files", 0..rows.len())],
            PickerMode::Everywhere => vec![
                ("Files", 0..picker.files.len()),
                ("Symbols", picker.files.len()..picker.files.len() + picker.symbols.len() + picker.lsp_symbols.len()),
                ("Actions", picker.files.len() + picker.symbols.len() + picker.lsp_symbols.len()..rows.len()),
            ],
        };
        let mut list = div().id("picker-list").max_h(px(420.0)).overflow_y_scroll().py_1();
        if rows.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(theme::fg_muted())
                    .child(SharedString::from(strings::PICKER_NO_MATCHES)),
            );
        }
        for (section, range) in bounds {
            if range.is_empty() {
                continue;
            }
            list = list.child(section_header(section));
            for index in range {
                list = list.child(self.render_picker_row(&rows[index], index, index == selected, cx));
            }
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_picker(window, cx)))
                .child(
                    div()
                        .mt(px(56.0))
                        .w(px(620.0))
                        .max_h(px(520.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::raised_bg())
                        .key_context("Palette")
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .on_action(cx.listener(|this, _: &acts::SelectPrev, _, cx| this.picker_move(-1, cx)))
                        .on_action(cx.listener(|this, _: &acts::SelectNext, _, cx| this.picker_move(1, cx)))
                        .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close_picker(window, cx)))
                        .child(div().px_2().py_1().child(Input::new(&picker.input)))
                        .child(div().h(px(1.0)).w_full().bg(theme::border()))
                        .child(list),
                )
                .into_any_element(),
        )
    }

    fn render_picker_row(
        &self,
        row: &PickerRow,
        index: usize,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (content, secondary) = match row {
            PickerRow::File(file) => {
                let indices: Vec<usize> = file.indices.iter().map(|&i| i as usize).collect();
                (highlighted(&file.rel_path, &indices), None)
            }
            PickerRow::Symbol(symbol) => {
                let indices: Vec<usize> = symbol.indices.iter().map(|&i| i as usize).collect();
                let secondary = match (&symbol.container, symbol.rel_path.as_str()) {
                    (Some(container), rel) => format!("{container} · {rel}"),
                    (None, rel) => rel.to_owned(),
                };
                let content = div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_color(theme::fg_muted())
                            .text_size(px(10.))
                            .child(SharedString::from(symbol_kind_label(symbol.kind))),
                    )
                    .child(highlighted(&symbol.name, &indices));
                (content.into_any_element(), Some(secondary))
            }
            PickerRow::Action(action) => (highlighted(action.title, &action.indices), Some(action.keys.to_owned())),
            PickerRow::Run(config) => (
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(theme::fg()).child(SharedString::from(config.name.clone())))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(theme::fg_muted())
                            .text_size(px(11.))
                            .child(SharedString::from(config.command.clone())),
                    )
                    .into_any_element(),
                None,
            ),
            PickerRow::Shell(line) => (
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme::accent())
                            .child(SharedString::from(strings::RUN_SHELL_PREFIX.to_owned())),
                    )
                    .child(div().truncate().text_color(theme::fg()).child(SharedString::from(line)))
                    .into_any_element(),
                None,
            ),
        };
        row_shell(content, secondary)
            .id(("picker-row", index))
            .when(is_selected, |row| row.bg(theme::selected_bg()))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if let Some(picker) = &mut this.picker {
                    picker.selected = index;
                }
                this.picker_accept(window, cx);
            }))
            .into_any_element()
    }
}

// -- diff overlay ---------------------------------------------------------------

impl Workbench {
    /// The read-only diff overlay (⌃D): one file's unified changes against
    /// HEAD, `+`/`-` colored, no syntax highlighting (per plan). Esc or a
    /// click outside closes it.
    fn render_diff(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let diff = self.diff.as_ref()?;
        let mut body = div()
            .id("diff-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_1()
            .font_family(theme::FONT_FAMILY)
            .text_size(px(12.))
            .line_height(px(theme::FONT_SIZE * 1.35));
        if diff.lines.is_empty() {
            body = body.child(
                div().px_3().py_2().text_color(theme::fg_muted()).child(SharedString::from(strings::DIFF_NO_CHANGES)),
            );
        }
        for (index, line) in diff.lines.iter().enumerate() {
            let (marker, color, text) = match line {
                DiffLine::Hunk(text) => ("  ", theme::fg_muted(), text.clone()),
                DiffLine::Del(text) => ("- ", theme::error(), text.clone()),
                DiffLine::Add(text) => ("+ ", theme::diff_added(), text.clone()),
            };
            body = body.child(
                div()
                    .id(("diff-line", index))
                    .h(px(18.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .when(index % 2 == 1, |row| row.bg(theme::app_bg()))
                    .child(div().flex_none().w(px(18.0)).text_color(color).child(SharedString::from(marker.to_owned())))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(if matches!(line, DiffLine::Hunk(_)) { theme::fg_muted() } else { theme::fg() })
                            .child(SharedString::from(text)),
                    ),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_diff(window, cx)))
                .child(
                    div()
                        .mt(px(48.0))
                        .w(px(760.0))
                        .h(px(560.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::raised_bg())
                        .key_context("Diff")
                        .track_focus(&self.diff_focus)
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close_diff(window, cx)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .h_8()
                                .border_b_1()
                                .border_color(theme::border())
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme::fg())
                                        .child(SharedString::from(strings::DIFF_TITLE)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_color(theme::fg_muted())
                                        .child(SharedString::from(diff.title.clone())),
                                )
                                .child(Button::new("diff-close").icon(IconName::Close).xsmall().ghost().on_click(
                                    cx.listener(|this, _: &ClickEvent, window, cx| this.close_diff(window, cx)),
                                )),
                        )
                        .child(body),
                )
                .into_any_element(),
        )
    }
}

// -- commit overlay -------------------------------------------------------------

impl Workbench {
    /// The commit overlay (⌘K): checked file list, multi-line message,
    /// Commit / Cancel. ⌘Enter commits; Esc cancels.
    fn render_commit(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let flow = self.commit.as_ref()?;
        let workbench = cx.entity().downgrade();
        let mut list = div().id("commit-files").flex_1().min_h_0().overflow_y_scroll().py_1();
        for (index, file) in flow.files.iter().enumerate() {
            let color = theme::git_status_color(file.kind);
            list = list.child(
                div()
                    .id(("commit-file-row", index))
                    .h(px(26.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .hover(|row| row.bg(theme::app_bg()))
                    .child(Checkbox::new(("commit-check", index)).checked(file.selected).on_click({
                        let workbench = workbench.clone();
                        move |checked, _, cx| {
                            workbench
                                .update(cx, |this, cx| {
                                    if let Some(file) = this.commit.as_mut().and_then(|flow| flow.files.get_mut(index))
                                    {
                                        file.selected = *checked;
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .when_some(color, |name, c| name.text_color(c))
                            .child(SharedString::from(file.name.clone())),
                    ),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_commit(window, cx)))
                .child(
                    div()
                        .mt(px(72.0))
                        .w(px(560.0))
                        .h(px(480.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::raised_bg())
                        .key_context("Commit")
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close_commit(window, cx)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .h_8()
                                .border_b_1()
                                .border_color(theme::border())
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme::fg())
                                        .child(SharedString::from(strings::COMMIT_TITLE)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_color(theme::fg_muted())
                                        .child(SharedString::from(format!("{} files", flow.files.len()))),
                                )
                                .child(Button::new("commit-close").icon(IconName::Close).xsmall().ghost().on_click(
                                    cx.listener(|this, _: &ClickEvent, window, cx| this.close_commit(window, cx)),
                                )),
                        )
                        .child(list)
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .border_t_1()
                                .border_color(theme::border())
                                .child(Textarea::new(&flow.input).h_full().bordered(false)),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(theme::border())
                                .child(Button::new("commit-cancel").label(strings::CANCEL).xsmall().ghost().on_click(
                                    cx.listener(|this, _: &ClickEvent, window, cx| this.close_commit(window, cx)),
                                ))
                                .child(
                                    Button::new("commit-submit")
                                        .label(strings::COMMIT_BUTTON)
                                        .xsmall()
                                        .secondary()
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.commit_confirm(cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn row_icon(row: &Row) -> gpui::AnyElement {
    match row.kind {
        EntryKind::Dir => {
            let name = if row.expanded { IconName::FolderOpen } else { IconName::FolderClosed };
            Icon::new(name).text_color(theme::accent()).into_any_element()
        }
        _ => Icon::new(IconName::File).text_color(theme::fg_muted()).into_any_element(),
    }
}
