//! The Find in Files dock page (M5): a query input over `ide_search::search`
//! results grouped per file. The panel is display + input only — Enter sends
//! `FindInFilesStart` through the bus, and the workbench snapshots the dirty
//! open buffers and runs the search on the background executor (it owns the
//! tabs, so it owns the overlay). The answer lands back through the bus once
//! the run completes; rows open the file at the match (the editor highlights
//! every hit on arrival). Project-wide replace stays built-later.

use gpui::{
    AppContext as _, ClickEvent, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, UniformListScrollHandle, Window, div, px, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};
use ide_search::{FileMatches, SearchSummary};

use crate::bus::{Bus, BusMessage};
use crate::strings;
use crate::theme;

/// One flattened list row (a file group header, or a match line).
enum FindRow {
    File(usize),
    Line(usize, usize),
}

const LINE_HEIGHT: f32 = 20.0;

pub struct FindPanel {
    bus: Bus,
    input: gpui::Entity<InputState>,
    query: String,
    /// The query the shown results answer to (rides along on `FindHit`, so
    /// the editor can highlight every hit in the opened buffer).
    last_query: String,
    /// The last completed run's results, grouped per file.
    files: Vec<FileMatches>,
    summary: Option<SearchSummary>,
    searching: bool,
    /// Guards against stale answers when a new search starts first.
    request: u32,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// The input's Change / PressEnter subscription, alive while the panel is.
    _sub: Option<gpui::Subscription>,
    focus: FocusHandle,
}

impl FindPanel {
    pub fn new(bus: Bus, window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(strings::FIND_FILES_PLACEHOLDER));
        let sub = Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } => this.start_search(cx),
            InputEvent::Change => {
                this.query = this.input.read(cx).value().to_string();
            }
            _ => {}
        }));
        Self {
            bus,
            input,
            query: String::new(),
            last_query: String::new(),
            files: Vec::new(),
            summary: None,
            searching: false,
            request: 0,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _sub: sub,
            focus: cx.focus_handle(),
        }
    }

    /// The query input's focus handle: ⇧⌘F hands it the focus so the search
    /// starts with typing, no click (the IDE rhythm; re-pressing ⇧⌘F with
    /// the page open returns there).
    pub fn query_focus(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.input.read(cx).presentation().focus_handle().clone()
    }

    /// Enter: hand the query to the workbench through the bus. Cancellation
    /// is implicit — a newer request drops the older answer. An empty query
    /// no longer dead-ends (M8): it clears the finished results, so the
    /// page always has a way back to its pristine state.
    pub fn start_search(&mut self, cx: &mut gpui::Context<Self>) {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            self.clear_results(cx);
            return;
        }
        self.request += 1;
        self.searching = true;
        self.selected = 0;
        self.last_query = query.clone();
        eprintln!("[find] searching {query:?}");
        self.bus.send(BusMessage::FindInFilesStart { request: self.request, query });
        cx.notify();
    }

    /// Drops the finished results and any in-flight answer (a bumped
    /// request id makes the late answer stale).
    fn clear_results(&mut self, cx: &mut gpui::Context<Self>) {
        if self.files.is_empty() && self.summary.is_none() && !self.searching {
            return;
        }
        self.request += 1;
        self.searching = false;
        self.files.clear();
        self.summary = None;
        self.last_query.clear();
        self.selected = 0;
        eprintln!("[find] results cleared");
        cx.notify();
    }

    /// The search finished: keep the answer only if its request is current.
    pub fn finished(
        &mut self,
        request: u32,
        files: Vec<FileMatches>,
        summary: SearchSummary,
        cx: &mut gpui::Context<Self>,
    ) {
        if request != self.request {
            return;
        }
        self.files = files;
        self.summary = Some(summary);
        self.searching = false;
        cx.notify();
    }

    /// The flattened row list behind the virtualized list.
    fn rows(&self) -> Vec<FindRow> {
        let mut rows = Vec::new();
        for (group, file) in self.files.iter().enumerate() {
            rows.push(FindRow::File(group));
            for (line, _) in file.matches.iter().enumerate() {
                rows.push(FindRow::Line(group, line));
            }
        }
        rows
    }

    fn open_hit(&mut self, group: usize, line: usize, cx: &mut gpui::Context<Self>) {
        let Some(file) = self.files.get(group) else { return };
        let Some(hit) = file.matches.get(line) else { return };
        let pos = ide_core::Pos::new(hit.line, hit.ranges.first().map_or(0, |(start, _)| *start));
        self.selected = self
            .rows()
            .iter()
            .position(|row| matches!(row, FindRow::Line(g, l) if *g == group && *l == line))
            .unwrap_or(self.selected);
        self.bus.send(BusMessage::FindHit { path: file.path.0.clone(), pos, query: self.last_query.clone() });
        cx.notify();
    }
}

impl Focusable for FindPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One match's ranges drawn as accent runs inside the line text (UTF-16
/// columns into `text`).
fn render_line_text(text: &str, ranges: &[(u32, u32)]) -> gpui::AnyElement {
    let mut row = div().flex().min_w_0();
    let mut cursor = 0usize;
    for (start, end) in ranges {
        let (start, end) = (*start as usize, (*end as usize).max(*start as usize));
        if start > cursor && start <= text.len() {
            row = row.child(div().text_color(theme::fg()).child(SharedString::from(text[cursor..start].to_owned())));
            cursor = start;
        }
        if end > cursor && end <= text.len() {
            row = row.child(
                div()
                    .text_color(theme::accent())
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(SharedString::from(text[cursor..end].to_owned())),
            );
            cursor = end;
        }
    }
    if cursor < text.len() {
        row = row.child(div().text_color(theme::fg()).truncate().child(SharedString::from(text[cursor..].to_owned())));
    }
    row.into_any_element()
}

impl Render for FindPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let count = rows.len();
        let this = cx.entity().downgrade();
        let status = if self.searching {
            strings::FIND_SEARCHING.to_owned()
        } else if let Some(summary) = &self.summary {
            format!(
                "{} matches in {} files · {} ms{}",
                summary.matches,
                summary.files,
                summary.elapsed_ms,
                if summary.limit_hit { " · limit" } else { "" }
            )
        } else {
            strings::FIND_HINT.to_owned()
        };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h_8()
            .border_b_1()
            .border_color(theme::border())
            .text_size(px(12.))
            .child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme::fg())
                    .child(SharedString::from(strings::FIND_FILES_TITLE)),
            )
            .child(div().min_w_16().child(Input::new(&self.input)))
            .child(div().truncate().text_color(theme::fg_muted()).child(SharedString::from(status)));
        let body = div().flex_1().min_h_0().child(
            uniform_list("find-rows", count, move |range, _, cx| {
                let Some(this) = this.upgrade() else { return Vec::new() };
                let panel = this.read(cx);
                range
                    .map(|index| {
                        let Some(row) = rows.get(index) else { return div().into_any_element() };
                        match row {
                            FindRow::File(group) => {
                                let Some(file) = panel.files.get(*group) else {
                                    return div().into_any_element();
                                };
                                div()
                                    .h(px(LINE_HEIGHT))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .bg(theme::app_bg())
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme::fg())
                                            .child(SharedString::from(file.rel_path.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme::fg_muted())
                                            .child(SharedString::from(format!("{} matches", file.matches.len()))),
                                    )
                                    .into_any_element()
                            }
                            FindRow::Line(group, line) => {
                                let Some(hit) = panel.files.get(*group).and_then(|file| file.matches.get(*line)) else {
                                    return div().into_any_element();
                                };
                                let (group, line) = (*group, *line);
                                div()
                                    .id(("find-row", index))
                                    .h(px(LINE_HEIGHT))
                                    .pl(px(20.0))
                                    .pr_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .font_family(theme::FONT_FAMILY)
                                    .on_click({
                                        let this = this.clone();
                                        move |_: &ClickEvent, _, cx| {
                                            this.update(cx, |panel, cx| panel.open_hit(group, line, cx));
                                        }
                                    })
                                    .hover(|row| row.bg(theme::raised_bg()))
                                    .child(
                                        div()
                                            .w(px(36.0))
                                            .flex_none()
                                            .text_right()
                                            .text_color(theme::fg_muted())
                                            .child(SharedString::from((hit.line + 1).to_string())),
                                    )
                                    .child(render_line_text(&hit.text, &hit.ranges))
                                    .into_any_element()
                            }
                        }
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .h_full(),
        );
        div().flex().flex_col().size_full().bg(theme::surface_bg()).child(header).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M8: Enter with an empty query clears the finished results (the page
    /// held them forever before); a second empty Enter is a no-op; and a
    /// stale in-flight answer cannot repopulate the cleared page.
    #[gpui::test]
    fn empty_enter_clears_results_and_stays_pristine(cx: &mut gpui::TestAppContext) {
        // The panel's paint reads the component Theme global (main.rs sets
        // it at startup; tests must seed it themselves).
        cx.update(|cx| {
            gpui_component::init(cx);
            gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        });
        let (bus, _rx) = Bus::channel();
        let (panel, cx) = cx.add_window_view(|window, cx| FindPanel::new(bus, window, cx));

        panel.update(cx, |panel, cx| {
            // A finished run: the answer's request id matches the current one.
            panel.finished(
                panel.request,
                vec![FileMatches {
                    path: ide_core::IpcPath::from("/tmp/m8-find/a.rs"),
                    rel_path: "a.rs".to_owned(),
                    matches: vec![ide_search::LineMatch {
                        line: 3,
                        text: "let find = 1;".to_owned(),
                        ranges: vec![(4, 8)],
                        text_offset: 0,
                    }],
                }],
                SearchSummary { files: 1, matches: 1, limit_hit: false, cancelled: false, elapsed_ms: 2 },
                cx,
            );
            assert_eq!(panel.files.len(), 1);
            // Empty query → clear.
            panel.start_search(cx);
            assert!(panel.files.is_empty() && panel.summary.is_none());
            assert!(!panel.searching);
        });
        cx.run_until_parked();

        // Already pristine: another empty Enter changes nothing.
        panel.update(cx, |panel, cx| {
            let before = panel.request;
            panel.start_search(cx);
            assert_eq!(panel.request, before);
        });

        // A stale answer (from before the clear) cannot repopulate.
        panel.update(cx, |panel, cx| {
            panel.finished(
                0,
                vec![FileMatches {
                    path: ide_core::IpcPath::from("/tmp/m8-find/stale.rs"),
                    rel_path: "stale.rs".to_owned(),
                    matches: Vec::new(),
                }],
                SearchSummary::default(),
                cx,
            );
            assert!(panel.files.is_empty(), "a stale answer repopulated the cleared page");
        });
    }
}
