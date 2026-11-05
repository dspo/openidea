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
use gpui_component::WindowExt as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_component::notification::Notification;
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

/// The bottom dock's active page. Run pages are per-run tabs (M7): `Run(i)`
/// indexes `Workbench::runs`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DockPage {
    Run(usize),
    Find,
}

/// An active dock/tree drag (M7): which handle, where the press started, and
/// the size the drag grew from. Positions/sizes are plain f32 window
/// coordinates.
#[derive(Clone, Copy)]
enum Drag {
    Tree { start_x: f32, start_width: f32 },
    Dock { start_y: f32, start_height: f32 },
}

/// One open file: the editor pane, its document in the backend, and the two
/// flags the chrome shows. The pane pushes its edits to the backend live, so
/// `dirty` simply mirrors the backend's `DocDirty` events; `stale` means the
/// file changed on disk while the buffer had unsaved edits. `pinned` (M7)
/// protects the tab: ⌘W and the close button skip it.
struct Tab {
    path: PathBuf,
    doc: DocId,
    pane: Entity<EditorPane>,
    dirty: bool,
    stale: bool,
    pinned: bool,
}

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

/// What the tree prompt asks (M7): a new file / folder to create in `dir`,
/// or a rename of `renaming`.
#[derive(Clone, Copy)]
enum TreePromptKind {
    NewFile,
    NewFolder,
    Rename,
}

/// The tree prompt overlay: a one-line name input over the tree mutation
/// context. Enter confirms through ide-vfs, Esc cancels.
struct TreePrompt {
    kind: TreePromptKind,
    /// The directory a new file/folder lands in, or the file being renamed.
    target: PathBuf,
    input: Entity<InputState>,
}

/// The delete confirmation (M7): deleting moves to the OS trash, but the
/// dialog says what goes.
struct TreeConfirm {
    path: PathBuf,
    is_dir: bool,
    name: String,
    /// The dialog's own focus, seeded once rendered so Esc retreats through
    /// the chain from the topmost surface.
    focus: FocusHandle,
}

/// The settings overlay (⌘,): font size steppers, the active tab's indent
/// policy (read-only), the keymap table (read-only, from the action
/// registry). The values live in `self.settings`; the flag only paints.
struct SettingsOverlay;

/// What a toast click does (M7): a toast is a notification, but some carry
/// a jump — the failed save's editor, the failed run's dock.
#[derive(Clone, Copy)]
enum ToastClick {
    FocusEditor,
    ShowRun,
}

/// A toast's click handler face (`Notification::on_click`).
type ToastHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut gpui::App)>;

/// One queued toast: flushed to the window's notification stack at the next
/// render (the bus pump has no window).
struct Toast {
    level: NotifyLevel,
    text: String,
    click: Option<ToastClick>,
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
    /// Toasts queued for the window's notification stack (M7): the bus pump
    /// has no window, so every toast lands here and the next render flushes.
    toasts: std::cell::RefCell<Vec<Toast>>,
    /// Dedupe memory (M8): the last flush time per identical (level, text)
    /// toast, pruned as the window passes.
    toast_seen: HashMap<u64, Instant>,
    servers: Vec<ServerRow>,
    /// Diagnostics per file, per producer (`Diagnostics` events replace one
    /// producer's list; the merged view reaches the pane). Sorted by position
    /// for the pane's line lookups.
    diagnostics: HashMap<PathBuf, HashMap<String, Vec<Diagnostic>>>,
    bus: Bus,
    /// The root's own focus handle: the fallback sink when nothing else is
    /// mounted to take the focus (e.g. after ⌘1 hides a focused tree — a
    /// focus left on an unmounted element silences every action, the M3
    /// failure class). The root carries every window-level action handler.
    root_focus: FocusHandle,
    /// The open Go to File / Search Everywhere / Run palette, if any.
    picker: Option<Picker>,
    /// The input's Change / PressEnter subscription, alive while the picker is.
    picker_sub: Option<gpui::Subscription>,
    /// The palette's index-query generation (M5 phase 7): every query change
    /// bumps it on the UI thread; answers from older generations drop, so a
    /// burst of keystrokes never lands stale rows.
    picker_gen: u32,
    /// The Run dock (M4), one panel per started run (M7 multi-run tabs);
    /// output streams through the bus. A panel lives until the project closes
    /// or the user closes its tab (which stops a live run).
    runs: Vec<Entity<RunPanel>>,
    /// The Find-in-Files dock page (M5), from the first ⇧⌘F until the
    /// project closes; searches run through the bus (the workbench owns the
    /// dirty-buffer snapshot).
    find_panel: Option<Entity<FindPanel>>,
    /// The dock's active page (a run tab index, or Find).
    dock_page: DockPage,
    /// An in-flight tree/dock resize drag (M7), if any.
    drag: Cell<Option<Drag>>,
    /// Closed tab paths, most recent first (M7): ⌘⇧T pops one back open.
    /// Cleared when the project changes (paths belong to a project).
    closed_tabs: Vec<PathBuf>,
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
    /// The tree prompt overlay (M7 context menu: new file / folder, rename)
    /// and its Enter subscription. The input takes the focus once rendered
    /// (the pending seed below).
    tree_prompt: Option<TreePrompt>,
    tree_prompt_sub: Option<gpui::Subscription>,
    tree_prompt_focus_pending: Cell<bool>,
    /// The delete confirmation overlay (M7), its focus handle, and the
    /// deferred focus seed (opened from a menu click).
    tree_confirm: Option<TreeConfirm>,
    tree_confirm_focus_pending: Cell<bool>,
    /// The settings overlay (⌘,) and its focus handle (Esc closes it).
    settings_overlay: Option<SettingsOverlay>,
    settings_focus: FocusHandle,
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
    /// Whether any live task exists (M8): its edges trace
    /// `[perf] indexing on/off` for the budget scenario's correlation.
    indexing: Cell<bool>,
    /// The open floating context menu (M8): a right-click menu is
    /// element-scoped gpui-component state, invisible to the Esc chain —
    /// when anything steals its focus, Esc lands here as a bare Cancel and
    /// the menu stays up (the M7 leftover). The workbench dismisses it in
    /// that case, so Esc always closes the topmost surface. `None` when no
    /// menu (or a stale one) is tracked.
    open_menu: Option<gpui::WeakEntity<PopupMenu>>,
    /// The open-menu tracking's own subscription (M8): alive while the menu
    /// is, cleared when the menu dismisses itself.
    menu_sub: Option<gpui::Subscription>,
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
            toasts: std::cell::RefCell::new(Vec::new()),
            toast_seen: HashMap::new(),
            servers: Vec::new(),
            diagnostics: HashMap::new(),
            bus,
            root_focus: cx.focus_handle(),
            picker: None,
            picker_sub: None,
            picker_gen: 0,
            runs: Vec::new(),
            find_panel: None,
            dock_page: DockPage::Run(0),
            drag: Cell::new(None),
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
            tree_prompt: None,
            tree_prompt_sub: None,
            tree_prompt_focus_pending: Cell::new(false),
            tree_confirm: None,
            tree_confirm_focus_pending: Cell::new(false),
            settings_overlay: None,
            settings_focus: cx.focus_handle(),
            restore_focus_pending: Cell::new(false),
            last_shift: Cell::new(None),
            shift_held: Cell::new(false),
            save_flow: Cell::new(None),
            closed_tabs: Vec::new(),
            tasks: Vec::new(),
            indexing: Cell::new(false),
            open_menu: None,
            menu_sub: None,
        }
    }

    /// Registers the just-built floating menu (M8): called from every
    /// `context_menu` builder (tree, tab bar — and the editor's, through the
    /// bus, since the pane does not know the workbench). The subscription
    /// clears the tracking when the menu closes by itself (item click,
    /// click-outside, its own focused Esc), so the chain never sees a ghost.
    fn menu_opened(&mut self, menu: gpui::Entity<PopupMenu>, cx: &mut Context<Self>) {
        self.open_menu = Some(menu.downgrade());
        self.menu_sub = Some(cx.subscribe(&menu, |this, _, _: &gpui::DismissEvent, _| {
            this.open_menu = None;
        }));
    }

    /// Esc's last resort: a floating menu lost its focus (so the menu's own
    /// Esc binding never fired). Dismissing emits `DismissEvent` — the
    /// component's own subscription hides it — and focus falls through to
    /// [`Self::restore_focus`]. A stale handle (the menu already went away
    /// via click-outside / an item click) just clears.
    fn dismiss_open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.open_menu.take() else {
            eprintln!("[esc] no open menu tracked");
            return false;
        };
        let Some(menu) = menu.upgrade() else {
            eprintln!("[esc] open menu handle stale");
            return false;
        };
        eprintln!("[esc] dismissing a focus-orphaned floating menu");
        menu.update(cx, |_, cx| cx.emit(gpui::DismissEvent));
        self.restore_focus(window, cx);
        true
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

    /// Queues a toast (M7): flushed at the next render into the window's
    /// notification stack — auto-hide, stacked, replaceable by id. This is
    /// the M1 debt paid: the status message used to sit forever. `pub(crate)`
    /// so `main.rs` can seed the crash toast of the previous unclean run (M8)
    /// before the first render flushes it.
    pub(crate) fn toast(&self, level: NotifyLevel, text: impl Into<String>) {
        self.toasts.borrow_mut().push(Toast { level, text: text.into(), click: None });
    }

    /// The same, with a click target.
    fn toast_with(&self, level: NotifyLevel, text: impl Into<String>, click: ToastClick) {
        self.toasts.borrow_mut().push(Toast { level, text: text.into(), click: Some(click) });
    }

    /// Drains the queue into the window's notifications. Identical toasts
    /// inside [`TOAST_DEDUPE_WINDOW`] are suppressed (M8): a burst of saves
    /// shows one toast instead of stacking five look-alikes.
    fn flush_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let drained = std::mem::take(&mut *self.toasts.borrow_mut());
        for toast in dedupe_toasts(drained, &mut self.toast_seen, Instant::now()) {
            let mut note = match toast.level {
                NotifyLevel::Error => Notification::error(toast.text.clone()),
                NotifyLevel::Warning => Notification::warning(toast.text.clone()),
                NotifyLevel::Info => Notification::info(toast.text.clone()),
            };
            if let Some(click) = toast.click {
                let workbench = cx.entity().downgrade();
                let on_click: ToastHandler = match click {
                    ToastClick::FocusEditor => {
                        Box::new(move |_: &ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                            workbench.update(cx, |this, cx| this.focus_active(window, cx)).ok();
                        })
                    }
                    ToastClick::ShowRun => Box::new(move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                        workbench
                            .update(cx, |this, cx| {
                                if !this.runs.is_empty() {
                                    this.dock_page = DockPage::Run(this.runs.len() - 1);
                                    if !this.settings.layout.dock_visible {
                                        this.settings.layout.dock_visible = true;
                                        settings::save(&this.settings);
                                    }
                                    cx.notify();
                                }
                            })
                            .ok();
                    }),
                };
                note = note.on_click(on_click);
            }
            window.push_notification(note, cx);
        }
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
                self.toast(
                    NotifyLevel::Error,
                    format!("{} {}: {}", strings::OPEN_PROJECT_FAILED, file_name(&root), e.message),
                );
                cx.notify();
                return;
            }
        };
        cx.set_global(ProjectHost(host.clone()));
        // The outgoing project's session persists before its tabs drop (M8).
        self.save_session(cx);
        self.host = Some(host.clone());
        self.tabs.clear();
        self.active = None;
        self.diagnostics.clear();
        self.git_status.clear();
        self.git_branch = None;
        self.runs.clear();
        self.find_panel = None;
        self.dock_page = DockPage::Run(0);
        self.closed_tabs.clear();
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
        // The incoming project's recorded session restores (M8).
        self.restore_session(cx);
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
                self.toast(NotifyLevel::Info, format!("{} {name}", strings::SAVED_TOAST));
            }
            Err(e) => {
                self.toast_with(
                    NotifyLevel::Error,
                    format!("{} {name}: {}", strings::SAVE_FAILED, e.message),
                    ToastClick::FocusEditor,
                );
            }
        }
        cx.notify();
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

    /// ⌃⌘R: stops a live run — the selected run tab's, else the first live
    /// one (the dock's Stop button in keymap form).
    fn on_stop_run(&mut self, _: &StopRun, _window: &mut Window, cx: &mut Context<Self>) {
        let selected = match self.dock_page {
            DockPage::Run(index) => self.runs.get(index).cloned(),
            DockPage::Find => None,
        };
        let target = selected
            .filter(|panel| panel.read(cx).running())
            .or_else(|| self.runs.iter().find(|panel| panel.read(cx).running()).cloned());
        if let (Some(panel), Some(host)) = (target, self.host.as_ref())
            && let Some(run_id) = panel.read(cx).run_id
        {
            if let Err(e) = host.stop_run(run_id) {
                eprintln!("[run] stop: {}", e.message);
            }
            cx.notify();
        }
    }

    /// ⌘1: shows/hides the project tree (the persisted layout follows).
    /// Hiding hands the focus to a live surface (the root when no editor is
    /// open), or ⌘1 itself would go dead with it.
    fn on_toggle_tree(&mut self, _: &acts::ToggleTree, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.layout.tree_visible = !self.settings.layout.tree_visible;
        if !self.settings.layout.tree_visible {
            self.restore_focus(window, cx);
        } else if self.active.is_none() {
            window.focus(&self.tree_focus, cx);
        }
        settings::save(&self.settings);
        cx.notify();
    }

    /// ⌘4: shows/hides the bottom dock. Showing selects the last page the
    /// dock had; with no panel ever created there is nothing to show (the M4
    /// contract: the dock exists from the first run/search onward). Hiding
    /// hands the focus to a live surface.
    fn on_toggle_dock(&mut self, _: &acts::ToggleDock, window: &mut Window, cx: &mut Context<Self>) {
        if self.runs.is_empty() && self.find_panel.is_none() {
            return;
        }
        self.settings.layout.dock_visible = !self.settings.layout.dock_visible;
        self.clamp_dock_page();
        if !self.settings.layout.dock_visible {
            self.restore_focus(window, cx);
        }
        settings::save(&self.settings);
        cx.notify();
    }

    /// Keeps `dock_page` pointing at an existing run tab (tabs can close).
    fn clamp_dock_page(&mut self) {
        if let DockPage::Run(index) = self.dock_page {
            if self.runs.is_empty() {
                self.dock_page = if self.find_panel.is_some() { DockPage::Find } else { DockPage::Run(0) };
            } else if index >= self.runs.len() {
                self.dock_page = DockPage::Run(self.runs.len() - 1);
            }
        } else if self.find_panel.is_none() {
            self.dock_page = DockPage::Run(self.runs.len().saturating_sub(1));
        }
    }

    /// Hands the focus to the topmost live surface: the still-open overlays
    /// first (topmost in the render order), then the active editor pane,
    /// then the (visible) tree, then the workbench root. Every hide/close
    /// path lands here — a focus left on an unmounted element silences all
    /// action dispatch (the M3 death), so nothing may strand it.
    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prompt) = &self.tree_prompt {
            window.focus(&prompt.input.read(cx).presentation().focus_handle().clone(), cx);
        } else if let Some(confirm) = self.tree_confirm.as_ref().map(|confirm| confirm.focus.clone()) {
            window.focus(&confirm, cx);
        } else if let Some(commit) = &self.commit {
            window.focus(&commit.input.read(cx).focus_handle(cx), cx);
        } else if self.diff.is_some() {
            window.focus(&self.diff_focus, cx);
        } else if self.settings_overlay.is_some() {
            window.focus(&self.settings_focus, cx);
        } else if let Some(picker) = &self.picker {
            window.focus(&picker.input.read(cx).presentation().focus_handle().clone(), cx);
        } else if let Some(index) = self.active
            && let Some(pane) = self.tabs.get(index).map(|tab| tab.pane.clone())
        {
            window.focus(&pane.read(cx).focus_handle(cx), cx);
        } else if self.host.is_some() && self.settings.layout.tree_visible {
            window.focus(&self.tree_focus, cx);
        } else {
            window.focus(&self.root_focus, cx);
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
        if !self.settings.layout.dock_visible {
            self.settings.layout.dock_visible = true;
            settings::save(&self.settings);
        }
        if let Some(panel) = &self.find_panel {
            window.focus(&panel.read(cx).query_focus(cx), cx);
        }
        cx.notify();
    }

    /// ⌃D: the diff overlay for the changed file at hand — the tree's
    /// selected row when it carries a git change, else the active tab's file.
    fn on_show_diff(&mut self, _: &ShowDiff, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.toast(NotifyLevel::Warning, strings::NO_PROJECT.to_owned());
            cx.notify();
            return;
        };
        if host.repo.is_none() {
            self.toast(NotifyLevel::Warning, strings::GIT_NOT_A_REPO.to_owned());
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
            self.toast(NotifyLevel::Warning, strings::DIFF_TARGET_HINT.to_owned());
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
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// ⌘K: the commit overlay — every changed file checked, an empty
    /// multi-line message.
    fn on_commit(&mut self, _: &acts::Commit, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit.is_some() {
            return;
        }
        let Some(host) = &self.host else {
            self.toast(NotifyLevel::Warning, strings::NO_PROJECT.to_owned());
            cx.notify();
            return;
        };
        if host.repo.is_none() {
            self.toast(NotifyLevel::Warning, strings::GIT_NOT_A_REPO.to_owned());
            cx.notify();
            return;
        }
        if self.git_status.is_empty() {
            self.toast(NotifyLevel::Info, strings::COMMIT_NO_CHANGES.to_owned());
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
            self.toast(NotifyLevel::Warning, strings::COMMIT_EMPTY_MESSAGE.to_owned());
            cx.notify();
            return;
        }
        let staged: Vec<PathBuf> =
            flow.files.iter().filter(|file| file.selected).map(|file| file.path.clone()).collect();
        let unstaged: Vec<PathBuf> =
            flow.files.iter().filter(|file| !file.selected).map(|file| file.path.clone()).collect();
        if staged.is_empty() {
            self.toast(NotifyLevel::Warning, strings::COMMIT_NO_CHANGES.to_owned());
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
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// Esc: retreat one surface — close the topmost overlay (settings,
    /// delete confirm, tree prompt, commit, diff, picker); with none open,
    /// hand the focus back to where editing happens. The chain order mirrors
    /// the render children (topmost first). This is the way out of the
    /// dock's query input after ⇧⌘F (the dock is not inside the pane, so the
    /// pane's own Esc chain never sees it, and ⌘F — an Editor-context action
    /// — is out of reach).
    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        // The Esc-chain trace (M8): which surface is topmost and who holds the
        // focus — the soak's diagnostic for "an Esc did nothing".
        let pane_focused = self
            .active
            .and_then(|index| self.tabs.get(index))
            .is_some_and(|tab| tab.pane.read(cx).focus_handle(cx).contains_focused(window, cx));
        eprintln!(
            "[esc] cancel: prompt={} confirm={} commit={} diff={} settings={} picker={} editor={} tree={} root={} focused_id={:?}",
            self.tree_prompt.is_some(),
            self.tree_confirm.is_some(),
            self.commit.is_some(),
            self.diff.is_some(),
            self.settings_overlay.is_some(),
            self.picker.is_some(),
            pane_focused,
            self.tree_focus.contains_focused(window, cx),
            self.root_focus.contains_focused(window, cx),
            window.focused(cx)
        );
        if self.settings_overlay.is_some() {
            self.close_settings(window, cx);
        } else if self.tree_confirm.is_some() {
            self.close_tree_confirm(window, cx);
        } else if self.tree_prompt.is_some() {
            self.close_tree_prompt(window, cx);
        } else if self.commit.is_some() {
            self.close_commit(window, cx);
        } else if self.diff.is_some() {
            self.close_diff(window, cx);
        } else if self.picker.is_some() {
            self.close_picker(window, cx);
        } else if self.dismiss_open_menu(window, cx) {
            // A floating menu the Esc chain cannot see: dismissed, focus
            // re-landed by dismiss_open_menu.
        } else {
            self.restore_focus(window, cx);
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
        // Focus goes back to the topmost live surface (the editing surface
        // when the palette was the last one).
        self.restore_focus(window, cx);
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
                        self.toast(NotifyLevel::Error, format!("{} cannot run: {}", action.title, e));
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

    // -- run (M4, multi-run tabs in M7) --------------------------------------
    //
    // ⌃R opens the Run palette; a picked (or typed shell) configuration
    // starts through the host on the background executor. Output streams
    // through the bus into the run's own dock tab; the exit lands as an
    // `IdeEvent` on the same bus. Runs run in parallel, each with its tab.

    fn start_run(&mut self, config: RunConfig, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.toast(NotifyLevel::Warning, strings::NO_PROJECT.to_owned());
            cx.notify();
            return;
        };
        let name = config.name.clone();
        let index = self.runs.len();
        self.runs.push(cx.new(|cx| RunPanel::new(host.clone(), name.clone(), cx)));
        self.dock_page = DockPage::Run(index);
        if !self.settings.layout.dock_visible {
            self.settings.layout.dock_visible = true;
            settings::save(&self.settings);
        }
        let bus = self.bus.clone();
        let on_output = {
            let bus = bus.clone();
            move |chunk: ide_run::OutputChunk| bus.send(BusMessage::RunOutput { index, chunk })
        };
        // start_run forks the process and spawns its own reader threads, so
        // it stays off the UI thread; the answer comes back through the bus
        // (with the tab index, so concurrent starts cannot cross routes).
        let task_name = name.clone();
        cx.background_executor()
            .spawn(async move {
                let result = host.start_run(&config, Box::new(on_output));
                bus.send(BusMessage::RunStarted { index, name: task_name, result });
            })
            .detach();
        // Elapsed-time ticks for the header / status bar while any run is
        // quiet (output chunks notify anyway).
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let live = this
                    .update(cx, |this, cx| {
                        let live = this.runs.iter().any(|panel| panel.read(cx).running());
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

    /// Closes a run tab (the tab's close button): a live run stops first.
    fn close_run_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(panel) = self.runs.get(index) else { return };
        if let (Some(run_id), true) = (panel.read(cx).run_id, panel.read(cx).running())
            && let Some(host) = &self.host
            && let Err(e) = host.stop_run(run_id)
        {
            eprintln!("[run] stop: {}", e.message);
        }
        self.runs.remove(index);
        self.clamp_dock_page();
        cx.notify();
    }

    /// The run status the status bar shows: the selected run tab's, else the
    /// last live one, else the newest run. `None` with no runs.
    fn run_status(&self, cx: &Context<Self>) -> Option<(String, gpui::Hsla)> {
        let selected = match self.dock_page {
            DockPage::Run(index) => self.runs.get(index),
            DockPage::Find => None,
        };
        let panel = selected
            .or_else(|| self.runs.iter().find(|panel| panel.read(cx).running()))
            .or_else(|| self.runs.last())?;
        Some(panel.read(cx).status())
    }

    // -- tab plumbing -----------------------------------------------------

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = self.tabs.remove(index);
        if let Some(host) = &self.host {
            host.close_document(tab.doc);
        }
        push_closed(&mut self.closed_tabs, tab.path);
        self.active = match self.tabs.len() {
            0 => None,
            len => Some(self.active.map_or(len - 1, |active| active.min(len - 1))),
        };
        self.save_session(cx);
        cx.notify();
    }

    /// ⌘W: closes the active tab — never a pinned one (the tab's own close
    /// button is hidden for pinned tabs too).
    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else { return };
        if self.tabs[index].pinned {
            return;
        }
        self.close_tab(index, cx);
        self.focus_active(window, cx);
    }

    /// ⌘⇧T: reopens the most recently closed tab (per-project stack).
    fn on_reopen_closed_tab(&mut self, _: &acts::ReopenClosedTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.closed_tabs.pop() else { return };
        self.open_path(&path, cx);
        self.focus_active(window, cx);
    }

    /// The tab's context menu: pin / unpin (M7). Pinned tabs skip ⌘W and the
    /// close button, so a misclick cannot lose them.
    fn toggle_pin(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.pinned = !tab.pinned;
            cx.notify();
        }
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = Some(index);
            self.save_session(cx);
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
        } else {
            // No active pane (the last tab just closed): fall back through
            // the chain — a focus left on the unmounted pane silences every
            // action (the M3 death class; caught live in M7).
            self.restore_focus(window, cx);
        }
    }

    /// Opens `path` in a tab (activating the existing one when already open).
    pub fn open_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.toast(NotifyLevel::Warning, strings::NO_PROJECT.to_owned());
            cx.notify();
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.active = Some(index);
            self.save_session(cx);
            self.tree.select(path.to_path_buf());
            cx.notify();
            return;
        }
        let info = match host.open_document(path) {
            Ok(info) => info,
            Err(e) => {
                self.toast(
                    NotifyLevel::Error,
                    format!("{} {}: {}", strings::LOADING_FAILED, file_name(path), e.message),
                );
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
        self.tabs.push(Tab {
            path: path.clone(),
            doc: doc_id,
            pane: pane.clone(),
            dirty: false,
            stale: false,
            pinned: false,
        });
        self.active = Some(self.tabs.len() - 1);
        // Every open feeds the index's recency (the empty palette query lists
        // recent files).
        if let Some(host) = &self.host {
            host.index.mark_recent(&path);
        }
        self.save_session(cx);
        self.tree.select(path);
        cx.notify();
    }

    // -- session restore (M8) ---------------------------------------------
    //
    // The open tabs, the active one and each caret persist per project root
    // and restore at startup / project open. Writes ride the structural
    // events only (tab open/close/activate, project switch, graceful quit)
    // — never typing, so the settings file never sees per-keystroke traffic
    // (the M7 write-amplification watchpoint).

    /// Records the open tabs + carets for the current root into the session
    /// map and persists the store. The carets are read from the panes HERE —
    /// cursor moves stay in memory (in the panes) between events.
    pub fn save_session(&mut self, cx: &mut Context<Self>) {
        // Scripted determinism comes from OPENIDEA_GUI_SETTINGS_DIR (each
        // harness session owns an isolated store), not from an env guard:
        // a guard here would block the save/restore round trip the restore
        // leg exists to exercise.
        let Some(host) = &self.host else { return };
        let root = host.root.clone();
        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let pos = tab.pane.read(cx).cursor_pos();
                settings::TabSession { path: tab.path.clone(), cursor_line: pos.line, cursor_col_utf16: pos.col }
            })
            .collect();
        settings::set_session(&mut self.settings.session, &root, settings::SessionTabs { tabs, active: self.active });
        settings::save(&self.settings);
    }

    /// Reopens the recorded session for the current root: files that still
    /// exist, in recorded order, the recorded active tab, each pane's caret
    /// placed and scrolled into view. Gone files shrink the entry on the
    /// next save. No-op for the scripted-session override.
    pub fn restore_session(&mut self, cx: &mut Context<Self>) {
        let Some(host) = &self.host else { return };
        let Some(entry) = self.settings.session.get(&settings::session_key(&host.root)).cloned() else { return };
        eprintln!("[session] restoring {} tab(s)", entry.tabs.len());
        for tab in &entry.tabs {
            if !tab.path.is_file() {
                eprintln!("[session] gone, skipping: {}", tab.path.display());
                continue;
            }
            self.open_path(&tab.path, cx);
            if let Some(last) = self.tabs.last() {
                let pos = ide_core::Pos::new(tab.cursor_line, tab.cursor_col_utf16);
                last.pane.update(cx, |pane, cx| pane.jump_to_offset(pos, cx));
            }
        }
        // The recorded active wins over "last opened" (open_path activates
        // each tab as it opens); a skipped file may have shifted the
        // numbering, so the index clamps to what actually opened.
        if let Some(index) = entry.active
            && index < self.tabs.len()
        {
            self.active = Some(index);
        }
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
                self.toast(level, title);
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
                    self.toast(
                        NotifyLevel::Warning,
                        message.clone().unwrap_or_else(|| format!("{name} is not running")),
                    );
                }
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::LanguageServerRemoved { server }) => {
                self.servers.retain(|row| row.id != server);
                cx.notify();
            }
            BusMessage::RunStarted { index, name, result } => {
                match result {
                    Ok(info) => {
                        if let Some(panel) = self.runs.get(index) {
                            panel.update(cx, |panel, cx| {
                                panel.started(&info);
                                cx.notify();
                            });
                        }
                        eprintln!("[run] started {name} (run {})", info.id.0);
                    }
                    Err(e) => {
                        if let Some(panel) = self.runs.get(index) {
                            panel.update(cx, |panel, cx| {
                                panel.start_failed(&e.message);
                                cx.notify();
                            });
                        }
                        self.toast_with(
                            NotifyLevel::Error,
                            format!("{} {name}: {}", strings::RUN_LAUNCH_FAILED, e.message),
                            ToastClick::ShowRun,
                        );
                    }
                }
                cx.notify();
            }
            BusMessage::RunOutput { index, chunk } => {
                if let Some(panel) = self.runs.get(index) {
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
                        self.toast(NotifyLevel::Info, strings::DIFF_NO_CHANGES.to_owned());
                    }
                    Err(e) => {
                        self.toast(NotifyLevel::Error, format!("{} {}: {}", strings::DIFF_FAILED, title, e.message));
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
                        self.toast(NotifyLevel::Info, format!("{} {short}", strings::COMMIT_TOAST));
                        eprintln!("[vcs] committed {id}");
                        self.refresh_git(cx);
                    }
                    Err(e) => {
                        self.toast(NotifyLevel::Error, format!("{}: {}", strings::COMMIT_FAILED, e.message));
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
                    self.toast(NotifyLevel::Warning, strings::NO_PROJECT.to_owned());
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
                            // Poison recovery: a panic mid-search must not turn
                            // every later search into a panic; the partial list
                            // is still usable data.
                            files.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(file)
                        });
                        let summary = summary.unwrap_or_default();
                        eprintln!(
                            "[find] {query:?}: {} files / {} matches in {} ms{}",
                            summary.files,
                            summary.matches,
                            summary.elapsed_ms,
                            if summary.limit_hit { " (limit)" } else { "" }
                        );
                        let files = files.into_inner().unwrap_or_default();
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
            BusMessage::FsCreated { path, is_dir, error } => {
                match error {
                    None => {
                        if !is_dir {
                            self.open_path(&path, cx);
                        }
                    }
                    Some(message) => {
                        self.toast(
                            NotifyLevel::Error,
                            format!("{} {}: {}", strings::FS_CREATE_FAILED, file_name(&path), message),
                        );
                    }
                }
                if let Some(host) = &self.host {
                    self.tree.refresh(host);
                }
                cx.notify();
            }
            BusMessage::FsRenamed { from, to, error } => {
                match error {
                    None => self.open_path(&to, cx),
                    Some(message) => {
                        self.toast(
                            NotifyLevel::Error,
                            format!("{} {}: {}", strings::FS_RENAME_FAILED, file_name(&from), message),
                        );
                    }
                }
                if let Some(host) = &self.host {
                    self.tree.refresh(host);
                }
                cx.notify();
            }
            BusMessage::FsDeleted { error } => {
                if let Some(message) = error {
                    self.toast(NotifyLevel::Error, format!("{}: {}", strings::FS_DELETE_FAILED, message));
                }
                if let Some(host) = &self.host {
                    self.tree.refresh(host);
                }
                self.refresh_git(cx);
                cx.notify();
            }
            BusMessage::MenuOpened { menu } => {
                if let Some(menu) = menu.upgrade() {
                    self.menu_opened(menu, cx);
                }
            }
            BusMessage::Ide(IdeEvent::RunExited { run, code }) => {
                eprintln!("[run] exited {run} code {code:?}");
                let mine = self.runs.iter().find(|panel| panel.read(cx).run_id == Some(run)).cloned();
                if let Some(panel) = mine {
                    panel.update(cx, |panel, cx| {
                        panel.exited(code);
                        cx.notify();
                    });
                    self.toast(NotifyLevel::Info, format!("{} {code:?}", strings::RUN_EXITED));
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
                        self.toast(NotifyLevel::Info, format!("{note} {name}"));
                    }
                    Some(message) => {
                        self.toast_with(
                            NotifyLevel::Error,
                            format!("{} {name}: {}", strings::SAVE_FAILED, message),
                            ToastClick::FocusEditor,
                        );
                    }
                }
                eprintln!("[save] {name} organized={organized} error={error:?}");
                cx.notify();
            }
            BusMessage::Ide(IdeEvent::Task { id, title, message, fraction, done, .. }) => {
                // gopls announces its load/index bursts here; the status bar
                // shows the live ones until their `end`. The indexing window
                // is traced (M8) so the budget scenario can correlate the
                // scroll-frame samples with the first big index.
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
                let indexing = !self.tasks.is_empty();
                if indexing != self.indexing.replace(indexing) {
                    eprintln!("[perf] indexing {}", if indexing { "on" } else { "off" });
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

    // -- tree mutations (M7 context menu; all IO through ide-vfs) ----------

    /// The directory a new file/folder lands in: the row itself for a
    /// directory, its parent for a file.
    fn target_dir(row_path: &Path, is_dir: bool) -> PathBuf {
        if is_dir {
            row_path.to_path_buf()
        } else {
            row_path.parent().map(Path::to_path_buf).unwrap_or_else(|| row_path.to_path_buf())
        }
    }

    /// Opens the New File prompt for `dir`.
    fn begin_new_file(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tree_prompt(TreePromptKind::NewFile, dir, window, cx);
    }

    /// Opens the New Folder prompt for `dir`.
    fn begin_new_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tree_prompt(TreePromptKind::NewFolder, dir, window, cx);
    }

    /// Opens the Rename prompt prefilled with the file's current name.
    fn begin_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tree_prompt(TreePromptKind::Rename, path, window, cx);
    }

    fn open_tree_prompt(&mut self, kind: TreePromptKind, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree_prompt.is_some() || self.tree_confirm.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx));
        // Rename prefills the current name, pre-selected (M8): typing
        // replaces it, IDEA-style, instead of appending after it.
        if matches!(kind, TreePromptKind::Rename) {
            let name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let len = name.len();
            input.update(cx, |state, cx| {
                state.set_value(name, window, cx);
                state.set_selected_range(0..len, cx);
            });
        }
        self.tree_prompt = Some(TreePrompt { kind, target, input: input.clone() });
        self.tree_prompt_sub = Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.tree_prompt_confirm(window, cx);
            }
        }));
        self.tree_prompt_focus_pending.set(true);
        cx.notify();
    }

    /// Closes the prompt without a mutation (Esc or the ✕).
    fn close_tree_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree_prompt.take().is_none() {
            return;
        }
        self.tree_prompt_sub.take();
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// Enter in the tree prompt: run the ide-vfs mutation on the background
    /// executor. Creation answers through `FsCreated` (a file opens in a
    /// tab); rename saves the dirty buffer first, detaches its tab without
    /// touching the ⌘⇧T stack, and `FsRenamed` reopens the new path.
    fn tree_prompt_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.tree_prompt.take() else { return };
        self.tree_prompt_sub.take();
        self.restore_focus(window, cx);
        let name = prompt.input.read(cx).value().trim().to_string();
        let Some(host) = self.host.clone() else { return };
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            cx.notify();
            return;
        }
        let bus = self.bus.clone();
        match prompt.kind {
            TreePromptKind::NewFile => {
                let path = prompt.target.join(&name);
                cx.background_executor()
                    .spawn(async move {
                        let error = host.create_file(&path).err().map(|e| e.message);
                        bus.send(BusMessage::FsCreated { path, is_dir: false, error });
                    })
                    .detach();
            }
            TreePromptKind::NewFolder => {
                let path = prompt.target.join(&name);
                cx.background_executor()
                    .spawn(async move {
                        let error = host.create_dir(&path).err().map(|e| e.message);
                        bus.send(BusMessage::FsCreated { path, is_dir: true, error });
                    })
                    .detach();
            }
            TreePromptKind::Rename => {
                let to = prompt.target.parent().map(|dir| dir.join(&name)).unwrap_or_else(|| prompt.target.clone());
                if to == prompt.target {
                    cx.notify();
                    return;
                }
                // A dirty buffer is saved first: after the rename the tab
                // reopens from the new path with everything the user typed.
                if let Some(index) = self.tabs.iter().position(|tab| tab.path == prompt.target)
                    && self.tabs[index].dirty
                    && let Err(e) = host.save_document(self.tabs[index].doc)
                {
                    self.toast_with(
                        NotifyLevel::Error,
                        format!("{} {}: {}", strings::SAVE_FAILED, file_name(&prompt.target), e.message),
                        ToastClick::FocusEditor,
                    );
                    cx.notify();
                    return;
                }
                self.detach_tab_at(&prompt.target, cx);
                let (from, host2) = (prompt.target.clone(), host.clone());
                cx.background_executor()
                    .spawn(async move {
                        let error = host2.rename(&from, &to).err().map(|e| e.message);
                        bus.send(BusMessage::FsRenamed { from, to, error });
                    })
                    .detach();
            }
        }
        cx.notify();
    }

    /// Detaches the tab of `path` (no ⌘⇧T entry): the rename and delete
    /// flows' tab half. `close_tab` is the ⌘W face; this is the model half.
    fn detach_tab_at(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.path == path) else { return };
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

    /// Closes every tab under `path` (a deleted or renamed directory's
    /// affected files), without ⌘⇧T entries.
    fn detach_tabs_under(&mut self, path: &Path, cx: &mut Context<Self>) {
        let affected: Vec<PathBuf> =
            self.tabs.iter().map(|tab| tab.path.clone()).filter(|p| p.starts_with(path)).collect();
        for p in affected {
            self.detach_tab_at(&p, cx);
        }
    }

    /// Opens the delete confirmation for a row.
    fn begin_delete(&mut self, path: PathBuf, is_dir: bool, cx: &mut Context<Self>) {
        if self.tree_prompt.is_some() || self.tree_confirm.is_some() {
            return;
        }
        self.tree_confirm = Some(TreeConfirm { is_dir, name: file_name(&path), path, focus: cx.focus_handle() });
        self.tree_confirm_focus_pending.set(true);
        cx.notify();
    }

    fn close_tree_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree_confirm.take().is_none() {
            return;
        }
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// The dialog's Delete button: close affected tabs, then move the path
    /// to the OS trash on the background executor (`FsDeleted` lands after).
    fn tree_confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirm) = self.tree_confirm.take() else { return };
        self.restore_focus(window, cx);
        let Some(host) = self.host.clone() else { return };
        self.detach_tabs_under(&confirm.path, cx);
        let bus = self.bus.clone();
        cx.background_executor()
            .spawn(async move {
                let error = host.delete(&confirm.path).err().map(|e| e.message);
                bus.send(BusMessage::FsDeleted { error });
            })
            .detach();
        cx.notify();
    }

    // -- settings overlay (M7) ---------------------------------------------

    /// ⌘,: the settings overlay. One at a time; the panel takes the focus so
    /// Esc lands on its Cancel handler.
    fn on_open_settings(&mut self, _: &acts::OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_overlay.is_some() {
            return;
        }
        self.settings_overlay = Some(SettingsOverlay);
        window.focus(&self.settings_focus, cx);
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_overlay.take().is_none() {
            return;
        }
        self.restore_focus(window, cx);
        cx.notify();
    }

    /// The font-size steppers: clamp, persist, re-metric. The panes re-read
    /// `theme::px_font_size` on their next render, so each is notified here.
    fn step_font(&mut self, delta: f32, cx: &mut Context<Self>) {
        let next = step_font_size(self.settings.font_size, delta);
        if next == self.settings.font_size {
            return;
        }
        self.settings.font_size = next;
        theme::set_font_size(next);
        settings::save(&self.settings);
        for tab in &self.tabs {
            tab.pane.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// The settings overlay's indent row: the active tab's language policy
    /// from the registry (a read-only display; the pane's editorconfig
    /// override still wins at edit time), or the default note without a file.
    fn indent_row_text(&self) -> String {
        let Some(index) = self.active else { return strings::SETTINGS_INDENT_DEFAULT.to_owned() };
        let Some(host) = &self.host else { return strings::SETTINGS_INDENT_DEFAULT.to_owned() };
        let path = &self.tabs[index].path;
        let id = host.languages.detect(path, None);
        host.languages.get(&id).map_or_else(
            || strings::SETTINGS_INDENT_DEFAULT.to_owned(),
            |config| format!("{} — {}", config.name, indent_label(config.indent)),
        )
    }

    /// The settings panel (⌘,): Esc or a click outside closes; changes
    /// persist immediately.
    fn render_settings(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.settings_overlay.as_ref()?;
        let font = self.settings.font_size;
        let indent = self.indent_row_text();
        let mut table = div().id("settings-keymap").flex_1().min_h_0().overflow_y_scroll().pb_2();
        for row in acts::REGISTRY {
            table = table.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .h(px(26.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div().text_size(px(12.)).text_color(theme::fg()).child(SharedString::from(row.title)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::fg_muted())
                                    .child(SharedString::from(row.category)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme::FONT_FAMILY)
                            .text_size(px(11.))
                            .text_color(theme::fg_muted())
                            .child(SharedString::from(row.keys)),
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
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_settings(window, cx)))
                .child(
                    div()
                        .mt(px(48.0))
                        .w(px(560.0))
                        .h(px(560.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::raised_bg())
                        .key_context("Settings")
                        .track_focus(&self.settings_focus)
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close_settings(window, cx)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .px_3()
                                .h_8()
                                .border_b_1()
                                .border_color(theme::border())
                                .text_size(px(12.))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme::fg())
                                        .child(SharedString::from(strings::SETTINGS_TITLE)),
                                ),
                        )
                        // Font size: steppers around the live value; a change
                        // re-metrics the panes on their next render.
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .h_9()
                                .border_b_1()
                                .border_color(theme::border())
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::fg())
                                        .child(SharedString::from(strings::SETTINGS_FONT)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Button::new("font-minus").icon(IconName::Minus).xsmall().ghost().on_click(
                                                cx.listener(|this, _: &ClickEvent, _, cx| this.step_font(-1.0, cx)),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .w(px(36.0))
                                                .flex()
                                                .justify_center()
                                                .font_family(theme::FONT_FAMILY)
                                                .text_size(px(12.))
                                                .text_color(theme::fg())
                                                .child(SharedString::from(format!("{font:.0}"))),
                                        )
                                        .child(
                                            Button::new("font-plus").icon(IconName::Plus).xsmall().ghost().on_click(
                                                cx.listener(|this, _: &ClickEvent, _, cx| this.step_font(1.0, cx)),
                                            ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .h_9()
                                .border_b_1()
                                .border_color(theme::border())
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::fg())
                                        .child(SharedString::from(strings::SETTINGS_INDENT)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme::fg_muted())
                                        .child(SharedString::from(indent)),
                                ),
                        )
                        .child(section_header(strings::SETTINGS_KEYMAP))
                        .child(table),
                )
                .into_any_element(),
        )
    }
}

/// The display-quoted name the dialogs show.
fn quoted(name: &str) -> String {
    format!("“{name}”")
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// One font-size step, clamped to the settings' range.
fn step_font_size(current: f32, delta: f32) -> f32 {
    let (min, max) = (*settings::FONT_SIZE_RANGE.start(), *settings::FONT_SIZE_RANGE.end());
    (current + delta).clamp(min, max)
}

/// The settings overlay's indent label for a language's policy (read-only;
/// the editor's editorconfig override still wins at edit time).
fn indent_label(indent: Option<ide_language::IndentConfig>) -> String {
    match indent {
        Some(config) if config.insert_spaces => format!("{}-space indent", config.tab_size),
        Some(config) => format!("tabs, width {}", config.tab_size),
        None => "4-space indent (default)".to_owned(),
    }
}

/// Records a closed tab's path for ⌘⇧T — most recent first, deduplicated,
/// capped (`CLOSED_TAB_CAP`).
fn push_closed(stack: &mut Vec<PathBuf>, path: PathBuf) {
    stack.retain(|p| p != &path);
    stack.insert(0, path);
    stack.truncate(CLOSED_TAB_CAP);
}

/// How many closed tabs ⌘⇧T remembers.
const CLOSED_TAB_CAP: usize = 20;

/// The toast dedupe window (M8): an identical (level, text) toast inside it
/// is suppressed. Matches the notification stack's auto-hide, so the window
/// covers exactly "the look-alike toast is still on screen".
const TOAST_DEDUPE_WINDOW: Duration = Duration::from_secs(5);

/// The dedupe key: severity plus the full text.
fn toast_key(level: NotifyLevel, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (level as u8).hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

/// The flush filter (pure for the unit test): drops toasts identical to one
/// already shown inside the window, records the survivors, and prunes the
/// memory of entries whose window has passed.
fn dedupe_toasts(toasts: Vec<Toast>, seen: &mut HashMap<u64, Instant>, now: Instant) -> Vec<Toast> {
    seen.retain(|_, at| now.duration_since(*at) < TOAST_DEDUPE_WINDOW);
    let mut kept = Vec::with_capacity(toasts.len());
    for toast in toasts {
        let key = toast_key(toast.level, &toast.text);
        if seen.get(&key).is_some_and(|at| now.duration_since(*at) < TOAST_DEDUPE_WINDOW) {
            continue;
        }
        seen.insert(key, now);
        kept.push(toast);
    }
    kept
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
        // where editing happens — the tree when no tab is active, so ⌘O and
        // the tree keys work immediately after launch (the root when the
        // persisted layout hides the tree). The first render also kicks the
        // git status refresh.
        if !self.tree_focused_once.get() {
            self.tree_focused_once.set(true);
            self.restore_focus(window, cx);
        }
        if !self.git_initialized.replace(true) {
            self.refresh_git(cx);
        }
        // A bus-side project open has no window: hand the focus on the next
        // render (⌘O and the tree keys work right after opening).
        if self.tree_focus_pending.replace(false) {
            self.restore_focus(window, cx);
        }
        // The tree prompt (context menu) takes the focus once rendered.
        if self.tree_prompt_focus_pending.replace(false)
            && let Some(prompt) = &self.tree_prompt
        {
            window.focus(&prompt.input.read(cx).presentation().focus_handle().clone(), cx);
        }
        // The delete confirmation (context menu) takes the focus once
        // rendered, so Esc retreats from it.
        if self.tree_confirm_focus_pending.replace(false)
            && let Some(confirm) = &self.tree_confirm
        {
            window.focus(&confirm.focus.clone(), cx);
        }
        // The diff overlay opens from the bus (no window there); take the
        // focus on the next render so Esc lands on its Cancel handler.
        if self.diff_focus_pending.replace(false) {
            window.focus(&self.diff_focus, cx);
        }
        // The toast queue drains here (the window exists from now on).
        self.flush_toasts(window, cx);
        // A bus message closed an overlay (the commit landed): focus returns
        // to the topmost live surface.
        if self.restore_focus_pending.replace(false) {
            self.restore_focus(window, cx);
        }
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .text_color(theme::fg())
            // The fallback focus sink (see `root_focus`): keeps action
            // dispatch alive whenever nothing else holds the focus.
            .track_focus(&self.root_focus)
            // Dock/tree resize drags (M7): the handles arm the drag; the move
            // drives it; the release ends it and persists the layout.
            .on_mouse_move(cx.listener(Self::on_drag_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_drag_end))
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
            .on_action(cx.listener(Self::on_toggle_tree))
            .on_action(cx.listener(Self::on_toggle_dock))
            .on_action(cx.listener(Self::on_reopen_closed_tab))
            .on_action(cx.listener(Self::on_open_settings))
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
                            .children(self.render_tree(cx))
                            .child(self.render_tree_handle(cx))
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
            .children(self.render_tree_prompt(cx))
            .children(self.render_tree_confirm(cx))
            .children(self.render_settings(cx))
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
    /// The left dock tree; `None` while hidden (⌘1 / persisted layout).
    fn render_tree(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.settings.layout.tree_visible {
            return None;
        }
        let project =
            self.host.as_ref().map(|host| host.name.clone()).unwrap_or_else(|| strings::NO_PROJECT.to_owned());
        let rows = self.tree.rows();
        let selected = self.tree.selected.clone();

        Some(
            div()
                .w(px(self.settings.layout.tree_width))
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
                .child(
                    div().id("tree-scroll").flex_1().min_h_0().overflow_y_scroll().py_1().text_size(px(13.)).children(
                        rows.into_iter().enumerate().map(|(index, row)| {
                            let is_selected = selected.as_ref() == Some(&row.path);
                            let indent = px(8.0 + 14.0 * row.depth as f32);
                            // A changed file's name carries its git kind's color.
                            let git_color =
                                self.git_status.get(&row.path).and_then(|kind| theme::git_status_color(*kind));
                            // The row's context menu (M7): new file / folder
                            // target the row's directory, rename / delete
                            // the row itself (the root row hides the latter
                            // two). All IO goes through ide-vfs (Host).
                            let is_root = self.host.as_ref().is_some_and(|host| row.path == host.root);
                            let workbench = cx.entity().downgrade();
                            let row_path = row.path.clone();
                            let is_dir = row.kind == EntryKind::Dir;
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
                                .context_menu(move |menu, window, cx| {
                                    eprintln!(
                                        "[menu] tree build: focused_present={} menu_focus={:?}",
                                        window.focused(cx).is_some(),
                                        menu.focus_handle(cx)
                                    );
                                    // Track the menu for the Esc chain (M8): when
                                    // something steals its focus, Esc lands on the
                                    // workbench as a bare Cancel and dismisses it.
                                    let built = cx.entity();
                                    workbench.update(cx, |this, cx| this.menu_opened(built, cx)).ok();
                                    let menu = menu
                                        .item(PopupMenuItem::new(strings::TREE_NEW_FILE).on_click({
                                            let workbench = workbench.clone();
                                            let dir = Workbench::target_dir(&row_path, is_dir);
                                            move |_, window, cx| {
                                                workbench
                                                    .update(cx, |this, cx| this.begin_new_file(dir.clone(), window, cx))
                                                    .ok();
                                            }
                                        }))
                                        .item(PopupMenuItem::new(strings::TREE_NEW_FOLDER).on_click({
                                            let workbench = workbench.clone();
                                            let dir = Workbench::target_dir(&row_path, is_dir);
                                            move |_, window, cx| {
                                                workbench
                                                    .update(cx, |this, cx| {
                                                        this.begin_new_folder(dir.clone(), window, cx)
                                                    })
                                                    .ok();
                                            }
                                        }));
                                    if is_root {
                                        return menu;
                                    }
                                    menu.separator()
                                        .item(PopupMenuItem::new(strings::TREE_RENAME).on_click({
                                            let workbench = workbench.clone();
                                            let path = row_path.clone();
                                            move |_, window, cx| {
                                                workbench
                                                    .update(cx, |this, cx| this.begin_rename(path.clone(), window, cx))
                                                    .ok();
                                            }
                                        }))
                                        .item(PopupMenuItem::new(strings::TREE_DELETE).on_click({
                                            let workbench = workbench.clone();
                                            let path = row_path.clone();
                                            move |_, _, cx| {
                                                workbench
                                                    .update(cx, |this, cx| this.begin_delete(path.clone(), is_dir, cx))
                                                    .ok();
                                            }
                                        }))
                                })
                                .child(row_icon(&row))
                                .child(
                                    div()
                                        .when_some(git_color, |name, color| name.text_color(color))
                                        .child(SharedString::from(row.name)),
                                )
                                .into_any_element()
                        }),
                    ),
                )
                .into_any_element(),
        )
    }

    /// The 4 px drag handle between tree and editor (M7): arms the width
    /// drag.
    fn render_tree_handle(&self, cx: &Context<Self>) -> gpui::AnyElement {
        div()
            .w(px(4.0))
            .h_full()
            .flex_none()
            .cursor_pointer()
            .hover(|bar| bar.bg(theme::accent()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_tree_handle_down))
            .into_any_element()
    }

    /// Arms the tree-width drag (the move drives it from the window root).
    fn on_tree_handle_down(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        self.drag.set(Some(Drag::Tree {
            start_x: f32::from(event.position.x),
            start_width: self.settings.layout.tree_width,
        }));
        window.set_window_cursor_style(gpui::CursorStyle::ResizeLeftRight);
    }

    /// Arms the dock-height drag.
    fn on_dock_handle_down(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, _: &mut Context<Self>) {
        self.drag.set(Some(Drag::Dock {
            start_y: f32::from(event.position.y),
            start_height: self.settings.layout.dock_height,
        }));
        window.set_window_cursor_style(gpui::CursorStyle::ResizeUpDown);
    }

    /// Drives an armed drag (the window root sees every move through the
    /// children). The dock grows upward: dragging toward the top edge
    /// enlarges it.
    fn on_drag_move(&mut self, event: &gpui::MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.get() else { return };
        match drag {
            Drag::Tree { start_x, start_width } => {
                let width = start_width + (f32::from(event.position.x) - start_x);
                self.settings.layout.tree_width =
                    width.clamp(*settings::TREE_WIDTH_RANGE.start(), *settings::TREE_WIDTH_RANGE.end());
            }
            Drag::Dock { start_y, start_height } => {
                let height = start_height - (f32::from(event.position.y) - start_y);
                self.settings.layout.dock_height =
                    height.clamp(*settings::DOCK_HEIGHT_RANGE.start(), *settings::DOCK_HEIGHT_RANGE.end());
            }
        }
        cx.notify();
    }

    /// Ends the drag: release the cursor, persist the layout.
    fn on_drag_end(&mut self, _: &gpui::MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            window.set_window_cursor_style(gpui::CursorStyle::Arrow);
            settings::save(&self.settings);
            cx.notify();
        }
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
        let active_pinned = self.tabs[active].pinned;
        // The tab bar's right-click menu targets the active tab (click a tab,
        // right-click, pin): the same face as the IDE convention without a
        // per-tab menu (TabBar takes `Tab` children only). Reopen closed and
        // close live here too.
        let bar = TabBar::new("editor-tabs")
            .selected_index(active)
            // Overflow: the tabs strip wheel-scrolls; the menu button lists
            // every tab so none is out of reach (M7).
            .menu(true)
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
                let mut tab_item = ComponentTab::new()
                    .label(label)
                    .selected(index == active)
                    // Pin marker (M7): a small accent dot; pinned tabs also
                    // lose their close button, so ⌘W stays the only way to
                    // close them — and ⌘W skips pinned.
                    .when(tab.pinned, |item| {
                        item.prefix(div().size(px(6.0)).flex_none().rounded_full().bg(theme::accent()))
                    });
                if !tab.pinned {
                    tab_item = tab_item.suffix(
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
                    );
                }
                tab_item
            }));
        div()
            .w_full()
            .context_menu(move |menu, _, cx| {
                // Track the menu for the Esc chain (M8) — same as the tree's.
                let built = cx.entity();
                workbench.update(cx, |this, cx| this.menu_opened(built, cx)).ok();
                menu.item(
                    PopupMenuItem::new(if active_pinned { strings::TAB_UNPIN } else { strings::TAB_PIN }).on_click({
                        let workbench = workbench.clone();
                        move |_, _, cx| {
                            workbench
                                .update(cx, |this, cx| {
                                    if let Some(active) = this.active {
                                        this.toggle_pin(active, cx);
                                    }
                                })
                                .ok();
                        }
                    }),
                )
                .item(PopupMenuItem::new(strings::TAB_CLOSE).on_click({
                    let workbench = workbench.clone();
                    move |_, _, cx| {
                        workbench
                            .update(cx, |this, cx| {
                                if let Some(active) = this.active
                                    && !this.tabs[active].pinned
                                {
                                    this.close_tab(active, cx);
                                }
                            })
                            .ok();
                    }
                }))
                .separator()
                .item(PopupMenuItem::new(strings::TAB_REOPEN).on_click({
                    let workbench = workbench.clone();
                    move |_, _, cx| {
                        workbench
                            .update(cx, |this, cx| {
                                if let Some(path) = this.closed_tabs.pop() {
                                    this.open_path(&path, cx);
                                }
                            })
                            .ok();
                    }
                }))
            })
            .child(bar)
    }

    // -- bottom dock ---------------------------------------------------------------

    /// The bottom dock: one tab per run (M7) plus the M5 Find page, above
    /// the active page. With a single page the M4 look (that panel alone, no
    /// tab bar, its own header) is preserved exactly. Hidden via ⌘4 or the
    /// persisted layout; the height drags (the 4 px handle on top).
    fn render_dock(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.settings.layout.dock_visible {
            return None;
        }
        let find = self.find_panel.clone();
        self.clamp_dock_page();
        let pages = self.runs.len() + usize::from(find.is_some());
        let body = match self.dock_page {
            DockPage::Run(index) if self.runs.get(index).is_some() => self.runs[index].clone().into_any_element(),
            // `find` (cloned above) stays owned for the tab-switch row after
            // this; the `?` returns None when the Find page was never created.
            _ => self.find_panel.clone()?.into_any_element(),
        };
        if pages <= 1 {
            return Some(self.dock_shell(cx, None, body));
        }
        // Multiple pages: the tab switch above the active one. Run tabs are
        // closable (a live run stops first); Find is a permanent page.
        let workbench = cx.entity().downgrade();
        let mut children = Vec::new();
        for (index, panel) in self.runs.iter().enumerate() {
            let name = panel.read(cx).config_name.clone();
            children.push(ComponentTab::new().label(name).selected(self.dock_page == DockPage::Run(index)).suffix(
                Button::new(("dock-tab-close", index)).icon(IconName::Close).xsmall().ghost().on_click({
                    let workbench = workbench.clone();
                    move |_, _, cx| {
                        workbench.update(cx, |this, cx| this.close_run_tab(index, cx)).ok();
                    }
                }),
            ));
        }
        if find.is_some() {
            children.push(ComponentTab::new().label(strings::FIND_FILES_TITLE));
        }
        let tabs = TabBar::new("dock-tabs")
            .selected_index(match self.dock_page {
                DockPage::Run(index) => index,
                DockPage::Find => self.runs.len(),
            })
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                let runs = this.runs.len();
                this.dock_page = if *index < runs { DockPage::Run(*index) } else { DockPage::Find };
                cx.notify();
            }))
            .children(children);
        Some(self.dock_shell(cx, Some(tabs), body))
    }

    fn dock_shell(&self, cx: &Context<Self>, header: Option<TabBar>, body: impl IntoElement) -> gpui::AnyElement {
        let mut shell = div()
            .h(px(self.settings.layout.dock_height))
            .flex_none()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(theme::border());
        if header.is_some() {
            shell = shell.child(self.render_dock_handle(cx));
        }
        if let Some(header) = header {
            shell = shell.child(header);
        }
        shell.child(div().flex_1().min_h_0().child(body)).into_any_element()
    }

    /// The 4 px drag handle above the dock (M7): arms the height drag.
    fn render_dock_handle(&self, cx: &Context<Self>) -> gpui::AnyElement {
        div()
            .h(px(4.0))
            .w_full()
            .flex_none()
            .cursor_pointer()
            .hover(|bar| bar.bg(theme::accent()))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_dock_handle_down))
            .into_any_element()
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
        // selected run tab (or the live one when the Find page shows).
        if let Some((text, color)) = self.run_status(cx) {
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

// -- tree prompt + delete confirmation (M7 context-menu mutations) -------------

impl Workbench {
    /// The one-line prompt over a New File / New Folder / Rename context
    /// action. Enter confirms (through ide-vfs), Esc cancels.
    fn render_tree_prompt(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let prompt = self.tree_prompt.as_ref()?;
        let title = match prompt.kind {
            TreePromptKind::NewFile => strings::TREE_NEW_FILE,
            TreePromptKind::NewFolder => strings::TREE_NEW_FOLDER,
            TreePromptKind::Rename => strings::TREE_RENAME,
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_tree_prompt(window, cx)))
                .child(
                    div()
                        .w(px(420.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(6.0))
                        .bg(theme::raised_bg())
                        .key_context("TreePrompt")
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close_tree_prompt(window, cx)))
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
                                        .child(SharedString::from(title)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_color(theme::fg_muted())
                                        .child(SharedString::from(prompt.target.display().to_string())),
                                ),
                        )
                        .child(div().px_2().py_2().child(Input::new(&prompt.input)))
                        .child(
                            div()
                                .px_3()
                                .pb_2()
                                .text_size(px(11.))
                                .text_color(theme::fg_muted())
                                .child(SharedString::from(strings::PROMPT_CONFIRM_HINT)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The delete confirmation: says what moves to the trash; a dirty open
    /// buffer's unsaved edits go with it (the dialog is the confirmation).
    fn render_tree_confirm(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let confirm = self.tree_confirm.as_ref()?;
        let message = if confirm.is_dir {
            format!("{} {} {}", strings::DELETE_FOLDER_MESSAGE, quoted(&confirm.name), strings::DELETE_AND_CONTENTS)
        } else {
            format!("{} {}?", strings::DELETE_FILE_MESSAGE, quoted(&confirm.name))
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_tree_confirm(window, cx)),
                )
                .child(
                    div()
                        .w(px(420.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(6.0))
                        .bg(theme::raised_bg())
                        .key_context("TreeConfirm")
                        .track_focus(&confirm.focus)
                        .on_mouse_down(
                            MouseButton::Left,
                            |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut gpui::App| cx.stop_propagation(),
                        )
                        .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close_tree_confirm(window, cx)))
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
                                        .child(SharedString::from(strings::DELETE_TITLE)),
                                ),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .text_size(px(12.))
                                .text_color(theme::fg())
                                .child(SharedString::from(message)),
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
                                .child(
                                    Button::new("tree-confirm-cancel")
                                        .label(strings::CANCEL)
                                        .xsmall()
                                        .ghost()
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            this.close_tree_confirm(window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("tree-confirm-delete")
                                        .label(strings::DELETE_BUTTON)
                                        .xsmall()
                                        .secondary()
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            this.tree_confirm_delete(window, cx)
                                        })),
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

#[cfg(test)]
mod closed_tab_tests {
    use super::*;

    #[test]
    fn closed_stack_is_most_recent_first_deduped_and_capped() {
        let mut stack = Vec::new();
        for i in 0..(CLOSED_TAB_CAP + 5) {
            push_closed(&mut stack, PathBuf::from(format!("/tmp/m7-tab-{i}")));
        }
        assert_eq!(stack.len(), CLOSED_TAB_CAP);
        assert_eq!(stack[0], PathBuf::from(format!("/tmp/m7-tab-{}", CLOSED_TAB_CAP + 4)));
        // Re-closing a path moves it back to the top.
        push_closed(&mut stack, PathBuf::from("/tmp/m7-tab-0"));
        assert_eq!(stack[0], PathBuf::from("/tmp/m7-tab-0"));
        assert_eq!(stack.len(), CLOSED_TAB_CAP);
    }

    #[test]
    fn tree_prompt_targets_the_rows_directory() {
        // A directory row receives the new entry itself; a file row's parent
        // does. A parentless row falls back to its own (relative) path.
        assert_eq!(Workbench::target_dir(Path::new("/p/internal"), true), PathBuf::from("/p/internal"));
        assert_eq!(Workbench::target_dir(Path::new("/p/internal/store.go"), false), PathBuf::from("/p/internal"));
    }

    #[test]
    fn font_steps_clamp_to_the_range() {
        let min = *settings::FONT_SIZE_RANGE.start();
        let max = *settings::FONT_SIZE_RANGE.end();
        assert_eq!(step_font_size(14.0, 1.0), 15.0);
        assert_eq!(step_font_size(14.0, -1.0), 13.0);
        assert_eq!(step_font_size(min, -1.0), min);
        assert_eq!(step_font_size(max, 1.0), max);
    }

    #[test]
    fn indent_label_reads_the_language_policy() {
        use ide_language::IndentConfig;
        assert_eq!(indent_label(Some(IndentConfig { tab_size: 4, insert_spaces: true })), "4-space indent");
        assert_eq!(indent_label(Some(IndentConfig { tab_size: 2, insert_spaces: true })), "2-space indent");
        assert_eq!(indent_label(Some(IndentConfig { tab_size: 4, insert_spaces: false })), "tabs, width 4");
        // No policy in the registry: the editor's default (4 spaces).
        assert_eq!(indent_label(None), "4-space indent (default)");
    }

    #[test]
    fn toast_bursts_dedupe_inside_the_window() {
        let toast = |text: &str| Toast { level: NotifyLevel::Info, text: text.to_owned(), click: None };
        let mut seen = HashMap::new();
        let t0 = Instant::now();

        // A burst of identical saves flushes exactly one toast.
        let kept =
            dedupe_toasts(vec![toast("Saved main.go"), toast("Saved main.go"), toast("Saved main.go")], &mut seen, t0);
        assert_eq!(kept.len(), 1);

        // A different text inside the window still lands.
        let kept = dedupe_toasts(vec![toast("Saved store.go")], &mut seen, t0 + Duration::from_millis(100));
        assert_eq!(kept.len(), 1);

        // The same text inside the window is suppressed.
        let kept = dedupe_toasts(vec![toast("Saved main.go")], &mut seen, t0 + Duration::from_secs(4));
        assert_eq!(kept.len(), 0);

        // After the window it lands again (and the memory was pruned).
        let kept = dedupe_toasts(vec![toast("Saved main.go")], &mut seen, t0 + Duration::from_secs(7));
        assert_eq!(kept.len(), 1);
        assert_eq!(seen.len(), 1, "pruned entries must not linger");

        // Severity is part of the identity: an error with the same text shows.
        let kept = dedupe_toasts(
            vec![Toast { level: NotifyLevel::Error, text: "Saved main.go".to_owned(), click: None }],
            &mut seen,
            t0 + Duration::from_secs(8),
        );
        assert_eq!(kept.len(), 1);
    }
}

/// The focus / Esc-chain regression (M7). The M3 failure class: a focus left
/// on an unmounted element — one hidden tree, one overlay closed from the
/// wrong place — silently swallows every action. Each overlay class is
/// opened, retreated from with the real Esc keystroke, and a follow-up
/// action must still land. Headless (`TestAppContext`), no host: the chain
/// mechanics live in the workbench, not the backend.
#[cfg(test)]
mod focus_chain_tests {
    use gpui::{KeyBinding, TestAppContext, VisualTestContext};

    use super::*;

    /// The bindings the regression drives — the workbench subset of
    /// main.rs's keymap (tests do not run main).
    fn bind_keys(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.bind_keys([
                KeyBinding::new("escape", Cancel, None),
                KeyBinding::new("shift-cmd-o", acts::GoToFile, None),
                KeyBinding::new("cmd-,", acts::OpenSettings, None),
            ]);
        });
    }

    /// Mounts the workbench without a host (the welcome shape: the tree is
    /// NOT rendered, so the fallback sink is the only live focus target).
    fn mount(cx: &mut TestAppContext) -> (Entity<Workbench>, VisualTestContext) {
        let (bus, _rx) = Bus::channel();
        let (workbench, visual) =
            cx.add_window_view(|_, cx| Workbench::new(None, bus, settings::Settings::default(), cx));
        (workbench, visual.clone())
    }

    /// Whether a window-level action lands: the palette opens, then closes.
    fn action_lands(workbench: &Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
        cx.dispatch_action(acts::GoToFile);
        let opened = cx.read_entity(workbench, |state, _| state.picker.is_some());
        cx.simulate_keystrokes("escape");
        let closed = cx.read_entity(workbench, |state, _| state.picker.is_none());
        opened && closed
    }

    /// Opens one overlay class and asserts the focus moved into it, so the
    /// Esc keystroke below retreats from the topmost surface.
    fn open_overlay(workbench: &Entity<Workbench>, cx: &mut VisualTestContext, kind: &str) {
        match kind {
            // The real gesture paths first.
            "settings" => cx.simulate_keystrokes("cmd-,"),
            "picker" => cx.simulate_keystrokes("shift-cmd-o"),
            // The rest open from clicks/menus whose wiring lives outside the
            // workbench; the overlay state is the contract under test.
            "tree_prompt" => workbench.update_in(cx, |this, window, cx| {
                this.open_tree_prompt(TreePromptKind::Rename, PathBuf::from("/tmp/m7-focus/file.rs"), window, cx)
            }),
            "tree_confirm" => {
                workbench.update(cx, |this, cx| this.begin_delete(PathBuf::from("/tmp/m7-focus/file.rs"), false, cx))
            }
            "commit" => workbench.update_in(cx, |this, window, cx| {
                let input = cx.new(|cx| TextareaState::new(window, cx));
                window.focus(&input.read(cx).focus_handle(cx), cx);
                this.commit = Some(CommitFlow { input, files: Vec::new() });
                cx.notify();
            }),
            "diff" => workbench.update_in(cx, |this, window, cx| {
                this.diff = Some(DiffState { title: "focus.rs".to_owned(), lines: Vec::new() });
                window.focus(&this.diff_focus.clone(), cx);
                cx.notify();
            }),
            other => unreachable!("unknown overlay class {other}"),
        }
        cx.run_until_parked();
    }

    fn overlay_open(workbench: &Entity<Workbench>, cx: &VisualTestContext, kind: &str) -> bool {
        cx.read_entity(workbench, |state, _| match kind {
            "settings" => state.settings_overlay.is_some(),
            "picker" => state.picker.is_some(),
            "tree_prompt" => state.tree_prompt.is_some(),
            "tree_confirm" => state.tree_confirm.is_some(),
            "commit" => state.commit.is_some(),
            "diff" => state.diff.is_some(),
            other => unreachable!("unknown overlay class {other}"),
        })
    }

    /// THE regression: every overlay class closes via Esc, and the window
    /// keeps dispatching actions afterwards. One stranded focus is the whole
    /// M3 bug class; this walks every close path there is.
    #[gpui::test]
    fn after_any_overlay_closes_actions_still_land(cx: &mut TestAppContext) {
        bind_keys(cx);
        let (workbench, mut cx) = mount(cx);

        // The welcome view's fallback sink holds the focus; an action lands.
        assert!(action_lands(&workbench, &mut cx));

        for kind in ["settings", "picker", "tree_prompt", "tree_confirm", "commit", "diff"] {
            open_overlay(&workbench, &mut cx, kind);
            assert!(overlay_open(&workbench, &cx, kind), "{kind} did not open");
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(!overlay_open(&workbench, &cx, kind), "{kind} did not close on Esc");
            assert!(action_lands(&workbench, &mut cx), "actions died after closing {kind}");
        }
    }

    /// Esc retreats ONE surface: with a palette underneath, the settings
    /// overlay's Esc leaves the palette open (and focused) for typing.
    #[gpui::test]
    fn esc_retreats_the_topmost_surface_only(cx: &mut TestAppContext) {
        bind_keys(cx);
        let (workbench, mut cx) = mount(cx);

        cx.simulate_keystrokes("shift-cmd-o");
        assert!(overlay_open(&workbench, &cx, "picker"));
        cx.simulate_keystrokes("cmd-,");
        assert!(overlay_open(&workbench, &cx, "settings"));

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!overlay_open(&workbench, &cx, "settings"), "topmost overlay must close");
        assert!(overlay_open(&workbench, &cx, "picker"), "the palette beneath survives");

        // The focus is back in the palette's query input, and a second Esc
        // closes the palette itself.
        let query_focused = cx.update(|window, cx| {
            workbench
                .read(cx)
                .picker
                .as_ref()
                .is_some_and(|picker| picker.input.read(cx).focus_handle(cx).is_focused(window))
        });
        assert!(query_focused, "the palette's input lost the focus");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!overlay_open(&workbench, &cx, "picker"));
    }

    /// `restore_focus` lands on a MOUNTED surface: without a host the tree
    /// is not rendered (the welcome view), and a hidden tree (⌘1) must not
    /// strand the focus either — the root sink takes both cases.
    #[gpui::test]
    fn restore_focus_never_lands_on_an_unmounted_tree(cx: &mut TestAppContext) {
        bind_keys(cx);
        let (workbench, mut cx) = mount(cx);

        // No host → no tree element → the root sink holds the focus.
        workbench.update_in(&mut cx, |this, window, cx| {
            this.restore_focus(window, cx);
            assert!(this.root_focus.is_focused(window), "the root sink must hold the focus");
            assert!(!this.tree_focus.is_focused(window), "the unmounted tree took the focus");
        });

        // The same when ⌘1 hid the tree: the root sink, never the hidden one.
        workbench.update_in(&mut cx, |this, window, cx| {
            this.settings.layout.tree_visible = false;
            this.restore_focus(window, cx);
            assert!(this.root_focus.is_focused(window), "a hidden tree stranded the focus");
        });

        // And an action still lands from the root sink.
        cx.simulate_keystrokes("shift-cmd-o");
        cx.run_until_parked();
        assert!(overlay_open(&workbench, &cx, "picker"));
    }

    /// `focus_active` with NO active pane (⌘W closed the last tab — caught
    /// live in the M7 scratch session) must fall back through the chain, not
    /// strand the focus on the unmounted pane.
    #[gpui::test]
    fn focus_active_without_a_tab_still_lands_on_a_live_surface(cx: &mut TestAppContext) {
        bind_keys(cx);
        let (workbench, mut cx) = mount(cx);

        workbench.update_in(&mut cx, |this, window, cx| {
            assert!(this.active.is_none());
            this.focus_active(window, cx);
            // No host → the tree is not rendered → the root sink takes it.
            assert!(this.root_focus.is_focused(window), "focus_active with no tabs stranded the focus");
        });
    }

    /// The floating-menu face (M8): the M7 leftover — a right-click menu
    /// that lost its focus never sees the menu's own Esc binding, so Esc
    /// landed as a bare Cancel and the menu stayed up (mouse-only UX). The
    /// workbench tracks the open menu and dismisses it as the Esc chain's
    /// last resort, focus re-landing on a live surface. The menu here is
    /// the real component entity, registered the way every builder does;
    /// its focus is deliberately never taken — the focus-orphaned shape.
    #[gpui::test]
    fn esc_dismisses_a_focus_orphaned_floating_menu(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        bind_keys(cx);
        let (workbench, mut cx) = mount(cx);

        let dismissed = Rc::new(Cell::new(false));
        let flag = dismissed.clone();
        let menu = cx.update(|window, cx| {
            let menu = gpui_component::menu::PopupMenu::build(window, cx, |menu, _, _| {
                menu.menu("Close", Box::new(acts::GoToFile))
            });
            // Held to the end: a dropped Subscription detaches its callback.
            let sub = cx.subscribe(&menu, move |_, _: &gpui::DismissEvent, _| flag.set(true));
            (menu, sub)
        });
        let (menu, _dismiss_sub) = menu;
        // The strong handle stands in for the component's element state (the
        // real menu's holder); the tracking itself only keeps a weak one.
        let holder = menu.clone();
        workbench.update(&mut cx, |this, cx| this.menu_opened(menu, cx));
        cx.run_until_parked();

        // Esc lands on the workbench chain (the menu holds no focus): it
        // must dismiss the orphaned menu, clear the tracking and keep the
        // window dispatching.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(dismissed.get(), "the orphaned menu must receive the dismissal");
        assert!(cx.read_entity(&workbench, |state, _| state.open_menu.is_none()), "the tracking must clear");
        assert!(action_lands(&workbench, &mut cx), "actions died after dismissing the menu");
        drop(holder);
    }
}
