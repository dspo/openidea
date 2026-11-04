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

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable as _, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::status_bar::StatusBar;
use gpui_component::tab::{Tab as ComponentTab, TabBar};
use gpui_component::{Icon, IconName, Selectable as _, Sizable as _};
use ide_core::event::NotifyLevel;
use ide_core::event::ServerStatus;
use ide_core::{Diagnostic, DocId, IdeEvent, ServerId};
use ide_vfs::EntryKind;

use crate::actions::{CloseTab, OpenFile, Redo, Save, Undo};
use crate::bus::{Bus, BusMessage};
use crate::editor::EditorPane;
use crate::file_tree::{FileTree, Row};
use crate::host::Host;
use crate::{strings, theme};

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

pub struct Workbench {
    host: Option<Arc<Host>>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    tree: FileTree,
    tree_focus: FocusHandle,
    /// Startup focus seed: with no tab open the tree takes focus so the
    /// window-level actions (⌘O) and the tree keys work immediately.
    tree_focused_once: Cell<bool>,
    message: Option<Message>,
    servers: Vec<ServerRow>,
    /// Diagnostics per file, per producer (`Diagnostics` events replace one
    /// producer's list; the merged view reaches the pane). Sorted by position
    /// for the pane's line lookups.
    diagnostics: HashMap<PathBuf, HashMap<String, Vec<Diagnostic>>>,
    bus: Bus,
}

impl Workbench {
    pub fn new(host: Option<Arc<Host>>, bus: Bus, cx: &mut Context<Self>) -> Self {
        let mut tree = FileTree::new();
        if let Some(host) = &host
            && let Err(e) = tree.open(host)
        {
            eprintln!("[workbench] project open: {}", e.message);
        }
        Self {
            host,
            tabs: Vec::new(),
            active: None,
            tree,
            tree_focus: cx.focus_handle(),
            tree_focused_once: Cell::new(false),
            message: None,
            servers: Vec::new(),
            diagnostics: HashMap::new(),
            bus,
        }
    }

    // -- actions ----------------------------------------------------------

    /// ⌘O: the native open dialog. rfd's dialog future must be *created* on
    /// the main thread (NSOpenPanel is main-thread-only; the future's own
    /// constructor asserts it) but may be awaited from the background
    /// executor; the pick comes back through the bus.
    fn on_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        let bus = self.bus.clone();
        let initial = self.host.as_ref().map(|host| host.root.clone());
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = initial {
            dialog = dialog.set_directory(&dir);
        }
        let picked = dialog.pick_file();
        cx.background_executor()
            .spawn(async move {
                let picked = picked.await.map(|handle| handle.path().to_path_buf());
                bus.send(BusMessage::Picked(picked));
            })
            .detach();
        cx.notify();
    }

    /// ⌘S: writes the document to disk through the VFS. The pane's edits
    /// already reached the backend document model (every keystroke goes
    /// through `Documents::edit`), so there is nothing to push.
    fn on_save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else { return };
        let Some(host) = self.host.clone() else { return };
        let (doc, name) = {
            let tab = &self.tabs[index];
            (tab.doc, file_name(&tab.path))
        };
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
        let pane = cx.new(|cx| EditorPane::new(host, info, cx));
        // Diagnostics may have landed before the tab existed (the servers
        // publish for every open file in the project).
        let existing = self.diagnostics.get(&path.to_path_buf());
        if let Some(merged) = existing.map(Self::merge_diagnostics) {
            pane.update(cx, |pane, cx| pane.set_diagnostics(merged, cx));
        }
        let path = path.to_path_buf();
        self.tabs.push(Tab { path: path.clone(), doc: doc_id, pane: pane.clone(), dirty: false, stale: false });
        self.active = Some(self.tabs.len() - 1);
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

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Seed the initial focus once (a view render cannot focus earlier):
        // the tree when no tab is active, so ⌘O and the tree keys work
        // immediately after launch.
        if !self.tree_focused_once.get() {
            self.tree_focused_once.set(true);
            if self.active.is_none() {
                window.focus(&self.tree_focus, cx);
            }
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .text_color(theme::fg())
            .on_action(cx.listener(Self::on_open_file))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(self.render_tree(cx))
                    .child(self.render_editor_area(cx)),
            )
            .child(self.render_status())
    }
}

// -- left dock -----------------------------------------------------------------

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
                        .child(SharedString::from(row.name))
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

    // -- status bar ----------------------------------------------------------------

    fn render_status(&self) -> impl IntoElement {
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
        if let Some(file) = file {
            left = left.child(div().text_color(theme::fg()).child(SharedString::from(file)));
        }
        let mut right = div().flex().items_center().gap_3().text_size(px(12.));
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
        right = right.child(div().text_color(state.1).child(SharedString::from(state.0)));
        if let Some((text, color)) = message {
            right = right.child(div().max_w_96().truncate().text_color(color).child(SharedString::from(text)));
        }
        StatusBar::new().left(left).right(right)
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
