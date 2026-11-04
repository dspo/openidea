//! The M0 spike's editor surface: a hand-drawn, single-line text element that
//! implements gpui's `InputHandler` (1:1 NSTextInputClient semantics) so the
//! macOS Pinyin IME can compose, anchor its candidate window, commit and
//! cancel against a canvas-style element — no stock input component involved.
//!
//! Buffer model: one `String` plus a UTF-16 caret and an optional UTF-16
//! marked range. Marked text lives inline in the buffer (that is what
//! NSTextInputClient means by "marked": it is in the document, just not
//! committed), which is what makes candidate-window geometry and commit
//! replacement fall out of ordinary range math.

use std::cell::Cell;
use std::ops::Range;

use gpui::{
    App, Bounds, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, InputHandler, InspectorElementId,
    InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, ParentElement, Pixels, Point,
    Render, ShapedLine, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, fill, point, px, relative,
    rgba, size,
};

// Palette lifted from the main app's src/styles/tokens.css: dark, low
// contrast greys, one cool accent. Hardcoded here on purpose — M0 deliberately
// does not carry the token/theme layer.
const BG_APP: u32 = 0x191a1c;
const BG_SURFACE: u32 = 0x212326;
const FG: u32 = 0xd1d3d9;
const FG_MUTED: u32 = 0x8c8f95;
const ACCENT: u32 = 0x3871e1;
/// Marked-text background: the accent at ~18% alpha over the surface.
const MARKED_BG: u32 = (ACCENT << 8) | 0x2e;

const FONT_FAMILY: &str = "Menlo";
const FONT_SIZE: f32 = 15.0;
const LINE_HEIGHT: f32 = 1.5;
const INFO_FONT_SIZE: f32 = 12.0;
/// Inset between the element bounds and the text line.
const TEXT_INSET: f32 = 24.0;

fn rgba_of(c: u32) -> gpui::Rgba {
    rgba((c << 8) | 0xff)
}

fn font() -> gpui::Font {
    gpui::Font { family: FONT_FAMILY.into(), ..Default::default() }
}

fn plain_run(len: usize, color: gpui::Hsla) -> TextRun {
    TextRun { len, font: font(), color, background_color: None, underline: None, strikethrough: None }
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

/// The single-line buffer with IME bookkeeping. All offsets are UTF-16.
struct Buffer {
    text: String,
    /// The in-flight composition, inline in `text`.
    marked: Option<Range<usize>>,
    caret: usize,
}

impl Buffer {
    fn new(seed: &str) -> Self {
        Self { text: seed.to_owned(), marked: None, caret: utf16_len(seed) }
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
}

/// Element geometry the IME handler needs between frames: where the text line
/// starts and how tall a line is, in window coordinates.
#[derive(Clone, Copy)]
struct Geometry {
    origin: Point<Pixels>,
    line_height: Pixels,
}

pub struct EditorPane {
    buf: Buffer,
    focus: FocusHandle,
    geometry: Cell<Option<Geometry>>,
    focused_once: Cell<bool>,
}

impl Focusable for EditorPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EditorPane {
    pub fn new(seed: &str, cx: &mut gpui::Context<Self>) -> Self {
        let mut buf = Buffer::new(seed);
        // Deterministic marked-state rendering for evidence screenshots:
        // injects through the same `set_marked` path the IME protocol drives
        // (the in-flight composition is too short-lived to screenshot
        // reliably). Clearly labeled synthetic — the real protocol trace is
        // in M0-REPORT.md.
        if std::env::var_os("OPENIDEA_M0_DEMO_MARKED").is_some() {
            buf.set_marked(None, "nihao", Some(5..5));
            eprintln!("[demo] synthetic marked text seeded via set_marked");
        }
        Self { buf, focus: cx.focus_handle(), geometry: Cell::new(None), focused_once: Cell::new(false) }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let ks = &ev.keystroke;
        let mods = &ks.modifiers;
        if mods.platform || mods.function {
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
            "home" => self.buf.caret = 0,
            "end" => self.buf.caret = self.buf.utf16_len(),
            "escape" => {}
            // Diagnostic: render a synthetic composition through the same
            // `set_marked` path the IME protocol uses (M0 can drive a real
            // Pinyin IME only by hand — see M0-REPORT.md).
            "m" if mods.control => {
                self.buf.set_marked(None, "zhongwen", Some(4..8));
                eprintln!("[key] synthetic marked text injected (ctrl+m)");
            }
            _ => {
                if let Some(text) = ks.key_char.as_ref()
                    && !mods.control
                    && !mods.alt
                {
                    self.buf.insert_plain(text);
                }
            }
        }
        cx.notify();
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(geometry) = self.geometry.get() {
            let x = ev.position.x - geometry.origin.x;
            self.buf.caret = utf16_index_at_x(window, &self.buf.text, x);
            cx.notify();
        }
    }

    /// One-line status readout for the screenshots and manual verification.
    fn status(&self) -> String {
        let marked = match &self.buf.marked {
            Some(range) => format!("{}..{}", range.start, range.end),
            None => "none".to_owned(),
        };
        format!("utf16len={} caret={} marked=[{}]", self.buf.utf16_len(), self.buf.caret, marked)
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
            .bg(rgba_of(BG_APP))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
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
    line: ShapedLine,
    caret: Bounds<Pixels>,
    status: ShapedLine,
    hint: ShapedLine,
}

/// The text runs for one frame: committed text plain, the marked range with a
/// distinct background and an accent underline.
fn build_runs(text: &str, marked: Option<Range<usize>>, fg: gpui::Hsla) -> Vec<TextRun> {
    let Some(range) = marked else {
        return vec![plain_run(text.len(), fg)];
    };
    let start = utf16_to_byte(text, range.start);
    let end = utf16_to_byte(text, range.end);
    let mut runs = Vec::with_capacity(3);
    if start > 0 {
        runs.push(plain_run(start, fg));
    }
    runs.push(TextRun {
        len: end - start,
        font: font(),
        color: fg,
        background_color: Some(rgba_of(MARKED_BG).into()),
        underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(rgba_of(ACCENT).into()), wavy: false }),
        strikethrough: None,
    });
    if end < text.len() {
        runs.push(plain_run(text.len() - end, fg));
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

/// Nearest caret (UTF-16 index) for a window-space x inside the text line.
fn utf16_index_at_x(window: &Window, text: &str, x: Pixels) -> usize {
    let mut units = 0usize;
    for (offset, ch) in text.char_indices() {
        let width = shaped_width(window, &text[..offset], px(FONT_SIZE));
        if width >= x {
            // Snap to the nearer boundary of the character under the mouse.
            let char_width = shaped_width(window, &text[..offset + ch.len_utf8()], px(FONT_SIZE)) - width;
            return if x - width > char_width / 2.0 { units + ch.len_utf16() } else { units };
        }
        units += ch.len_utf16();
    }
    utf16_len(text)
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
        Some("m0-editor".into())
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
        let font_size = px(FONT_SIZE);
        let line_height = px(FONT_SIZE * LINE_HEIGHT);
        let origin = point(bounds.origin.x + px(TEXT_INSET), bounds.origin.y + px(TEXT_INSET));

        let text = pane.buf.text.clone();
        let marked = pane.buf.marked.clone();
        let caret = pane.buf.caret;

        let fg = rgba_of(FG).into();
        let line = window.text_system().shape_line(
            text.clone().into(),
            font_size,
            &build_runs(&text, marked.clone(), fg),
            None,
        );

        let caret_x = shaped_width(window, &pane.buf.slice_utf16(0..caret), font_size);
        let caret = Bounds::new(point(origin.x + caret_x, origin.y), size(px(2.0), line_height));

        let info_font =
            gpui::Font { family: FONT_FAMILY.into(), weight: gpui::FontWeight::NORMAL, ..Default::default() };
        let muted = rgba_of(FG_MUTED).into();
        let status_run = TextRun {
            len: pane.status().len(),
            font: info_font.clone(),
            color: muted,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let status = window.text_system().shape_line(pane.status().into(), px(INFO_FONT_SIZE), &[status_run], None);
        let hint = "Pinyin: ni hao + Space commits | Esc cancels | ctrl+m injects a fake composition | Cmd+Q quits";
        let hint_run = TextRun {
            len: hint.len(),
            font: info_font,
            color: muted,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let hint = window.text_system().shape_line(hint.into(), px(INFO_FONT_SIZE), &[hint_run], None);

        PrepaintState { line, caret, status, hint }
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
        window.paint_quad(fill(bounds, rgba_of(BG_SURFACE)));

        let line_height = px(FONT_SIZE * LINE_HEIGHT);
        let origin = point(bounds.origin.x + px(TEXT_INSET), bounds.origin.y + px(TEXT_INSET));

        let _ = prepaint.line.paint(origin, line_height, TextAlign::Left, None, window, cx);
        window.paint_quad(fill(prepaint.caret, rgba_of(ACCENT)));

        // Info lines hug the bottom edge of the element.
        let info_line_height = px(INFO_FONT_SIZE * 1.4);
        let status_y = bounds.origin.y + bounds.size.height - px(TEXT_INSET) - info_line_height * 2.0;
        let _ = prepaint.status.paint(point(origin.x, status_y), info_line_height, TextAlign::Left, None, window, cx);
        let _ = prepaint.hint.paint(
            point(origin.x, status_y + info_line_height),
            info_line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );

        // Publish the line geometry for `bounds_for_range` between frames,
        // then register the IME handler for the upcoming frame.
        let pane = self.view.read(cx);
        pane.geometry.set(Some(Geometry { origin, line_height }));
        let focus = pane.focus.clone();
        window.handle_input(&focus, M0InputHandler { view: self.view.clone() }, cx);
    }
}

// ---------------------------------------------------------------------------
// M0InputHandler — the NSTextInputClient face of the element
// ---------------------------------------------------------------------------

/// gpui's input protocol for the editor element. Every mutation logs its
/// payload to stderr: that trace is the M0 evidence of the protocol round-trip.
struct M0InputHandler {
    view: Entity<EditorPane>,
}

impl InputHandler for M0InputHandler {
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

    /// Candidate-window anchor: the window-space rect of a UTF-16 range. The
    /// platform converts this to screen coordinates (`firstRectForCharacterRange`).
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let pane = self.view.read(cx);
        let geometry = pane.geometry.get()?;
        let start = pane.buf.clamp(range_utf16.start);
        let end = pane.buf.clamp(range_utf16.end).max(start);
        let x0 = shaped_width(window, &pane.buf.slice_utf16(0..start), px(FONT_SIZE));
        let x1 = shaped_width(window, &pane.buf.slice_utf16(0..end), px(FONT_SIZE));
        let bounds = Bounds::new(point(geometry.origin.x + x0, geometry.origin.y), size(x1 - x0, geometry.line_height));
        eprintln!("[ime] firstRect {range_utf16:?} -> {bounds:?}");
        Some(bounds)
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, window: &mut Window, cx: &mut App) -> Option<usize> {
        let pane = self.view.read(cx);
        let geometry = pane.geometry.get()?;
        let text = pane.buf.text.clone();
        Some(utf16_index_at_x(window, &text, point.x - geometry.origin.x))
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
