//! The M1 editor surface: the M0 spike's hand-drawn text element, grown from
//! one line to a viewport of lines, still on the same simple buffer — a
//! `String` plus a UTF-16 caret and an optional UTF-16 marked range (no rope,
//! no tree-sitter: that is M2). The `InputHandler` implementation is the same
//! 1:1 NSTextInputClient exposure M0 verified end to end with the macOS
//! Pinyin IME; only the geometry (caret/bounds → line-aware coordinates) is
//! new.
//!
//! Marked text lives inline in the buffer (that is what NSTextInputClient
//! means by "marked": it is in the document, just not committed), which is
//! what makes candidate-window geometry and commit replacement fall out of
//! ordinary range math.

use std::cell::Cell;
use std::ops::Range;

use gpui::{
    App, Bounds, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, InputHandler, InspectorElementId,
    InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, ParentElement, Pixels, Point,
    Render, ScrollWheelEvent, ShapedLine, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, fill,
    point, px, relative, rgba, size,
};

use crate::theme;

fn rgba_of(c: u32) -> gpui::Rgba {
    rgba((c << 8) | 0xff)
}

fn plain_run(len: usize, color: gpui::Hsla) -> TextRun {
    TextRun { len, font: theme::font(), color, background_color: None, underline: None, strikethrough: None }
}

/// UTF-16 length of a string.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset of the char boundary at (or past) a UTF-16 offset.
fn utf16_to_byte(text: &str, target: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units >= target {
            return offset;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// The buffer with IME bookkeeping. All offsets are UTF-16.
struct Buffer {
    text: String,
    /// The in-flight composition, inline in `text`.
    marked: Option<Range<usize>>,
    caret: usize,
    /// Bumped by every content mutation so the workbench can tell edits from
    /// caret moves when it observes the pane.
    edits: u32,
}

impl Buffer {
    fn new(text: String) -> Self {
        let caret = utf16_len(&text);
        Self { text, marked: None, caret, edits: 0 }
    }

    fn utf16_len(&self) -> usize {
        utf16_len(&self.text)
    }

    fn clamp(&self, utf16: usize) -> usize {
        utf16.min(self.utf16_len())
    }

    fn slice_utf16(&self, range: Range<usize>) -> String {
        let start = utf16_to_byte(&self.text, self.clamp(range.start));
        let end = utf16_to_byte(&self.text, self.clamp(range.end));
        self.text[start..end].to_owned()
    }

    /// Replace a UTF-16 range with `new_text`; returns the inserted range.
    fn replace(&mut self, base: Range<usize>, new_text: &str) -> Range<usize> {
        let start = self.clamp(base.start);
        let end = self.clamp(base.end).max(start);
        let (byte_start, byte_end) = (utf16_to_byte(&self.text, start), utf16_to_byte(&self.text, end));
        self.text.replace_range(byte_start..byte_end, new_text);
        let len = utf16_len(new_text);
        self.edits += 1;
        start..start + len
    }

    /// `setMarkedText:selectedRange:replacementRange:`. `range_utf16` (when
    /// given) replaces that range; otherwise the previous marked range, or an
    /// insertion at the caret. An empty `new_text` cancels the composition
    /// (the macOS Esc path).
    fn set_marked(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, new_selected: Option<Range<usize>>) {
        let base = range_utf16.or_else(|| self.marked.clone()).unwrap_or(self.caret..self.caret);
        let marked = self.replace(base, new_text);
        self.marked = if new_text.is_empty() || marked.is_empty() { None } else { Some(marked.clone()) };
        // The platform's selected range is relative to the marked text start.
        self.caret = match new_selected {
            Some(selected) => {
                let start = selected.start.clamp(0, marked.len());
                let end = selected.end.clamp(start, marked.len());
                marked.start + end
            }
            None => marked.end,
        };
    }

    /// `insertText:replacementRange:`. Committing replaces the marked range
    /// when there is one; otherwise inserts at the caret.
    fn insert_commit(&mut self, range_utf16: Option<Range<usize>>, text: &str) {
        let base = range_utf16.or_else(|| self.marked.clone()).unwrap_or(self.caret..self.caret);
        let inserted = self.replace(base, text);
        self.marked = None;
        self.caret = inserted.end;
    }

    fn unmark(&mut self) {
        self.marked = None;
    }

    fn insert_plain(&mut self, text: &str) {
        self.insert_commit(None, text);
    }

    fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        let end = self.caret;
        let start = utf16_to_byte(&self.text, self.caret - 1);
        let start = utf16_len(&self.text[..start]);
        let removed = self.replace(start..end, "");
        self.caret = removed.start;
        // Deleting into a marked region would desync the IME; drop it.
        self.marked = None;
    }

    fn delete_forward(&mut self) {
        let len = self.utf16_len();
        if self.caret >= len {
            return;
        }
        let start = self.caret;
        let end_byte = utf16_to_byte(&self.text, self.caret + 1);
        let end = utf16_len(&self.text[..end_byte]);
        self.replace(start..end, "");
        self.marked = None;
    }

    fn caret_left(&mut self) {
        if self.caret > 0 {
            let byte = utf16_to_byte(&self.text, self.caret - 1);
            self.caret = utf16_len(&self.text[..byte]);
        }
    }

    fn caret_right(&mut self) {
        let len = self.utf16_len();
        if self.caret < len {
            let byte = utf16_to_byte(&self.text, self.caret + 1);
            self.caret = utf16_len(&self.text[..byte]);
        }
    }

    fn caret_home(&mut self) {
        let index = LineIndex::build(&self.text);
        self.caret = index.starts[index.line_of(self.caret)];
    }

    fn caret_end(&mut self) {
        let index = LineIndex::build(&self.text);
        let line = index.line_of(self.caret);
        self.caret = index.span_of(&self.text, line).end;
    }
}

/// UTF-16 offsets of every line's first character; the `\n` itself is the gap
/// between a line and the next (so a caret sitting on it renders at the end
/// of the line before it, like every native text view).
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn build(text: &str) -> Self {
        let mut starts = vec![0usize];
        let mut units = 0usize;
        for ch in text.chars() {
            units += ch.len_utf16();
            if ch == '\n' {
                starts.push(units);
            }
        }
        Self { starts }
    }

    fn count(&self) -> usize {
        self.starts.len()
    }

    /// The line a caret at `utf16` renders on: the last line whose start is
    /// at or before the offset.
    fn line_of(&self, utf16: usize) -> usize {
        match self.starts.binary_search(&utf16) {
            Ok(line) => line,
            Err(next) => next - 1,
        }
    }

    /// UTF-16 content span of `line` (the `\n` excluded).
    fn span_of(&self, text: &str, line: usize) -> Range<usize> {
        let start = self.starts[line];
        let end = if line + 1 < self.starts.len() { self.starts[line + 1] - 1 } else { utf16_len(text) };
        start..end.max(start)
    }
}

/// Element geometry the IME handler needs between frames, in window
/// coordinates: where the text area starts, how tall a line is, which line is
/// scrolled to the top and the viewport height.
#[derive(Clone, Copy)]
struct Geometry {
    origin: Point<Pixels>,
    line_height: Pixels,
    scroll: f32,
    height: Pixels,
}

pub struct EditorPane {
    buf: Buffer,
    focus: FocusHandle,
    geometry: Cell<Option<Geometry>>,
    /// Top visible line, in lines (clamped on use).
    scroll: Cell<f32>,
    focused_once: Cell<bool>,
}

impl Focusable for EditorPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EditorPane {
    pub fn new(text: String, cx: &mut gpui::Context<Self>) -> Self {
        Self {
            buf: Buffer::new(text),
            focus: cx.focus_handle(),
            geometry: Cell::new(None),
            scroll: Cell::new(0.0),
            focused_once: Cell::new(false),
        }
    }

    /// Full buffer text, the save/push source of truth.
    pub fn text(&self) -> String {
        self.buf.text.clone()
    }

    /// Backend-driven replacement (external change reload): clamps the caret
    /// into the new text and restarts the scroll at the top.
    pub fn set_text(&mut self, text: String, cx: &mut gpui::Context<Self>) {
        self.buf = Buffer::new(text);
        self.scroll.set(0.0);
        cx.notify();
    }

    /// How many content mutations happened, for edit-vs-move observation.
    pub fn edit_count(&self) -> u32 {
        self.buf.edits
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let ks = &ev.keystroke;
        let mods = &ks.modifiers;
        if mods.platform || mods.function || mods.control || mods.alt {
            return;
        }
        if self.buf.marked.is_some() {
            // While composing, macOS routes every key through the IME first;
            // reaching here means the IME passed it through. Leave it alone.
            eprintln!("[key] passthrough while composing: {}", ks.key);
            return;
        }
        match ks.key.as_str() {
            "backspace" => self.buf.backspace(),
            "delete" => self.buf.delete_forward(),
            "left" => self.buf.caret_left(),
            "right" => self.buf.caret_right(),
            "home" => self.buf.caret_home(),
            "end" => self.buf.caret_end(),
            "enter" => self.buf.insert_plain("\n"),
            "escape" => {}
            _ => {
                if let Some(text) = ks.key_char.as_ref() {
                    self.buf.insert_plain(text);
                }
            }
        }
        self.clamp_scroll_to_caret();
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(geometry) = self.geometry.get() else { return };
        let index = LineIndex::build(&self.buf.text);
        let visible = ((geometry.height - px(theme::TEXT_INSET) * 2.0) / geometry.line_height) as i64;
        let row = (((ev.position.y - geometry.origin.y) / geometry.line_height) as i64 + self.scroll.get() as i64)
            .clamp(0, visible.max(1) - 1)
            .clamp(0, index.count() as i64 - 1) as usize;
        let span = index.span_of(&self.buf.text, row);
        let line = self.buf.slice_utf16(span.clone());
        let x = ev.position.x - geometry.origin.x;
        let col = utf16_index_at_x(window, &line, x, theme::px_font_size());
        self.buf.caret = span.start + col;
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let delta = match ev.delta {
            gpui::ScrollDelta::Pixels(point) => point.y / theme::px_line_height(),
            gpui::ScrollDelta::Lines(point) => point.y,
        };
        self.scroll.set(self.scroll.get() - delta);
        self.clamp_scroll();
        cx.notify();
    }

    /// Keeps the viewport inside the grid of lines the buffer has.
    fn clamp_scroll(&mut self) {
        let max = self.line_count().saturating_sub(1) as f32;
        self.scroll.set(self.scroll.get().clamp(0.0, max));
    }

    fn line_count(&self) -> usize {
        LineIndex::build(&self.buf.text).count()
    }

    /// Scrolls the caret's line into view after caret-moving edits.
    fn clamp_scroll_to_caret(&mut self) {
        let index = LineIndex::build(&self.buf.text);
        let caret_line = index.line_of(self.buf.caret) as f32;
        let visible =
            self.geometry.get().map_or(20.0, |g| (g.height - px(theme::TEXT_INSET) * 2.0) / g.line_height).max(1.0);
        let scroll = self.scroll.get();
        if caret_line < scroll {
            self.scroll.set(caret_line);
        } else if caret_line >= scroll + visible {
            self.scroll.set(caret_line - visible + 1.0);
        }
        self.clamp_scroll();
    }
}

impl Render for EditorPane {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if !self.focused_once.get() {
            self.focused_once.set(true);
            window.focus(&self.focus, cx);
        }
        gpui::div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .bg(theme::surface_bg())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_key_down(cx.listener(Self::on_key))
            .child(EditorElement { view: cx.entity() })
    }
}

// ---------------------------------------------------------------------------
// EditorElement — paint-only element: plan in prepaint, draw + register the
// input handler in paint
// ---------------------------------------------------------------------------

struct EditorElement {
    view: Entity<EditorPane>,
}

struct PrepaintState {
    /// The visible lines in buffer order: (line index, shaped line).
    lines: Vec<(usize, ShapedLine)>,
    caret: Bounds<Pixels>,
}

/// The text runs for one visible line: committed text plain; the marked
/// range's intersection with the line gets a distinct background and an
/// accent underline. `marked` is a buffer-wide UTF-16 range, `span` the
/// line's.
fn line_runs(line: &str, marked: Option<Range<usize>>, span: Range<usize>, fg: gpui::Hsla) -> Vec<TextRun> {
    let relative = marked.map(|marked| {
        let start = marked.start.clamp(span.start, span.end) - span.start;
        let end = marked.end.clamp(span.start, span.end) - span.start;
        start..end
    });
    let Some(relative) = relative.filter(|r| r.start < r.end) else {
        return vec![plain_run(line.len(), fg)];
    };
    let start = utf16_to_byte(line, relative.start);
    let end = utf16_to_byte(line, relative.end);
    let mut runs = Vec::with_capacity(3);
    if start > 0 {
        runs.push(plain_run(start, fg));
    }
    runs.push(TextRun {
        len: end - start,
        font: theme::font(),
        color: fg,
        background_color: Some(rgba_of(theme::MARKED_BG).into()),
        underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(rgba_of(theme::ACCENT).into()), wavy: false }),
        strikethrough: None,
    });
    if end < line.len() {
        runs.push(plain_run(line.len() - end, fg));
    }
    runs
}

/// Width of `text` shaped as one plain run — the prefix-width primitive every
/// caret/range measurement here builds on.
fn shaped_width(window: &Window, text: &str, font_size: Pixels) -> Pixels {
    if text.is_empty() {
        return px(0.0);
    }
    let run = plain_run(text.len(), gpui::black());
    window.text_system().shape_line(text.into(), font_size, &[run], None).width()
}

/// Nearest caret (UTF-16 index inside `line`) for a window-space x.
fn utf16_index_at_x(window: &Window, line: &str, x: Pixels, font_size: Pixels) -> usize {
    let mut units = 0usize;
    for (offset, ch) in line.char_indices() {
        let width = shaped_width(window, &line[..offset], font_size);
        if width >= x {
            // Snap to the nearer boundary of the character under the mouse.
            let char_width = shaped_width(window, &line[..offset + ch.len_utf8()], font_size) - width;
            return if x - width > char_width / 2.0 { units + ch.len_utf16() } else { units };
        }
        units += ch.len_utf16();
    }
    utf16_len(line)
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("m1-editor".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        let layout_id = window.request_layout(style, std::iter::empty(), cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let pane = self.view.read(cx);
        let font_size = theme::px_font_size();
        let line_height = theme::px_line_height();
        let origin = point(bounds.origin.x + px(theme::TEXT_INSET), bounds.origin.y + px(theme::TEXT_INSET));

        let text = pane.buf.text.clone();
        let marked = pane.buf.marked.clone();
        let caret = pane.buf.caret;
        let index = LineIndex::build(&text);
        let scroll = pane.scroll.get();
        let first = (scroll.floor().max(0.0) as usize).min(index.count().saturating_sub(1));
        let visible = ((bounds.size.height - px(theme::TEXT_INSET) * 2.0) / line_height).ceil() as usize;
        let last = (first + visible).min(index.count());

        let fg = theme::fg();
        let lines = (first..last)
            .map(|line| {
                let span = index.span_of(&text, line);
                let content = &text[utf16_to_byte(&text, span.start)..utf16_to_byte(&text, span.end)];
                let shaped = window.text_system().shape_line(
                    content.to_string().into(),
                    font_size,
                    &line_runs(content, marked.clone(), span, fg),
                    None,
                );
                (line, shaped)
            })
            .collect();

        // Caret rect: the line it renders on plus its prefix width.
        let caret_line = index.line_of(caret);
        let caret_span = index.span_of(&text, caret_line);
        let prefix = pane.buf.slice_utf16(caret_span.start..caret);
        let caret_x = shaped_width(window, &prefix, font_size);
        let caret_y = origin.y + line_height * (caret_line as f32 - scroll);
        let caret = Bounds::new(point(origin.x + caret_x, caret_y), size(px(2.0), line_height));

        pane.geometry.set(Some(Geometry { origin, line_height, scroll, height: bounds.size.height }));

        PrepaintState { lines, caret }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(geometry) = self.view.read(cx).geometry.get() else { return };
        let line_height = geometry.line_height;
        let origin = geometry.origin;

        for (line, shaped) in prepaint.lines.drain(..) {
            let y = origin.y + line_height * (line as f32 - geometry.scroll);
            let _ = shaped.paint(point(origin.x, y), line_height, TextAlign::Left, None, window, cx);
        }
        window.paint_quad(fill(prepaint.caret, rgba_of(theme::ACCENT)));

        // Publish the geometry for `bounds_for_range` between frames, then
        // register the IME handler for the upcoming frame.
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(&focus, EditorInputHandler { view: self.view.clone() }, cx);
    }
}

// ---------------------------------------------------------------------------
// EditorInputHandler — the NSTextInputClient face of the element
// ---------------------------------------------------------------------------

/// gpui's input protocol for the editor element. Every mutation logs its
/// payload to stderr — the M0 trace format, kept so IME behaviour in the
/// tabbed workbench stays comparable against the M0 report.
struct EditorInputHandler {
    view: Entity<EditorPane>,
}

impl InputHandler for EditorInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        let range = self.view.read(cx).buf.caret;
        Some(UTF16Selection { range: range..range, reversed: false })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view.read(cx).buf.marked.clone()
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        let pane = self.view.read(cx);
        let clamped = pane.buf.clamp(range_utf16.start)..pane.buf.clamp(range_utf16.end);
        if clamped.is_empty() {
            return None;
        }
        *adjusted_range = Some(clamped.clone());
        Some(pane.buf.slice_utf16(clamped))
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        eprintln!("[ime] insertText range={:?} text={:?}", replacement_range, text);
        self.view.update(cx, |pane, cx| {
            pane.buf.insert_commit(replacement_range, text);
            pane.clamp_scroll_to_caret();
            cx.notify();
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        eprintln!("[ime] setMarkedText range={:?} text={:?} selected={:?}", range_utf16, new_text, new_selected_range);
        self.view.update(cx, |pane, cx| {
            pane.buf.set_marked(range_utf16, new_text, new_selected_range);
            pane.clamp_scroll_to_caret();
            cx.notify();
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        eprintln!("[ime] unmarkText");
        self.view.update(cx, |pane, cx| {
            pane.buf.unmark();
            cx.notify();
        });
    }

    /// Candidate-window anchor: the window-space rect of a UTF-16 range,
    /// mapped onto the range's start line. The platform converts this to
    /// screen coordinates (`firstRectForCharacterRange`).
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let pane = self.view.read(cx);
        let geometry = pane.geometry.get()?;
        let text = pane.buf.text.clone();
        let index = LineIndex::build(&text);
        let line = index.line_of(pane.buf.clamp(range_utf16.start));
        let span = index.span_of(&text, line);
        let start = pane.buf.clamp(range_utf16.start).max(span.start);
        let end = pane.buf.clamp(range_utf16.end).min(span.end).max(start);
        let x0 = shaped_width(window, &pane.buf.slice_utf16(span.start..start), theme::px_font_size());
        let x1 = shaped_width(window, &pane.buf.slice_utf16(span.start..end), theme::px_font_size());
        let bounds = Bounds::new(
            point(geometry.origin.x + x0, geometry.origin.y + geometry.line_height * (line as f32 - geometry.scroll)),
            size(x1 - x0, geometry.line_height),
        );
        eprintln!("[ime] firstRect {range_utf16:?} -> {bounds:?}");
        Some(bounds)
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut App) -> Option<usize> {
        let pane = self.view.read(cx);
        let geometry = pane.geometry.get()?;
        let text = pane.buf.text.clone();
        let index = LineIndex::build(&text);
        let visible = ((geometry.height - px(theme::TEXT_INSET) * 2.0) / geometry.line_height).max(1.0) as i64;
        let row = (((point.y - geometry.origin.y) / geometry.line_height) as i64 + geometry.scroll as i64)
            .clamp(0, visible - 1);
        if row < 0 || row as usize >= index.count() {
            return None;
        }
        let span = index.span_of(&text, row as usize);
        let line_text = pane.buf.slice_utf16(span.clone());
        let col = utf16_index_at_x(window, &line_text, point.x - geometry.origin.x, theme::px_font_size());
        Some(span.start + col)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    /// Route printable keys through the IME first when a CJK input source is
    /// active: without this, the first Pinyin keystroke would be inserted as
    /// plain ASCII instead of starting a composition (see the macOS keydown
    /// routing in gpui-pre-macos/src/window.rs).
    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
}
