//! The Run dock (M4): a bottom-dock panel streaming one run's output. Lines
//! arrive through the workbench's bus pump (ide-run batches at ~8 ms), are
//! kept as `(stream, text)` rows — capped, oldest dropped — and render
//! through gpui's virtualized `uniform_list`, so a noisy `cargo build` only
//! lays out the visible window. `follow` pins the view to the tail until the
//! user scrolls up.

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    ClickEvent, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, Styled, Window, div, px, uniform_list,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{IconName, Sizable as _};
use ide_run::OutputChunk;

use crate::host::Host;
use crate::{strings, theme};

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

pub struct RunPanel {
    host: Arc<Host>,
    /// The configuration's display name (the dock header).
    pub config_name: String,
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
    scroll: gpui::UniformListScrollHandle,
    focus: FocusHandle,
}

impl RunPanel {
    pub fn new(host: Arc<Host>, config_name: String, cx: &mut gpui::Context<Self>) -> Self {
        Self {
            host,
            config_name,
            run_id: None,
            starting: true,
            started_at: None,
            exited: None,
            lines: Vec::new(),
            pending: (Stream::System, String::new()),
            follow: true,
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
            .child(Button::new("run-jump-end").icon(IconName::ArrowDown).xsmall().ghost().on_click(cx.listener(
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
        let body = div()
            .flex_1()
            .min_h_0()
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                this.on_wheel(event);
                cx.notify();
            }))
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
            );
        div().flex().flex_col().size_full().bg(theme::surface_bg()).child(header).child(body)
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
