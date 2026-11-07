//! The Run dock (M4): a bottom-dock panel streaming one run's output. Lines
//! arrive through the workbench's bus pump (ide-run batches at ~8 ms), are
//! kept as `(stream, text)` rows — capped, oldest dropped — and render
//! through gpui's virtualized `uniform_list`, so a noisy `cargo build` only
//! lays out the visible window. `follow` pins the view to the tail until the
//! user scrolls up.
//!
//! M12: a test-tagged configuration carries a [`TestRunShared`] (the parsed
//! tree) and renders the suite/case form above the output — state icons,
//! durations, the totals line, per-node rerun, failure jump and the
//! expected/actual diff chip. The raw output stays below (toggleable).

use std::sync::Arc;
use std::time::Instant;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    ClickEvent, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled, Window, div, px, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};
use ide_run::OutputChunk;

use crate::bus::Bus;
use crate::host::Host;
use crate::test_tree::{self, TestCase, TestState, TestSuite};
use crate::{strings, theme};

// The full Lucide catalog (the legacy gpui_component subset misses the
// tree's state faces).
use gpui_kit_assets::IconName as KitIcon;

/// One rendered output line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
    System,
}

/// ~2000 lines of headroom; older output is dropped from the top.
const MAX_LINES: usize = 2000;
const LINE_HEIGHT: f32 = 18.0;

/// What a fresh panel carries: its dock index, the configuration it runs
/// (the rerun template), its runner family, and the parsed test tree for
/// test runs.
pub struct RunPanelSeed {
    pub index: usize,
    pub config_name: String,
    pub config: Option<ide_run::RunConfig>,
    pub runner: Option<test_tree::Runner>,
    pub test: Option<Arc<test_tree::TestRunShared>>,
}

pub struct RunPanel {
    host: Arc<Host>,
    bus: Bus,
    /// This panel's dock index (the rerun messages route by it).
    pub index: usize,
    /// The configuration's display name (the dock header).
    pub config_name: String,
    /// The configuration this run started from (the rerun's template).
    pub config: Option<ide_run::RunConfig>,
    /// The parsed test tree, for test-tagged runs (`None` = plain run).
    pub test: Option<Arc<test_tree::TestRunShared>>,
    /// The runner family (the tree's per-node filter faces differ).
    runner: Option<test_tree::Runner>,
    /// The live run (present while starting or running).
    pub run_id: Option<ide_core::RunId>,
    /// Whether the start call has returned a live process yet (the window
    /// between the action and the background spawn's answer).
    pub starting: bool,
    started_at: Option<Instant>,
    /// `(exit code, when)` once `RunExited` landed; `None` code = unknown.
    exited: Option<(Option<i32>, Instant)>,
    lines: Vec<(Stream, String)>,
    /// A chunk's trailing fragment without its newline yet.
    pending: (Stream, String),
    follow: bool,
    /// The output section's visibility under a test tree (the tree is the
    /// primary face; the raw stream is the evidence).
    show_output: bool,
    scroll: gpui::UniformListScrollHandle,
    focus: FocusHandle,
}

impl RunPanel {
    pub fn new(host: Arc<Host>, bus: Bus, seed: RunPanelSeed, cx: &mut gpui::Context<Self>) -> Self {
        Self {
            host,
            bus,
            index: seed.index,
            config_name: seed.config_name,
            config: seed.config,
            runner: seed.runner,
            test: seed.test,
            run_id: None,
            starting: true,
            started_at: None,
            exited: None,
            lines: Vec::new(),
            pending: (Stream::System, String::new()),
            follow: true,
            show_output: false,
            scroll: gpui::UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        }
    }

    /// The start call returned: `info` is the live run (its id routes output
    /// and the exit event), or [`Self::start_failed`] reports the error.
    pub fn started(&mut self, info: &ide_run::RunInfo) {
        self.run_id = Some(info.id);
        self.starting = false;
        self.started_at = Some(Instant::now());
    }

    pub fn start_failed(&mut self, message: &str) {
        self.starting = false;
        self.append(Stream::System, &format!("{message}\n"));
    }

    pub fn running(&self) -> bool {
        self.run_id.is_some() && self.exited.is_none()
    }

    /// The header's status text and its color.
    pub fn status(&self) -> (String, gpui::Hsla) {
        if self.starting {
            return (strings::RUN_STARTING.to_owned(), theme::warning());
        }
        if let Some((code, ended)) = &self.exited {
            let elapsed = self.started_at.map(|at| format_elapsed(ended.duration_since(at))).unwrap_or_default();
            let text = match code {
                Some(code) => format!("{} {code} · {elapsed}", strings::RUN_EXIT_CODE),
                None => format!("{} · {elapsed}", strings::RUN_EXITED),
            };
            let color = if *code == Some(0) { theme::fg_muted() } else { theme::error() };
            return (text, color);
        }
        let elapsed = self.started_at.map_or_else(String::new, |at| format_elapsed(at.elapsed()));
        (format!("{}{elapsed}", strings::RUN_RUNNING), theme::accent())
    }

    /// Appends one chunk, splitting it into lines (a chunk boundary may sit
    /// mid-line: the fragment carries into the next chunk). Oldest lines drop
    /// past the cap.
    pub fn append(&mut self, stream: Stream, text: &str) {
        if self.exited.is_some() {
            return;
        }
        let mut text = text;
        // Complete the carried fragment first (ide-run batches per stream, so
        // a fragment's continuation arrives on the same stream).
        if !self.pending.1.is_empty() {
            match text.find('\n') {
                Some(end) => {
                    self.pending.1.push_str(&text[..end]);
                    let (s, line) = std::mem::replace(&mut self.pending, (Stream::System, String::new()));
                    self.push_line(s, line);
                    text = &text[end + 1..];
                }
                None => {
                    self.pending.1.push_str(text);
                    return;
                }
            }
        }
        let mut lines = text.split('\n');
        let last = lines.next_back().unwrap_or("");
        for line in lines {
            self.push_line(stream, line.to_owned());
        }
        if !last.is_empty() {
            self.pending = (stream, last.to_owned());
        }
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
    }

    /// The run exited: freeze the timer, surface the code.
    pub fn exited(&mut self, code: Option<i32>) {
        self.exited = Some((code, Instant::now()));
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.pending = (Stream::System, String::new());
        self.scroll.scroll_to_bottom();
    }

    /// The user scrolled up: stop chasing the tail.
    pub fn on_wheel(&mut self, event: &ScrollWheelEvent) {
        let up = match &event.delta {
            ScrollDelta::Pixels(delta) => delta.y > px(0.0),
            ScrollDelta::Lines(delta) => delta.y > 0.0,
        };
        if up {
            self.follow = false;
        }
    }

    pub fn jump_to_end(&mut self) {
        self.follow = true;
        self.scroll.scroll_to_bottom();
    }

    fn push_line(&mut self, stream: Stream, line: String) {
        self.lines.push((stream, line));
        if self.lines.len() > MAX_LINES {
            let drop = self.lines.len() - MAX_LINES;
            self.lines.drain(..drop);
        }
    }

    fn line_color(stream: Stream) -> gpui::Hsla {
        match stream {
            Stream::Stdout => theme::fg(),
            Stream::Stderr => theme::error(),
            Stream::System => theme::fg_muted(),
        }
    }

    fn send_rerun(&self, filter: test_tree::TestFilter) {
        self.bus.send(crate::bus::BusMessage::TestRerun { index: self.index, filter });
    }

    /// The expected/actual overlay request (the header's failure-diff chip).
    fn send_diff(&self, title: &str, expected: &str, actual: &str) {
        self.bus.send(crate::bus::BusMessage::TestDiff {
            title: format!("{title} · {}", strings::TEST_DIFF_TITLE),
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        });
    }

    /// The state's icon face and color.
    fn state_icon(state: TestState) -> (KitIcon, gpui::Hsla) {
        match state {
            TestState::Passed => (KitIcon::Check, theme::diff_added()),
            TestState::Failed => (KitIcon::X, theme::error()),
            TestState::Running => (KitIcon::LoaderCircle, theme::accent()),
            TestState::Skipped => (KitIcon::SkipForward, theme::fg_muted()),
            TestState::Pending => (KitIcon::Circle, theme::fg_muted()),
        }
    }

    /// One suite row: aggregate icon, name, total, rerun.
    fn render_suite_row(&self, index: usize, suite: &TestSuite) -> gpui::AnyElement {
        let (icon, color) = Self::state_icon(suite.state);
        div()
            .id(("test-suite", index))
            .h(px(22.0))
            .px_2()
            .mt_1()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(12.))
            .hover(|h| h.bg(theme::raised_bg()))
            .child(Icon::new(icon).text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme::fg())
                    .child(SharedString::from(suite.name.clone())),
            )
            .when_some(suite.duration_ms, |row, ms| {
                row.child(
                    div().text_size(px(10.)).text_color(theme::fg_muted()).child(SharedString::from(format_ms(ms))),
                )
            })
            .child({
                let bus = self.bus.clone();
                let index = self.index;
                // The suite rerun's selection face is per runner: the file
                // for the JSON reporters (the suite id's head), a fresh All
                // for cargo (its suite id is no filter), the id otherwise.
                let filter = match self.runner {
                    Some(test_tree::Runner::Vitest) | Some(test_tree::Runner::Jest) => test_tree::TestFilter::Suite {
                        name: suite.id.split("::").next().unwrap_or(&suite.id).to_owned(),
                    },
                    Some(test_tree::Runner::Cargo) => test_tree::TestFilter::All,
                    _ => test_tree::TestFilter::Suite { name: suite.id.clone() },
                };
                Button::new(("test-suite-rerun", index))
                    .icon(KitIcon::Play)
                    .xsmall()
                    .ghost()
                    .tooltip("Rerun suite")
                    .on_click(move |_, _, _cx| {
                        bus.send(crate::bus::BusMessage::TestRerun { index, filter: filter.clone() });
                    })
            })
            .into_any_element()
    }

    /// One case row: state icon, leaf name, failure message, duration, and
    /// for failures the jump (click), the rerun, and the diff chip.
    fn render_case_row(&self, index: usize, case: &TestCase) -> gpui::AnyElement {
        let (icon, color) = Self::state_icon(case.state);
        // Every capture is an owned clone: the closures outlive the borrow
        // of `case` (the render pass's tree snapshot). The row click jumps
        // to the failure's location; the diff chip (before the play button,
        // clear of the scrollbar) opens the expected/actual overlay.
        let case_name = case.name.clone();
        let case_id = case.id.clone();
        let jump_file = case.file.clone();
        let jump_line = case.line;
        let diff_expected = case.expected.clone().unwrap_or_default();
        let diff_actual = case.actual.clone().unwrap_or_default();
        let has_diff = case.expected.is_some() && case.actual.is_some();
        let mut row = div()
            .id(SharedString::from(format!("test-case-{index}-{case_id}")))
            .h(px(22.0))
            .pl(px(24.0))
            .pr_1()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(12.))
            .hover(|h| h.bg(theme::raised_bg()))
            .when_some(
                jump_file.map(|file| (Some(file), jump_line)).or(has_diff.then_some((None, None))),
                |row, (file, line)| {
                    let bus = self.bus.clone();
                    let case_name = case_name.clone();
                    let diff_expected = diff_expected.clone();
                    let diff_actual = diff_actual.clone();
                    row.on_click(move |_: &ClickEvent, _, _: &mut gpui::App| {
                        // Jump to the failure's location; a case the parser
                        // located nowhere falls through to its diff.
                        if let Some(file) = &file {
                            bus.send(crate::bus::BusMessage::TestJump { file: Some(file.clone()), line });
                        } else {
                            bus.send(crate::bus::BusMessage::TestDiff {
                                title: format!("{case_name} · {}", strings::TEST_DIFF_TITLE),
                                expected: diff_expected.clone(),
                                actual: diff_actual.clone(),
                            });
                        }
                    })
                },
            )
            .child(Icon::new(icon).text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(match case.state {
                        TestState::Failed => theme::error(),
                        _ => theme::fg(),
                    })
                    .child(SharedString::from(case_name.clone())),
            )
            .when_some(case.message.clone(), |row, message| {
                row.child(
                    div()
                        .max_w(px(360.0))
                        .min_w_0()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(theme::fg_muted())
                        .child(SharedString::from(message)),
                )
            })
            .when_some(case.duration_ms, |row, ms| {
                row.child(
                    div().text_size(px(10.)).text_color(theme::fg_muted()).child(SharedString::from(format_ms(ms))),
                )
            });
        if has_diff {
            let bus = self.bus.clone();
            let diff_title = case_name.clone();
            row = row.child(
                Button::new(SharedString::from(format!("test-case-diff-{index}-{}", case.id)))
                    .icon(KitIcon::Diff)
                    .xsmall()
                    .ghost()
                    .tooltip("Expected vs actual")
                    .on_click(move |_, _, _cx| {
                        bus.send(crate::bus::BusMessage::TestDiff {
                            title: format!("{diff_title} · {}", strings::TEST_DIFF_TITLE),
                            expected: diff_expected.clone(),
                            actual: diff_actual.clone(),
                        });
                    }),
            );
        }
        row = row.child({
            let bus = self.bus.clone();
            let index = self.index;
            let rerun_filter =
                test_tree::TestFilter::Case { id: case.id.clone(), name: case.name.clone(), file: case.file.clone() };
            Button::new(SharedString::from(format!("test-case-rerun-{index}-{}", case.id)))
                .icon(KitIcon::Play)
                .xsmall()
                .ghost()
                .tooltip("Rerun this test")
                .on_click(move |_, _, _cx| {
                    bus.send(crate::bus::BusMessage::TestRerun { index, filter: rerun_filter.clone() });
                })
        });
        row.into_any_element()
    }

    /// The output section (the uniform_list face, unchanged from M4).
    /// `wheel` is the follow-the-tail chase (a cx.listener result, whose
    /// shape is the div handler's own).
    fn render_output(
        &self,
        wheel: impl Fn(&ScrollWheelEvent, &mut Window, &mut gpui::App) + 'static,
        this: gpui::Entity<Self>,
        lines: usize,
    ) -> gpui::AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .on_scroll_wheel(wheel)
            .child(
                uniform_list("run-lines", lines, move |range, _, cx| {
                    let panel = this.read(cx);
                    range
                        .map(|index| {
                            let (stream, text) = &panel.lines[index];
                            div()
                                .h(px(LINE_HEIGHT))
                                .px_2()
                                .flex()
                                .items_center()
                                .font_family(theme::FONT_FAMILY)
                                .text_size(px(12.))
                                .text_color(Self::line_color(*stream))
                                .truncate()
                                .child(SharedString::from(text.clone()))
                        })
                        .collect()
                })
                .track_scroll(&self.scroll)
                .h_full(),
            )
            .into_any_element()
    }
}

impl Focusable for RunPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 { format!("{secs:.1}s") } else { format!("{}m{:02.0}s", secs as u64 / 60, secs % 60.0) }
}

fn format_ms(ms: f64) -> String {
    if ms >= 1000.0 { format!("{:.1}s", ms / 1000.0) } else { format!("{ms:.0}ms") }
}

impl Render for RunPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let (status, status_color) = self.status();
        let this = cx.entity();
        let lines = self.lines.len();
        let mut header = div()
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
                    .child(SharedString::from(strings::RUN_TITLE.to_owned())),
            )
            .child(div().text_color(theme::fg()).child(SharedString::from(self.config_name.clone())))
            .child(div().text_color(status_color).child(SharedString::from(status)));
        if self.running() {
            header = header.child(Button::new("run-stop").label(strings::RUN_STOP).xsmall().secondary().on_click(
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(run_id) = this.run_id
                        && let Err(e) = this.host.stop_run(run_id)
                    {
                        eprintln!("[run] stop: {}", e.message);
                    }
                    cx.notify();
                }),
            ));
        }
        header = header
            .child(Button::new("run-jump-end").icon(KitIcon::ArrowDown).xsmall().ghost().on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.jump_to_end();
                    cx.notify();
                },
            )))
            .child(Button::new("run-clear").label(strings::RUN_CLEAR).xsmall().ghost().on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.clear();
                    cx.notify();
                },
            )));
        // A test run renders the tree above the output (the tree is the
        // primary face); a plain run is the output stream alone.
        let Some(test) = self.test.clone() else {
            let wheel = cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                this.on_wheel(event);
                cx.notify();
            });
            return div()
                .flex()
                .flex_col()
                .size_full()
                .bg(theme::surface_bg())
                .child(header)
                .child(div().flex_1().min_h_0().flex().flex_col().child(self.render_output(wheel, this, lines)));
        };
        let state = test.state.lock().unwrap().clone();
        let summary = state.summary;
        let mut root = div().flex().flex_col().size_full().bg(theme::surface_bg()).child(header);
        // The totals line + the rerun-all button (IDEA's test-toolbar face).
        root = root.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h_7()
                .text_size(px(11.))
                .border_b_1()
                .border_color(theme::border())
                .child(div().text_color(if summary.failed > 0 { theme::error() } else { theme::fg_muted() }).child(
                    SharedString::from(format!(
                        "{} passed / {} failed / {} skipped",
                        summary.passed, summary.failed, summary.skipped
                    )),
                ))
                .child(
                    div()
                        .text_color(match state.phase {
                            test_tree::TestPhase::Starting => theme::warning(),
                            test_tree::TestPhase::Running => theme::accent(),
                            test_tree::TestPhase::Finished => theme::fg_muted(),
                        })
                        .child(SharedString::from(format!("{:?}", state.phase).to_lowercase())),
                )
                .child(div().flex_1())
                .when_some(
                    // The run's failure diff face: the first failed case
                    // carrying an expected/actual pair (IDEA's failure
                    // panel, reduced to the diff chip).
                    state
                        .suites
                        .iter()
                        .flat_map(|s| &s.cases)
                        .find(|c| c.state == TestState::Failed && c.expected.is_some() && c.actual.is_some()),
                    |header, case| {
                        let title = case.name.clone();
                        let expected = case.expected.clone().unwrap_or_default();
                        let actual = case.actual.clone().unwrap_or_default();
                        header.child(
                            Button::new("test-failure-diff")
                                .icon(KitIcon::Diff)
                                .xsmall()
                                .ghost()
                                .tooltip(format!("Expected vs actual — {}", case.name))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, _| {
                                    this.send_diff(&title, &expected, &actual);
                                })),
                        )
                    },
                )
                .child(
                    Button::new("test-rerun-all")
                        .icon(KitIcon::RotateCw)
                        .xsmall()
                        .ghost()
                        .tooltip("Rerun all")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                            this.send_rerun(test_tree::TestFilter::All);
                        })),
                )
                .child(
                    Button::new("test-toggle-output")
                        .icon(if self.show_output { KitIcon::ChevronDown } else { KitIcon::ChevronUp })
                        .xsmall()
                        .ghost()
                        .tooltip("Toggle raw output")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.show_output = !this.show_output;
                            cx.notify();
                        })),
                ),
        );
        // The tree: suites with their cases; failures carry message + diff.
        let mut tree = div().id("test-tree").flex_1().min_h_0().overflow_y_scroll().py_1();
        if state.suites.is_empty() {
            tree = tree.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(theme::fg_muted())
                    .child(SharedString::from(strings::TEST_TREE_WAITING.to_owned())),
            );
        }
        for (index, suite) in state.suites.iter().enumerate() {
            tree = tree.child(self.render_suite_row(index, suite));
            for (case_index, case) in suite.cases.iter().enumerate() {
                tree = tree.child(self.render_case_row(index * 1000 + case_index, case));
            }
        }
        root = root.child(tree);
        if self.show_output {
            let wheel = cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                this.on_wheel(event);
                cx.notify();
            });
            root = root.child(
                div()
                    .h(px(160.0))
                    .flex_none()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(self.render_output(wheel, this, lines)),
            );
        }
        root
    }
}

/// The chunk's `(stream, text)` face.
pub fn stream_of(chunk: &OutputChunk) -> (Stream, &str) {
    match chunk {
        OutputChunk::Stdout { text } => (Stream::Stdout, text),
        OutputChunk::Stderr { text } => (Stream::Stderr, text),
        OutputChunk::System { text } => (Stream::System, text),
    }
}
