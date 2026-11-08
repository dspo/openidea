//! The status bar as its own entity (M14a, the M10 carry-over's structural
//! fix): caret and document events land here through [`StatusBarView::sync`],
//! which notifies only when a displayed value actually changed. Typing no
//! longer schedules a whole-shell render — the workbench keeps its cached
//! frame and only this small view re-renders.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, WeakEntity, Window, div, px,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use ide_core::event::ServerStatus;

use gpui_component::status_bar::StatusBar;

use crate::strings;
use crate::theme;
use crate::workbench::Workbench;

/// The active tab's save state, as the status bar renders it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    Saved,
    Unsaved,
    Stale,
}

/// One language-server row (name + health) as pushed by the workbench.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusServer {
    pub name: String,
    pub status: ServerStatus,
}

/// One live `$/progress` task row as pushed by the workbench.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusTask {
    pub title: String,
    pub message: Option<String>,
    pub fraction: Option<f32>,
}

/// Everything the status bar paints, pushed from the workbench's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Pushed {
    pub project: String,
    pub file: Option<String>,
    pub save: SaveState,
    pub caret: (u32, u32),
    pub errors: usize,
    pub branch: Option<String>,
    pub operation: Option<String>,
    pub servers: Vec<StatusServer>,
    pub tasks: Vec<StatusTask>,
    pub run: Option<(String, gpui::Rgba)>,
}

pub struct StatusBarView {
    workbench: WeakEntity<Workbench>,
    pushed: Option<Pushed>,
}

impl StatusBarView {
    pub fn new(workbench: WeakEntity<Workbench>) -> Self {
        Self { workbench, pushed: None }
    }

    /// One bulk push per workbench pump / render: fields the shell would have
    /// re-rendered for are diffed here instead, so a no-op costs a compare and
    /// a keystroke repaints just this view.
    pub fn sync(&mut self, next: Pushed, cx: &mut Context<Self>) {
        if self.pushed.as_ref() != Some(&next) {
            self.pushed = Some(next);
            cx.notify();
        }
    }

    /// The show/hide map is the workbench settings' (the Status Bar Widgets
    /// menu writes it there); read at render, never owned.
    fn widget_shown(&self, cx: &App, id: &str) -> bool {
        self.workbench.upgrade().map(|wb| wb.read(cx).status_widget_shown(id)).unwrap_or(true)
    }
}

impl Render for StatusBarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(pushed) = self.pushed.clone() else { return div().into_any_element() };
        let shown = |id: &str| self.widget_shown(cx, id);
        // The menu closure owns its own copy of the visibility values (it
        // outlives the render borrow, like the pre-entity code's map clone).
        let menu_shown = [
            ("project", shown("project")),
            ("branch", shown("branch")),
            ("position", shown("position")),
            ("problems", shown("problems")),
            ("encoding", shown("encoding")),
            ("linesep", shown("linesep")),
            ("servers", shown("servers")),
        ];
        let menu_workbench = self.workbench.clone();
        let menu_status = cx.entity().downgrade();
        let (project, file, state) = match (&pushed.file, pushed.save) {
            (Some(name), save) => (
                pushed.project.clone(),
                Some(name.clone()),
                match save {
                    SaveState::Stale => (strings::SAVE_STATE_STALE, theme::warning()),
                    SaveState::Unsaved => (strings::SAVE_STATE_UNSAVED, theme::accent()),
                    SaveState::Saved => (strings::SAVE_STATE_SAVED, theme::fg_muted()),
                },
            ),
            (None, _) => (pushed.project.clone(), None, (strings::SAVE_STATE_SAVED, theme::fg_muted())),
        };
        let mut left = div().flex().items_center().gap_2().text_size(px(12.));
        if shown("project") {
            left = left.child(div().text_color(theme::fg_muted()).child(SharedString::from(project)));
        }
        if shown("branch")
            && let Some(branch) = &pushed.branch
        {
            // The branches popup's IDEA entry: the widget itself is the
            // click target (the branch label rides along).
            left = left
                .child(
                    div().text_color(theme::fg_muted()).child(SharedString::from(strings::GIT_BRANCH_LABEL.to_owned())),
                )
                .child(
                    div()
                        .id("status-branch")
                        .cursor_pointer()
                        .hover(|widget| widget.text_color(theme::accent()))
                        .on_click({
                            let workbench = self.workbench.clone();
                            move |_: &gpui::ClickEvent, window, cx| {
                                workbench.update(cx, |this, cx| this.open_branches_popup(false, window, cx)).ok();
                            }
                        })
                        .child(div().text_color(theme::fg()).child(SharedString::from(branch.clone()))),
                );
        }
        if let Some(operation) = &pushed.operation {
            left = left.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::warning())
                    .child(SharedString::from(format!("{operation} in progress"))),
            );
        }
        if shown("position") {
            let (line, col) = pushed.caret;
            let position = format!("{}:{}", line + 1, col + 1);
            left = left.child(
                div()
                    .text_color(theme::fg())
                    .when_some(file, |widget, name| widget.child(SharedString::from(format!("{name} "))))
                    .child(SharedString::from(position)),
            );
        } else if let Some(file) = file {
            left = left.child(div().text_color(theme::fg()).child(SharedString::from(file)));
        }
        let mut right = div().flex().items_center().gap_3().text_size(px(12.));
        // The Problems count (the F2 walker's set); a click opens the window.
        if shown("problems") {
            let errors = pushed.errors;
            let count = if errors == 0 { strings::NO_PROBLEMS_SHORT.to_owned() } else { format!("{errors}") };
            right = right.child(
                div()
                    .id("status-problems")
                    .cursor_pointer()
                    .on_click({
                        let workbench = self.workbench.clone();
                        move |_: &gpui::ClickEvent, _, cx| {
                            workbench.update(cx, |this, cx| this.toggle_window("problems", cx)).ok();
                        }
                    })
                    .when(errors > 0, |widget| widget.text_color(theme::error()))
                    .child(SharedString::from(count)),
            );
        }
        if shown("encoding") {
            right =
                right.child(div().text_color(theme::fg_muted()).child(SharedString::from(strings::STATUS_ENCODING)));
        }
        if shown("linesep") {
            right = right.child(div().text_color(theme::fg_muted()).child(SharedString::from(strings::STATUS_LINESEP)));
        }
        // The run indicator (running · elapsed, or the exit code), pushed by
        // the workbench's run events.
        if shown("run")
            && let Some((text, color)) = &pushed.run
        {
            right = right
                .child(div().truncate().text_color(gpui::Hsla::from(*color)).child(SharedString::from(text.clone())));
        }
        // Language-server rows: name + status, colored by health.
        if shown("servers") {
            for row in &pushed.servers {
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
        }
        // Live server tasks (indexing, package loads): the anti-"hang" face.
        if shown("tasks") {
            for task in &pushed.tasks {
                let mut text = task.message.clone().unwrap_or_else(|| task.title.clone());
                if let Some(fraction) = task.fraction {
                    text = format!("{text} {}%", (fraction * 100.0).round() as u32);
                }
                right = right.child(div().text_color(theme::fg()).child(SharedString::from(task.title.clone()))).child(
                    div().max_w(px(160.0)).truncate().text_color(theme::warning()).child(SharedString::from(text)),
                );
            }
        }
        right = right.child(div().text_color(state.1).child(SharedString::from(state.0)));
        // The Status Bar Widgets menu (M10): the named widgets' show/hide
        // checks, persisted. Untitled faces (save state) stay always on.
        let widgets: [(&str, &str); 7] = [
            ("project", strings::W_PROJECT),
            ("branch", strings::W_BRANCH),
            ("position", strings::W_POSITION),
            ("problems", strings::W_PROBLEMS),
            ("encoding", strings::W_ENCODING),
            ("linesep", strings::W_LINESEP),
            ("servers", strings::W_SERVERS),
        ];
        div()
            .relative()
            .context_menu(move |menu, _, _| {
                let mut menu = menu.label(SharedString::from(strings::STATUS_WIDGETS_MENU));
                for (id, title) in widgets {
                    let workbench = menu_workbench.clone();
                    let status = menu_status.clone();
                    let shown = menu_shown.iter().find(|(key, _)| *key == id).is_some_and(|(_, on)| *on);
                    menu = menu.item(PopupMenuItem::new(title).checked(shown).on_click(move |_, _, cx| {
                        workbench.update(cx, |this, cx| this.toggle_status_widget(id, cx)).ok();
                        status.update(cx, |_, cx| cx.notify()).ok();
                    }));
                }
                menu
            })
            .child(StatusBar::new().left(left).right(right))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pushed(project: &str, file: Option<&str>, save: SaveState, caret: (u32, u32), errors: usize) -> Pushed {
        Pushed {
            project: project.to_owned(),
            file: file.map(str::to_owned),
            save,
            caret,
            errors,
            branch: None,
            operation: None,
            servers: Vec::new(),
            tasks: Vec::new(),
            run: None,
        }
    }

    #[test]
    fn pushed_equality_is_value_based() {
        let a = pushed("demo", Some("main.rs"), SaveState::Saved, (0, 0), 0);
        let mut b = pushed("demo", Some("main.rs"), SaveState::Saved, (0, 0), 0);
        assert_eq!(a, b);
        b.caret = (1, 2);
        assert_ne!(a, b, "a caret move must read as a change");
        b = pushed("demo", Some("main.rs"), SaveState::Unsaved, (1, 2), 0);
        assert_ne!(a, b, "a save-state flip must read as a change");
        b.errors = 3;
        assert_ne!(a, b, "a problem-count change must read as a change");
    }
}
