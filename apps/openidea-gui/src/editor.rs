//! The M2 editor surface, phase 1: the M0/M1 hand-drawn element moved onto
//! `ide-text`'s rope. The pane holds a local [`ide_text::Document`] that
//! mirrors the backend document version-for-version: every user edit applies
//! the same `ChangeSet` locally and through `Documents::edit` (the backend
//! keeps dirty / save / reload semantics, no full-text pushes anymore), and
//! backend-originated patches (`DocPatched`: external reload) are reconciled
//! **by version** — an exact base-version match applies incrementally,
//! anything else resyncs from a snapshot. The `InputHandler` implementation
//! is the same NSTextInputClient face M0 verified end to end with the macOS
//! Pinyin IME; multi-cursor, highlighting and folding arrive in later M2
//! phases.
//!
//! Marked text lives inline in the buffer (that is what NSTextInputClient
//! means by "marked": it is in the document, just not committed), which is
//! what makes candidate-window geometry and commit replacement fall out of
//! ordinary range math.

use std::cell::Cell;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Bounds, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, InputHandler, InspectorElementId,
    InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, ParentElement, Pixels, Point,
    Render, ScrollWheelEvent, ShapedLine, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, fill,
    point, px, relative, rgba, size,
};
use ide_core::{ChangeSet, DocId, TextChange};
use ide_workspace::DocumentInfo;

use crate::host::Host;
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
fn byte_at_utf16(text: &str, target: usize) -> usize {
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        if units >= target {
            return offset;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// The rope-backed buffer with IME bookkeeping. All offsets are UTF-16 of the
/// `\n`-normalized text (`ide_text` normalizes on construction, exactly like
/// the backend document this mirrors).
struct Buffer {
    doc: ide_text::Document,
    /// The in-flight composition, inline in `doc`.
    marked: Option<Range<usize>>,
    caret: usize,
}

impl Buffer {
    fn new(text: &str) -> Self {
        let doc = ide_text::Document::new(text);
        let caret = doc.len_utf16() as usize;
        Self { doc, marked: None, caret }
    }

    fn utf16_len(&self) -> usize {
        self.doc.len_utf16() as usize
    }

    fn clamp(&self, utf16: usize) -> u32 {
        utf16.min(self.utf16_len()) as u32
    }

    fn slice_utf16(&self, range: Range<usize>) -> String {
        let start = self.clamp(range.start);
        let end = self.clamp(range.end).max(start);
        self.doc.slice(start, end)
    }

    /// The line a caret at `utf16` renders on: a caret sitting on a `\n`
    /// renders at the end of the line before it, like every native text view.
    fn line_of(&self, utf16: usize) -> u32 {
        self.doc.offset_to_pos(self.clamp(utf16)).line
    }

    fn line_count(&self) -> u32 {
        self.doc.line_count()
    }

    /// UTF-16 offset of `line`'s first character.
    fn line_start(&self, line: u32) -> usize {
        self.doc.pos_to_offset(ide_core::Pos::new(line, 0)) as usize
    }

    /// UTF-16 content span of `line` (the `\n` excluded).
    fn line_span(&self, line: u32) -> Range<usize> {
        let start = self.line_start(line);
        let end = if line + 1 < self.line_count() { self.line_start(line + 1) - 1 } else { self.utf16_len() };
        start..end.max(start)
    }

    /// Replace a UTF-16 range with `new_text`, as a change for [`Buffer`]'s
    /// document; returns the inserted range and the change. Applying it is
    /// the pane's job (it also pushes the change to the backend).
    fn replace(&mut self, base: Range<usize>, new_text: &str) -> (Range<usize>, TextChange) {
        let start = self.clamp(base.start) as usize;
        let end = (self.clamp(base.end) as usize).max(start);
        let change = TextChange { from: start as u32, to: end as u32, insert: new_text.to_owned() };
        let len = utf16_len(new_text);
        (start..start + len, change)
    }

    /// `setMarkedText:selectedRange:replacementRange:`. `range_utf16` (when
    /// given) replaces that range; otherwise the previous marked range, or an
    /// insertion at the caret. An empty `new_text` cancels the composition
    /// (the macOS Esc path). Returns the change for the pane to apply.
    fn set_marked(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
    ) -> TextChange {
        let base = range_utf16.or_else(|| self.marked.clone()).unwrap_or(self.caret..self.caret);
        let (marked, change) = self.replace(base, new_text);
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
        change
    }

    /// `insertText:replacementRange:`. Committing replaces the marked range
    /// when there is one; otherwise inserts at the caret.
    fn insert_commit(&mut self, range_utf16: Option<Range<usize>>, text: &str) -> TextChange {
        let base = range_utf16.or_else(|| self.marked.clone()).unwrap_or(self.caret..self.caret);
        let (inserted, change) = self.replace(base, text);
        self.marked = None;
        self.caret = inserted.end;
        change
    }

    fn unmark(&mut self) {
        self.marked = None;
    }

    fn insert_plain(&mut self, text: &str) -> TextChange {
        self.insert_commit(None, text)
    }

    fn backspace(&mut self) -> Option<TextChange> {
        if self.caret == 0 {
            return None;
        }
        let end = self.caret;
        let start = self.clamp(self.caret - 1) as usize;
        let (_, change) = self.replace(start..end, "");
        self.caret = change.from as usize;
        // Deleting into a marked region would desync the IME; drop it.
        self.marked = None;
        Some(change)
    }

    fn delete_forward(&mut self) -> Option<TextChange> {
        let len = self.utf16_len();
        if self.caret >= len {
            return None;
        }
        let start = self.caret;
        let end = (self.caret + 1).min(len);
        let (_, change) = self.replace(start..end, "");
        self.marked = None;
        Some(change)
    }

    fn caret_left(&mut self) {
        if self.caret > 0 {
            self.caret = self.clamp(self.caret - 1) as usize;
        }
    }

    fn caret_right(&mut self) {
        let len = self.utf16_len();
        if self.caret < len {
            self.caret = (self.caret + 1).min(len);
        }
    }

    fn caret_home(&mut self) {
        self.caret = self.line_start(self.line_of(self.caret));
    }

    fn caret_end(&mut self) {
        let line = self.line_of(self.caret);
        self.caret = self.line_span(line).end;
    }
}

/// Element geometry the IME handler needs between frames, in window
/// coordinates: where the gutter and the text area start, how tall a line is,
/// which line is scrolled to the top and the viewport height.
#[derive(Clone, Copy)]
struct Geometry {
    /// Left edge of the line-number gutter (the element's left inset).
    gutter_x: Pixels,
    /// Gutter width (numbers plus paddings); clicks inside it are not edits.
    gutter_w: Pixels,
    /// Left edge of the text area (gutter plus its own padding).
    origin: Point<Pixels>,
    line_height: Pixels,
    scroll: f32,
    height: Pixels,
}

pub struct EditorPane {
    host: Arc<Host>,
    doc_id: DocId,
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
    pub fn new(host: Arc<Host>, info: DocumentInfo, cx: &mut gpui::Context<Self>) -> Self {
        Self {
            host,
            doc_id: info.id,
            buf: Buffer::new(&info.text),
            focus: cx.focus_handle(),
            geometry: Cell::new(None),
            scroll: Cell::new(0.0),
            focused_once: Cell::new(false),
        }
    }

    /// Applies one user edit locally and pushes it to the backend document
    /// (which owns the dirty flag). A backend rejection resyncs from the
    /// snapshot so the two never diverge silently.
    fn edit(&mut self, change: TextChange, cx: &mut gpui::Context<Self>) {
        let version = self.buf.doc.version();
        let set = ChangeSet { changes: vec![change] };
        if let Err(e) = self.buf.doc.apply_detailed(version, &set) {
            eprintln!("[editor] local apply failed: {}", e.message);
            return;
        }
        if let Err(e) = self.host.edit(self.doc_id, version, set) {
            eprintln!("[editor] backend rejected the edit ({}); resyncing", e.message);
            self.resync();
        }
        self.clamp_scroll_to_caret();
        cx.notify();
    }

    /// A backend-originated patch (`DocPatched`: external reload, tooling
    /// edits). Reconciled **by version**: an exact base-version match applies
    /// incrementally with the caret clamped, anything else falls back to a
    /// full snapshot (M1 compared whole texts instead).
    pub fn backend_patched(&mut self, base_version: u32, changes: Vec<TextChange>, cx: &mut gpui::Context<Self>) {
        if self.buf.doc.version() == base_version {
            match self.buf.doc.apply_detailed(base_version, &ChangeSet { changes }) {
                Ok(_) => {
                    self.buf.marked = None;
                    self.buf.caret = self.buf.caret.min(self.buf.utf16_len());
                    self.clamp_scroll();
                }
                Err(_) => self.resync(),
            }
        } else {
            self.resync();
        }
        cx.notify();
    }

    /// Replaces the buffer from the backend's snapshot (out-of-sync recovery).
    fn resync(&mut self) {
        if let Ok(info) = self.host.snapshot(self.doc_id) {
            self.buf = Buffer::new(&info.text);
            self.scroll.set(0.0);
        }
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
            "backspace" => {
                if let Some(change) = self.buf.backspace() {
                    self.edit(change, cx);
                }
            }
            "delete" => {
                if let Some(change) = self.buf.delete_forward() {
                    self.edit(change, cx);
                }
            }
            "left" => self.buf.caret_left(),
            "right" => self.buf.caret_right(),
            "home" => self.buf.caret_home(),
            "end" => self.buf.caret_end(),
            "enter" => {
                let change = self.buf.insert_plain("\n");
                self.edit(change, cx);
            }
            "escape" => {}
            _ => {
                if let Some(text) = ks.key_char.as_ref() {
                    let change = self.buf.insert_plain(text);
                    self.edit(change, cx);
                }
            }
        }
        self.clamp_scroll_to_caret();
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(geometry) = self.geometry.get() else { return };
        // Gutter clicks are not text edits (fold chevrons arrive in phase 5).
        if ev.position.x < geometry.gutter_x + geometry.gutter_w {
            return;
        }
        let count = self.buf.line_count();
        let visible = ((geometry.height - px(theme::TEXT_INSET) * 2.0) / geometry.line_height) as i64;
        let row = (((ev.position.y - geometry.origin.y) / geometry.line_height) as i64 + self.scroll.get() as i64)
            .clamp(0, visible.max(1) - 1)
            .clamp(0, count as i64 - 1) as u32;
        let span = self.buf.line_span(row);
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
    fn clamp_scroll(&self) {
        let max = self.buf.line_count().saturating_sub(1) as f32;
        self.scroll.set(self.scroll.get().clamp(0.0, max));
    }

    /// Scrolls the caret's line into view after caret-moving edits.
    fn clamp_scroll_to_caret(&self) {
        let caret_line = self.buf.line_of(self.buf.caret) as f32;
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
    /// The laid-out rows in buffer order: (line index, shaped line). One row
    /// of buffer above and below the viewport, so slow scrolls never show a
    /// gap; everything outside is never shaped.
    rows: Vec<(u32, ShapedLine)>,
    /// Where the caret renders (its line and the gutter metrics for painting).
    caret: Bounds<Pixels>,
    caret_line: u32,
    gutter_w: Pixels,
    numbers_w: Pixels,
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
    let start = byte_at_utf16(line, relative.start);
    let end = byte_at_utf16(line, relative.end);
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

/// Window-space x of the caret at UTF-16 `index` inside `line`: one shape of
/// the whole line, then one walk to the boundary (no per-prefix reshaping).
fn x_at_utf16(window: &Window, line: &str, index: usize, font_size: Pixels) -> Pixels {
    let run = plain_run(line.len(), gpui::black());
    let shaped = window.text_system().shape_line(line.into(), font_size, &[run], None);
    let mut cursor = shaped.cursor();
    cursor.take_until(byte_at_utf16(line, index));
    cursor.x_offset()
}

/// Nearest caret (UTF-16 index inside `line`) for a window-space x: one shape,
/// then a linear walk snapping to the nearer boundary of each character.
fn utf16_index_at_x(window: &Window, line: &str, x: Pixels, font_size: Pixels) -> usize {
    if x <= px(0.0) {
        return 0;
    }
    let run = plain_run(line.len(), gpui::black());
    let shaped = window.text_system().shape_line(line.into(), font_size, &[run], None);
    let mut cursor = shaped.cursor();
    let mut units = 0usize;
    let mut prev_x = px(0.0);
    for (offset, ch) in line.char_indices() {
        cursor.take_until(offset + ch.len_utf8());
        let end_x = cursor.x_offset();
        if end_x >= x {
            let char_width = end_x - prev_x;
            return units + if x - prev_x > char_width / 2.0 { ch.len_utf16() } else { 0 };
        }
        prev_x = end_x;
        units += ch.len_utf16();
    }
    units
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
        Some("m2-editor".into())
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

        let marked = pane.buf.marked.clone();
        let caret = pane.buf.caret;
        let line_count = pane.buf.line_count();
        let scroll = pane.scroll.get();

        // Gutter geometry: the widest line number sets the width (at least
        // three digits, so small files do not look cramped).
        let digits = line_count.to_string().len().max(3);
        let numbers_w = shaped_width(window, &"8".repeat(digits), font_size);
        let gutter_w = px(theme::GUTTER_PAD_L) + numbers_w + px(theme::GUTTER_PAD_R);
        let gutter_x = bounds.origin.x;
        let origin = point(gutter_x + gutter_w, bounds.origin.y + px(theme::TEXT_INSET));

        // Only the visible rows are laid out, with one row of buffer above
        // and below; a 50 MB file costs the same as a small one.
        let first_visible = (scroll.floor().max(0.0) as u32).min(line_count.saturating_sub(1));
        let visible = ((bounds.size.height - px(theme::TEXT_INSET) * 2.0) / line_height).ceil() as u32;
        let first = first_visible.saturating_sub(1);
        let last = (first_visible + visible + 1).min(line_count);

        let fg = theme::fg();
        let rows = (first..last)
            .map(|line| {
                let span = pane.buf.line_span(line);
                let content = pane.buf.slice_utf16(span.clone());
                let runs = line_runs(&content, marked.clone(), span, fg);
                let shaped = window.text_system().shape_line(content.into(), font_size, &runs, None);
                (line, shaped)
            })
            .collect();

        // Caret rect: the line it renders on plus its prefix width.
        let caret_line = pane.buf.line_of(caret);
        let caret_span = pane.buf.line_span(caret_line);
        let caret_line_text = pane.buf.slice_utf16(caret_span.clone());
        let caret_x = x_at_utf16(window, &caret_line_text, caret - caret_span.start, font_size);
        let caret_y = origin.y + line_height * (caret_line as f32 - scroll);
        let caret = Bounds::new(point(origin.x + caret_x, caret_y), size(px(2.0), line_height));

        pane.geometry.set(Some(Geometry {
            gutter_x,
            gutter_w,
            origin,
            line_height,
            scroll,
            height: bounds.size.height,
        }));

        PrepaintState { rows, caret, caret_line, gutter_w, numbers_w }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(geometry) = self.view.read(cx).geometry.get() else { return };
        let font_size = theme::px_font_size();
        let line_height = geometry.line_height;
        let origin = geometry.origin;
        let gutter_x = geometry.gutter_x;

        // Clip to the element: long lines and mid-scroll rows must not paint
        // over the workbench chrome around the editor.
        window.paint_layer(bounds, |window| {
            // Gutter: right-aligned line numbers, the caret's row brighter,
            // separated from the text by the border color.
            for (row, _) in prepaint.rows.iter() {
                let number = (row + 1).to_string();
                let width = shaped_width(window, &number, font_size);
                let color = if *row == prepaint.caret_line { theme::fg() } else { theme::fg_muted() };
                let x = gutter_x + px(theme::GUTTER_PAD_L) + prepaint.numbers_w - width;
                let y = origin.y + line_height * (*row as f32 - geometry.scroll);
                let run = plain_run(number.len(), color);
                let shaped = window.text_system().shape_line(number.into(), font_size, &[run], None);
                let _ = shaped.paint(point(x, y), line_height, TextAlign::Left, None, window, cx);
            }
            let separator = Bounds::new(
                point(gutter_x + prepaint.gutter_w - px(1.0), bounds.origin.y),
                size(px(1.0), bounds.size.height),
            );
            window.paint_quad(fill(separator, theme::border()));

            for (row, shaped) in prepaint.rows.drain(..) {
                let y = origin.y + line_height * (row as f32 - geometry.scroll);
                let _ = shaped.paint(point(origin.x, y), line_height, TextAlign::Left, None, window, cx);
            }
            window.paint_quad(fill(prepaint.caret, rgba_of(theme::ACCENT)));
        });

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
/// tabbed workbench stays comparable against the M0/M1 reports.
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
        let clamped = pane.buf.clamp(range_utf16.start) as usize..pane.buf.clamp(range_utf16.end) as usize;
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
            let change = pane.buf.insert_commit(replacement_range, text);
            pane.edit(change, cx);
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
            let change = pane.buf.set_marked(range_utf16, new_text, new_selected_range);
            pane.edit(change, cx);
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
        let line = pane.buf.line_of(pane.buf.clamp(range_utf16.start) as usize);
        let span = pane.buf.line_span(line);
        let start = (pane.buf.clamp(range_utf16.start) as usize).max(span.start);
        let end = (pane.buf.clamp(range_utf16.end) as usize).min(span.end).max(start);
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
        let count = pane.buf.line_count();
        let visible = ((geometry.height - px(theme::TEXT_INSET) * 2.0) / geometry.line_height).max(1.0) as i64;
        let row = (((point.y - geometry.origin.y) / geometry.line_height) as i64 + geometry.scroll as i64)
            .clamp(0, visible - 1);
        if row < 0 || row as u32 >= count {
            return None;
        }
        let span = pane.buf.line_span(row as u32);
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
