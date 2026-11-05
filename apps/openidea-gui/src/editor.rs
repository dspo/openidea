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

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::AppContext as _;
use gpui::{
    App, Bounds, ClickEvent, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, InputHandler,
    InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString,
    StatefulInteractiveElement as _, Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, fill, point,
    px, relative, rgba, size,
};
use gpui_component::button::ButtonVariants as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::ContextMenuExt as _;
use gpui_component::{IconName, Sizable as _};
use ide_core::{
    CancellationToken, ChangeSet, Diagnostic, DocId, IdeResult, Location, Pos, Severity, TextChange, WorkspaceEdit,
};
use ide_lsp::LspManager;
use ide_lsp::types::{CompletionItem, CompletionKind, CompletionList, GotoKind, Renamed};
use ide_workspace::DocumentInfo;
use tree_sitter::Tree;

use crate::actions as acts;
use crate::bus::{Bus, BusMessage};
use crate::highlight::{Highlighter, Kind, ParseResult, input_edit};
use crate::host::Host;
use crate::strings;
use crate::theme;

fn rgba_of(c: u32) -> gpui::Rgba {
    rgba((c << 8) | 0xff)
}

/// The find match cap (the count shows `10000+` past it).
const MAX_FIND_MATCHES: usize = 10_000;

/// Every occurrence of `needle` in `haystack` as UTF-16 ranges — literal,
/// ascending, non-overlapping, case-folded unless `case_sensitive` (each
/// char folds to its first lowercase form; the multi-char foldings of some
/// locales are out of scope for code search).
fn match_ranges(haystack: &str, needle: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let fold = |c: char| if case_sensitive { c } else { c.to_lowercase().next().unwrap_or(c) };
    let needle: Vec<char> = needle.chars().map(fold).collect();
    // (folded char, UTF-16 start, UTF-16 width) per haystack char.
    let mut chars = Vec::with_capacity(haystack.len());
    let mut utf16 = 0usize;
    for c in haystack.chars() {
        chars.push((fold(c), utf16, c.len_utf16()));
        utf16 += c.len_utf16();
    }
    let mut at = 0usize;
    'scan: while at + needle.len() <= chars.len() {
        for (offset, want) in needle.iter().enumerate() {
            if chars[at + offset].0 != *want {
                at += 1;
                continue 'scan;
            }
        }
        let last = &chars[at + needle.len() - 1];
        out.push(chars[at].1..last.1 + last.2);
        at += needle.len();
    }
    out
}

/// The document's indent policy: what Tab inserts and Enter keeps. Resolved
/// once at pane creation — `.editorconfig` wins over the language's
/// `languages/*.toml` `indent`, which wins over the default (4 spaces).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Indent {
    tab_size: u32,
    insert_spaces: bool,
}

impl Default for Indent {
    fn default() -> Self {
        Self { tab_size: 4, insert_spaces: true }
    }
}

impl Indent {
    /// The one-unit string Tab inserts (`"\t"` or spaces).
    fn unit(&self) -> String {
        if self.insert_spaces { " ".repeat(self.tab_size.max(1) as usize) } else { "\t".to_owned() }
    }

    /// Removes one unit from leading whitespace `s` (Shift-Tab's step).
    fn dedent(&self, s: &str) -> String {
        if !self.insert_spaces {
            return s.strip_prefix('\t').map_or_else(|| s.to_owned(), str::to_owned);
        }
        let spaces = s.len() - s.trim_start_matches(' ').len();
        let drop = spaces.min(self.tab_size.max(1) as usize);
        s[drop..].to_owned()
    }
}

/// The pane's indent policy for a document: editorconfig's
/// style/size/width over the language's `indent`, over the default.
fn indent_for(info: &DocumentInfo, languages: &ide_language::LanguageRegistry) -> Indent {
    let mut indent = Indent::default();
    if let Some(config) = languages.get(&info.language)
        && let Some(configured) = config.indent
    {
        indent = Indent { tab_size: configured.tab_size, insert_spaces: configured.insert_spaces };
    }
    if let Some(props) = &info.editorconfig {
        if let Some(style) = props.indent_style {
            indent.insert_spaces = style == ide_workspace::editorconfig::IndentStyle::Space;
        }
        if let Some(size) = props.indent_size {
            indent.tab_size = size.max(1);
        }
        if let Some(width) = props.tab_width {
            indent.tab_size = width.max(1);
        }
    }
    indent
}

/// The indentation Enter gives the new line: the current line's own leading
/// whitespace, plus one unit when its code (a trailing line comment stripped)
/// ends with `{` and the caret sat at end of line, minus one unit when the
/// text after the caret starts with `}` (the closing brace keeps its own
/// line's indent).
fn enter_indent(
    current_line: &str,
    caret_at_line_end: bool,
    rest: &str,
    line_comment: Option<&str>,
    indent: &Indent,
) -> String {
    let leading = current_line.len() - current_line.trim_start_matches([' ', '\t']).len();
    let mut out = current_line[..leading].to_owned();
    let mut code = current_line.trim_end();
    if let Some(comment) = line_comment
        && let Some((before, _)) = code.split_once(comment)
    {
        code = before.trim_end();
    }
    if caret_at_line_end && code.ends_with('{') {
        out.push_str(&indent.unit());
    }
    if rest.trim_start().starts_with('}') {
        out = indent.dedent(&out);
    }
    out
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
/// the backend document this mirrors). Carets live in the pane's selections.
struct Buffer {
    doc: ide_text::Document,
    /// The in-flight composition, inline in `doc`.
    marked: Option<Range<usize>>,
}

/// One cursor with optional selection. Offsets are UTF-16; `head` is the
/// caret (where editing happens) and may sit before `anchor` (`reversed`),
/// `goal_col` preserves the wanted column across vertical moves (None =
/// derive from the head).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection {
    anchor: usize,
    head: usize,
    reversed: bool,
    goal_col: Option<usize>,
}

impl Selection {
    /// A bare caret.
    fn caret(offset: usize) -> Self {
        Self { anchor: offset, head: offset, reversed: false, goal_col: None }
    }

    fn with_head(mut self, head: usize) -> Self {
        self.reversed = head < self.anchor;
        self.head = head;
        self
    }

    /// Sorted `(start, end)` of the selected range.
    fn normalized(&self) -> (usize, usize) {
        if self.reversed { (self.head, self.anchor) } else { (self.anchor, self.head) }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Cursors after a batch that replaced each range with its insert: one
    /// caret at the end of each inserted text, in post-batch coordinates.
    fn after_inserts(changes: &[TextChange]) -> Vec<Selection> {
        let mut out = Vec::with_capacity(changes.len());
        let mut delta: i64 = 0;
        for change in changes {
            let inserted = change.insert.chars().map(char::len_utf16).sum::<usize>();
            let head = (change.from as i64 + delta).max(0) as usize + inserted;
            out.push(Selection::caret(head));
            delta += inserted as i64 - (change.to - change.from) as i64;
        }
        out
    }
}

/// What a press started: a normal anchor drag, a ⌥ column selection, or a
/// scrollbar thumb drag (`grab` is the distance from the drag start to the
/// thumb's leading edge, so the thumb does not jump to the pointer).
#[derive(Clone, Copy)]
enum Drag {
    Normal { anchor: usize },
    Columnar { from: Point<Pixels> },
    ScrollV { grab: Pixels },
    ScrollH { grab: Pixels },
}

/// Motions the keymap maps onto. Word/line variants are chosen by modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineHome,
    LineEnd,
    DocStart,
    DocEnd,
}

/// The editor commands the action handlers ask for.
#[derive(Clone, Copy, Debug)]
enum Command {
    Move(Motion, bool),
    Delete { forward: bool, word: bool },
    Newline,
    Tab,
    Outdent,
    Paste,
    Cancel,
    ToggleHud,
}

/// Why an edit batch happened; decides undo grouping and caret mapping.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cause {
    /// Printable input across cursors (groups in the undo timeline).
    Typing,
    /// Backspace/Delete across cursors (groups like typing).
    Delete,
    /// An IME commit: undo removes the whole composition, not just the final
    /// text (the marked keystrokes were never undo entries).
    ImeCommit,
    /// A clipboard paste (⌘V): its own undo step, cursors land after the
    /// inserted text like typing.
    Paste,
    /// Tab/Shift-Tab line indentation: its own undo step, selections kept
    /// (indentation happens at line starts, the selection rides along).
    Indent,
    /// A document format (⌘⌥L): its own undo step (a whole-file rewrite must
    /// not merge into typing), selections kept (clamped).
    Format,
    /// A live-template expansion: its own undo step (Esc/undo removes the
    /// whole expansion); the session's stops manage the cursors.
    Template,
    /// Find & replace (⌘F): replace / replace-all batches, one undo step per
    /// invocation (a whole-file rewrite must not merge into typing),
    /// selections managed by the find flow.
    Replace,
    /// Undo/redo/IME marked-text application: no undo entry, selections are
    /// managed by the caller.
    Mechanical,
}

/// A folded buffer line range (`start_line` and `end_line` inclusive). The
/// start line keeps rendering; `start_line + 1..=end_line` collapse into one
/// placeholder row. Sorted, non-overlapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fold {
    start_line: u32,
    end_line: u32,
}

/// One applied edit batch with its inverse and the cursor states around it.
struct UndoEntry {
    /// Forward changes (ascending, non-overlapping, against `base_version`).
    changes: Vec<TextChange>,
    /// The same batch undone, in the coordinates *after* the batch.
    inverse: Vec<TextChange>,
    selections_before: Vec<Selection>,
    selections_after: Vec<Selection>,
}

/// Consecutive typing/deleting batches merge into one undo step.
struct UndoGroup {
    entries: Vec<UndoEntry>,
}

impl Buffer {
    fn new(text: &str) -> Self {
        Self { doc: ide_text::Document::new(text), marked: None }
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

    /// Char index of a UTF-16 offset (clamped, mid-surrogate rounds down).
    fn char_at(&self, utf16: usize) -> usize {
        self.doc.rope().utf16_cu_to_char(self.clamp(utf16) as usize)
    }

    fn utf16_of(&self, ch: usize) -> usize {
        self.doc.rope().char_to_utf16_cu(ch)
    }

    /// The UTF-16 offset of the previous char boundary (0 at the top).
    fn prev_boundary(&self, utf16: usize) -> usize {
        let ch = self.char_at(utf16);
        if ch > 0 { self.utf16_of(ch - 1) } else { 0 }
    }

    fn next_boundary(&self, utf16: usize) -> usize {
        let ch = self.char_at(utf16);
        let count = self.doc.rope().len_chars();
        if ch < count { self.utf16_of(ch + 1) } else { self.utf16_len() }
    }

    /// The char sitting at (i.e. starting at) a boundary.
    fn char_of(&self, utf16: usize) -> char {
        self.doc.rope().char(self.char_at(utf16))
    }

    /// Same as [`Buffer::char_of`], the class at a boundary.
    fn class_at(&self, utf16: usize) -> u8 {
        char_class(self.char_of(utf16))
    }

    /// The word-class run boundary left of `offset` (a whitespace run is
    /// crossed first, then one same-class run is consumed).
    fn word_start(&self, offset: usize) -> usize {
        let mut i = offset.min(self.utf16_len());
        while i > 0 && char_class(self.char_of(self.prev_boundary(i))) == 0 {
            i = self.prev_boundary(i);
        }
        if i == 0 {
            return 0;
        }
        let class = char_class(self.char_of(self.prev_boundary(i)));
        while i > 0 && char_class(self.char_of(self.prev_boundary(i))) == class {
            i = self.prev_boundary(i);
        }
        i
    }

    /// The word-class run boundary right of `offset`.
    fn word_end(&self, offset: usize) -> usize {
        let len = self.utf16_len();
        let mut i = offset.min(len);
        while i < len && self.class_at(i) == 0 {
            i = self.next_boundary(i);
        }
        if i >= len {
            return len;
        }
        let class = self.class_at(i);
        while i < len && self.class_at(i) == class {
            i = self.next_boundary(i);
        }
        i
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
}

/// Whitespace / word / other, the three classes word motions jump between.
fn char_class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

/// The diagnostic the pointer rests on: the pane parks it for the hover
/// tooltip (plain text in M3) until the pointer moves off it.
#[derive(Clone, PartialEq, Eq)]
struct HoverDiag {
    message: String,
    severity: Severity,
    /// The producer ("compiler", "syntax", an analyzer name) — what the
    /// tooltip's classification line shows.
    source: Option<String>,
    /// The buffer line the pointer was on (the tooltip anchors under it).
    line: u32,
    /// The diagnostic's own range: what the one-click fixes anchor at.
    range: ide_core::Range,
}

/// The one-click fixes of the hovered diagnostic (M6): requested once per
/// diagnostic, keyed by its line + message so a pointer wiggle re-requests
/// nothing.
struct HoverFixes {
    key: (u32, String),
    actions: Vec<ide_lsp::types::CodeAction>,
}

/// The LSP hover card parked under the pointer's line (M5 phase 4).
#[derive(Clone, PartialEq, Eq)]
struct HoverCard {
    line: u32,
    markdown: bool,
    contents: String,
}

/// An active template expansion: tab stops with buffer UTF-16 ranges, the
/// active index walking them. The active stop's occurrences are the pane's
/// selections while it is current — that is the linked editing (typing
/// replaces every occurrence through the ordinary multi-cursor path).
struct TemplateSession {
    stops: Vec<crate::templates::Stop>,
    active: usize,
}

/// The rename flow: an input overlay prefilled with the current name, then
/// the server's workspace edit shown for confirmation before it applies.
/// `confirmed` guards the Enter that is already in flight. The input view
/// unmounts once the request goes out, so [`EditorPane::command`] hands
/// focus back to the pane right away — a focus left on the unmounted input
/// would take the pane's handlers out of the dispatch chain (found the hard
/// way during the live pass).
enum RenameFlow {
    Input { input: gpui::Entity<InputState>, confirmed: bool },
    Preview { edit: WorkspaceEdit, name: String },
}

/// The open completion popup: the server's items for the request, the
/// buffer offset the typed word began at (the client-side filter and the
/// default replace base), and the selection into the filtered view.
struct CompletionPopup {
    items: Vec<CompletionItem>,
    replace_start: usize,
    selected: usize,
    is_incomplete: bool,
}

/// In-buffer find matches (M5 phase 6): the query with its case flag and the
/// UTF-16 ranges it hits, recomputed on every query change and after every
/// edit batch while present. Lives independently of the bar: a Find-in-Files
/// hit arrival paints matches with no bar open (⌘F adopts the query).
struct FindMatches {
    query: String,
    case_sensitive: bool,
    ranges: Vec<Range<usize>>,
    /// The match navigation points at (`ranges[current]` is the selection).
    current: usize,
    /// The cap stopped the scan (the count shows `10k+`).
    capped: bool,
}

/// The open find bar (⌘F): the modal query input (rename-input pattern — it
/// holds the focus while up), the replace row's input behind its toggle, and
/// the query input's event subscription. Enter/Shift-Enter walk the matches
/// through `PressEnter { shift }`; Esc closes and the pane takes back.
struct FindBar {
    query: Entity<InputState>,
    replace: Entity<InputState>,
    replace_visible: bool,
    _sub: gpui::Subscription,
}

/// One inlay hint rendered into a row (M6): its UTF-16 position in the line,
/// the padded label as it joins the row's text, its shaped width (what every
/// x past the position shifts by), and the kind (the label's color).
#[derive(Clone)]
struct RowHint {
    at: usize,
    label: String,
    width: f32,
    kind: ide_lsp::types::InlayKind,
}

impl CompletionPopup {
    /// The items matching the currently typed word (a case-insensitive
    /// subsequence of `filter_text`, falling back to `label`), server order
    /// kept (rust-analyzer pre-sorts by relevance). Snippet-format items
    /// participate (M6): gopls answers member access almost entirely as
    /// snippets and they expand through the template sessions.
    fn filtered(&self, filter: &str) -> Vec<&CompletionItem> {
        let lower = filter.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                let text = item.filter_text.as_ref().unwrap_or(&item.label).to_lowercase();
                let mut at = 0;
                for ch in lower.chars() {
                    match text[at..].find(ch) {
                        Some(found) => at += found + ch.len_utf16().max(1),
                        None => return false,
                    }
                }
                true
            })
            .collect()
    }

    /// The one-line letter badge for the completion kind.
    fn kind_badge(kind: Option<CompletionKind>) -> &'static str {
        use CompletionKind as K;
        match kind {
            Some(K::Method | K::Function | K::Constructor) => "f",
            Some(K::Variable | K::Constant) => "v",
            Some(K::Field | K::Property | K::EnumMember) => "m",
            Some(K::Class | K::Struct | K::Interface | K::Enum) => "s",
            Some(K::Module | K::File | K::Folder) => "M",
            Some(K::Keyword) => "k",
            Some(K::Snippet) => "S",
            Some(K::Unit | K::Value | K::Text | K::Color) => "t",
            Some(K::TypeParameter) => "T",
            Some(K::Reference) | Some(K::Event) | Some(K::Operator) => "r",
            None => "?",
        }
    }
}

/// One scrollbar's frame geometry, published between frames so the mouse
/// handlers can hit-test and drag the thumb.
#[derive(Clone, Copy)]
struct Scrollbar {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
}

impl Scrollbar {
    fn hit_thumb(&self, p: Point<Pixels>) -> bool {
        p.x >= self.thumb.origin.x
            && p.x <= self.thumb.origin.x + self.thumb.size.width
            && p.y >= self.thumb.origin.y
            && p.y <= self.thumb.origin.y + self.thumb.size.height
    }

    fn hit_track(&self, p: Point<Pixels>) -> bool {
        p.x >= self.track.origin.x
            && p.x <= self.track.origin.x + self.track.size.width
            && p.y >= self.track.origin.y
            && p.y <= self.track.origin.y + self.track.size.height
    }
}

/// Element geometry the IME handler needs between frames, in window
/// coordinates: where the gutter and the text area start, how tall a line is,
/// which line is scrolled to the top, the horizontal scroll offset, the text
/// area's size and the two scrollbars.
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
    /// Horizontal scroll offset in px (rows paint at `origin.x - scroll_x`).
    scroll_x: f32,
    height: Pixels,
    /// Width of the text area (element width minus the gutter).
    width: Pixels,
    vscroll: Scrollbar,
    hscroll: Scrollbar,
}

/// The F12 perf HUD's counters. Cells, so the element's prepaint/paint phases
/// (which hold only a shared view borrow) can publish what the HUD paints.
#[derive(Default)]
struct Hud {
    visible: Cell<bool>,
    /// Rolling frame starts, newest last (bounded).
    frames: RefCell<Vec<std::time::Instant>>,
    last_frame_ms: Cell<f32>,
    /// Instant of the last user input; diffed at the next prepaint.
    last_input: Cell<Option<std::time::Instant>>,
    input_latency_ms: Cell<Option<f32>>,
    rows_laid_out: Cell<u32>,
    spans_painted: Cell<u32>,
    spans_total: Cell<u32>,
    /// Rows carrying inlay hints this frame (M6).
    hints_rows: Cell<u32>,
    parse_started: Cell<Option<std::time::Instant>>,
    parse_ms: Cell<Option<f32>>,
    parse_version: Cell<u32>,
}

impl Hud {
    fn note_input(&self) {
        self.last_input.set(Some(std::time::Instant::now()));
    }

    /// FPS over the trailing ~half second, and the newest frame interval.
    fn frame(&self) -> (f32, f32) {
        let mut frames = self.frames.borrow_mut();
        let now = std::time::Instant::now();
        if let Some(last) = frames.last() {
            self.last_frame_ms.set((now - *last).as_secs_f32() * 1000.0);
        }
        frames.push(now);
        while frames.len() > 240 {
            frames.remove(0);
        }
        let fps = match (frames.first(), frames.last()) {
            (Some(first), Some(last)) if *last > *first => (frames.len() as f32 - 1.0) / (*last - *first).as_secs_f32(),
            _ => 0.0,
        };
        (fps, self.last_frame_ms.get())
    }
}

pub struct EditorPane {
    host: Arc<Host>,
    doc_id: DocId,
    /// The document's path (same-file jumps stay in the pane).
    path: std::path::PathBuf,
    buf: Buffer,
    focus: FocusHandle,
    geometry: Cell<Option<Geometry>>,
    /// Top visible line, in lines (clamped on use).
    scroll: Cell<f32>,
    /// Horizontal scroll offset in px (clamped on use; rows paint shifted).
    scroll_x: Cell<f32>,
    /// The widest line width observed so far (shaped rows report their width;
    /// exact for anything the viewport has shown, which is what the
    /// horizontal clamp and scrollbar need).
    max_line_w: Cell<f32>,
    /// The document's indent policy (Tab / Enter / Shift-Tab).
    indent: Indent,
    /// The language's line comment (Enter's `{`-detection strips it).
    line_comment: Option<String>,
    focused_once: Cell<bool>,
    /// Syntax highlighting (phase 3): the query, the incrementally parsed
    /// tree, and the whole-document spans the paint path slices per line.
    highlighter: Option<Arc<Highlighter>>,
    language: ide_core::LanguageId,
    large: bool,
    /// The backend refuses edits on read-only documents (large files are
    /// flagged read-only); the pane short-circuits instead of resync-storming.
    read_only: bool,
    tree: Option<Tree>,
    /// The tree-sitter spans (one source); the paint list `spans` is the
    /// semantic overlay merged over these (M5 phase 2).
    syntax_spans: Vec<(Range<usize>, Kind)>,
    /// The merged paint list (semantic over tree-sitter, sorted
    /// non-overlapping byte ranges).
    spans: Vec<(Range<usize>, Kind)>,
    /// The last semantic-token answer as byte ranges (empty without a
    /// capable server or before the first answer).
    semantic_spans: Vec<(Range<usize>, Kind)>,
    /// Semantic request generation (stale answers drop).
    semantic_gen: u32,
    /// Retry budget while the server starts; an explicit "no support"
    /// answer (Ok None) stops retrying.
    semantic_retries: u32,
    parse_in_flight: bool,
    /// An edit landed while a parse ran; reschedule when it lands.
    parse_dirty: bool,
    /// Edits that landed while the tree was handed to an in-flight parse.
    pending_edits: Vec<ide_text::ByteEdit>,
    /// Bumped when the buffer is replaced wholesale; landed parses from an
    /// older generation are dropped.
    parse_generation: u32,
    // Multi-cursor state (phase 4).
    selections: Vec<Selection>,
    drag: Cell<Option<Drag>>,
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
    /// When the last grouped edit happened and whether it was one.
    last_edit: Option<(std::time::Instant, bool)>,
    // Folding (phase 5): the syntax candidates plus the user's folded subset.
    fold_candidates: Vec<(u32, u32)>,
    folds: Vec<Fold>,
    // Diagnostics (M3 phase 3): the merged list for this file, sorted by
    // position, and what the pointer rests on.
    diagnostics: Vec<Diagnostic>,
    hover: Cell<Option<HoverDiag>>,
    // Completion (M3 phase 4): the open popup, the debounce/request
    // generation (stale answers are dropped), and the request's cancel token.
    completion: Option<CompletionPopup>,
    completion_gen: u32,
    completion_cancel: Option<CancellationToken>,
    // Navigation (M3 phase 5): a jump list of locations (definitions,
    // usages) and the rename flow, if one is under way.
    jump: Option<(Vec<Location>, usize)>,
    rename: Option<RenameFlow>,
    // Code actions (M5 phase 3): the open quick-fix menu and its selection.
    code_actions: Option<(Vec<ide_lsp::types::CodeAction>, usize)>,
    // Signature help (M5 phase 4): the open popup, caret-anchored like the
    // completion popup; ↑/↓ cycle the active signature.
    signature: Option<ide_lsp::types::SignatureHelp>,
    // The LSP hover card (M5 phase 4): the landed answer for the offset the
    // pointer rests on (markdown or plain text), plus request bookkeeping so
    // mouse moves schedule exactly one debounced request per token.
    hover_card: Option<HoverCard>,
    hover_req_gen: u32,
    hover_req_offset: Option<usize>,
    // The hovered diagnostic's one-click fixes (M6), when they landed.
    hover_fixes: Option<HoverFixes>,
    // Live templates (M5 phase 5): the active expansion session, if any.
    template: Option<TemplateSession>,
    // Find & replace (M5 phase 6): the in-buffer match list and the open
    // bar (the bar implies matches; matches may exist without the bar).
    find_hits: Option<FindMatches>,
    find_bar: Option<FindBar>,
    // Inlay hints (M6): the viewport's hints (sorted by position) and the
    // buffer-line range the current answer covers; edits invalidate, scroll
    // extends. `hints_by_line` is the last frame's render form — the prepaint
    // fills it, the x-mapping helpers and the mouse handlers read it.
    inlays: Vec<ide_lsp::types::InlayHint>,
    inlay_cover: Option<(u32, u32)>,
    inlay_gen: u32,
    inlays_on: bool,
    hints_by_line: RefCell<std::collections::HashMap<u32, Vec<RowHint>>>,
    bus: Bus,
    /// F12 perf overlay counters (phase 7).
    hud: Hud,
    /// When the pane was created; the first paint logs the open latency.
    created_at: std::time::Instant,
    first_paint_done: Cell<bool>,
}

impl Focusable for EditorPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EditorPane {
    pub fn new(host: Arc<Host>, bus: Bus, info: DocumentInfo, cx: &mut gpui::Context<Self>) -> Self {
        let language = info.language.clone();
        let indent = indent_for(&info, &host.languages);
        let line_comment = host.languages.get(&language).and_then(|config| config.line_comment);
        let mut pane = Self {
            host,
            doc_id: info.id,
            path: info.path.0.clone(),
            buf: Buffer::new(&info.text),
            focus: cx.focus_handle(),
            geometry: Cell::new(None),
            scroll: Cell::new(0.0),
            scroll_x: Cell::new(0.0),
            max_line_w: Cell::new(0.0),
            indent,
            line_comment,
            focused_once: Cell::new(false),
            highlighter: None,
            language,
            large: info.large,
            read_only: info.read_only,
            tree: None,
            syntax_spans: Vec::new(),
            spans: Vec::new(),
            semantic_spans: Vec::new(),
            semantic_gen: 0,
            semantic_retries: 0,
            parse_in_flight: false,
            parse_dirty: false,
            pending_edits: Vec::new(),
            parse_generation: 0,
            selections: vec![Selection::caret(utf16_len(&info.text))],
            drag: Cell::new(None),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            fold_candidates: Vec::new(),
            folds: Vec::new(),
            diagnostics: Vec::new(),
            hover: Cell::new(None),
            completion: None,
            completion_gen: 0,
            completion_cancel: None,
            jump: None,
            rename: None,
            code_actions: None,
            signature: None,
            hover_card: None,
            hover_req_gen: 0,
            hover_req_offset: None,
            hover_fixes: None,
            template: None,
            find_hits: None,
            find_bar: None,
            inlays: Vec::new(),
            inlay_cover: None,
            inlay_gen: 0,
            inlays_on: true,
            hints_by_line: RefCell::new(std::collections::HashMap::new()),
            bus,
            hud: Hud::default(),
            created_at: std::time::Instant::now(),
            first_paint_done: Cell::new(false),
        };
        pane.start_syntax(cx);
        pane.schedule_inlays(cx);
        pane
    }

    /// Starts syntax highlighting when the language has a bundled grammar and
    /// the file is not flagged large. Languages whose grammar has no bundled
    /// highlights query (Go at M5) skip the parse entirely — the semantic
    /// overlay is then the only paint source, requested on open.
    fn start_syntax(&mut self, cx: &mut gpui::Context<Self>) {
        if self.large {
            return;
        }
        if let Some(highlighter) = Highlighter::for_language(&self.language) {
            self.highlighter = Some(highlighter.clone());
            self.schedule_parse(highlighter, cx);
        } else {
            self.schedule_semantic(cx);
        }
    }

    // -- semantic tokens (M5 phase 2) ----------------------------------------
    //
    // One debounced request per settled parse; the answer overlays the
    // grammar spans. Edits bump the generation, so a stale answer drops, and
    // the in-flight byte ranges ride the edit deltas until it lands.

    /// Schedules the debounced semantic-token request (a settled parse or a
    /// landing overlay implies the text changed).
    fn schedule_semantic(&mut self, cx: &mut gpui::Context<Self>) {
        if self.large || self.semantic_retries > 5 {
            return;
        }
        self.semantic_gen += 1;
        let req = self.semantic_gen;
        let version = self.buf.doc.version();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        self.host.runtime.spawn(async move {
            // The debounce lives on the tokio side: a burst of keystrokes
            // collapses into the last generation.
            tokio::time::sleep(Duration::from_millis(400)).await;
            let result = lsp.semantic_tokens(doc).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| {
                    if pane.semantic_gen == req {
                        pane.semantic_ready(req, version, result, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Lands a semantic answer: only the current generation at the same
    /// buffer version is accepted; a missing capability stops retrying, a
    /// failed (starting/missing) server burns one retry round.
    fn semantic_ready(
        &mut self,
        req: u32,
        version: u32,
        result: IdeResult<Option<ide_lsp::types::SemanticTokens>>,
        cx: &mut gpui::Context<Self>,
    ) {
        if req != self.semantic_gen {
            return;
        }
        match result {
            Ok(Some(tokens)) if version == self.buf.doc.version() => {
                self.semantic_retries = 0;
                self.semantic_spans = crate::semantic::token_spans(&tokens, self.buf.doc.rope());
                self.merge_spans();
                eprintln!("[lsp] semantic tokens {} spans", self.semantic_spans.len());
                cx.notify();
            }
            // Server ready without semantic support: stop asking. A stale
            // version re-requests through the next parse instead.
            Ok(_) => self.semantic_retries = 5,
            Err(_) => {
                self.semantic_retries += 1;
                if self.semantic_retries <= 5 {
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let lsp = self.host.lsp.clone();
                    let doc = self.doc_id;
                    self.host.runtime.spawn(async move {
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        let result = lsp.semantic_tokens(doc).await;
                        let _ = tx.send(result);
                    });
                    let req = self.semantic_gen;
                    cx.spawn(async move |this, cx| {
                        if let Ok(result) = rx.await {
                            this.update(cx, |pane, cx| {
                                if pane.semantic_gen == req {
                                    pane.semantic_ready(req, version, result, cx);
                                }
                            })
                            .ok();
                        }
                    })
                    .detach();
                }
            }
        }
    }

    // -- inlay hints (M6 phase 3) ---------------------------------------------
    //
    // Viewport-driven `textDocument/inlayHint`: the request covers the visible
    // lines plus a viewport of slack, the answer renders inline (the label
    // joins the row's shaped text, pushing the rest of the line right), and
    // every x-mapping helper shifts by the widths of the hints it passes.

    /// Schedules the debounced hint request over the visible window widened by
    /// a viewport of slack. A range the current answer already covers is free
    /// (scrolling back costs nothing); edits drop the cover and re-request.
    fn schedule_inlays(&mut self, cx: &mut gpui::Context<Self>) {
        if self.large || self.read_only || !self.inlays_on {
            return;
        }
        let viewport =
            self.geometry.get().map_or(24.0, |g| (g.height - px(theme::TEXT_INSET) * 2.0) / g.line_height).ceil()
                as u32;
        let first_visible = self.scroll.get().floor().max(0.0) as u32;
        let slack = viewport.max(12);
        let first = first_visible.saturating_sub(slack);
        let last = (first_visible + viewport + slack).min(self.buf.line_count());
        if let Some((lo, hi)) = self.inlay_cover
            && lo <= first
            && last <= hi
        {
            eprintln!("[lsp] inlay request skipped (covered {}..{})", lo, hi);
            return;
        }
        self.inlay_gen += 1;
        let req = self.inlay_gen;
        let version = self.buf.doc.version();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let range = ide_core::Range::new(ide_core::Pos::new(first, 0), ide_core::Pos::new(last, 0));
        self.host.runtime.spawn(async move {
            // The debounce lives on the tokio side (keystroke bursts collapse).
            tokio::time::sleep(Duration::from_millis(250)).await;
            let result = lsp.inlay_hints(doc, range).await;
            let _ = tx.send((range, result));
        });
        cx.spawn(async move |this, cx| {
            if let Ok((range, result)) = rx.await {
                this.update(cx, |pane, cx| pane.inlays_ready(req, version, range, result, cx)).ok();
            }
        })
        .detach();
    }

    /// Lands a hint answer for the current generation: hints outside the
    /// buffer (a stale answer racing an edit) drop, and the cover remembers
    /// what this answer speaks for.
    fn inlays_ready(
        &mut self,
        req: u32,
        version: u32,
        range: ide_core::Range,
        result: IdeResult<Vec<ide_lsp::types::InlayHint>>,
        cx: &mut gpui::Context<Self>,
    ) {
        if req != self.inlay_gen {
            return;
        }
        match result {
            // Only an answer for the current text is accepted: the server
            // computed it against its own document state, which can lag the
            // pane's edits — a stale answer paints labels at wrong offsets.
            Ok(hints) if version == self.buf.doc.version() => {
                let count = self.buf.line_count();
                self.inlays = hints.into_iter().filter(|hint| hint.position.line < count).collect();
                self.inlay_cover = Some((range.start.line, range.end.line));
                eprintln!("[lsp] inlay hints {} for lines {}..{}", self.inlays.len(), range.start.line, range.end.line);
                cx.notify();
            }
            // The text moved on mid-request: ask again (the debounce has
            // settled by now, so this converges).
            Ok(_) => {
                self.inlay_cover = None;
                self.schedule_inlays(cx);
            }
            Err(e) => {
                eprintln!("[lsp] inlay hints: {}", e.message);
            }
        }
    }

    /// The row's hints in render form (padded label, shaped width), the
    /// buffer's hint list sliced to `line`. Called per visible row from the
    /// prepaint, which also publishes them into `hints_by_line` for the
    /// x-mapping helpers.
    fn row_hints(&self, line: u32, window: &Window, font_size: Pixels) -> Vec<RowHint> {
        let start = self.inlays.partition_point(|hint| hint.position.line < line);
        let end = start + self.inlays[start..].partition_point(|hint| hint.position.line == line);
        let line_len = self.buf.line_span(line).len();
        let mut hints = Vec::new();
        for hint in &self.inlays[start..end] {
            if hint.label.is_empty() {
                continue;
            }
            // The label joins the row text with the server's paddings, so the
            // shaped width is what every x past the position shifts by.
            let mut label = String::new();
            if hint.padding_left {
                label.push(' ');
            }
            label.push_str(&hint.label);
            if hint.padding_right {
                label.push(' ');
            }
            let at = (hint.position.col as usize).min(line_len);
            hints.push(RowHint {
                at,
                label: label.clone(),
                width: f32::from(shaped_width(window, &label, font_size)),
                kind: hint.kind,
            });
        }
        hints
    }

    /// Schedules an incremental re-parse on the background executor. The tree
    /// is handed to the parse as its incremental base; edits landing mid-parse
    /// are journalled in `pending_edits` and folded into the fresh tree when
    /// it lands.
    fn schedule_parse(&mut self, highlighter: Arc<Highlighter>, cx: &mut gpui::Context<Self>) {
        if self.parse_in_flight {
            self.parse_dirty = true;
            return;
        }
        self.parse_in_flight = true;
        self.hud.parse_started.set(Some(std::time::Instant::now()));
        let rope = self.buf.doc.rope().clone();
        let old_tree = self.tree.take();
        let version = self.buf.doc.version();
        let generation = self.parse_generation;
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move { highlighter.parse(&rope.to_string(), old_tree.as_ref(), version) })
                .await;
            let _ = this.update(cx, |pane, cx| pane.tree_swapped(parsed, version, generation, cx));
        })
        .detach();
    }

    /// Lands a finished parse: folds the journalled edits into the fresh tree
    /// (which may then be edited further), keeps its spans, and re-schedules
    /// when edits queued behind it. Results from an older generation (the
    /// buffer was replaced wholesale) are dropped.
    fn tree_swapped(
        &mut self,
        parsed: Option<ParseResult>,
        _parsed_for: u32,
        generation: u32,
        cx: &mut gpui::Context<Self>,
    ) {
        self.parse_in_flight = false;
        if let (Some(started), Some((_, _, _, version))) = (self.hud.parse_started.get(), parsed.as_ref()) {
            self.hud.parse_ms.set(Some(started.elapsed().as_secs_f32() * 1000.0));
            self.hud.parse_version.set(*version);
        }
        if generation == self.parse_generation
            && let Some((tree, spans, fold_candidates, _)) = parsed
            && self.tree.is_none()
        {
            let mut tree = tree;
            for edit in self.pending_edits.drain(..) {
                tree.edit(&input_edit(&edit));
            }
            self.tree = Some(tree);
            self.syntax_spans = spans;
            self.merge_spans();
            // The paint list changed: refresh the semantic overlay for the
            // new text (debounced, generation-guarded).
            self.schedule_semantic(cx);
            // Keep the user's folds that still exist as candidates; their
            // ends follow the (possibly edited) syntax.
            self.fold_candidates = fold_candidates;
            self.folds.retain_mut(|fold| {
                match self.fold_candidates.iter().find(|(start, _)| *start == fold.start_line) {
                    Some((_, end)) => {
                        fold.end_line = *end;
                        fold.end_line > fold.start_line
                    }
                    None => false,
                }
            });
        }
        if self.parse_dirty {
            self.parse_dirty = false;
            if let Some(highlighter) = self.highlighter.clone() {
                self.schedule_parse(highlighter, cx);
            }
        }
        cx.notify();
    }

    /// The merged paint list: semantic tokens over the grammar capture, so
    /// the server's precise kinds win where they exist.
    fn merge_spans(&mut self) {
        let len = self.buf.doc.rope().len_bytes();
        self.spans = crate::highlight::overlay(&self.syntax_spans, &self.semantic_spans, len);
    }

    /// Applies byte-coordinate edits to the syntax tree, shifts the folded
    /// ranges with the line deltas, and schedules an incremental re-parse.
    /// The semantic overlay rides the byte deltas too; spans the edit ran
    /// through drop until the next answer lands.
    fn after_edits(&mut self, byte_edits: &[ide_text::ByteEdit], cx: &mut gpui::Context<Self>) {
        for edit in byte_edits {
            let (start, old_end, new_end) = (edit.start.row as u32, edit.old_end.row as u32, edit.new_end.row as u32);
            let shift = new_end as i64 - old_end as i64;
            self.folds.retain_mut(|fold| {
                if fold.end_line < start {
                    true
                } else if fold.start_line > old_end {
                    fold.start_line = (fold.start_line as i64 + shift).max(0) as u32;
                    fold.end_line = (fold.end_line as i64 + shift).max(0) as u32;
                    fold.end_line > fold.start_line
                } else {
                    false // the edit ran through the folded range: unfold it
                }
            });
            let (edit_start, edit_old_end, edit_shift) = (edit.start_byte as i64, edit.old_end_byte as i64, shift);
            self.semantic_spans.retain_mut(|(range, _)| {
                let (range_start, range_end) = (range.start as i64, range.end as i64);
                if range_end <= edit_start {
                    true
                } else if range_start >= edit_old_end {
                    range.start = (range_start + edit_shift).max(0) as usize;
                    range.end = (range_end + edit_shift).max(0) as usize;
                    range.start < range.end
                } else {
                    false // the edit ran through the token: overlay refreshes
                }
            });
        }
        if !self.semantic_spans.is_empty() {
            self.merge_spans();
        }
        if let Some(tree) = &mut self.tree {
            for edit in byte_edits {
                tree.edit(&input_edit(edit));
            }
        } else if self.parse_in_flight {
            self.pending_edits.extend_from_slice(byte_edits);
        }
        if let Some(highlighter) = self.highlighter.clone() {
            self.schedule_parse(highlighter, cx);
        } else {
            // Semantic-only paint (no bundled query): the overlay refreshes
            // per settled edit round, exactly like the parse-driven path.
            self.schedule_semantic(cx);
        }
        // Hints speak for positions the edit moved: invalidate and re-request
        // (debounced on the runtime side like every edit round).
        self.inlay_cover = None;
        self.schedule_inlays(cx);
    }

    /// The one edit pipeline: apply ascending non-overlapping changes locally
    /// and to the backend, feed the syntax tree, record the undo entry (per
    /// `Cause`), and map the cursors through the batch.
    fn apply_edits(&mut self, changes: Vec<TextChange>, cause: Cause, cx: &mut gpui::Context<Self>) {
        if self.read_only {
            return;
        }
        let mut changes = changes;
        changes.sort_by_key(|c| (c.from, c.to));
        changes.retain(|c| c.from < c.to || !c.insert.is_empty());
        if changes.is_empty() {
            return;
        }
        let version = self.buf.doc.version();
        // The session re-maps its stops through this batch (a copy: the
        // original moves into the undo entry below).
        let batch = changes.clone();
        // The inverse needs the text the batch replaces, captured pre-apply.
        let originals: Vec<String> =
            changes.iter().map(|c| self.buf.slice_utf16(c.from as usize..c.to as usize)).collect();
        let set = ChangeSet { changes: changes.clone() };
        let byte_edits = match self.buf.doc.apply_detailed(version, &set) {
            Ok((_, byte_edits)) => byte_edits,
            Err(e) => {
                eprintln!("[editor] local apply failed: {}", e.message);
                return;
            }
        };
        if let Err(e) = self.host.edit(self.doc_id, version, set) {
            eprintln!("[editor] backend rejected the edit ({}); resyncing", e.message);
            self.resync(cx);
            return;
        }

        // Inverse in post-batch coordinates. An IME commit's undo removes the
        // committed text entirely (marked keystrokes were never entries), so
        // its inverse is a plain deletion.
        let inverse = inverse_changes(&changes, &originals, cause == Cause::ImeCommit);

        // Signature help: '(' or ',' requests at the primary caret, ')' or a
        // newline closes; other typing keeps it up (the server refreshes on
        // the next argument separator).
        if matches!(cause, Cause::Typing | Cause::ImeCommit) {
            let committed = changes.iter().rev().find(|c| !c.insert.is_empty()).map(|c| c.insert.clone());
            match committed.as_deref().and_then(|text| text.chars().next()) {
                Some('(' | ',') => self.request_signature_help(cx),
                Some(')' | '\n') => self.close_signature(cx),
                _ => {}
            }
        }
        let selections_before = std::mem::take(&mut self.selections);
        let selections_after = match cause {
            Cause::Typing | Cause::Delete | Cause::Paste => Selection::after_inserts(&changes),
            // Mechanical/indent/format/replace batches keep the caller's
            // cursors: they are restored (undo/redo), ride along (indentation
            // at line starts), stay put (format, replace — the find flow
            // re-selects what it wants) or clamp (IME, patches) afterwards.
            Cause::ImeCommit | Cause::Mechanical | Cause::Indent | Cause::Format | Cause::Template | Cause::Replace => {
                selections_before.clone()
            }
        };
        self.selections = normalize_selections(
            selections_after.into_iter().map(|s| clamp_selection(s, self.buf.utf16_len())).collect(),
        );
        if !matches!(cause, Cause::Mechanical) {
            self.push_undo(UndoEntry { changes, inverse, selections_before, selections_after: Vec::new() }, cause);
        }
        self.last_edit =
            Some((std::time::Instant::now(), matches!(cause, Cause::Typing | Cause::Delete | Cause::ImeCommit)));
        self.hover.set(None);
        // A menu parked over stale offsets is gone once the text moves.
        if matches!(cause, Cause::Typing | Cause::Delete | Cause::Paste | Cause::Template) {
            self.close_code_actions(cx);
        }
        // Text moved: hover cards and pending requests answer for old offsets.
        if !matches!(cause, Cause::Mechanical) {
            self.hover_card = None;
            self.hover_req_offset = None;
        }
        // The macOS IME commits printable input per character (Cause::
        // ImeCommit); plain keydowns (Cause::Typing) and deletions re-filter.
        if matches!(cause, Cause::Typing | Cause::Delete | Cause::ImeCommit) {
            self.schedule_completion(cx);
        }
        // A paste is not typing: any open popup's filter is stale now.
        if matches!(cause, Cause::Paste) {
            self.close_completion(cx);
        }
        self.after_edits(&byte_edits, cx);
        if self.template.is_some() {
            self.template_after_batch(&batch, cause);
        }
        // The find matches describe the text that just moved: recompute
        // (navigation stays near where it pointed).
        if self.find_hits.is_some() {
            self.find_recompute(cx);
        }
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    /// Records an entry, grouping consecutive typing/deleting batches. The
    /// `selections_after` are filled in once the batch's cursors are known.
    fn push_undo(&mut self, mut entry: UndoEntry, cause: Cause) {
        entry.selections_after = self.selections.clone();
        self.redo.clear();
        let grouped = matches!(cause, Cause::Typing | Cause::Delete)
            && self.last_edit.is_some_and(|(at, batch)| batch && at.elapsed() < std::time::Duration::from_millis(800));
        if grouped && let Some(group) = self.undo.last_mut() {
            group.entries.push(entry);
        } else {
            self.undo.push(UndoGroup { entries: vec![entry] });
        }
    }

    /// The buffer's full text (UTF-16-normalized `\n`), for callers outside
    /// the pane (the diff view reads a dirty buffer instead of the disk).
    pub fn buffer_text(&self) -> String {
        self.buf.slice_utf16(0..self.buf.utf16_len())
    }

    /// ⌘Z: inverts the newest undo group (newest entry first) and parks the
    /// inversions on the redo stack.
    pub fn undo(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(group) = self.undo.pop() else { return };
        for entry in group.entries.iter().rev() {
            self.apply_edits(entry.inverse.clone(), Cause::Mechanical, cx);
        }
        if let Some(first) = group.entries.first() {
            let len = self.buf.utf16_len();
            self.selections =
                normalize_selections(first.selections_before.iter().map(|s| clamp_selection(s.clone(), len)).collect());
        }
        self.redo.push(group);
        self.last_edit = None;
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    /// ⌘⇧Z: replays the newest redo group (oldest entry first).
    pub fn redo(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(group) = self.redo.pop() else { return };
        for entry in group.entries.iter() {
            self.apply_edits(entry.changes.clone(), Cause::Mechanical, cx);
        }
        if let Some(last) = group.entries.last() {
            let len = self.buf.utf16_len();
            self.selections =
                normalize_selections(last.selections_after.iter().map(|s| clamp_selection(s.clone(), len)).collect());
        }
        self.undo.push(group);
        self.last_edit = None;
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    /// The replacement ranges a text insertion covers: every selection's
    /// normalized range (a bare caret is an empty insert range), sorted and
    /// deduplicated — the shape `insert_text` and paste both consume.
    fn insert_text(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        let changes: Vec<TextChange> = paste_ranges(&self.selections)
            .into_iter()
            .map(|(start, end)| TextChange { from: start as u32, to: end as u32, insert: text.to_owned() })
            .collect();
        self.apply_edits(changes, Cause::Typing, cx);
    }

    /// Enter with auto-indent: the new line keeps the current line's leading
    /// whitespace, plus one indent unit when the caret sat at end of a line
    /// whose text ends with `{`, minus one when the text after the caret
    /// starts with `}`. One change per cursor, one typing batch.
    fn newline(&mut self, cx: &mut gpui::Context<Self>) {
        // Enter inside an expansion ends it (the stops would mis-map).
        self.template = None;
        let mut changes: Vec<TextChange> = Vec::new();
        for selection in &self.selections {
            let (start, end) = selection.normalized();
            let line = self.buf.line_of(start);
            let span = self.buf.line_span(line);
            let text = self.buf.slice_utf16(span.clone());
            let at_eol = end >= span.end;
            let rest = self.buf.slice_utf16(end..span.end);
            let indent = enter_indent(&text, at_eol, &rest, self.line_comment.as_deref(), &self.indent);
            // The cursor's own line (multi-cursor Enter on one line: each
            // cursor splits at its own offset).
            changes.push(TextChange { from: start as u32, to: end as u32, insert: format!("\n{indent}") });
        }
        self.apply_edits(changes, Cause::Typing, cx);
    }

    /// Tab: a live template session walks its stops first, then an abbrev at
    /// the caret expands; otherwise a non-empty selection indents its lines
    /// and bare carets insert one indent unit at each cursor.
    fn tab(&mut self, cx: &mut gpui::Context<Self>) {
        if self.advance_template(cx) {
            return;
        }
        if self.try_expand_template(cx) {
            return;
        }
        if self.selections.iter().any(|s| !s.is_empty()) {
            self.indent_lines(1, cx);
        } else {
            let unit = self.indent.unit();
            self.insert_text(&unit, cx);
        }
    }

    /// Indents (or with `-1` outdents) every buffer line a selection touches:
    /// one change per line in a single batch; selections ride along clamped.
    /// Its own undo step ([`Cause::Indent`]).
    fn indent_lines(&mut self, delta: i8, cx: &mut gpui::Context<Self>) {
        let unit = self.indent.unit();
        let mut changes: Vec<TextChange> = Vec::new();
        for selection in &self.selections {
            let (start, end) = selection.normalized();
            let first = self.buf.line_of(start);
            // A caret at column 0 does not count the line below the caret; a
            // selection's tail line is touched only when text is selected on it.
            let last = if end > start { self.buf.line_of(end - 1) } else { first };
            for line in first..=last {
                let from = self.buf.line_start(line) as u32;
                match delta {
                    1 => changes.push(TextChange { from, to: from, insert: unit.clone() }),
                    _ => {
                        let span = self.buf.line_span(line);
                        let text = self.buf.slice_utf16(span.clone());
                        let dedented = self.indent.dedent(&text);
                        if dedented.len() < text.len() {
                            changes.push(TextChange {
                                from,
                                to: from + (text.len() - dedented.len()) as u32,
                                insert: String::new(),
                            });
                        }
                    }
                }
            }
        }
        if changes.is_empty() {
            return;
        }
        changes.sort_by_key(|c| (c.from, c.to));
        changes.dedup_by_key(|c| (c.from, c.to));
        self.apply_edits(changes, Cause::Indent, cx);
    }

    /// ⌘V: pastes `text` at every cursor (replacing each selection) as one
    /// batch — its own undo step, and no completion follow-up.
    fn paste(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        let changes: Vec<TextChange> = paste_ranges(&self.selections)
            .into_iter()
            .map(|(start, end)| TextChange { from: start as u32, to: end as u32, insert: text.to_owned() })
            .collect();
        self.apply_edits(changes, Cause::Paste, cx);
    }

    /// Backspace / delete across cursors: a non-empty selection loses its
    /// range, an empty caret loses one char boundary (⌥: one word).
    fn delete(&mut self, forward: bool, word: bool, cx: &mut gpui::Context<Self>) {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for selection in &self.selections {
            let (start, end) = selection.normalized();
            if start < end {
                ranges.push((start, end));
                continue;
            }
            if word {
                let target = if forward { self.buf.word_end(start) } else { self.buf.word_start(start) };
                ranges.push((start.min(target), start.max(target)));
            } else if forward {
                let next = self.buf.next_boundary(start);
                if next > start {
                    ranges.push((start, next));
                }
            } else {
                let prev = self.buf.prev_boundary(start);
                if prev < start {
                    ranges.push((prev, start));
                }
            }
        }
        ranges.sort_unstable();
        ranges.dedup();
        let changes: Vec<TextChange> = ranges
            .into_iter()
            .map(|(s, e)| TextChange { from: s as u32, to: e as u32, insert: String::new() })
            .collect();
        self.apply_edits(changes, Cause::Delete, cx);
    }

    /// Moves every cursor (`extend` keeps the anchors). Vertical moves keep
    /// each cursor's goal column; other motions drop it.
    fn move_cursors(&mut self, motion: Motion, extend: bool, cx: &mut gpui::Context<Self>) {
        self.close_code_actions(cx);
        self.close_signature(cx);
        let vertical = matches!(motion, Motion::Up | Motion::Down);
        let targets: Vec<usize> = self.selections.iter().map(|s| self.motion_target(motion, s)).collect();
        for (index, selection) in self.selections.iter_mut().enumerate() {
            let head = selection.head;
            let target = targets[index];
            if vertical {
                if selection.goal_col.is_none() {
                    selection.goal_col = Some(head - self.buf.line_start(self.buf.line_of(head)));
                }
            } else {
                selection.goal_col = None;
            }
            // Collapsing motions on a non-empty selection move to its edge
            // before stepping (Left → start, Right → end), like native views.
            if !extend && selection.is_empty() {
                selection.anchor = target;
                selection.head = target;
                selection.reversed = false;
            } else if !extend {
                *selection = Selection::caret(target);
            } else {
                *selection = selection.clone().with_head(target);
            }
        }
        let len = self.buf.utf16_len();
        self.selections =
            normalize_selections(self.selections.iter().map(|s| clamp_selection(s.clone(), len)).collect());
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    /// The new head for `motion` from a selection's current state.
    fn motion_target(&self, motion: Motion, selection: &Selection) -> usize {
        let head = selection.head;
        match motion {
            Motion::Left => {
                if selection.is_empty() {
                    self.buf.prev_boundary(head)
                } else {
                    selection.normalized().0
                }
            }
            Motion::Right => {
                if selection.is_empty() {
                    self.buf.next_boundary(head)
                } else {
                    selection.normalized().1
                }
            }
            Motion::Up | Motion::Down => {
                let line = self.buf.line_of(head) as i64;
                let target = if motion == Motion::Up { line - 1 } else { line + 1 };
                if target < 0 || target >= self.buf.line_count() as i64 {
                    return head;
                }
                let target = target as u32;
                let span = self.buf.line_span(target);
                let col = selection.goal_col.unwrap_or(head - self.buf.line_start(self.buf.line_of(head)));
                span.start + col.min(span.len())
            }
            Motion::WordLeft => self.buf.word_start(head),
            Motion::WordRight => self.buf.word_end(head),
            Motion::LineHome => {
                let span = self.buf.line_span(self.buf.line_of(head));
                let text = self.buf.slice_utf16(span.clone());
                let indent = text.len() - text.trim_start_matches([' ', '\t']).len();
                let first = span.start + utf16_len(&text[..indent]);
                if head == first { span.start } else { first }
            }
            Motion::LineEnd => self.buf.line_span(self.buf.line_of(head)).end,
            Motion::DocStart => 0,
            Motion::DocEnd => self.buf.utf16_len(),
        }
    }

    /// Escape: collapse everything to the primary caret.
    fn collapse_cursors(&mut self, cx: &mut gpui::Context<Self>) {
        if self.selections.len() > 1 {
            let primary = self.selections[0].clone();
            self.selections = vec![Selection::caret(primary.head)];
            cx.notify();
        }
    }

    /// A backend-originated patch (`DocPatched`: external reload, tooling
    /// edits). Reconciled **by version**: an exact base-version match applies
    /// incrementally with the cursors clamped, anything else falls back to a
    /// full snapshot (M1 compared whole texts instead).
    pub fn backend_patched(&mut self, base_version: u32, changes: Vec<TextChange>, cx: &mut gpui::Context<Self>) {
        if self.buf.doc.version() == base_version {
            match self.buf.doc.apply_detailed(base_version, &ChangeSet { changes }) {
                Ok((_, byte_edits)) => {
                    self.buf.marked = None;
                    let len = self.buf.utf16_len();
                    self.selections =
                        normalize_selections(self.selections.iter().map(|s| clamp_selection(s.clone(), len)).collect());
                    self.clamp_scroll();
                    self.after_edits(&byte_edits, cx);
                }
                Err(_) => self.resync(cx),
            }
        } else {
            self.resync(cx);
        }
        cx.notify();
    }

    /// Replaces the buffer from the backend's snapshot (out-of-sync recovery).
    fn resync(&mut self, cx: &mut gpui::Context<Self>) {
        if let Ok(info) = self.host.snapshot(self.doc_id) {
            self.buf = Buffer::new(&info.text);
            self.selections = vec![Selection::caret(self.buf.utf16_len())];
            self.undo.clear();
            self.redo.clear();
            self.scroll.set(0.0);
            self.scroll_x.set(0.0);
            self.max_line_w.set(0.0);
            // The old tree and its queued edits describe text that no longer
            // exists anywhere; the in-flight parse (if any) is invalidated.
            self.parse_generation += 1;
            self.tree = None;
            self.syntax_spans.clear();
            self.spans.clear();
            self.semantic_spans.clear();
            self.semantic_gen += 1;
            self.pending_edits.clear();
            self.template = None;
            self.fold_candidates.clear();
            self.folds.clear();
            // Hints answer for the replaced text's positions (a server patch
            // landing while another was in flight resyncs here): drop them or
            // the labels paint into the new text at old columns.
            self.inlays.clear();
            self.inlay_cover = None;
            self.hints_by_line.borrow_mut().clear();
            self.schedule_inlays(cx);
            if let Some(highlighter) = self.highlighter.clone() {
                self.schedule_parse(highlighter, cx);
            }
            // The find matches describe the replaced text: recompute.
            if self.find_hits.is_some() {
                self.find_recompute(cx);
            }
        }
    }

    // -- navigation & rename (M3 phase 5) --------------------------------------
    //
    // ⌘B / ⌘-click ask the server for definitions, ⌥F7 for project-wide
    // references: one location jumps directly, several open the jump list,
    // none surfaces a status message. ⇧F6 runs prepareRename → a small input
    // overlay → textDocument/rename → the workspace edit preview with
    // Apply/Cancel (apply goes through Documents, so open tabs receive the
    // patch as DocPatched { workspace_edit }).

    /// The action face: every request follows the same spawn → oneshot →
    /// entity-update shape as completion.
    pub fn goto_definition(&mut self, cx: &mut gpui::Context<Self>) {
        self.request_locations(
            "definition",
            |lsp, doc, pos, _cancel| Box::pin(async move { lsp.goto(doc, pos, GotoKind::Definition).await }),
            cx,
        );
    }

    pub fn find_usages(&mut self, cx: &mut gpui::Context<Self>) {
        self.request_locations(
            "references",
            |lsp, doc, pos, cancel| {
                Box::pin(async move { lsp.references(doc, pos, true, true, cancel).await.map(|r| r.locations) })
            },
            cx,
        );
    }

    fn request_locations(
        &mut self,
        feature: &'static str,
        call: impl Fn(
            Arc<LspManager>,
            DocId,
            Pos,
            CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = IdeResult<Vec<Location>>> + Send>>
        + Send
        + 'static,
        cx: &mut gpui::Context<Self>,
    ) {
        self.jump = None;
        self.close_completion(cx);
        let cancel = CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let pos = self.caret_pos();
        self.host.runtime.spawn(async move {
            let result = call(lsp, doc, pos, cancel).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.locations_ready(feature, result, cx)).ok();
            }
        })
        .detach();
    }

    /// Lands a locations answer: jump straight away when there is one,
    /// otherwise open the jump list, otherwise say nothing was found.
    fn locations_ready(
        &mut self,
        feature: &'static str,
        result: IdeResult<Vec<Location>>,
        cx: &mut gpui::Context<Self>,
    ) {
        match result {
            Ok(locations) if locations.len() == 1 => {
                if let Some(only) = locations.into_iter().next() {
                    self.jump_to(&only, cx);
                }
            }
            Ok(locations) if !locations.is_empty() => {
                self.jump = Some((locations, 0));
                cx.notify();
            }
            _ => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Warning,
                    title: format!("No {feature} found"),
                    body: None,
                });
            }
        }
    }

    /// Jumps to a location: within this file the caret moves; across files
    /// the workbench opens (or activates) the tab and places the caret.
    fn jump_to(&mut self, location: &Location, cx: &mut gpui::Context<Self>) {
        let path: std::path::PathBuf = location.path.0.clone();
        self.jump = None;
        let start = location.range.start;
        if path == self.path {
            self.jump_to_offset(Pos::new(start.line, start.col), cx);
        } else {
            self.bus.send(BusMessage::OpenLocation { path, pos: Pos::new(start.line, start.col) });
        }
        cx.notify();
    }

    /// Places the primary caret at an IDE position and scrolls it into view.
    pub fn jump_to_offset(&mut self, pos: Pos, cx: &mut gpui::Context<Self>) {
        let offset = (self.buf.line_start(pos.line) + pos.col as usize).min(self.buf.utf16_len());
        self.selections = vec![Selection::caret(offset)];
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    /// ⇧F6: the identifier around the caret opens the input overlay
    /// immediately (locally computed range — what prepareRename returns for
    /// well-formed sources anyway; its server round trip is an M4 refinement).
    pub fn start_rename(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.close_completion(cx);
        self.jump = None;
        if self.rename.is_some() {
            return;
        }
        let caret = self.selections.first().map_or(0, |s| s.head);
        let mut start = caret.min(self.buf.utf16_len());
        while start > 0 {
            let prev = self.buf.prev_boundary(start);
            if !(self.buf.char_of(prev).is_alphanumeric() || self.buf.char_of(prev) == '_') {
                break;
            }
            start = prev;
        }
        let mut end = caret;
        let len = self.buf.utf16_len();
        while end < len {
            let next = self.buf.next_boundary(end);
            if next == end || !(self.buf.char_of(next).is_alphanumeric() || self.buf.char_of(next) == '_') {
                break;
            }
            end = next;
        }
        if start == end {
            self.host.sink.emit(ide_core::IdeEvent::Notify {
                level: ide_core::event::NotifyLevel::Warning,
                title: "Place the caret on an identifier to rename".into(),
                body: None,
            });
            return;
        }
        let current = self.buf.slice_utf16(start..end);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("new name"));
        input.update(cx, |state, cx| state.set_value(current.clone(), window, cx));
        self.rename = Some(RenameFlow::Input { input, confirmed: false });
        cx.notify();
    }

    /// Enter on the input: asks the server for the workspace edit and shows
    /// the preview (Apply / Cancel). Guarded against double-Enter while the
    /// request is in flight.
    fn rename_confirm(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(RenameFlow::Input { input, confirmed }) = &mut self.rename else { return };
        if *confirmed {
            return;
        }
        *confirmed = true;
        let new_name = input.read(cx).value().to_string();
        if new_name.is_empty() {
            return;
        }
        let caret = self.selections.first().map_or(0, |s| s.head);
        let pos = {
            let line = self.buf.line_of(caret);
            Pos::new(line, (caret - self.buf.line_start(line)) as u32)
        };
        let cancel = CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        self.host.runtime.spawn(async move {
            let result = lsp.rename(doc, pos, new_name.clone(), cancel).await;
            let _ = tx.send((result, new_name));
        });
        cx.spawn(async move |this, cx| {
            if let Ok((result, name)) = rx.await {
                this.update(cx, |pane, cx| pane.rename_preview(result, name, cx)).ok();
            }
        })
        .detach();
    }

    /// Lands the rename answer and switches to the preview stage.
    fn rename_preview(&mut self, result: IdeResult<Renamed>, name: String, cx: &mut gpui::Context<Self>) {
        match result {
            Ok(renamed) if !renamed.edit.files.is_empty() => {
                self.rename = Some(RenameFlow::Preview { edit: renamed.edit, name });
            }
            Ok(_) => {
                self.rename = None;
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Warning,
                    title: "Rename produced no edits".into(),
                    body: None,
                });
            }
            Err(e) => {
                self.rename = None;
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Error,
                    title: format!("Rename failed: {}", e.message),
                    body: None,
                });
            }
        }
        cx.notify();
    }

    /// Apply on the preview: Documents lands the edit atomically, and the
    /// pane takes focus back from the (now dropped) input.
    fn rename_apply(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(RenameFlow::Preview { edit, .. }) = &self.rename {
            let result = self.host.apply_workspace_edit(edit);
            self.rename = None;
            window.focus(&self.focus, cx);
            if let Err(e) = result {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Error,
                    title: format!("Rename failed: {}", e.message),
                    body: None,
                });
            }
        }
        cx.notify();
    }

    fn rename_cancel_flow(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.rename = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    // -- code actions & format (M5 phase 3) ----------------------------------
    //
    // ⌥⏎ asks the server for quick fixes at the caret and opens a menu; the
    // chosen action applies its workspace edit through Documents (the M3
    // rename path), resolving first when the server deferred it, or executes
    // the command for command-only actions. ⌘⌥L formats through the server
    // and applies the returned edits as one undoable batch.

    /// The caret as an empty LSP range (what quick-fix requests want).
    fn caret_range(&self) -> ide_core::Range {
        let pos = self.caret_pos();
        ide_core::Range::point(pos)
    }

    pub fn request_code_actions(&mut self, cx: &mut gpui::Context<Self>) {
        self.close_code_actions(cx);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        // A diagnostic on the caret's line anchors the request (the lightbulb
        // shape): the server's answer keys its quickfixes on the diagnostics
        // that overlap the requested range, and a bare caret at the line end
        // overlaps nothing.
        let line = self.buf.line_of(self.selections.first().map_or(0, |s| s.head));
        let range = self
            .diagnostics
            .iter()
            .find(|d| d.range.start.line == line || d.range.end.line == line)
            .map(|d| d.range)
            .unwrap_or_else(|| self.caret_range());
        self.host.runtime.spawn(async move {
            let result = lsp.code_actions(doc, range, Vec::new()).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.code_actions_ready(result, cx)).ok();
            }
        })
        .detach();
    }

    fn code_actions_ready(&mut self, result: IdeResult<Vec<ide_lsp::types::CodeAction>>, cx: &mut gpui::Context<Self>) {
        match result {
            Ok(mut actions) if !actions.is_empty() => {
                // The server's disabled suggestions cannot apply — drop them.
                actions.retain(|action| action.disabled_reason.is_none());
                // Fixes before intentions (the IDEA menu shape): the server's
                // preferred pick first, then quickfixes, refactors, and the
                // source.* tools at the bottom.
                let rank = |action: &ide_lsp::types::CodeAction| match action.kind.as_deref() {
                    Some("quickfix") => 1,
                    Some(k) if k.starts_with("refactor") => 2,
                    Some(k) if k.starts_with("source") => 3,
                    _ => 4,
                };
                actions.sort_by_key(|action| (!action.is_preferred, rank(action)));
                if actions.is_empty() {
                    self.host.sink.emit(ide_core::IdeEvent::Notify {
                        level: ide_core::event::NotifyLevel::Info,
                        title: strings::NO_CODE_ACTIONS.into(),
                        body: None,
                    });
                    cx.notify();
                    return;
                }
                eprintln!("[lsp] code actions {}", actions.len());
                self.code_actions = Some((actions, 0));
            }
            Ok(_) => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Info,
                    title: strings::NO_CODE_ACTIONS.into(),
                    body: None,
                });
            }
            Err(e) => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Warning,
                    title: format!("{}: {}", strings::CODE_ACTIONS_UNAVAILABLE, e.message),
                    body: None,
                });
            }
        }
        cx.notify();
    }

    /// Enter on the menu: edit-bearing actions apply right away; the rest
    /// resolve first (the edit may arrive), then either apply or execute.
    fn apply_code_action(&mut self, cx: &mut gpui::Context<Self>) {
        let Some((actions, selected)) = self.code_actions.take() else { return };
        let Some(action) = actions.into_iter().nth(selected) else { return };
        cx.notify();
        if action.edit.is_some() {
            self.finish_code_action(action, cx);
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        self.host.runtime.spawn(async move {
            let resolved = lsp.resolve_code_action(doc, action).await;
            let _ = tx.send(resolved);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(resolved)) = rx.await {
                this.update(cx, |pane, cx| pane.finish_code_action(resolved, cx)).ok();
            }
        })
        .detach();
    }

    /// Applies a resolved action: its edit (if any) through Documents, then
    /// any leftover command (`workspace/executeCommand`; the server's edit
    /// answers ride the apply-edit hook installed at open).
    fn finish_code_action(&mut self, action: ide_lsp::types::CodeAction, cx: &mut gpui::Context<Self>) {
        if let Some(edit) = &action.edit
            && let Err(e) = self.host.apply_workspace_edit(edit)
        {
            self.host.sink.emit(ide_core::IdeEvent::Notify {
                level: ide_core::event::NotifyLevel::Error,
                title: format!("{}: {}", strings::CODE_ACTION_FAILED, e.message),
                body: None,
            });
        }
        let Some(command) = action.command else { return };
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        eprintln!("[lsp] execute command {}", command.command);
        self.host.runtime.spawn(async move {
            let _ = lsp.execute_command(doc, command).await;
        });
        cx.notify();
    }

    fn close_code_actions(&mut self, cx: &mut gpui::Context<Self>) {
        if self.code_actions.take().is_some() {
            cx.notify();
        }
    }

    fn code_actions_move(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        if let Some((actions, selected)) = &mut self.code_actions {
            let count = actions.len().max(1) as isize;
            *selected = ((*selected as isize + delta).rem_euclid(count)) as usize;
            cx.notify();
        }
    }

    /// ⌘⌥L: the server's formatting edits as one batch (Cause::Format keeps
    /// the selection; undo reverts the whole rewrite).
    pub fn format_document(&mut self, cx: &mut gpui::Context<Self>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let options = ide_lsp::types::FormatOptions {
            tab_size: self.indent.tab_size,
            insert_spaces: self.indent.insert_spaces,
            on_save: false,
            tool: None,
        };
        self.host.runtime.spawn(async move {
            let result = lsp.format(doc, None, options).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.format_ready(result, cx)).ok();
            }
        })
        .detach();
    }

    /// The whole document as an LSP range (source actions want everything).
    fn document_range(&self) -> ide_core::Range {
        let last = self.buf.line_count().saturating_sub(1);
        ide_core::Range::new(ide_core::Pos::new(0, 0), ide_core::Pos::new(last, 0))
    }

    /// ⌃⌥O: the server's `source.organizeImports` action applied directly —
    /// no menu; unused imports dropped and the import block sorted are gopls's
    /// own behavior. Edit-bearing after resolve; the applied workspace edit
    /// lands through Documents like every rename/quick-fix (⌘S runs the same
    /// shape on the workbench side before writing the file).
    pub fn organize_imports(&mut self, cx: &mut gpui::Context<Self>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let range = self.document_range();
        self.host.runtime.spawn(async move {
            let result = organize_imports_action(&lsp, doc, range).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.organize_ready(result, cx)).ok();
            }
        })
        .detach();
    }

    fn organize_ready(&mut self, result: IdeResult<Option<ide_lsp::types::CodeAction>>, cx: &mut gpui::Context<Self>) {
        match result {
            Ok(Some(action)) => self.finish_code_action(action, cx),
            Ok(None) => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Info,
                    title: strings::ORGANIZE_NO_ACTIONS.into(),
                    body: None,
                });
            }
            Err(e) => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Warning,
                    title: format!("{}: {}", strings::CODE_ACTIONS_UNAVAILABLE, e.message),
                    body: None,
                });
            }
        }
        cx.notify();
    }

    /// ⌥⌘I: hints default on (the Go DX state); off clears the painted set
    /// and stops asking. On again re-requests the viewport.
    fn toggle_inlays(&mut self, cx: &mut gpui::Context<Self>) {
        self.inlays_on = !self.inlays_on;
        if self.inlays_on {
            self.inlay_cover = None;
            self.schedule_inlays(cx);
        } else {
            self.inlays.clear();
            self.inlay_cover = None;
            self.hints_by_line.borrow_mut().clear();
        }
        cx.notify();
    }

    fn format_ready(&mut self, result: IdeResult<Vec<ide_core::TextEdit>>, cx: &mut gpui::Context<Self>) {
        match result {
            Ok(edits) if !edits.is_empty() => {
                let changes = text_edits_to_changes(&edits, &self.buf);
                eprintln!("[lsp] format {} edits", changes.len());
                self.apply_edits(changes, Cause::Format, cx);
            }
            Ok(_) => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Info,
                    title: strings::ALREADY_FORMATTED.into(),
                    body: None,
                });
            }
            Err(e) => {
                self.host.sink.emit(ide_core::IdeEvent::Notify {
                    level: ide_core::event::NotifyLevel::Warning,
                    title: format!("{}: {}", strings::FORMAT_UNAVAILABLE, e.message),
                    body: None,
                });
            }
        }
    }

    // -- signature help & hover card (M5 phase 4) -----------------------------
    //
    // '(' / ',' request textDocument/signatureHelp; the popup rides the
    // primary caret (the completion popup's anchor math) and renders the
    // active signature with its active parameter accented. Hovering a token
    // with no diagnostic schedules one debounced hover request; the answer
    // renders as a markdown (or plain) card under its line.

    fn request_signature_help(&mut self, cx: &mut gpui::Context<Self>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let pos = self.caret_pos();
        self.host.runtime.spawn(async move {
            let result = lsp.signature_help(doc, pos).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.signature_ready(result, cx)).ok();
            }
        })
        .detach();
    }

    fn signature_ready(
        &mut self,
        result: IdeResult<Option<ide_lsp::types::SignatureHelp>>,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Ok(Some(help)) = result {
            self.signature = Some(help);
        } else {
            self.signature = None;
        }
        cx.notify();
    }

    fn close_signature(&mut self, cx: &mut gpui::Context<Self>) {
        if self.signature.take().is_some() {
            cx.notify();
        }
    }

    fn signature_cycle(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        if let Some(help) = &mut self.signature {
            let count = help.signatures.len().max(1) as isize;
            help.active_signature = ((help.active_signature as isize + delta).rem_euclid(count)) as u32;
            cx.notify();
        }
    }

    /// Schedules the pointer's hover request (debounced on the tokio side; a
    /// mouse move across tokens collapses into the last generation).
    fn schedule_lsp_hover(&mut self, offset: usize, cx: &mut gpui::Context<Self>) {
        self.hover_req_gen += 1;
        let req = self.hover_req_gen;
        self.hover_req_offset = Some(offset);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let line = self.buf.line_of(offset);
        let pos = Pos::new(line, (offset - self.buf.line_start(line)) as u32);
        self.host.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let result = lsp.hover(doc, pos, CancellationToken::new()).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.hover_ready(req, offset, result, cx)).ok();
            }
        })
        .detach();
    }

    fn hover_ready(
        &mut self,
        req: u32,
        offset: usize,
        result: IdeResult<Option<ide_lsp::types::Hover>>,
        cx: &mut gpui::Context<Self>,
    ) {
        if req != self.hover_req_gen {
            return;
        }
        self.hover_req_offset = None;
        if let Ok(Some(hover)) = result {
            let line = self.buf.line_of(offset);
            self.hover_card =
                Some(HoverCard { line, markdown: hover.contents.kind == "markdown", contents: hover.contents.value });
            cx.notify();
        }
    }

    // -- live templates (M5 phase 5) ------------------------------------------
    //
    // Tab on an abbrev for the document's language expands it (the frontend's
    // builtin Go/Rust sets); the session walks tab stops with Tab/Shift-Tab,
    // typing at the active stop replaces every linked occurrence (the stops
    // are the selections), and `$END$`/Esc/undo/click ends it.

    /// Tab's template half: walk the active session's stops.
    fn advance_template(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        let Some(mut session) = self.template.take() else { return false };
        session.active += 1;
        if session.active < session.stops.len() {
            self.select_template_stop(&mut session);
            self.template = Some(session);
            self.clamp_scroll_to_primary();
        }
        cx.notify();
        true
    }

    /// Shift-Tab's template half: back one stop.
    fn previous_template(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        let Some(mut session) = self.template.take() else { return false };
        if session.active > 0 {
            session.active -= 1;
            self.select_template_stop(&mut session);
            self.template = Some(session);
        }
        self.clamp_scroll_to_primary();
        cx.notify();
        true
    }

    /// Selects the session's active stop: its ranges become the selections
    /// (typing replaces all linked occurrences); `$END$` ends the session.
    fn select_template_stop(&mut self, session: &mut TemplateSession) {
        let stop = &session.stops[session.active];
        if stop.name.is_empty() {
            let (at, _) = stop.ranges[0];
            self.selections = vec![Selection::caret(at)];
            self.template = None;
        } else {
            self.selections = stop
                .ranges
                .iter()
                .map(|(start, end)| Selection { anchor: *start, head: *end, reversed: false, goal_col: None })
                .collect();
        }
    }

    /// Tab on an abbrev expands it: one batch replaces the abbrev with the
    /// expanded body (its own undo step), then the session opens on the first
    /// stop. Returns false when the word before the caret is no abbrev.
    fn try_expand_template(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        if self.selections.len() != 1 || !self.selections[0].is_empty() {
            return false;
        }
        let caret = self.selections[0].head;
        let start = self.typed_word_start();
        if start == caret {
            return false;
        }
        let word = self.buf.slice_utf16(start..caret);
        let Some(expansion_template) =
            crate::templates::for_language(self.language.as_str()).find(|t| t.abbrev == word)
        else {
            return false;
        };
        let line = self.buf.line_of(caret);
        let line_text = self.buf.slice_utf16(self.buf.line_span(line));
        let base_indent = &line_text[..line_text.len() - line_text.trim_start_matches([' ', '\t']).len()];
        let expansion = crate::templates::expand(expansion_template, &self.indent.unit(), base_indent);
        let insert_at = start;
        let changes = vec![TextChange { from: start as u32, to: caret as u32, insert: expansion.text.clone() }];
        self.apply_edits(changes, Cause::Template, cx);
        let session = TemplateSession {
            stops: expansion
                .stops
                .into_iter()
                .map(|mut stop| {
                    stop.ranges = stop.ranges.iter().map(|(from, to)| (from + insert_at, to + insert_at)).collect();
                    stop
                })
                .collect(),
            active: 0,
        };
        eprintln!("[template] expanded {}", expansion_template.id);
        let mut session = session;
        self.select_template_stop(&mut session);
        self.template = Some(session);
        self.clamp_scroll_to_primary();
        cx.notify();
        true
    }

    /// Re-maps the session's stops through an applied batch. Typing/paste/IME
    /// at the active stop rewrite its ranges (the stops were the selections);
    /// other stops shift by the batch deltas; undo/redo ends the session.
    fn template_after_batch(&mut self, changes: &[TextChange], cause: Cause) {
        let Some(session) = &mut self.template else { return };
        match cause {
            Cause::Typing | Cause::Paste | Cause::ImeCommit => {
                let active = session.active;
                let mut delta: i64 = 0;
                let mut new_ranges = Vec::with_capacity(changes.len());
                for change in changes {
                    let inserted = change.insert.chars().map(char::len_utf16).sum::<usize>() as i64;
                    let from = (change.from as i64 + delta).max(0) as usize;
                    new_ranges.push((from, from + inserted.max(0) as usize));
                    delta += inserted - (change.to as i64 - change.from as i64);
                }
                for (index, stop) in session.stops.iter_mut().enumerate() {
                    if index != active {
                        stop.ranges = stop.ranges.iter().filter_map(|r| map_stop_range(*r, changes)).collect();
                    }
                }
                session.stops[active].ranges = new_ranges;
            }
            Cause::Mechanical => self.template = None,
            _ => {}
        }
    }

    // -- find & replace (M5 phase 6) -----------------------------------------
    //
    // ⌘F opens the bar (the query input holds focus, rename-input pattern):
    // Enter/Shift-Enter walk the matches, ⌥⌘F (or the arrow button) reveals
    // the replace row. Matches paint as translucent rects under the text and
    // survive the bar closing (⌘F reopens with the query; Esc clears them) —
    // which is also the shape a Find-in-Files hit arrival paints (the
    // workbench calls show_find_hits, no bar).

    /// ⌘F: opens the bar, seeding the query from the selection (≤ 200 chars)
    /// or from the current match list (a Find-in-Files arrival adopts its
    /// query). Already open: nothing (the input keeps the focus).
    fn open_find(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.find_bar.is_some() {
            return;
        }
        self.close_completion(cx);
        let seed = self.find_hits.as_ref().map_or_else(
            || {
                let (start, end) = self.selections.first().map_or((0, 0), |s| s.normalized());
                if end > start && end - start <= 200 { self.buf.slice_utf16(start..end) } else { String::new() }
            },
            |hits| hits.query.clone(),
        );
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(strings::FIND_PLACEHOLDER));
        query.update(cx, |state, cx| state.set_value(seed.clone(), window, cx));
        let replace = cx.new(|cx| InputState::new(window, cx).placeholder(strings::REPLACE_PLACEHOLDER));
        // Single-line inputs emit PressEnter and propagate the keystroke, so
        // the pane sees the Newline action too — swallowed in `command` while
        // the bar is up; this subscription does the actual walking (it is the
        // only place Shift-Enter is distinguishable).
        let sub = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                let text = this.find_bar.as_ref().map_or(String::new(), |bar| bar.query.read(cx).value().to_string());
                if let Some(hits) = &mut this.find_hits {
                    hits.query = text;
                }
                this.find_recompute(cx);
            }
            InputEvent::PressEnter { shift, .. } => this.find_step(if *shift { -1 } else { 1 }, cx),
            _ => {}
        });
        let had_hits = self.find_hits.is_some();
        if !had_hits {
            self.find_hits =
                Some(FindMatches { query: seed, case_sensitive: false, ranges: Vec::new(), current: 0, capped: false });
        }
        self.find_bar = Some(FindBar { query, replace, replace_visible: false, _sub: sub });
        if !had_hits {
            self.find_recompute(cx);
        }
        cx.notify();
    }

    /// Esc on the bar (or a click in the text): the bar closes, the focus
    /// returns to the pane, the matches stay painted until Esc again.
    fn close_find_bar(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.find_bar.take().is_none() {
            return;
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Find-in-Files arrival: highlight every hit of `query` in this buffer.
    /// The bar stays closed; ⌘F adopts the query (open_find's seed).
    pub fn show_find_hits(&mut self, query: String, cx: &mut gpui::Context<Self>) {
        let case_sensitive = self.find_hits.as_ref().is_some_and(|hits| hits.case_sensitive);
        self.find_hits = Some(FindMatches { query, case_sensitive, ranges: Vec::new(), current: 0, capped: false });
        self.find_recompute(cx);
    }

    /// Recomputes the match list from the query (literal, case-folded per the
    /// toggle), line by line, capped. The current match stays the first one
    /// at-or-after where it pointed (or the caret).
    fn find_recompute(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(hits) = &mut self.find_hits else { return };
        let previous = hits.ranges.get(hits.current).map(|range| range.start);
        hits.ranges.clear();
        hits.capped = false;
        let query = hits.query.clone();
        if !query.is_empty() {
            'lines: for line in 0..self.buf.line_count() {
                let span = self.buf.line_span(line);
                let text = self.buf.slice_utf16(span.clone());
                for hit in match_ranges(&text, &query, hits.case_sensitive) {
                    hits.ranges.push(span.start + hit.start..span.start + hit.end);
                    if hits.ranges.len() >= MAX_FIND_MATCHES {
                        hits.capped = true;
                        break 'lines;
                    }
                }
            }
        }
        if hits.ranges.is_empty() {
            hits.current = 0;
        } else {
            let from = previous.unwrap_or_else(|| self.selections.first().map_or(0, |s| s.head));
            hits.current = hits.ranges.partition_point(|range| range.end <= from).min(hits.ranges.len() - 1);
        }
        cx.notify();
    }

    /// Enter / Shift-Enter: the current match becomes the selection (the
    /// ordinary selection paint), scrolling into view; wrapping at the ends.
    fn find_step(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let Some(hits) = &mut self.find_hits else { return };
        let len = hits.ranges.len() as isize;
        if len == 0 {
            return;
        }
        hits.current = ((hits.current as isize + delta).rem_euclid(len)) as usize;
        let range = hits.ranges[hits.current].clone();
        self.selections = vec![Selection::caret(range.start).with_head(range.end)];
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    fn find_toggle_case(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(hits) = &mut self.find_hits else { return };
        hits.case_sensitive = !hits.case_sensitive;
        self.find_recompute(cx);
    }

    fn find_toggle_replace(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(bar) = &mut self.find_bar else { return };
        bar.replace_visible = !bar.replace_visible;
        cx.notify();
    }

    /// The replace row's text (empty without the bar — unreachable through
    /// the UI, the buttons live in the bar).
    fn find_replacement(&self, cx: &gpui::App) -> String {
        self.find_bar.as_ref().map_or(String::new(), |bar| bar.replace.read(cx).value().to_string())
    }

    /// Replaces the current match (one [`Cause::Replace`] batch) and parks
    /// the selection on the next match at-or-after the replaced range.
    fn find_replace_one(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(range) = self.find_hits.as_ref().and_then(|hits| hits.ranges.get(hits.current).cloned()) else {
            return;
        };
        let replacement = self.find_replacement(cx);
        let change = TextChange { from: range.start as u32, to: range.end as u32, insert: replacement };
        self.apply_edits(vec![change], Cause::Replace, cx);
        // apply_edits recomputed the matches: advance past the replaced text.
        if let Some(hits) = &mut self.find_hits {
            if hits.ranges.is_empty() {
                hits.current = 0;
            } else {
                hits.current = hits.ranges.partition_point(|hit| hit.end <= range.end).min(hits.ranges.len() - 1);
            }
        }
        if let Some(range) = self.find_hits.as_ref().and_then(|hits| hits.ranges.get(hits.current).cloned()) {
            self.selections = vec![Selection::caret(range.start).with_head(range.end)];
        }
        cx.notify();
    }

    /// Replaces every match in one batch (its own undo step removes the
    /// whole rewrite).
    fn find_replace_all(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(hits) = self.find_hits.as_ref() else { return };
        if hits.ranges.is_empty() {
            return;
        }
        let replacement = self.find_replacement(cx);
        let changes = hits
            .ranges
            .iter()
            .map(|range| TextChange { from: range.start as u32, to: range.end as u32, insert: replacement.clone() })
            .collect();
        self.apply_edits(changes, Cause::Replace, cx);
        if let Some(hits) = &mut self.find_hits {
            hits.current = 0;
        }
        cx.notify();
    }

    /// The count label: `n of m`, `m` with a `+` when the cap stopped the
    /// scan, the hint / no-matches text for the degenerate states.
    fn find_count_label(&self) -> String {
        let Some(hits) = &self.find_hits else { return String::new() };
        if hits.query.is_empty() {
            return strings::FIND_HINT.to_owned();
        }
        if hits.ranges.is_empty() {
            return strings::FIND_NO_MATCHES.to_owned();
        }
        let total = if hits.capped { format!("{}+", MAX_FIND_MATCHES) } else { hits.ranges.len().to_string() };
        format!("{} of {}", hits.current + 1, total)
    }

    // -- completion (M3 phase 4) ---------------------------------------------
    //
    // Typing schedules a debounced LSP request (immediately for a server
    // trigger character); the answer opens a self-drawn popup overlay at the
    // caret, filtered client-side against the typed word. Enter applies the
    // selected item through the one edit pipeline; Esc closes.

    /// The primary caret as an IDE position (UTF-16 columns).
    fn caret_pos(&self) -> Pos {
        let head = self.selections.first().map_or(0, |s| s.head);
        let line = self.buf.line_of(head);
        Pos::new(line, (head - self.buf.line_start(line)) as u32)
    }

    /// The word being typed at the primary caret: the run of identifier
    /// characters ending at the caret (the popup's filter and the items'
    /// default replace base).
    fn typed_word_start(&self) -> usize {
        let head = self.selections.first().map_or(0, |s| s.head);
        let mut start = head;
        while start > 0 {
            let prev = self.buf.prev_boundary(start);
            if !self.buf.char_of(prev).is_alphanumeric() && self.buf.char_of(prev) != '_' {
                break;
            }
            start = prev;
        }
        start
    }

    /// Schedules the debounced request; a trigger character skips the wait.
    fn schedule_completion(&mut self, cx: &mut gpui::Context<Self>) {
        let caret = self.selections.first().map_or(0, |s| s.head);
        let typed = self.buf.slice_utf16(self.typed_word_start()..caret);
        // Typing beyond the word (space, newline, punctuation) dismisses; a
        // server trigger character requests right away with it.
        let triggers = [":", ".", "'", "("];
        let trigger = if typed.is_empty() {
            triggers
                .iter()
                .find(|t| {
                    self.buf.slice_utf16(caret.saturating_sub(t.chars().map(char::len_utf16).sum::<usize>())..caret)
                        == **t
                })
                .map(|t| t.to_string())
        } else {
            None
        };
        let word_ended = typed.is_empty() && trigger.is_none();
        if word_ended {
            self.close_completion(cx);
            return;
        }
        // An open popup re-queries faster: the user is narrowing down.
        let debounce = if self.completion.as_ref().is_some_and(|open| open.is_incomplete) { 60 } else { 140 };
        self.completion_gen += 1;
        let req = self.completion_gen;
        let runtime = self.host.runtime.clone();
        cx.spawn(async move |this, cx| {
            runtime
                .spawn(async move {
                    // The debounce lives on the tokio side so a burst of
                    // keystrokes collapses into the last generation.
                    tokio::time::sleep(Duration::from_millis(debounce)).await;
                })
                .await
                .ok();
            this.update(cx, |pane, cx| {
                if pane.completion_gen == req {
                    pane.request_completion(trigger.clone(), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Fires the LSP completion request onto the app runtime; the answer
    /// lands in [`EditorPane::completion_ready`].
    fn request_completion(&mut self, trigger: Option<String>, cx: &mut gpui::Context<Self>) {
        self.completion_gen += 1;
        let req = self.completion_gen;
        let cancel = CancellationToken::new();
        self.completion_cancel = Some(cancel.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        let pos = self.caret_pos();
        self.host.runtime.spawn(async move {
            let result = lsp.completion(doc, pos, trigger, cancel).await;
            let _ = tx.send(result);
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                this.update(cx, |pane, cx| pane.completion_ready(req, result, cx)).ok();
            }
        })
        .detach();
    }

    /// Lands an answer: opens (or refreshes) the popup unless the caret kept
    /// moving past this request's generation.
    fn completion_ready(&mut self, req: u32, result: IdeResult<CompletionList>, cx: &mut gpui::Context<Self>) {
        if req != self.completion_gen {
            return;
        }
        let Ok(list) = result else {
            // No server, unsupported, or timed out: the popup simply stays as
            // it is (typing re-requests).
            return;
        };
        // Snippet-format items stay (gopls answers member access entirely as
        // snippets — M6 opened that seam); only a truly empty list closes.
        if list.items.is_empty() {
            self.close_completion(cx);
            return;
        }
        let replace_start = match self.completion.as_ref() {
            Some(open) => open.replace_start,
            None => self.typed_word_start(),
        };
        let selected = self.completion.as_ref().map_or(0, |open| open.selected.min(list.items.len() - 1));
        self.completion =
            Some(CompletionPopup { items: list.items, replace_start, selected, is_incomplete: list.is_incomplete });
        // The completion list supersedes a signature popup over the same spot.
        self.signature = None;
        cx.notify();
    }

    /// The filter: the text between the popup's word start and the caret.
    fn completion_filter(&self) -> String {
        self.completion
            .as_ref()
            .and_then(|open| {
                let caret = self.selections.first().map_or(0, |s| s.head);
                (caret >= open.replace_start).then(|| self.buf.slice_utf16(open.replace_start..caret))
            })
            .unwrap_or_default()
    }

    fn jump_move(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        if let Some((locations, selected)) = &mut self.jump {
            *selected = ((*selected as isize + delta).rem_euclid(locations.len().max(1) as isize)) as usize;
            cx.notify();
        }
    }

    fn jump_accept(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some((locations, selected)) = self.jump.clone()
            && let Some(location) = locations.get(selected)
        {
            self.jump_to(location, cx);
            return;
        }
        self.jump = None;
        cx.notify();
    }

    fn completion_move(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        if let Some(mut open) = self.completion.take() {
            let count = open.filtered(&self.completion_filter()).len().max(1) as isize;
            open.selected = ((open.selected as isize + delta).rem_euclid(count)) as usize;
            self.completion = Some(open);
            cx.notify();
        }
    }

    /// Closes the popup without touching the buffer.
    fn close_completion(&mut self, cx: &mut gpui::Context<Self>) {
        if self.completion.take().is_some() {
            if let Some(token) = self.completion_cancel.take() {
                token.cancel();
            }
            cx.notify();
        }
    }

    /// Applies the selected item: its `replace_range` (or the typed word)
    /// becomes `insert_text`, plus any additional edits, in one batch through
    /// the ordinary pipeline (so undo, tree-sitter and server sync all see an
    /// ordinary edit). A snippet-format item expands through the template
    /// session machinery: the placeholder stops open linked (Tab walks them,
    /// typing mirrors across repeats), and the caret rides the final stop.
    fn completion_accept(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(open) = self.completion.take() else { return };
        self.template = None;
        let filter = self.completion_filter();
        let visible = open.filtered(&filter);
        let Some(item) = visible.get(open.selected.min(visible.len().saturating_sub(1))).map(|item| (*item).clone())
        else {
            self.completion = Some(open);
            return;
        };
        let caret = self.selections.first().map_or(0, |s| s.head);
        let (start, end) = match &item.replace_range {
            Some(range) => {
                let start = self.buf.line_start(range.start.line).min(self.buf.utf16_len()) + range.start.col as usize;
                let end = self.buf.line_start(range.end.line).min(self.buf.utf16_len()) + range.end.col as usize;
                (start.min(caret).min(end), end.max(start))
            }
            None => (open.replace_start, caret),
        };
        let mut changes = vec![TextChange {
            from: start.min(end) as u32,
            to: end.max(start) as u32,
            insert: item.insert_text.clone(),
        }];
        for edit in &item.additional_edits {
            let from =
                self.buf.line_start(edit.range.start.line).min(self.buf.utf16_len()) + edit.range.start.col as usize;
            let to = self.buf.line_start(edit.range.end.line).min(self.buf.utf16_len()) + edit.range.end.col as usize;
            changes.push(TextChange { from: from as u32, to: to.max(from) as u32, insert: edit.new_text.clone() });
        }
        self.close_completion(cx);
        if item.is_snippet
            && let Some(expansion) = crate::templates::lsp_snippet_to_expansion(&item.insert_text)
        {
            // Same batch shape, template cause: the stops open linked and the
            // undo step removes the whole expansion.
            let insert_at = start.min(end);
            let mut changes =
                vec![TextChange { from: insert_at as u32, to: end.max(start) as u32, insert: expansion.text.clone() }];
            let mut shift: i64 = 0;
            for edit in &item.additional_edits {
                let from = self.buf.line_start(edit.range.start.line).min(self.buf.utf16_len())
                    + edit.range.start.col as usize;
                let to =
                    self.buf.line_start(edit.range.end.line).min(self.buf.utf16_len()) + edit.range.end.col as usize;
                let (lo, hi) = (from.min(to), from.max(to));
                let inserted = crate::editor::utf16_len(&edit.new_text) as i64;
                if lo <= insert_at {
                    shift += inserted - (hi - lo) as i64;
                }
                changes.push(TextChange { from: lo as u32, to: hi as u32, insert: edit.new_text.clone() });
            }
            self.apply_edits(changes, Cause::Template, cx);
            let offset = (insert_at as i64 + shift).max(0) as usize;
            let mut session = TemplateSession {
                stops: expansion
                    .stops
                    .into_iter()
                    .map(|mut stop| {
                        stop.ranges = stop.ranges.iter().map(|(from, to)| (from + offset, to + offset)).collect();
                        stop
                    })
                    .collect(),
                active: 0,
            };
            self.select_template_stop(&mut session);
            self.template = Some(session);
            self.clamp_scroll_to_primary();
            cx.notify();
            return;
        }
        self.apply_edits(changes, Cause::Typing, cx);
        // The caret lands at the end of the inserted text.
        self.selections = vec![Selection::caret(start.min(end) + crate::editor::utf16_len(&item.insert_text))];
        self.clamp_scroll_to_primary();
        cx.notify();
    }

    /// Replaces the pane's diagnostics (the workbench merges the producers).
    pub fn set_diagnostics(&mut self, merged: Vec<Diagnostic>, cx: &mut gpui::Context<Self>) {
        self.diagnostics = merged;
        self.hover.set(None);
        self.hover_card = None;
        self.hover_req_offset = None;
        self.hover_fixes = None;
        cx.notify();
    }

    /// The diagnostics of one buffer line, by their start line (the list is
    /// position-sorted). The gutter marks these; squiggles also show what
    /// merely crosses the line.
    fn line_diagnostics(&self, line: u32) -> &[Diagnostic] {
        let start = self.diagnostics.partition_point(|d| d.range.start.line < line);
        let end = start + self.diagnostics[start..].partition_point(|d| d.range.start.line == line);
        &self.diagnostics[start..end]
    }

    /// UTF-16 buffer range of a diagnostic, clamped into the document.
    fn diag_range(&self, d: &Diagnostic) -> Range<usize> {
        let start = self.buf.line_start(d.range.start.line).min(self.buf.utf16_len()) + d.range.start.col as usize;
        let end = self.buf.line_start(d.range.end.line).min(self.buf.utf16_len()) + d.range.end.col as usize;
        start..end.max(start)
    }

    // -- IME (composition happens at the primary cursor; others freeze) ----

    /// `setMarkedText:selectedRange:replacementRange:` at the primary cursor.
    fn ime_set_marked(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.hud.note_input();
        let caret = self.selections.first().map_or(0, |s| s.head);
        let base = range_utf16.clone().or_else(|| self.buf.marked.clone()).unwrap_or(caret..caret);
        let (marked, change) = self.buf.replace(base, new_text);
        self.buf.marked = if new_text.is_empty() || marked.is_empty() { None } else { Some(marked.clone()) };
        // The platform's selected range is relative to the marked text start.
        let caret = match new_selected {
            Some(selected) => {
                let start = selected.start.clamp(0, marked.len());
                let end = selected.end.clamp(start, marked.len());
                marked.start + end
            }
            None => marked.end,
        };
        if self.selections.is_empty() {
            self.selections.push(Selection::caret(caret));
        } else {
            self.selections[0] = Selection::caret(caret);
        }
        // Marked keystrokes are not undo entries; committing (below) removes
        // the whole composition in one step.
        self.apply_edits(vec![change], Cause::Mechanical, cx);
    }

    /// `insertText:replacementRange:` — commits the composition (replacing
    /// the marked range) or inserts at the primary cursor.
    fn ime_commit(&mut self, range_utf16: Option<Range<usize>>, text: &str, cx: &mut gpui::Context<Self>) {
        self.hud.note_input();
        // The commit replaces what the pane holds selected. An empty IME
        // range (plain `insertText`, macOS commits per character) with no
        // marked text means the primary selection is the composed range —
        // falling back to a bare caret here made a template stop's selected
        // occurrence keep its default text while the commit appended after
        // it (found driving the M5 exit loop).
        let base = range_utf16.clone().or_else(|| self.buf.marked.clone()).unwrap_or_else(|| {
            self.selections.first().map_or(0..0, |s| {
                let (start, end) = s.normalized();
                start..end
            })
        });
        self.buf.marked = None;
        // A live template's active stop: the commit mirrors to every linked
        // occurrence (the other cursors froze during composition).
        let linked = self.template.as_ref().and_then(|session| {
            let stop = session.stops.get(session.active)?;
            (!stop.name.is_empty()).then(|| stop.ranges.clone())
        });
        if let Some(ranges) = linked
            && ranges.iter().any(|(start, end)| base.start >= *start && base.end <= *end)
        {
            // Plain per-character commits arrive as an empty range at the
            // caret: mirror the insert at the same occurrence-relative
            // offset into every linked occurrence (a whole-occurrence
            // replace here ate the other occurrences' typed text — found
            // driving the M5 exit loop). A non-empty base (a composition,
            // or a replaced selection) replaces each whole occurrence: the
            // other cursors froze during composition, so their full text is
            // what the commit means to replace.
            let matched = ranges
                .iter()
                .find(|(start, end)| base.start >= *start && base.end <= *end)
                .map(|(start, _)| base.start - *start)
                .unwrap_or(0);
            let mut changes = Vec::with_capacity(ranges.len());
            for (start, end) in ranges {
                if base.start >= start && base.end <= end {
                    changes.push(TextChange { from: base.start as u32, to: base.end as u32, insert: text.to_owned() });
                } else if base.start == base.end {
                    let at = start + matched.min(end - start);
                    changes.push(TextChange { from: at as u32, to: at as u32, insert: text.to_owned() });
                } else {
                    changes.push(TextChange { from: start as u32, to: end as u32, insert: text.to_owned() });
                }
            }
            if self.selections.is_empty() {
                self.selections.push(Selection::caret(base.start + utf16_len(text)));
            } else {
                self.selections[0] = Selection::caret(base.start + utf16_len(text));
            }
            self.apply_edits(changes, Cause::ImeCommit, cx);
            return;
        }
        let (inserted, change) = self.buf.replace(base, text);
        if self.selections.is_empty() {
            self.selections.push(Selection::caret(inserted.end));
        } else {
            self.selections[0] = Selection::caret(inserted.end);
        }
        self.apply_edits(vec![change], Cause::ImeCommit, cx);
    }

    // -- commands -----------------------------------------------------------
    //
    // Every editor behaviour is a gpui action (see `actions.rs`); the render
    // registers the handlers, they funnel through [`EditorPane::command`]
    // which guards composition and notes the perf HUD's input timestamp.
    // Printable input is not an action: it keeps its raw `on_key_down`
    // fallback below.

    fn command(&mut self, cmd: Command, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.hud.note_input();
        // The HUD toggle stays live while composing (M2 behaviour); every
        // other command yields to the IME: while composing, macOS routes keys
        // through the IME first, and reaching here means it passed one through.
        if let Command::ToggleHud = cmd {
            let on = !self.hud.visible.get();
            self.hud.visible.set(on);
            eprintln!(
                "[perf] hud {} — rows {} spans {}/{} parse {:?} ms@v{} input {:?} ms frame {:.1} ms",
                if on { "on" } else { "off" },
                self.hud.rows_laid_out.get(),
                self.hud.spans_painted.get(),
                self.hud.spans_total.get(),
                self.hud.parse_ms.get(),
                self.hud.parse_version.get(),
                self.hud.input_latency_ms.get(),
                self.hud.last_frame_ms.get()
            );
            cx.notify();
            return;
        }
        if self.buf.marked.is_some() {
            return;
        }
        // The rename input is modal: Enter confirms, Esc cancels; typing goes
        // through the input's own handler (it holds focus via render).
        if let Some(RenameFlow::Input { confirmed, .. }) = &self.rename {
            match cmd {
                Command::Newline => {
                    if !*confirmed {
                        self.rename_confirm(cx);
                    }
                    // The input view unmounts as the request goes out; the
                    // pane takes over so the preview's Enter still lands here.
                    window.focus(&self.focus, cx);
                    return;
                }
                Command::Cancel => {
                    self.rename_cancel_flow(window, cx);
                    return;
                }
                Command::Move(..)
                | Command::Delete { .. }
                | Command::Paste
                | Command::Tab
                | Command::Outdent
                | Command::ToggleHud => return,
            }
        }
        if matches!(self.rename, Some(RenameFlow::Preview { .. })) {
            match cmd {
                Command::Cancel => {
                    self.rename_cancel_flow(window, cx);
                    return;
                }
                Command::Newline => {
                    self.rename_apply(window, cx);
                    return;
                }
                _ => return,
            }
        }
        // The find bar is modal like the rename input: the query input owns
        // printable keys, arrows and Enter (its PressEnter { shift }
        // subscription walks the matches, so the propagated Newline and the
        // rest of the commands are swallowed here); Esc closes and the focus
        // returns to the pane.
        if self.find_bar.is_some() {
            if let Command::Cancel = cmd {
                self.close_find_bar(window, cx);
            }
            return;
        }
        // The jump list owns navigation and confirm like the completion popup.
        if self.jump.is_some() {
            match cmd {
                Command::Move(Motion::Up, false) => {
                    self.jump_move(-1, cx);
                    return;
                }
                Command::Move(Motion::Down, false) => {
                    self.jump_move(1, cx);
                    return;
                }
                Command::Newline => {
                    self.jump_accept(cx);
                    return;
                }
                Command::Cancel => {
                    self.jump = None;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        // The signature popup cycles signatures with ↑/↓ and closes on Esc;
        // motions and edits close it and fall through to their normal work.
        if self.signature.is_some() {
            match cmd {
                Command::Move(Motion::Up, false) => {
                    self.signature_cycle(-1, cx);
                    return;
                }
                Command::Move(Motion::Down, false) => {
                    self.signature_cycle(1, cx);
                    return;
                }
                Command::Cancel => {
                    self.close_signature(cx);
                    return;
                }
                Command::Newline => self.close_signature(cx),
                Command::Move(..) | Command::Delete { .. } | Command::Paste => self.close_signature(cx),
                _ => {}
            }
        }
        // The quick-fix menu owns navigation and confirm like the jump list.
        if self.code_actions.is_some() {
            match cmd {
                Command::Move(Motion::Up, false) => {
                    self.code_actions_move(-1, cx);
                    return;
                }
                Command::Move(Motion::Down, false) => {
                    self.code_actions_move(1, cx);
                    return;
                }
                Command::Newline => {
                    self.apply_code_action(cx);
                    return;
                }
                Command::Cancel => {
                    self.close_code_actions(cx);
                    return;
                }
                _ => {}
            }
        }
        // While the completion popup is open it owns navigation, accept and
        // cancel; every other command (typing continues through the raw key
        // path) re-filters through schedule_completion.
        if self.completion.is_some() {
            match cmd {
                Command::Move(Motion::Up, false) => {
                    self.completion_move(-1, cx);
                    return;
                }
                Command::Move(Motion::Down, false) => {
                    self.completion_move(1, cx);
                    return;
                }
                Command::Move(Motion::DocStart, false) => {
                    self.completion_move(-999, cx);
                    return;
                }
                Command::Move(Motion::DocEnd, false) => {
                    self.completion_move(999, cx);
                    return;
                }
                Command::Newline => {
                    self.completion_accept(cx);
                    return;
                }
                Command::Cancel => {
                    self.close_completion(cx);
                    return;
                }
                _ => {}
            }
        }
        match cmd {
            Command::Move(motion, extend) => self.move_cursors(motion, extend, cx),
            Command::Delete { forward, word } => self.delete(forward, word, cx),
            Command::Newline => self.newline(cx),
            Command::Tab => self.tab(cx),
            Command::Outdent => {
                if self.previous_template(cx) {
                    return;
                }
                self.indent_lines(-1, cx)
            }
            Command::Paste => {
                let text = cx.read_from_clipboard().and_then(|item| item.text());
                if let Some(text) = text {
                    self.paste(&text, cx);
                }
            }
            Command::Cancel => {
                if self.template.take().is_some() {
                    cx.notify();
                }
                // Esc with the bar closed clears the find highlights (the
                // second Esc of the IDEA rhythm).
                if self.find_hits.take().is_some() {
                    cx.notify();
                }
                self.collapse_cursors(cx)
            }
            Command::ToggleHud => unreachable!("handled above"),
        }
    }

    // -- keys ---------------------------------------------------------------

    /// The raw-input path: printable characters the IME did not consume (and
    /// unbound keys) land here. Commands travel as actions instead.
    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let ks = &ev.keystroke;
        self.hud.note_input();
        if ks.modifiers.function {
            return;
        }
        if self.buf.marked.is_some() {
            eprintln!("[key] passthrough while composing: {}", ks.key);
            return;
        }
        if self.rename.is_some() || self.find_bar.is_some() {
            // A modal input (rename, the find bar's query) owns typing. The
            // input handles most keys, but the ones it handles-and-propagates
            // (Enter on a single line) land here with the pane's selections
            // still set — find navigation even parks them on a match range —
            // and typing then rewrote the buffer at the match (found driving
            // the M5 exit loop).
            return;
        }
        if ks.modifiers.platform || ks.modifiers.alt || ks.modifiers.control {
            return;
        }
        if let Some(text) = ks.key_char.as_ref() {
            self.insert_text(text, cx);
        }
    }

    // -- mouse -------------------------------------------------------------

    /// The x offset the row's hints add before `at` (UTF-16 in the line):
    /// every hint the position passes shifts the text right by its width.
    fn hint_shift(&self, line: u32, at: usize) -> f32 {
        self.hints_by_line
            .borrow()
            .get(&line)
            .map_or(0.0, |hints| hints.iter().filter(|hint| hint.at < at).map(|hint| hint.width).sum())
    }

    /// Buffer offset for a point inside one row's line, hints included: text
    /// segments and hint labels alternate left to right, and a point inside a
    /// label resolves to the hint's position (the token it annotates).
    fn offset_in_line(&self, window: &Window, line: u32, text_x: Pixels) -> usize {
        let span = self.buf.line_span(line);
        let line_text = self.buf.slice_utf16(span.clone());
        let font_size = theme::px_font_size();
        let hints = self.hints_by_line.borrow().get(&line).cloned().unwrap_or_default();
        if hints.is_empty() {
            return span.start + utf16_index_at_x(window, &line_text, text_x, font_size);
        }
        let mut consumed = 0.0f32;
        let mut prev = 0usize;
        for hint in &hints {
            let piece = &line_text[prev..hint.at];
            let piece_len = piece.chars().map(char::len_utf16).sum::<usize>();
            let width = f32::from(x_at_utf16(window, piece, piece_len, font_size));
            if text_x < px(consumed) + px(width) {
                let col = utf16_index_at_x(window, piece, text_x - px(consumed), font_size);
                return span.start + prev + col;
            }
            consumed += width;
            if text_x < px(consumed) + px(hint.width) {
                return span.start + hint.at;
            }
            consumed += hint.width;
            prev = hint.at;
        }
        let rest = &line_text[prev..];
        if prev < line_text.len() {
            let col = utf16_index_at_x(window, rest, text_x - px(consumed), font_size);
            return span.start + prev + col;
        }
        span.end
    }

    /// Buffer offset for a window point (its row's line, clamped into it).
    fn offset_at(&self, window: &Window, geometry: &Geometry, position: Point<Pixels>) -> usize {
        let (line, _) = self.resolve_display(self.row_at(geometry, position.y));
        let text_x = position.x - geometry.origin.x + px(geometry.scroll_x);
        self.offset_in_line(window, line, text_x)
    }

    /// The display row a window y falls on. The viewport-relative row clamps
    /// into the visible band *before* the scroll offset joins: clamping the
    /// final row instead would pull scrolled views' lower rows onto one line.
    fn row_at(&self, geometry: &Geometry, y: Pixels) -> u32 {
        let count = self.display_count();
        let visible = ((geometry.height - px(theme::TEXT_INSET) * 2.0) / geometry.line_height) as i64;
        let row = (((y - geometry.origin.y) / geometry.line_height) as i64).clamp(0, visible.max(1) - 1)
            + self.scroll.get() as i64;
        (row.clamp(0, count.max(1) as i64 - 1)).max(0) as u32
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(geometry) = self.geometry.get() else { return };
        // The scrollbars take their hits first: drags and page jumps, never
        // caret/edit logic.
        if geometry.vscroll.hit_thumb(ev.position) {
            self.drag.set(Some(Drag::ScrollV { grab: ev.position.y - geometry.vscroll.thumb.origin.y }));
            return;
        }
        if geometry.vscroll.hit_track(ev.position) {
            // A track click pages toward the click: the thumb centers there.
            let track_y = geometry.vscroll.track.origin.y;
            let travel = (geometry.vscroll.track.size.height - geometry.vscroll.thumb.size.height).max(px(1.0));
            let ratio = ((ev.position.y - track_y - geometry.vscroll.thumb.size.height / 2.0) / travel).clamp(0.0, 1.0);
            self.scroll.set(ratio * self.scroll_range());
            self.drag.set(Some(Drag::ScrollV { grab: geometry.vscroll.thumb.size.height / 2.0 }));
            cx.notify();
            return;
        }
        if geometry.hscroll.hit_thumb(ev.position) {
            self.drag.set(Some(Drag::ScrollH { grab: ev.position.x - geometry.hscroll.thumb.origin.x }));
            return;
        }
        if geometry.hscroll.hit_track(ev.position) {
            let track_x = geometry.hscroll.track.origin.x;
            let travel = (geometry.hscroll.track.size.width - geometry.hscroll.thumb.size.width).max(px(1.0));
            let ratio = ((ev.position.x - track_x - geometry.hscroll.thumb.size.width / 2.0) / travel).clamp(0.0, 1.0);
            self.scroll_x.set(ratio * self.max_scroll_x());
            self.drag.set(Some(Drag::ScrollH { grab: geometry.hscroll.thumb.size.width / 2.0 }));
            cx.notify();
            return;
        }
        // A click in the text area closes the find bar (clicks inside the bar
        // itself stop propagation before reaching here).
        if self.find_bar.take().is_some() {
            window.focus(&self.focus, cx);
        }
        self.template = None;
        let display_row = self.row_at(&geometry, ev.position.y);
        // The gutter folds: a chevron row toggles its fold.
        if ev.position.x < geometry.gutter_x + geometry.gutter_w {
            let (line, placeholder) = self.resolve_display(display_row);
            if let Some(fold) = placeholder {
                self.unfold_placeholder(fold.start_line, cx);
            } else if self.fold_candidates.iter().any(|(start, _)| *start == line) {
                self.toggle_fold(line, cx);
            }
            return;
        }
        // Clicking a placeholder row unfolds it.
        if let (_, Some(fold)) = self.resolve_display(display_row) {
            self.unfold_placeholder(fold.start_line, cx);
            return;
        }
        if ev.modifiers.alt {
            // ⌥ starts a column drag (the selections form once it moves);
            // a bare ⌥-click (released without moving) toggles a cursor at
            // the point — M2's click-add-cursor, displaced from ⌘ because
            // ⌘-click is go-to-definition in M3 (IntelliJ's macOS mapping).
            self.drag.set(Some(Drag::Columnar { from: ev.position }));
            return;
        }
        let offset = self.offset_at(window, &geometry, ev.position);
        if ev.modifiers.platform && !ev.modifiers.shift {
            // ⌘-click: go to the definition under the point (⌥-click keeps
            // M2's click-add-cursor; ⌥-drag still makes a column selection).
            self.selections = vec![Selection::caret(offset)];
            self.goto_definition(cx);
            return;
        }
        if ev.modifiers.shift {
            let primary = self.selections.first().cloned().unwrap_or_else(|| Selection::caret(offset));
            self.selections = vec![primary.with_head(offset)];
        } else {
            self.selections = vec![Selection::caret(offset)];
            self.drag.set(Some(Drag::Normal { anchor: offset }));
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let Some(geometry) = self.geometry.get() else { return };
        match self.drag.get() {
            Some(Drag::Normal { anchor }) => {
                let head = self.offset_at(window, &geometry, ev.position);
                if head != anchor || !self.selections.first().is_some_and(|s| s.is_empty() && s.anchor == anchor) {
                    self.selections = vec![Selection::caret(anchor).with_head(head)];
                    cx.notify();
                }
            }
            Some(Drag::Columnar { from }) if ev.position != from => {
                self.selections = self.columnar_selections(window, &geometry, from, ev.position);
                cx.notify();
            }
            Some(Drag::ScrollV { grab }) => {
                let track_y = geometry.vscroll.track.origin.y;
                let travel = (geometry.vscroll.track.size.height - geometry.vscroll.thumb.size.height).max(px(1.0));
                let ratio = ((ev.position.y - grab - track_y) / travel).clamp(0.0, 1.0);
                self.scroll.set(ratio * self.scroll_range());
                cx.notify();
            }
            Some(Drag::ScrollH { grab }) => {
                let track_x = geometry.hscroll.track.origin.x;
                let travel = (geometry.hscroll.track.size.width - geometry.hscroll.thumb.size.width).max(px(1.0));
                let ratio = ((ev.position.x - grab - track_x) / travel).clamp(0.0, 1.0);
                self.scroll_x.set(ratio * self.max_scroll_x());
                cx.notify();
            }
            _ => self.update_hover(ev.position, window, cx),
        }
    }

    /// Parks (or clears) what the pointer rests on: a diagnostic's tooltip,
    /// else the token's LSP hover card (one debounced request per token).
    /// Only a change notifies — moves within one range re-render nothing.
    fn update_hover(&mut self, position: Point<Pixels>, window: &Window, cx: &mut gpui::Context<Self>) {
        let offset = self.geometry.get().map(|geometry| self.offset_at(window, &geometry, position));
        let next = offset.and_then(|offset| {
            let line = self.buf.line_of(offset);
            self.diagnostics
                .iter()
                .find(|d| {
                    let range = self.diag_range(d);
                    // An empty range (bare caret position) still shows on hover.
                    (range.start..range.end.max(range.start + 1)).contains(&offset)
                })
                .map(|d| HoverDiag {
                    message: d.message.clone(),
                    severity: d.severity,
                    source: d.source.clone(),
                    line,
                    range: d.range,
                })
        });
        let has_diag = next.is_some();
        // A diagnostic under the pointer also asks for its fixes once (the
        // tooltip renders them as one-click rows).
        if let Some(diag) = &next {
            let key = (diag.line, diag.message.clone());
            if self.hover_fixes.as_ref().is_none_or(|fixes| fixes.key != key) {
                self.request_hover_fixes(diag.range, key, cx);
            }
        } else if self.hover_fixes.take().is_some() {
            // nothing else to do; dropping the fixes with the hover
        }
        match (has_diag, offset) {
            (true, _) => {
                // The diagnostic tooltip wins; the LSP card retires.
                self.hover_card = None;
                self.hover_req_offset = None;
            }
            (false, Some(offset)) => {
                if self.hover_answer_stale(offset) {
                    self.schedule_lsp_hover(offset, cx);
                }
            }
            (false, None) => {}
        }
        if self.hover.replace(next.clone()) != next {
            cx.notify();
        }
    }

    /// Whether `offset` needs a hover request (nothing landed for it yet and
    /// none is in flight).
    fn hover_answer_stale(&self, offset: usize) -> bool {
        if self.hover_req_offset == Some(offset) {
            return false;
        }
        self.hover_card.as_ref().is_none_or(|card| {
            // The card anchors per line; a move within the same line keeps it.
            let line = self.buf.line_of(offset);
            card.line != line
        })
    }

    /// The hovered diagnostic's code actions (the tooltip's fix rows). The
    /// same menu shape the ⌥⌘-path uses: disabled suggestions drop, fixes
    /// sort before intentions.
    fn request_hover_fixes(&mut self, range: ide_core::Range, key: (u32, String), cx: &mut gpui::Context<Self>) {
        self.hover_fixes = None;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let lsp = self.host.lsp.clone();
        let doc = self.doc_id;
        self.host.runtime.spawn(async move {
            let result = lsp.code_actions(doc, range, Vec::new()).await;
            let _ = tx.send((key, result));
        });
        cx.spawn(async move |this, cx| {
            if let Ok((key, result)) = rx.await {
                this.update(cx, |pane, _cx| {
                    if let Ok(actions) = result {
                        let mut actions = actions;
                        actions.retain(|action| action.disabled_reason.is_none());
                        let rank = |action: &ide_lsp::types::CodeAction| match action.kind.as_deref() {
                            Some("quickfix") => 1,
                            Some(k) if k.starts_with("refactor") => 2,
                            Some(k) if k.starts_with("source") => 3,
                            _ => 4,
                        };
                        actions.sort_by_key(|action| (!action.is_preferred, rank(action)));
                        if !actions.is_empty() {
                            pane.hover_fixes = Some(HoverFixes { key, actions });
                        }
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// One-click apply from the tooltip (the same funnel the ⌥⏎ menu uses).
    fn apply_hover_fix(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        let Some(fixes) = self.hover_fixes.take() else { return };
        let Some(action) = fixes.actions.into_iter().nth(index) else { return };
        self.hover.set(None);
        self.finish_code_action(action, cx);
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(Drag::Columnar { from }) = self.drag.get()
            && ev.position == from
            && let Some(geometry) = self.geometry.get()
        {
            // Bare ⌥-click: toggle a cursor at the point (M2 behaviour).
            let offset = self.offset_at(window, &geometry, ev.position);
            let mut selections = self.selections.clone();
            if let Some(at) = selections.iter().position(|s| s.is_empty() && s.head == offset) {
                if selections.len() > 1 {
                    selections.remove(at);
                }
            } else {
                selections.push(Selection::caret(offset));
                selections = normalize_selections(selections);
            }
            self.selections = selections;
            cx.notify();
        }
        self.drag.set(None);
    }

    /// One cursor per visual row between two window points, spanning the
    /// points' x range (zero-width where both ends map to the same offset).
    fn columnar_selections(
        &self,
        window: &Window,
        geometry: &Geometry,
        from: Point<Pixels>,
        to: Point<Pixels>,
    ) -> Vec<Selection> {
        let (top, bottom) = if from.y <= to.y { (from, to) } else { (to, from) };
        let down = from.y <= to.y;
        let row0 = self.row_at(geometry, top.y);
        let row1 = self.row_at(geometry, bottom.y);
        let (x_left, x_right) = if from.x <= to.x { (from.x, to.x) } else { (to.x, from.x) };
        let mut selections = Vec::new();
        for display in row0..=row1 {
            let (line, placeholder) = self.resolve_display(display);
            if placeholder.is_some() {
                continue;
            }
            // The x-to-offset mapping goes through the hint-aware helper, so
            // a column drag over an annotated row resolves like a click.
            let scroll_x = px(geometry.scroll_x);
            let start = self.offset_in_line(window, line, x_left - geometry.origin.x + scroll_x);
            let end = self.offset_in_line(window, line, x_right - geometry.origin.x + scroll_x);
            selections.push(if down {
                Selection { anchor: start, head: end, reversed: false, goal_col: None }
            } else {
                Selection { anchor: end, head: start, reversed: true, goal_col: None }
            });
        }
        selections
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut gpui::Context<Self>) {
        let (dy_lines, dx_px) = match ev.delta {
            gpui::ScrollDelta::Pixels(point) => (point.y / theme::px_line_height(), f32::from(point.x)),
            gpui::ScrollDelta::Lines(point) => (point.y, point.x * theme::FONT_SIZE * 0.6),
        };
        if ev.modifiers.shift {
            // Shift turns a vertical wheel into horizontal motion (mice).
            self.scroll_x.set(self.scroll_x.get() - dy_lines * f32::from(theme::px_line_height()));
        } else {
            self.scroll.set(self.scroll.get() - dy_lines);
            self.scroll_x.set(self.scroll_x.get() - dx_px);
        }
        self.clamp_scroll();
        self.clamp_scroll_x();
        // A vertical scroll may have entered an uncovered range: hints for it
        // request (free when the cover still spans the view).
        self.schedule_inlays(cx);
        cx.notify();
    }

    /// Keeps the viewport inside the grid of lines the buffer has.
    fn clamp_scroll(&self) {
        let max = self.display_count().saturating_sub(1) as f32;
        self.scroll.set(self.scroll.get().clamp(0.0, max));
    }

    /// The topmost scroll value the viewport can reach (display rows minus
    /// the rows that fit) — the scrollbar's ratio maps onto exactly this.
    fn scroll_range(&self) -> f32 {
        let visible =
            self.geometry.get().map_or(20.0, |g| (g.height - px(theme::TEXT_INSET) * 2.0) / g.line_height).max(1.0);
        (self.display_count() as f32 - visible).max(0.0)
    }

    /// The farthest the viewport may shift right: the widest observed line
    /// minus the text area (plus the scrollbar's breathing room).
    fn max_scroll_x(&self) -> f32 {
        self.geometry
            .get()
            .map_or(0.0, |g| (self.max_line_w.get() - f32::from(g.width) + theme::SCROLLBAR_SIZE).max(0.0))
    }

    fn clamp_scroll_x(&self) {
        let max = self.max_scroll_x();
        self.scroll_x.set(self.scroll_x.get().clamp(0.0, max));
    }

    /// Scrolls the primary cursor's line into view.
    fn clamp_scroll_to_primary(&self) {
        let caret = self.selections.first().map_or(0, |s| s.head);
        let caret_line = self.display_of_line(self.buf.line_of(caret)) as f32;
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

    // -- display map (folding) ----------------------------------------------
    //
    // Display rows are the buffer rows with every folded range's inner rows
    // replaced by one placeholder row under its start line.

    fn display_count(&self) -> u32 {
        display_count(&self.folds, self.buf.line_count())
    }

    fn display_of_line(&self, line: u32) -> u32 {
        display_of_line(&self.folds, line)
    }

    fn resolve_display(&self, display: u32) -> (u32, Option<Fold>) {
        resolve_display(&self.folds, self.buf.line_count(), display)
    }

    /// Folds or unfolds the candidate starting at `line`.
    fn toggle_fold(&mut self, line: u32, cx: &mut gpui::Context<Self>) {
        if let Some(at) = self.folds.iter().position(|f| f.start_line == line) {
            self.folds.remove(at);
        } else if let Some((start, end)) = self.fold_candidates.iter().find(|(s, _)| *s == line) {
            // Not already hidden inside another fold (candidates may nest).
            if self.folds.iter().all(|f| line > f.end_line || line < f.start_line) {
                self.folds.push(Fold { start_line: *start, end_line: *end });
                self.folds.sort_by_key(|f| f.start_line);
            }
        } else {
            return;
        }
        cx.notify();
    }

    /// Unfolds the range the placeholder at `line` belongs to.
    fn unfold_placeholder(&mut self, line: u32, cx: &mut gpui::Context<Self>) {
        self.folds.retain(|f| f.start_line != line);
        cx.notify();
    }
}

/// The post-batch inverse of an ascending change batch: each change becomes a
/// replacement of its inserted text with the original. `strip_inserts` (an
/// IME commit) deletes instead of restoring, so undo removes the whole
/// composition.
fn inverse_changes(changes: &[TextChange], originals: &[String], strip_inserts: bool) -> Vec<TextChange> {
    let mut inverse = Vec::with_capacity(changes.len());
    let mut delta: i64 = 0;
    for (change, original) in changes.iter().zip(originals) {
        let inserted = change.insert.chars().map(char::len_utf16).sum::<usize>();
        let from = (change.from as i64 + delta).max(0) as u32;
        let insert = if strip_inserts { String::new() } else { original.clone() };
        inverse.push(TextChange { from, to: from + inserted as u32, insert });
        delta += inserted as i64 - (change.to - change.from) as i64;
    }
    inverse
}

// -- display map (folding) --------------------------------------------------
//
// Display rows are the buffer rows with every folded range's inner rows
// replaced by one placeholder row under its start line. Free functions over
// the sorted, non-overlapping fold list so the math stays unit-testable.

/// Hidden buffer rows strictly above `line`.
fn hidden_before(folds: &[Fold], line: u32) -> u32 {
    folds.iter().map(|f| f.end_line.min(line.saturating_sub(1)).saturating_sub(f.start_line)).sum()
}

/// Total display rows: visible buffer lines plus one placeholder per fold
/// (each fold keeps its start line and hides `end - start` rows).
fn display_count(folds: &[Fold], line_count: u32) -> u32 {
    line_count - hidden_before(folds, u32::MAX) + folds.len() as u32
}

/// The display row a buffer line renders on; lines hidden inside a fold
/// clamp onto that fold's placeholder row.
fn display_of_line(folds: &[Fold], line: u32) -> u32 {
    line - hidden_before(folds, line)
}

/// The buffer content of a display row: a buffer line, or (for a placeholder
/// row) the fold whose placeholder it is.
fn resolve_display(folds: &[Fold], line_count: u32, display: u32) -> (u32, Option<Fold>) {
    let mut remaining = display;
    let mut line = 0u32;
    for fold in folds {
        let visible_before = fold.start_line - line;
        if remaining < visible_before {
            return (line + remaining, None);
        }
        remaining -= visible_before;
        if remaining == 0 {
            return (fold.start_line, None); // the fold's first line renders
        }
        remaining -= 1;
        if remaining == 0 {
            return (fold.start_line, Some(*fold)); // its placeholder row
        }
        remaining -= 1;
        line = fold.end_line + 1;
    }
    let last = line_count.saturating_sub(1);
    (line.min(last) + remaining.min(last.saturating_sub(line)), None)
}

/// Clamp one selection into the document length.
fn clamp_selection(mut selection: Selection, len: usize) -> Selection {
    selection.anchor = selection.anchor.min(len);
    selection.head = selection.head.min(len);
    if selection.head < selection.anchor && !selection.reversed {
        selection.reversed = true;
    } else if selection.head > selection.anchor && selection.reversed {
        selection.reversed = false;
    }
    selection
}

/// Sorts cursors, deduplicates identical ones, and merges selections that
/// overlap (an edit that fuses two cursors leaves one).
fn normalize_selections(mut selections: Vec<Selection>) -> Vec<Selection> {
    if selections.is_empty() {
        return vec![Selection::caret(0)];
    }
    selections.sort_by_key(|s| {
        let (start, end) = s.normalized();
        (start, end)
    });
    let mut out: Vec<Selection> = Vec::with_capacity(selections.len());
    for selection in selections {
        let overlapping = match out.last() {
            Some(last) => {
                let (_, last_end) = last.normalized();
                let (start, _) = selection.normalized();
                start < last_end
            }
            None => false,
        };
        if overlapping {
            // Overlapping: keep the union as a plain selection. `overlapping`
            // is only reachable with `out` non-empty (its guard reads `last`).
            if let Some(last) = out.last_mut() {
                let (last_start, _) = last.normalized();
                let (_, end) = selection.normalized();
                *last = Selection { anchor: last_start, head: end, reversed: false, goal_col: None };
            }
        } else if out.last() != Some(&selection) {
            out.push(selection);
        }
    }
    out
}

/// The replacement ranges a text insertion covers: every selection's
/// normalized range (a bare caret is an empty insert range — the text lands
/// there), sorted and deduplicated. Typing, newline and paste all replace
/// these ranges in one batch.
fn paste_ranges(selections: &[Selection]) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = selections.iter().map(|s| s.normalized()).collect();
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

/// Server `TextEdit`s (IDE positions, UTF-16) as ascending non-overlapping
/// buffer changes — the shape one `apply_edits` batch needs (formatting).
fn text_edits_to_changes(edits: &[ide_core::TextEdit], buf: &Buffer) -> Vec<TextChange> {
    let mut changes: Vec<TextChange> = edits
        .iter()
        .map(|edit| {
            let from = buf.line_start(edit.range.start.line).min(buf.utf16_len()) + edit.range.start.col as usize;
            let to = buf.line_start(edit.range.end.line).min(buf.utf16_len()) + edit.range.end.col as usize;
            TextChange { from: from.min(to) as u32, to: to.max(from) as u32, insert: edit.new_text.clone() }
        })
        .filter(|change| change.from < change.to || !change.insert.is_empty())
        .collect();
    changes.sort_by_key(|change| (change.from, change.to));
    changes
}

/// Fetches the server's `source.organizeImports` action over `range` (⌘S's
/// pre-save step and ⌃⌥O): the first action of that kind, resolved when the
/// server deferred its edit. `None` when the server offers none (nothing to
/// organize, or not a language it serves).
pub async fn organize_imports_action(
    lsp: &ide_lsp::LspManager,
    doc: DocId,
    range: ide_core::Range,
) -> IdeResult<Option<ide_lsp::types::CodeAction>> {
    const SOURCE_ORGANIZE: &str = "source.organizeImports";
    let mut actions = lsp.code_actions(doc, range, vec![SOURCE_ORGANIZE.to_owned()]).await?;
    actions.retain(|action| action.kind.as_deref() == Some(SOURCE_ORGANIZE) && action.disabled_reason.is_none());
    let Some(action) = actions.into_iter().next() else { return Ok(None) };
    if action.edit.is_some() {
        return Ok(Some(action));
    }
    Ok(Some(lsp.resolve_code_action(doc, action).await?))
}

/// Shifts one UTF-16 range through an ascending change batch (pre-batch
/// coordinates, the same convention `after_edits` uses for the syntax tree);
/// a batch that ran through the range drops it.
fn map_stop_range(range: (usize, usize), changes: &[TextChange]) -> Option<(usize, usize)> {
    let mut range = range;
    for change in changes {
        let (from, to) = (change.from as usize, change.to as usize);
        if range.1 <= from {
            continue;
        }
        if range.0 >= to {
            let shift = change.insert.chars().map(char::len_utf16).sum::<usize>() as i64 - (to - from) as i64;
            range.0 = (range.0 as i64 + shift).max(0) as usize;
            range.1 = (range.1 as i64 + shift).max(0) as usize;
        } else {
            return None;
        }
    }
    Some(range)
}

/// Joins a row's inlay hints into its shaped text (M6): the runs (byte-length
/// segments over `line`) split at each hint position and the padded labels
/// interleave as their own runs — the muted kind tone on a subtle chip, so a
/// hint reads as meta, not code. Returns the augmented string and runs; the
/// widths of the inserted labels are what every x past a position shifts by
/// (see [`EditorPane::hint_shift`]).
fn augment_with_hints(line: &str, hints: &[RowHint], runs: Vec<TextRun>) -> (String, Vec<TextRun>) {
    let mut out = String::new();
    let mut out_runs: Vec<TextRun> = Vec::new();
    let mut queue: std::collections::VecDeque<TextRun> = runs.into_iter().collect();
    let mut emitted = 0usize; // bytes of the original line already in `out`
    for hint in hints {
        let at = byte_at_utf16(line, hint.at);
        // The original text before the hint, splitting the boundary run.
        let mut have = emitted;
        while have < at {
            let Some(mut run) = queue.pop_front() else { break };
            let run_end = have + run.len;
            if run_end <= at {
                out.push_str(&line[have..run_end]);
                out_runs.push(run);
                have = run_end;
            } else {
                let head = at - have;
                let tail = TextRun {
                    len: run.len - head,
                    font: run.font.clone(),
                    color: run.color,
                    background_color: run.background_color,
                    underline: run.underline,
                    strikethrough: run.strikethrough,
                };
                run.len = head;
                out.push_str(&line[have..at]);
                out_runs.push(run);
                queue.push_front(tail);
                have = at;
            }
        }
        emitted = have;
        // The label itself.
        let color = match hint.kind {
            ide_lsp::types::InlayKind::Type => theme::hint_text(),
            _ => theme::fg_muted(),
        };
        out.push_str(&hint.label);
        out_runs.push(TextRun {
            len: hint.label.len(),
            font: theme::font(),
            color,
            background_color: Some(rgba_of(theme::BG_RAISED).into()),
            underline: None,
            strikethrough: None,
        });
    }
    // Whatever the hints left behind.
    while let Some(mut run) = queue.pop_front() {
        let len = run.len;
        let end = (emitted + len).min(line.len());
        if emitted < end {
            out.push_str(&line[emitted..end]);
            run.len = end - emitted;
            out_runs.push(run);
        }
        emitted += len;
    }
    (out, out_runs)
}

impl Render for EditorPane {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if !self.focused_once.get() {
            self.focused_once.set(true);
            window.focus(&self.focus, cx);
        }
        let focus = self.focus.clone();
        // Every editor behaviour is an action (bindings in `main.rs`); the
        // handlers funnel through `command`. Verbose on purpose: gpui's
        // `on_action` is typed per action, and macros cannot sit mid-chain.
        gpui::div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context("Editor")
            .bg(theme::surface_bg())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_key_down(cx.listener(Self::on_key))
            .on_action(
                cx.listener(|this, _: &acts::TogglePerfHud, window, cx| this.command(Command::ToggleHud, window, cx)),
            )
            .on_action(cx.listener(|this, _: &acts::Cancel, window, cx| this.command(Command::Cancel, window, cx)))
            .on_action(cx.listener(|this, _: &acts::Newline, window, cx| this.command(Command::Newline, window, cx)))
            .on_action(cx.listener(|this, _: &acts::TabIndent, window, cx| this.command(Command::Tab, window, cx)))
            .on_action(
                cx.listener(|this, _: &acts::OutdentLine, window, cx| this.command(Command::Outdent, window, cx)),
            )
            .on_action(cx.listener(|this, _: &acts::Paste, window, cx| this.command(Command::Paste, window, cx)))
            .on_action(cx.listener(|this, _: &acts::DeleteBackward, window, cx| {
                this.command(Command::Delete { forward: false, word: false }, window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::DeleteForward, window, cx| {
                this.command(Command::Delete { forward: true, word: false }, window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::DeleteWordBackward, window, cx| {
                this.command(Command::Delete { forward: false, word: true }, window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::DeleteWordForward, window, cx| {
                this.command(Command::Delete { forward: true, word: true }, window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorLeft, window, cx| {
                this.command(Command::Move(Motion::Left, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorRight, window, cx| {
                this.command(Command::Move(Motion::Right, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorUp, window, cx| {
                this.command(Command::Move(Motion::Up, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorDown, window, cx| {
                this.command(Command::Move(Motion::Down, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorWordLeft, window, cx| {
                this.command(Command::Move(Motion::WordLeft, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorWordRight, window, cx| {
                this.command(Command::Move(Motion::WordRight, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorLineHome, window, cx| {
                this.command(Command::Move(Motion::LineHome, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorLineEnd, window, cx| {
                this.command(Command::Move(Motion::LineEnd, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorDocStart, window, cx| {
                this.command(Command::Move(Motion::DocStart, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorDocEnd, window, cx| {
                this.command(Command::Move(Motion::DocEnd, false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectLeft, window, cx| {
                this.command(Command::Move(Motion::Left, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectRight, window, cx| {
                this.command(Command::Move(Motion::Right, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectUp, window, cx| {
                this.command(Command::Move(Motion::Up, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectDown, window, cx| {
                this.command(Command::Move(Motion::Down, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectWordLeft, window, cx| {
                this.command(Command::Move(Motion::WordLeft, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectWordRight, window, cx| {
                this.command(Command::Move(Motion::WordRight, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectLineHome, window, cx| {
                this.command(Command::Move(Motion::LineHome, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectLineEnd, window, cx| {
                this.command(Command::Move(Motion::LineEnd, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectDocStart, window, cx| {
                this.command(Command::Move(Motion::DocStart, true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectDocEnd, window, cx| {
                this.command(Command::Move(Motion::DocEnd, true), window, cx)
            }))
            // Hover tooltip: a plain-text bubble under the hovered diagnostic.
            .children(self.render_hover_tooltip(cx))
            // LSP hover card (markdown or plain) under the pointer's token.
            .children(self.render_hover_card(cx))
            // Signature help popup above the primary caret.
            .children(self.render_signature(cx))
            // Completion popup: the filtered item list at the caret.
            .children(self.render_completion_popup(window, cx))
            // Jump list: definitions / usages across files.
            .children(self.render_jump_list(cx))
            // Quick-fix menu (⌥⏎).
            .children(self.render_code_actions(cx))
            // Find bar (⌘F) with its replace row.
            .children(self.render_find_bar(window, cx))
            // Rename: input overlay, then the workspace-edit preview.
            .children(self.render_rename(window, cx))
            .on_action(cx.listener(|this, _: &acts::GotoDefinition, _, cx| this.goto_definition(cx)))
            .on_action(cx.listener(|this, _: &acts::FindUsages, _, cx| this.find_usages(cx)))
            .on_action(cx.listener(|this, _: &acts::Rename, window, cx| this.start_rename(window, cx)))
            .on_action(cx.listener(|this, _: &acts::CodeActions, _, cx| this.request_code_actions(cx)))
            .on_action(cx.listener(|this, _: &acts::FormatDocument, _, cx| this.format_document(cx)))
            .on_action(cx.listener(|this, _: &acts::OrganizeImports, _, cx| this.organize_imports(cx)))
            .on_action(cx.listener(|this, _: &acts::ToggleInlayHints, _, cx| this.toggle_inlays(cx)))
            .on_action(cx.listener(|this, _: &acts::Find, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &acts::ToggleReplace, _, cx| this.find_toggle_replace(cx)))
            // The right-click menu (M7): the existing actions as entries,
            // dispatched into this pane's own handlers through its focus —
            // one source of truth, no second implementation.
            .context_menu(move |menu, _, _| {
                menu.action_context(focus.clone())
                    .menu("Go to Definition", Box::new(acts::GotoDefinition))
                    .menu("Find Usages", Box::new(acts::FindUsages))
                    .menu("Rename…", Box::new(acts::Rename))
                    .separator()
                    .menu("Quick Fix…", Box::new(acts::CodeActions))
                    .menu("Format Document", Box::new(acts::FormatDocument))
                    .menu("Organize Imports", Box::new(acts::OrganizeImports))
            })
            // Navigation/refactoring handlers arrive in later M3 phases.
            .child(EditorElement { view: cx.entity() })
    }
}

impl EditorPane {
    /// The jump list (definitions / usages): `rel_path:line` rows from the
    /// last frame's geometry, centered like a command palette.
    fn render_jump_list(&self, _cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let (locations, selected) = self.jump.as_ref()?;
        let geometry = self.geometry.get()?;
        let x = geometry.origin.x + px(80.0);
        let y = geometry.origin.y + px(40.0);
        let mut list = gpui::div().flex().flex_col();
        for (index, location) in locations.iter().enumerate().take(14) {
            let is_selected = index == *selected;
            let rel = location.path.0.strip_prefix(&self.host.root).unwrap_or(location.path.0.as_path());
            let mut row = gpui::div().flex().items_center().gap_2().px_2().h(px(22.0)).text_size(px(12.)).child(
                gpui::div().text_color(theme::accent()).child(SharedString::from(format!(
                    "{}:{}",
                    rel.display(),
                    location.range.start.line + 1
                ))),
            );
            if is_selected {
                row = row.bg(theme::selected_bg());
            }
            list = list.child(row);
        }
        Some(
            gpui::div()
                .absolute()
                .left(x)
                .top(y)
                .w(px(460.0))
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised_bg())
                .py_1()
                .child(list)
                .into_any_element(),
        )
    }

    /// The rename overlays: an input box over the caret (Input) or a
    /// workspace-edit preview with Apply / Cancel (Preview).
    /// The find bar (⌘F): the query row (input, count, case toggle,
    /// previous/next, replace toggle, close) plus the replace row behind the
    /// toggle. Parked under the editor's top edge, right-aligned; clicks
    /// inside stop propagation so they never place a caret.
    fn render_find_bar(&self, window: &mut Window, cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let bar = self.find_bar.as_ref()?;
        let case = self.find_hits.as_ref().is_some_and(|hits| hits.case_sensitive);
        // The input keeps focus while the overlay is up.
        let handle = bar.query.read(cx).presentation().focus_handle().clone();
        if window.focused(cx).is_none_or(|f| f != handle) {
            window.focus(&handle, cx);
        }
        let case_button = {
            let button = gpui_component::button::Button::new("find-case").label(strings::FIND_CASE).xsmall().ghost();
            if case { button.text_color(theme::accent()) } else { button }
        };
        let query_row = gpui::div()
            .flex()
            .items_center()
            .gap_1()
            .child(gpui::div().w(px(220.0)).child(Input::new(&bar.query)))
            .child(
                gpui::div()
                    .min_w(px(84.0))
                    .max_w(px(140.0))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(theme::fg_muted())
                    .child(SharedString::from(self.find_count_label())),
            )
            .child(case_button.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.find_toggle_case(cx))))
            .child(
                gpui_component::button::Button::new("find-prev")
                    .icon(IconName::ChevronUp)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.find_step(-1, cx))),
            )
            .child(
                gpui_component::button::Button::new("find-next")
                    .icon(IconName::ChevronDown)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.find_step(1, cx))),
            )
            .child(
                gpui_component::button::Button::new("find-toggle-replace")
                    .icon(IconName::Replace)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.find_toggle_replace(cx))),
            )
            .child(
                gpui_component::button::Button::new("find-close")
                    .icon(IconName::Close)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_find_bar(window, cx))),
            );
        let mut body = gpui::div().flex().flex_col().gap_1().child(query_row);
        if bar.replace_visible {
            body = body.child(
                gpui::div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(gpui::div().w(px(220.0)).child(Input::new(&bar.replace)))
                    .child(
                        gpui_component::button::Button::new("find-replace")
                            .label(strings::REPLACE_BUTTON)
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.find_replace_one(cx))),
                    )
                    .child(
                        gpui_component::button::Button::new("find-replace-all")
                            .label(strings::REPLACE_ALL_BUTTON)
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.find_replace_all(cx))),
                    ),
            );
        }
        Some(
            gpui::div()
                .absolute()
                .top(px(theme::TEXT_INSET))
                .right(px(theme::SCROLLBAR_SIZE + 12.0))
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised_bg())
                .p_2()
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _: &mut Window, cx: &mut gpui::App| {
                    cx.stop_propagation()
                })
                .child(body)
                .into_any_element(),
        )
    }

    fn render_rename(&self, window: &mut Window, cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let geometry = self.geometry.get()?;
        match &self.rename {
            Some(RenameFlow::Input { input, .. }) => {
                // The input keeps focus while the overlay is up.
                let handle = input.read(cx).presentation().focus_handle().clone();
                if window.focused(cx).is_none_or(|f| f != handle) {
                    window.focus(&handle, cx);
                }
                Some(
                    gpui::div()
                        .absolute()
                        .left(geometry.origin.x + px(80.0))
                        .top(geometry.origin.y + px(60.0))
                        .w(px(320.0))
                        .border_1()
                        .border_color(theme::accent())
                        .bg(theme::raised_bg())
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            gpui::div()
                                .text_size(px(11.))
                                .text_color(theme::fg_muted())
                                .child("Rename — Enter to continue, Esc to cancel"),
                        )
                        .child(Input::new(input))
                        .into_any_element(),
                )
            }
            Some(RenameFlow::Preview { edit, name }) => {
                let mut rows = gpui::div().flex().flex_col().gap_1();
                let apply = cx.listener(|this, _, window, cx| this.rename_apply(window, cx));
                let cancel = cx.listener(|this, _, window, cx| this.rename_cancel_flow(window, cx));
                let mut total = 0usize;
                for file in &edit.files {
                    if let ide_core::FileEdit::Edit { path, edits, .. } = file {
                        total += edits.len();
                        let rel = path.0.strip_prefix(&self.host.root).unwrap_or(path.0.as_path());
                        rows = rows.child(
                            gpui::div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(12.))
                                .child(
                                    gpui::div()
                                        .text_color(theme::fg())
                                        .child(SharedString::from(rel.display().to_string())),
                                )
                                .child(
                                    gpui::div()
                                        .text_color(theme::fg_muted())
                                        .child(SharedString::from(format!("{} edits", edits.len()))),
                                ),
                        );
                    }
                }
                Some(
                    gpui::div()
                        .absolute()
                        .left(geometry.origin.x + px(80.0))
                        .top(geometry.origin.y + px(60.0))
                        .w(px(520.0))
                        .border_1()
                        .border_color(theme::accent())
                        .bg(theme::raised_bg())
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(gpui::div().text_size(px(12.)).text_color(theme::fg()).child(SharedString::from(
                            format!("Rename to `{name}` — {total} edits in {} files", edit.files.len()),
                        )))
                        .child(rows.max_h(px(260.0)).overflow_hidden())
                        .child(
                            gpui::div()
                                .flex()
                                .gap_2()
                                .child(
                                    gpui_component::button::Button::new("rename-apply")
                                        .label("Apply")
                                        .primary()
                                        .on_click(apply),
                                )
                                .child(
                                    gpui_component::button::Button::new("rename-cancel")
                                        .label("Cancel")
                                        .on_click(cancel),
                                ),
                        )
                        .into_any_element(),
                )
            }
            None => None,
        }
    }
    /// The quick-fix menu (⌥⏎): the server's actions for the caret, parked
    /// near the top of the text area like the jump list. ↑/↓ move, Enter
    /// applies, Esc closes (all through [`EditorPane::command`]).
    fn render_code_actions(&self, _cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let (actions, selected) = self.code_actions.as_ref()?;
        let geometry = self.geometry.get()?;
        let x = geometry.origin.x + px(80.0) - px(geometry.scroll_x);
        let y = geometry.origin.y + px(40.0);
        let mut list = gpui::div().flex().flex_col();
        for (index, action) in actions.iter().enumerate().take(14) {
            let is_selected = index == *selected;
            let mut row = gpui::div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(22.0))
                .text_size(px(12.))
                .child(gpui::div().text_color(theme::fg()).child(SharedString::from(action.title.clone())));
            if let Some(kind) = &action.kind {
                row = row.child(
                    gpui::div()
                        .text_color(theme::fg_muted())
                        .text_size(px(10.))
                        .child(SharedString::from(kind.clone())),
                );
            }
            if action.is_preferred {
                row = row.child(
                    gpui::div().text_color(theme::accent()).text_size(px(10.)).child(strings::CODE_ACTION_PREFERRED),
                );
            }
            if is_selected {
                row = row.bg(theme::selected_bg());
            }
            list = list.child(row);
        }
        Some(
            gpui::div()
                .absolute()
                .left(x)
                .top(y)
                .w(px(520.0))
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised_bg())
                .py_1()
                .child(list)
                .into_any_element(),
        )
    }

    /// The completion popup, anchored at the primary caret from the last
    /// frame's geometry. The typed word filters server order; ↑/↓ move, Enter
    /// accepts, Esc closes (all through [`EditorPane::command`]).
    fn render_completion_popup(&self, window: &mut Window, _cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let open = self.completion.as_ref()?;
        let geometry = self.geometry.get()?;
        let caret = self.selections.first().map_or(0, |s| s.head);
        let caret_line = self.buf.line_of(caret);
        let span = self.buf.line_span(caret_line);
        let line_text = self.buf.slice_utf16(span.clone());
        let x = x_at_utf16(window, &line_text, caret - span.start, theme::px_font_size())
            + px(self.hint_shift(caret_line, caret - span.start));
        let row = self.display_of_line(caret_line) as f32 - geometry.scroll;
        let y = geometry.origin.y + geometry.line_height * (row + 1.0) + px(2.0);

        let filter = self.completion_filter();
        let visible = open.filtered(&filter);
        if visible.is_empty() {
            return None;
        }
        let selected = open.selected.min(visible.len() - 1);
        // A sliding window of rows around the selection, at most ten.
        const MAX_ROWS: usize = 10;
        let first = selected.saturating_sub(MAX_ROWS / 2).min(visible.len().saturating_sub(MAX_ROWS));
        let rows = &visible[first..(first + MAX_ROWS).min(visible.len())];

        let mut list = gpui::div().flex().flex_col();
        for (index, item) in rows.iter().enumerate() {
            let is_selected = first + index == selected;
            let mut row = gpui::div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(22.0))
                .text_size(px(12.))
                .child(
                    gpui::div()
                        .w(px(16.0))
                        .text_size(px(10.))
                        .text_color(theme::accent())
                        .child(CompletionPopup::kind_badge(item.kind)),
                )
                .child(
                    gpui::div()
                        .text_color(if is_selected { theme::fg() } else { theme::fg_muted() })
                        .child(SharedString::from(item.label.clone())),
                );
            if let Some(detail) = &item.label_detail {
                row = row.child(gpui::div().text_color(theme::fg_muted()).child(SharedString::from(detail.clone())));
            }
            if let Some(detail) = &item.detail {
                row = row.child(
                    gpui::div()
                        .flex_1()
                        .text_right()
                        .truncate()
                        .text_color(theme::fg_muted())
                        .child(SharedString::from(detail.clone())),
                );
            }
            if is_selected {
                row = row.bg(theme::selected_bg());
            }
            list = list.child(row);
        }
        Some(
            gpui::div()
                .absolute()
                .left(geometry.origin.x + x - px(geometry.scroll_x))
                .top(y)
                .w(px(420.0))
                .max_h(px((MAX_ROWS as f32) * 22.0 + 8.0))
                .overflow_hidden()
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised_bg())
                .py_1()
                .child(list)
                .into_any_element(),
        )
    }

    /// The hover tooltip for the diagnostic under the pointer, positioned
    /// under its line from the last frame's geometry.
    /// The signature help popup (M5 phase 4): the active signature with its
    /// active parameter accented, other signatures dimmed below, anchored
    /// above the primary caret (the completion popup's geometry math).
    fn render_signature(&self, _cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let help = self.signature.as_ref()?;
        let geometry = self.geometry.get()?;
        let caret = self.selections.first().map_or(0, |s| s.head);
        let caret_row = self.display_of_line(self.buf.line_of(caret)) as f32 - geometry.scroll;
        let x = geometry.origin.x + px(24.0) - px(geometry.scroll_x);
        // Three rows of content parked above the caret line (clamped at the top).
        let rows = help.signatures.len().min(4) as f32 + 0.6;
        let y = (geometry.origin.y + geometry.line_height * (caret_row - rows - 0.6)).max(geometry.origin.y + px(2.0));

        let active = help.active_signature as usize;
        let mut list = gpui::div().flex().flex_col().gap_1();
        for (index, sig) in help.signatures.iter().enumerate().take(4) {
            let is_active = index == active;
            let param_range = if is_active { help.active_parameter } else { u32::MAX };
            list = list.child(render_signature_label(sig, param_range, is_active));
        }
        Some(
            gpui::div()
                .absolute()
                .left(x)
                .top(y)
                .max_w(px(560.0))
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised_bg())
                .px_2()
                .py_1()
                .text_size(px(12.))
                .child(list)
                .into_any_element(),
        )
    }

    /// The LSP hover card (M5 phase 4): markdown (or plain) under the
    /// pointer's token line. Follows the diagnostic tooltip's anchor math.
    fn render_hover_card(&self, _cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let card = self.hover_card.as_ref()?;
        // A diagnostic tooltip at the same spot wins (it is recomputed first).
        let diag = self.hover.replace(None);
        let has_diag = diag.is_some();
        self.hover.set(diag);
        if has_diag {
            return None;
        }
        let geometry = self.geometry.get()?;
        let row = self.display_of_line(card.line);
        let y = geometry.origin.y + geometry.line_height * ((row as f32 - geometry.scroll) + 1.0) + px(4.0);
        let x = geometry.origin.x + px(24.0) - px(geometry.scroll_x);
        let mut body = gpui::div().flex().flex_col().gap_1();
        if card.markdown {
            for block in crate::markdown::block(&card.contents) {
                body = body.child(render_markdown_block(block));
            }
        } else {
            body = body.child(gpui::div().text_color(theme::fg()).child(SharedString::from(card.contents.clone())));
        }
        Some(
            gpui::div()
                .absolute()
                .left(x)
                .top(y)
                .max_w(px(560.0))
                .max_h(px(320.0))
                .overflow_hidden()
                .border_1()
                .border_color(theme::border())
                .bg(theme::raised_bg())
                .px_2()
                .py_1()
                .text_size(px(12.))
                .child(body)
                .into_any_element(),
        )
    }

    fn render_hover_tooltip(&self, cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let hover = self.hover.replace(None);
        self.hover.set(hover.clone());
        let hover = hover?;
        let geometry = self.geometry.get()?;
        let row = self.display_of_line(hover.line);
        let y = geometry.origin.y + geometry.line_height * ((row as f32 - geometry.scroll) + 1.0) + px(4.0);
        let x = geometry.origin.x + px(24.0);
        // The classification line: producer + severity, the gutter mark's
        // classes made legible (syntax vs compiler vs an analyzer).
        let mut lines = gpui::div().flex().flex_col();
        let mut head = gpui::div().text_color(theme::fg()).child(SharedString::from(hover.message.clone()));
        if let Some(source) = &hover.source {
            head = head.child(
                gpui::div()
                    .mt_0p5()
                    .text_size(px(10.))
                    .text_color(theme::severity_color(hover.severity))
                    .child(SharedString::from(format!("{source} · {}", severity_word(hover.severity)))),
            );
        }
        lines = lines.child(head);
        // The one-click fixes, when the server answered for this diagnostic.
        let key = (hover.line, hover.message.clone());
        if let Some(fixes) = self.hover_fixes.as_ref()
            && fixes.key == key
        {
            for (index, action) in fixes.actions.iter().enumerate().take(6) {
                if action.kind.as_deref() != Some("quickfix") {
                    continue;
                }
                let selected = index;
                lines = lines.child(
                    gpui::div()
                        .id(("hover-fix", index))
                        .mt_0p5()
                        .px_1()
                        .py_0p5()
                        .rounded(px(3.0))
                        .text_color(theme::accent())
                        .hover(|row| row.bg(theme::selected_bg()))
                        .child(SharedString::from(format!("fix: {}", action.title)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            this.apply_hover_fix(selected, cx);
                        })),
                );
            }
        }
        Some(
            gpui::div()
                .absolute()
                .left(x)
                .top(y)
                .max_w(px(480.0))
                .border_1()
                .border_color(theme::severity_color(hover.severity))
                .bg(theme::raised_bg())
                .px_2()
                .py_1()
                .text_size(px(12.))
                .child(lines)
                .into_any_element(),
        )
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
    /// The laid-out display rows: buffer lines with folded inner rows
    /// replaced by placeholder rows. One row of slack above and below the
    /// viewport, so slow scrolls never show a gap.
    rows: Vec<DisplayedRow>,
    /// One caret per selection head.
    carets: Vec<Bounds<Pixels>>,
    /// Selection rectangles for the visible rows (painted under the text).
    selection_rects: Vec<Bounds<Pixels>>,
    /// Find match rectangles for the visible rows (painted under the
    /// selections), each with its "current" flag (stronger tint).
    find_rects: Vec<(Bounds<Pixels>, bool)>,
    /// Gutter chevron rows: (display row, already folded).
    chevrons: Vec<(u32, bool)>,
    /// Gutter diagnostic marks: (display row, strongest severity on the line).
    diag_marks: Vec<(u32, Severity)>,
    /// The primary cursor's buffer line, for the brighter gutter number.
    caret_line: u32,
    gutter_w: Pixels,
    numbers_w: Pixels,
}

struct DisplayedRow {
    /// The display row index (the paint position).
    display: u32,
    /// The buffer line, `None` for a fold placeholder row.
    line: Option<u32>,
    shaped: ShapedLine,
}

fn text_run(range: Range<usize>, color: gpui::Hsla) -> TextRun {
    TextRun {
        len: range.len(),
        font: theme::font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// The text runs for one visible line. Syntax spans (byte ranges of the whole
/// document) are sliced onto the line; the marked range's intersection keeps
/// the M0 composition style (tinted background + accent underline) and
/// overrides highlighting on its line.
fn line_runs(
    line: &str,
    span: Range<usize>,
    marked: Option<Range<usize>>,
    line_start_byte: usize,
    spans: &[(Range<usize>, Kind)],
    diags: &[(Range<usize>, Severity)],
    fg: gpui::Hsla,
) -> Vec<TextRun> {
    let len = line.len();
    if let Some(marked) = marked
        && marked.start < span.end
        && marked.end > span.start
    {
        let mut runs = vec![plain_run(len, fg)];
        let relative =
            marked.start.clamp(span.start, span.end) - span.start..marked.end.clamp(span.start, span.end) - span.start;
        let start = byte_at_utf16(line, relative.start);
        let end = byte_at_utf16(line, relative.end);
        if end > start {
            runs[0] = TextRun {
                len: start,
                font: theme::font(),
                color: fg,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            runs.push(TextRun {
                len: end - start,
                font: theme::font(),
                color: fg,
                background_color: Some(rgba_of(theme::MARKED_BG).into()),
                underline: Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(rgba_of(theme::ACCENT).into()),
                    wavy: false,
                }),
                strikethrough: None,
            });
            if end < len {
                runs.push(plain_run(len - end, fg));
            }
        }
        return runs;
    }

    // Per-byte underline severity: the strongest diagnostic covering the byte.
    let mut underline = vec![None::<Severity>; len];
    for (range, severity) in diags {
        let start = byte_at_utf16(line, range.start);
        let end = byte_at_utf16(line, range.end).max(start).min(len);
        for slot in underline[start..end].iter_mut() {
            if severity_rank(*severity) > slot.map(severity_rank).unwrap_or(0) {
                *slot = Some(*severity);
            }
        }
    }

    // Highlight runs (syntax spans sliced onto this line's bytes), split at
    // every underline change so a squiggle can ride the syntax segments.
    let mut segments: Vec<(usize, usize, gpui::Hsla)> = Vec::new();
    let mut cursor = 0usize;
    let idx = spans.partition_point(|s| s.0.end <= line_start_byte);
    for (range, kind) in &spans[idx..] {
        if range.start >= line_start_byte + len {
            break;
        }
        let start = range.start.saturating_sub(line_start_byte).min(len);
        let end = range.end.saturating_sub(line_start_byte).min(len);
        if start > cursor {
            segments.push((cursor, start, fg));
        }
        if end > start {
            segments.push((start, end, theme::syntax_color(*kind)));
            cursor = end;
        }
    }
    if cursor < len {
        segments.push((cursor, len, fg));
    }
    let mut runs: Vec<TextRun> = Vec::new();
    for (start, end, color) in segments {
        // Split where the underline severity changes; the segments already
        // ascend without gaps, so runs stay contiguous.
        let mut at = start;
        while at < end {
            let severity = underline[at];
            let mut stop = at + 1;
            while stop < end && underline[stop] == severity {
                stop += 1;
            }
            let mut run = text_run(at..stop, color);
            if let Some(severity) = severity {
                run.underline = Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(theme::severity_color(severity)),
                    wavy: true,
                });
            }
            runs.push(run);
            at = stop;
        }
    }
    if runs.is_empty() {
        runs.push(plain_run(len, fg));
    }
    runs
}

/// One signature label with the active parameter accented (UTF-16 range into
/// the label); inactive signatures paint muted.
fn render_signature_label(sig: &ide_lsp::types::Signature, active_parameter: u32, is_active: bool) -> gpui::Div {
    let label = sig.label.as_str();
    let color = if is_active { theme::fg() } else { theme::fg_muted() };
    let param = if is_active { sig.parameters.get(active_parameter as usize).copied() } else { None };
    let mut row = gpui::div().flex().flex_wrap();
    let segments: Vec<(String, bool)> = match param {
        Some((start, end)) => {
            let s = byte_at_utf16(label, start as usize);
            let e = byte_at_utf16(label, end.max(start) as usize).max(s);
            vec![
                (label[..s].to_owned(), false),
                (label[s..e].to_owned(), true),
                (label[e.min(label.len())..].to_owned(), false),
            ]
        }
        None => vec![(label.to_owned(), false)],
    };
    for (text, highlighted) in segments {
        if text.is_empty() {
            continue;
        }
        let span = if highlighted {
            gpui::div().font_weight(gpui::FontWeight::MEDIUM).text_color(theme::accent())
        } else {
            gpui::div().text_color(color)
        };
        row = row.child(span.child(SharedString::from(text)));
    }
    row
}

/// The markdown block renderer's gpui face: fenced code rows on the app
/// background, headings medium, list items bulleted, inline `code` in the
/// accent tone and **bold** as medium runs.
fn render_markdown_block(block: crate::markdown::Block) -> gpui::Div {
    match block {
        crate::markdown::Block::Code(lines) => gpui::div()
            .flex()
            .flex_col()
            .bg(theme::app_bg())
            .px_2()
            .py_1()
            .text_color(theme::fg())
            .font_family(theme::FONT_FAMILY)
            .text_size(px(11.))
            .children(lines.into_iter().map(|line| gpui::div().child(SharedString::from(line)))),
        crate::markdown::Block::Heading(level, text) => {
            let size = if level <= 1 { px(14.0) } else { px(13.0) };
            gpui::div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme::fg())
                .text_size(size)
                .child(render_inline(crate::markdown::inline(&text)))
        }
        crate::markdown::Block::ListItem(text) => gpui::div()
            .flex()
            .gap_1()
            .child(gpui::div().text_color(theme::fg_muted()).child("•"))
            .child(render_inline(crate::markdown::inline(&text))),
        crate::markdown::Block::Paragraph(text) => {
            gpui::div().text_color(theme::fg()).child(render_inline(crate::markdown::inline(&text)))
        }
    }
}

/// Inline markdown runs as one wrapped row of spans.
fn render_inline(runs: Vec<crate::markdown::Inline>) -> gpui::AnyElement {
    let mut row = gpui::div().flex().flex_wrap();
    for run in runs {
        let span = match run {
            crate::markdown::Inline::Plain(text) => gpui::div().text_color(theme::fg()).child(SharedString::from(text)),
            crate::markdown::Inline::Code(text) => {
                gpui::div().text_color(theme::accent()).font_family(theme::FONT_FAMILY).child(SharedString::from(text))
            }
            crate::markdown::Inline::Bold(text) => gpui::div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme::fg())
                .child(SharedString::from(text)),
        };
        row = row.child(span);
    }
    row.into_any_element()
}

/// Error squiggles win over warnings, warnings over info hints.
/// The tooltip's word for a severity ("compiler · error").
fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 3,
        Severity::Warning => 2,
        Severity::Info | Severity::Hint => 1,
    }
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
        let caret_buffer_line = pane.selections.first().map_or(0, |s| pane.buf.line_of(s.head));
        let scroll = pane.scroll.get();

        // Gutter geometry: the widest line number sets the width (at least
        // three digits, so small files do not look cramped).
        let digits = pane.buf.line_count().to_string().len().max(3);
        let numbers_w = shaped_width(window, &"8".repeat(digits), font_size);
        let gutter_w = px(theme::GUTTER_PAD_L) + numbers_w + px(theme::GUTTER_PAD_R);
        let gutter_x = bounds.origin.x;
        let origin = point(gutter_x + gutter_w, bounds.origin.y + px(theme::TEXT_INSET));
        let text_area_w = bounds.size.width - gutter_w;

        // Only the visible display rows are laid out, with one row of slack
        // above and below; a 50 MB file costs the same as a small one.
        let display_count = pane.display_count();
        let first_visible = (scroll.floor().max(0.0) as u32).min(display_count.saturating_sub(1));
        let visible = ((bounds.size.height - px(theme::TEXT_INSET) * 2.0) / line_height).ceil() as u32;
        let first = first_visible.saturating_sub(1);
        let last = (first_visible + visible + 1).min(display_count);

        let fg = theme::fg();
        let muted = theme::fg_muted();
        let mut rows = Vec::with_capacity((last - first) as usize);
        let mut chevrons = Vec::new();
        let mut diag_marks = Vec::new();
        let mut frame_max_w = px(0.0);
        let mut hinted_rows = 0u32;
        pane.hints_by_line.borrow_mut().clear();
        for display in first..last {
            let (line, placeholder) = pane.resolve_display(display);
            let shaped = match placeholder {
                Some(fold) => {
                    let label = format!("⋯ {} lines", fold.end_line - fold.start_line);
                    let run = plain_run(label.len(), muted);
                    window.text_system().shape_line(label.into(), font_size, &[run], None)
                }
                None => {
                    let span = pane.buf.line_span(line);
                    let content = pane.buf.slice_utf16(span.clone());
                    let line_start_byte = pane.buf.doc.rope().line_to_byte(line as usize);
                    // The line's diagnostics as line-local UTF-16 ranges. What
                    // starts here also marks the gutter.
                    let mut line_diags = Vec::new();
                    let mut mark: Option<Severity> = None;
                    for d in pane.line_diagnostics(line) {
                        let range = pane.diag_range(d);
                        let local = range.start.clamp(span.start, span.end) - span.start
                            ..range.end.clamp(span.start, span.end) - span.start;
                        line_diags.push((local, d.severity));
                        if d.range.start.line == line && severity_rank(d.severity) > mark.map_or(0, severity_rank) {
                            mark = Some(d.severity);
                        }
                    }
                    if let Some(severity) = mark {
                        diag_marks.push((display, severity));
                    }
                    let runs = line_runs(&content, span, marked.clone(), line_start_byte, &pane.spans, &line_diags, fg);
                    // Inlay hints join the row text inline: split the runs at
                    // each hint and interleave the labels (the rest of the
                    // line shifts right by their widths).
                    let hints = pane.row_hints(line, window, font_size);
                    let (painted, runs) = if hints.is_empty() {
                        (content, runs)
                    } else {
                        hinted_rows += 1;
                        pane.hints_by_line.borrow_mut().insert(line, hints.clone());
                        augment_with_hints(&content, &hints, runs)
                    };
                    window.text_system().shape_line(painted.into(), font_size, &runs, None)
                }
            };
            let folded_here = placeholder.is_some();
            let foldable_here = !folded_here && pane.fold_candidates.iter().any(|(start, _)| *start == line);
            if folded_here || foldable_here {
                chevrons.push((display, folded_here));
            }
            frame_max_w = frame_max_w.max(shaped.width());
            rows.push(DisplayedRow { display, line: placeholder.is_none().then_some(line), shaped });
        }

        // Horizontal scroll: widen the observed-max with this frame's rows,
        // clamp, then pan the primary caret into view. Everything below
        // (carets, rects, geometry) uses this final offset.
        pane.max_line_w.set(pane.max_line_w.get().max(f32::from(frame_max_w)));
        let text_area_w = f32::from(text_area_w);
        let max_x = (pane.max_line_w.get() - text_area_w + theme::SCROLLBAR_SIZE).max(0.0);
        {
            let caret = pane.selections.first().map_or(0, |s| s.head);
            let caret_line = pane.buf.line_of(caret);
            let span = pane.buf.line_span(caret_line);
            let caret_x =
                f32::from(x_at_utf16(window, &pane.buf.slice_utf16(span.clone()), caret - span.start, font_size))
                    + pane.hint_shift(caret_line, caret - span.start);
            let pad = 48.0;
            let mut x = pane.scroll_x.get().clamp(0.0, max_x);
            if caret_x < x {
                x = (caret_x - pad).max(0.0);
            } else if caret_x > x + text_area_w - pad {
                x = (caret_x - text_area_w + pad).max(0.0);
            }
            pane.scroll_x.set(x.clamp(0.0, max_x));
        }
        let scroll_x = pane.scroll_x.get();

        // Carets (one per selection head) and selection rectangles for the
        // visible rows (positions go through the display map and shift left
        // with the horizontal scroll).
        let mut carets = Vec::with_capacity(pane.selections.len());
        let mut selection_rects = Vec::new();
        for selection in &pane.selections {
            let head_line = pane.buf.line_of(selection.head);
            let head_display = pane.display_of_line(head_line);
            if (first..last).contains(&head_display) {
                let span = pane.buf.line_span(head_line);
                let line_text = pane.buf.slice_utf16(span.clone());
                let head_x = f32::from(x_at_utf16(window, &line_text, selection.head - span.start, font_size))
                    + pane.hint_shift(head_line, selection.head - span.start)
                    - f32::from(px(scroll_x));
                let head_y = origin.y + line_height * (head_display as f32 - scroll);
                carets.push(Bounds::new(point(origin.x + px(head_x), head_y), size(px(2.0), line_height)));
            }
            let (start, end) = selection.normalized();
            if start == end {
                continue;
            }
            let start_line = pane.buf.line_of(start);
            let end_line = pane.buf.line_of(end).max(start_line);
            for display in first..last {
                let (row, placeholder) = pane.resolve_display(display);
                if placeholder.is_some() || row < start_line || row > end_line {
                    continue;
                }
                let span = pane.buf.line_span(row);
                let line_text = pane.buf.slice_utf16(span.clone());
                let row_len = line_text.chars().map(char::len_utf16).sum::<usize>();
                let x0 = if row > start_line {
                    px(0.0)
                } else {
                    x_at_utf16(window, &line_text, start - span.start, font_size)
                        + px(pane.hint_shift(row, start - span.start))
                };
                let x1 = if row < end_line {
                    // The whole row: the text plus whatever hints widen it.
                    shaped_width(window, &line_text, font_size) + px(pane.hint_shift(row, row_len))
                } else {
                    x_at_utf16(window, &line_text, end - span.start, font_size)
                        + px(pane.hint_shift(row, end - span.start))
                };
                let y = origin.y + line_height * (display as f32 - scroll);
                let left = origin.x + x0 - px(scroll_x);
                selection_rects.push(Bounds::new(point(left, y), size((x1 - x0).max(px(0.0)), line_height)));
            }
        }

        // Find matches for the visible rows (the sorted ranges make each
        // row's slice a partition point plus a short run).
        let mut find_rects = Vec::new();
        if let Some(hits) = &pane.find_hits
            && !hits.ranges.is_empty()
        {
            for display in first..last {
                let (row, placeholder) = pane.resolve_display(display);
                if placeholder.is_some() {
                    continue;
                }
                let span = pane.buf.line_span(row);
                let first_hit = hits.ranges.partition_point(|range| range.end <= span.start);
                let y = origin.y + line_height * (display as f32 - scroll);
                for (index, range) in hits.ranges[first_hit..].iter().enumerate() {
                    if range.start >= span.end {
                        break;
                    }
                    let line_text = pane.buf.slice_utf16(span.clone());
                    let local_start = range.start.clamp(span.start, span.end) - span.start;
                    let local_end = range.end.clamp(span.start, span.end) - span.start;
                    let x0 =
                        x_at_utf16(window, &line_text, local_start, font_size) + px(pane.hint_shift(row, local_start));
                    let x1 = x_at_utf16(window, &line_text, local_end, font_size) + px(pane.hint_shift(row, local_end));
                    let left = origin.x + x0 - px(scroll_x);
                    find_rects.push((
                        Bounds::new(point(left, y), size((x1 - x0).max(px(2.0)), line_height)),
                        first_hit + index == hits.current,
                    ));
                }
            }
        }

        // Scrollbar frames (published for hit-testing and drag mapping;
        // painting happens in the paint phase).
        let margin = px(2.0);
        let v_track = Bounds::new(
            point(bounds.origin.x + bounds.size.width - px(theme::SCROLLBAR_SIZE) - margin, bounds.origin.y + margin),
            size(px(theme::SCROLLBAR_SIZE), (bounds.size.height - margin * 2.0).max(px(0.0))),
        );
        let total = pane.display_count().max(1) as f32;
        let visible_rows = ((bounds.size.height - px(theme::TEXT_INSET) * 2.0) / line_height).max(1.0);
        let v_over = (total - visible_rows).max(1.0);
        let v_thumb_h = (v_track.size.height * (visible_rows / total).min(1.0)).max(px(24.0)).min(v_track.size.height);
        let v_travel = (v_track.size.height - v_thumb_h).max(px(1.0));
        let v_thumb = Bounds::new(
            point(v_track.origin.x, v_track.origin.y + pane.scroll.get().clamp(0.0, v_over) / v_over * v_travel),
            size(v_track.size.width, v_thumb_h),
        );
        let h_track = Bounds::new(
            point(bounds.origin.x + margin, bounds.origin.y + bounds.size.height - px(theme::SCROLLBAR_SIZE) - margin),
            size((bounds.size.width - margin * 2.0).max(px(0.0)), px(theme::SCROLLBAR_SIZE)),
        );
        let h_thumb_w = (h_track.size.width * (text_area_w / pane.max_line_w.get().max(1.0)).min(1.0))
            .max(px(24.0))
            .min(h_track.size.width);
        let h_travel = (h_track.size.width - h_thumb_w).max(px(1.0));
        let h_thumb = Bounds::new(
            point(h_track.origin.x + (pane.scroll_x.get() / max_x.max(1.0) * h_travel).min(h_travel), h_track.origin.y),
            size(h_thumb_w, h_track.size.height),
        );

        if !pane.first_paint_done.replace(true) {
            eprintln!("[perf] first paint {:.0} ms after open", pane.created_at.elapsed().as_secs_f32() * 1000.0);
        }

        // HUD counters: input→pixel latency lands at the first prepaint after
        // an input; row/span counts describe this frame.
        if let Some(input) = pane.hud.last_input.get() {
            pane.hud.input_latency_ms.set(Some(input.elapsed().as_secs_f32() * 1000.0));
            pane.hud.last_input.set(None);
        }
        pane.hud.rows_laid_out.set(rows.len() as u32);
        pane.hud.hints_rows.set(hinted_rows);
        pane.hud.spans_total.set(pane.spans.len() as u32);
        let visible_lo =
            rows.iter().filter_map(|row| row.line.map(|line| pane.buf.line_start(line))).min().unwrap_or(usize::MAX);
        let visible_hi =
            rows.iter().filter_map(|row| row.line.map(|line| pane.buf.line_span(line).end)).max().unwrap_or(0);
        let lo_idx = pane.spans.partition_point(|(range, _)| range.end <= visible_lo);
        let hi_idx = pane.spans.partition_point(|(range, _)| range.start < visible_hi);
        pane.hud.spans_painted.set(hi_idx.saturating_sub(lo_idx) as u32);

        pane.geometry.set(Some(Geometry {
            gutter_x,
            gutter_w,
            origin,
            line_height,
            scroll,
            scroll_x: pane.scroll_x.get(),
            height: bounds.size.height,
            width: px(text_area_w),
            vscroll: Scrollbar { track: v_track, thumb: v_thumb },
            hscroll: Scrollbar { track: h_track, thumb: h_thumb },
        }));

        PrepaintState {
            rows,
            carets,
            selection_rects,
            find_rects,
            chevrons,
            diag_marks,
            caret_line: caret_buffer_line,
            gutter_w,
            numbers_w,
        }
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
            // Selections under the text, then the rows (shifted left by the
            // horizontal scroll), then the gutter (repainted over any text
            // scrolled under it), then the carets and the scrollbars.
            for (rect, current) in &prepaint.find_rects {
                let color = if *current { theme::find_current_bg() } else { theme::find_bg() };
                window.paint_quad(fill(*rect, color));
            }
            for rect in &prepaint.selection_rects {
                window.paint_quad(fill(*rect, rgba_of(theme::SELECTION_BG)));
            }
            for row in prepaint.rows.drain(..) {
                let y = origin.y + line_height * (row.display as f32 - geometry.scroll);
                let _ = row.shaped.paint(
                    point(origin.x - px(geometry.scroll_x), y),
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }

            // Gutter: right-aligned line numbers, the caret's row brighter,
            // separated from the text by the border color; fold chevrons at
            // the left edge.
            let gutter_bg = Bounds::new(
                point(bounds.origin.x, bounds.origin.y),
                size(gutter_x + geometry.gutter_w - bounds.origin.x, bounds.size.height),
            );
            window.paint_quad(fill(gutter_bg, theme::surface_bg()));
            for row in &prepaint.rows {
                let y = origin.y + line_height * (row.display as f32 - geometry.scroll);
                if let Some(line) = row.line {
                    let number = (line + 1).to_string();
                    let width = shaped_width(window, &number, font_size);
                    let color = if Some(line) == Some(prepaint.caret_line) { theme::fg() } else { theme::fg_muted() };
                    let x = gutter_x + px(theme::GUTTER_PAD_L) + prepaint.numbers_w - width;
                    let run = plain_run(number.len(), color);
                    let shaped = window.text_system().shape_line(number.into(), font_size, &[run], None);
                    let _ = shaped.paint(point(x, y), line_height, TextAlign::Left, None, window, cx);
                }
            }
            for (display, folded) in &prepaint.chevrons {
                let glyph = if *folded { "▸" } else { "▾" };
                let run = plain_run(glyph.len(), theme::fg_muted());
                let shaped = window.text_system().shape_line(glyph.into(), font_size, &[run], None);
                let y = origin.y + line_height * (*display as f32 - geometry.scroll);
                let _ = shaped.paint(point(gutter_x + px(2.0), y), line_height, TextAlign::Left, None, window, cx);
            }
            // Diagnostic marks: a severity-colored bar at the gutter's right
            // edge, before the separator.
            for (display, severity) in &prepaint.diag_marks {
                let y = origin.y + line_height * (*display as f32 - geometry.scroll);
                let bar = Bounds::new(
                    point(gutter_x + prepaint.gutter_w - px(theme::GUTTER_PAD_R) + px(3.0), y + px(4.0)),
                    size(px(3.0), line_height - px(8.0)),
                );
                window.paint_quad(fill(bar, theme::severity_color_rgba(*severity)));
            }
            let separator = Bounds::new(
                point(gutter_x + prepaint.gutter_w - px(1.0), bounds.origin.y),
                size(px(1.0), bounds.size.height),
            );
            window.paint_quad(fill(separator, theme::border()));

            for caret in &prepaint.carets {
                window.paint_quad(fill(*caret, rgba_of(theme::ACCENT)));
            }

            // Scrollbars: subtle track, brighter thumb (M5).
            window.paint_quad(fill(geometry.vscroll.track, theme::scrollbar_track()));
            window.paint_quad(fill(geometry.vscroll.thumb, theme::scrollbar_thumb()));
            window.paint_quad(fill(geometry.hscroll.track, theme::scrollbar_track()));
            window.paint_quad(fill(geometry.hscroll.thumb, theme::scrollbar_thumb()));

            // F12 perf HUD, top right, above everything.
            let hud = &self.view.read(cx).hud;
            if hud.visible.get() {
                let (fps, frame_ms) = hud.frame();
                let latency = hud.input_latency_ms.get().map_or("-".to_owned(), |ms| format!("{ms:.1}"));
                let parse = hud.parse_ms.get().map_or("-".to_owned(), |ms| format!("{ms:.1}"));
                let lines = [
                    format!("{fps:.0} fps · frame {frame_ms:.1} ms"),
                    format!(
                        "rows {} · spans {}/{}",
                        hud.rows_laid_out.get(),
                        hud.spans_painted.get(),
                        hud.spans_total.get()
                    ),
                    format!("key→px {latency} ms · parse {parse} ms @v{}", hud.parse_version.get()),
                    format!(
                        "cursors {} · folds {} · hints {}",
                        self.view.read(cx).selections.len(),
                        self.view.read(cx).folds.len(),
                        hud.hints_rows.get()
                    ),
                ];
                let font_size = theme::px_font_size();
                for (index, line) in lines.iter().enumerate() {
                    let width = shaped_width(window, line, font_size);
                    let x = bounds.origin.x + bounds.size.width - width - px(10.0);
                    let y = bounds.origin.y + px(6.0) + line_height * index as f32;
                    let background = Bounds::new(point(x - px(6.0), y), size(width + px(12.0), line_height));
                    window.paint_quad(fill(background, rgba_of(theme::BG_APP << 8 | 0xe0)));
                    let run = plain_run(line.len(), theme::fg());
                    let shaped = window.text_system().shape_line(line.as_str().into(), font_size, &[run], None);
                    let _ = shaped.paint(point(x, y), line_height, TextAlign::Left, None, window, cx);
                }
            }
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
        // The primary selection is the IME's; the others freeze during
        // composition.
        let (range, reversed) = self.view.read(cx).selections.first().map_or((0..0, false), |s| {
            let (start, end) = s.normalized();
            (start..end, s.reversed)
        });
        Some(UTF16Selection { range, reversed })
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
        self.view.update(cx, |pane, cx| pane.ime_commit(replacement_range, text, cx));
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
        self.view.update(cx, |pane, cx| pane.ime_set_marked(range_utf16, new_text, new_selected_range, cx));
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        eprintln!("[ime] unmarkText");
        self.view.update(cx, |pane, cx| {
            pane.buf.marked = None;
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
        let x0 = shaped_width(window, &pane.buf.slice_utf16(span.start..start), theme::px_font_size())
            + px(pane.hint_shift(line, start - span.start));
        let x1 = shaped_width(window, &pane.buf.slice_utf16(span.start..end), theme::px_font_size())
            + px(pane.hint_shift(line, end - span.start));
        let bounds = Bounds::new(
            point(
                geometry.origin.x + x0 - px(geometry.scroll_x),
                geometry.origin.y + geometry.line_height * (line as f32 - geometry.scroll),
            ),
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
        let col = utf16_index_at_x(
            window,
            &line_text,
            point.x - geometry.origin.x + px(geometry.scroll_x),
            theme::px_font_size(),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(from: u32, to: u32, insert: &str) -> TextChange {
        TextChange { from, to, insert: insert.into() }
    }

    fn doc_info(language: &str, editorconfig: Option<ide_workspace::EditorConfigProps>) -> DocumentInfo {
        DocumentInfo {
            id: DocId::next(),
            path: ide_core::IpcPath(std::path::PathBuf::from("/tmp/x.rs")),
            text: String::new(),
            version: 0,
            language: ide_core::LanguageId::new(language),
            encoding: "utf-8".into(),
            line_ending: ide_text::LineEnding::Lf,
            read_only: false,
            dirty: false,
            large: false,
            editorconfig,
            uri: None,
            ui_mode: None,
        }
    }

    #[test]
    fn indent_resolves_language_then_editorconfig() {
        let languages = ide_language::LanguageRegistry::load(None).expect("builtin languages");
        // go.toml declares tab-size 4, insert-spaces false.
        let go = indent_for(&doc_info("go", None), &languages);
        assert_eq!(go, Indent { tab_size: 4, insert_spaces: false });
        assert_eq!(go.unit(), "\t");
        // Rust has no explicit indent: the default applies.
        assert_eq!(indent_for(&doc_info("rust", None), &languages), Indent::default());
        // editorconfig wins over the language.
        let props = ide_workspace::EditorConfigProps {
            indent_style: Some(ide_workspace::editorconfig::IndentStyle::Space),
            indent_size: Some(2),
            tab_width: None,
            end_of_line: None,
            charset: None,
            trim_trailing_whitespace: None,
            insert_final_newline: None,
            max_line_length: None,
        };
        let go_ec = indent_for(&doc_info("go", Some(props)), &languages);
        assert_eq!(go_ec, Indent { tab_size: 2, insert_spaces: true });
        assert_eq!(go_ec.unit(), "  ");
    }

    #[test]
    fn indent_unit_and_dedent() {
        let spaces = Indent { tab_size: 4, insert_spaces: true };
        assert_eq!(spaces.unit(), "    ");
        assert_eq!(spaces.dedent("        x"), "    x");
        assert_eq!(spaces.dedent("  x"), "x");
        assert_eq!(spaces.dedent("x"), "x");
        let tabs = Indent { tab_size: 4, insert_spaces: false };
        assert_eq!(tabs.unit(), "\t");
        assert_eq!(tabs.dedent("\t\tx"), "\tx");
        assert_eq!(tabs.dedent("    x"), "    x", "a tab policy only removes real tabs");
    }

    #[test]
    fn enter_keeps_and_grows_indent() {
        let spaces = Indent { tab_size: 4, insert_spaces: true };
        // Plain continuation keeps the line's own indent.
        assert_eq!(enter_indent("    let x = 1;", true, "", None, &spaces), "    ");
        // Caret mid-line keeps the indent too.
        assert_eq!(enter_indent("fn main() {", false, "}", None, &spaces), "");
        // A line ending in '{' at end-of-line grows by one unit.
        assert_eq!(enter_indent("fn main() {", true, "", None, &spaces), "    ");
        // A closing brace after the caret dedents back one unit.
        assert_eq!(enter_indent("    fn main() {", true, "}", None, &spaces), "    ");
        // A trailing line comment does not hide the '{'.
        assert_eq!(enter_indent("if x { // note", true, "", Some("//"), &spaces), "    ");
        assert_eq!(enter_indent("let x = { 1 };", true, "", None, &spaces), "", "brace not at the end: no growth");
        // Tab policy: the unit is a real tab; splitting before a `}` puts the
        // caret one unit deeper than the closing brace's own indent.
        let tabs = Indent { tab_size: 4, insert_spaces: false };
        assert_eq!(enter_indent("func main() {", true, "", None, &tabs), "\t");
        assert_eq!(enter_indent("\tfunc main() {", true, "}", None, &tabs), "\t");
        assert_eq!(enter_indent("\tx := 1", true, "", None, &tabs), "\t");
    }

    #[test]
    fn selections_normalize_sort_dedup_and_merge() {
        let mut selections = vec![
            Selection::caret(30),
            Selection::caret(5),
            Selection::caret(5),
            Selection { anchor: 12, head: 20, reversed: false, goal_col: None },
            Selection { anchor: 22, head: 17, reversed: true, goal_col: None },
        ];
        // Overlapping pair (12..20 and 17..22) merges into one plain range.
        selections = normalize_selections(selections);
        let spans: Vec<(usize, usize)> = selections.iter().map(|s| s.normalized()).collect();
        assert_eq!(spans, vec![(5, 5), (12, 22), (30, 30)]);
    }

    #[test]
    fn after_inserts_maps_to_post_batch_positions() {
        // Two cursors typing "hi" over empty ranges at 2 and 10.
        let after = Selection::after_inserts(&[tc(2, 2, "hi"), tc(10, 10, "hi")]);
        let heads: Vec<usize> = after.iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![4, 14], "the first insert shifts the second cursor");
        // Deletions collapse to the start of the deleted range.
        let after = Selection::after_inserts(&[tc(4, 8, "")]);
        assert_eq!(after[0].head, 4);
    }

    #[test]
    fn paste_ranges_cover_carets_and_selections() {
        // Paste ranges come from the selections: bare carets are empty insert
        // ranges (the text lands there), selections are replaced wholesale.
        // Three bare cursors: each becomes an insert point, ascending.
        let ranges = paste_ranges(&[Selection::caret(8), Selection::caret(2), Selection::caret(5)]);
        assert_eq!(ranges, vec![(2, 2), (5, 5), (8, 8)]);
        // A selection replaces its whole range; a reversed one (caret before
        // the anchor) normalizes to the same ascending range.
        let ranges = paste_ranges(&[Selection { anchor: 4, head: 9, reversed: false, goal_col: None }]);
        assert_eq!(ranges, vec![(4, 9)]);
        let ranges = paste_ranges(&[Selection { anchor: 9, head: 4, reversed: true, goal_col: None }]);
        assert_eq!(ranges, vec![(4, 9)]);
        // Mixed: one selection plus one caret, sorted, no dedup needed.
        let ranges =
            paste_ranges(&[Selection::caret(12), Selection { anchor: 1, head: 3, reversed: false, goal_col: None }]);
        assert_eq!(ranges, vec![(1, 3), (12, 12)]);
        // Duplicate cursors collapse into one insertion.
        let ranges = paste_ranges(&[Selection::caret(6), Selection::caret(6)]);
        assert_eq!(ranges, vec![(6, 6)]);
        // End-to-end: replacing each range with the pasted text moves cursors
        // to the end of the inserted text (the same math apply_edits runs).
        let changes: Vec<TextChange> =
            paste_ranges(&[Selection::caret(2), Selection { anchor: 5, head: 7, reversed: false, goal_col: None }])
                .into_iter()
                .map(|(start, end)| tc(start as u32, end as u32, "XY"))
                .collect();
        let heads: Vec<usize> = Selection::after_inserts(&changes).iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![4, 9], "caret after insert; the selection's tail shifts by the delta");
    }

    #[test]
    fn inverse_of_typing_and_of_ime_commit() {
        // Typing "x" over a selection restores the original text on undo.
        let inv = inverse_changes(&[tc(0, 3, "x")], &["abc".into()], false);
        assert_eq!(inv, vec![tc(0, 1, "abc")]);
        // A pure insertion restores emptiness.
        let inv = inverse_changes(&[tc(2, 2, "hi")], &["".into()], false);
        assert_eq!(inv, vec![tc(2, 4, "")]);
        // An IME commit over marked text deletes it all on undo.
        let inv = inverse_changes(&[tc(2, 5, "你好")], &["nih".into()], true);
        assert_eq!(inv, vec![tc(2, 4, "")]);
        // Multi-change batches shift later positions by earlier deltas.
        let inv = inverse_changes(&[tc(0, 0, "ab"), tc(5, 7, "")], &["".into(), "cd".into()], false);
        // Inverses live in post-batch coordinates: the deletion moved to 7.
        assert_eq!(inv, vec![tc(0, 2, ""), tc(7, 7, "cd")]);
    }

    #[test]
    fn clamp_selection_respects_length_and_direction() {
        let s = clamp_selection(Selection { anchor: 3, head: 9, reversed: false, goal_col: None }, 5);
        assert_eq!(s.normalized(), (3, 5));
        assert!(!s.reversed);
        let s = clamp_selection(Selection { anchor: 4, head: 1, reversed: true, goal_col: None }, 2);
        assert_eq!(s.normalized(), (1, 2));
        assert!(s.reversed);
    }

    #[test]
    fn word_boundaries_jump_class_runs() {
        let buf = Buffer::new("let alpha_x = 42;\n");
        assert_eq!(buf.utf16_len(), 18);
        // From the end of "alpha_x" back lands on its start (word run).
        assert_eq!(buf_slice(&buf, buf.word_start(10), buf.word_end(10)), "alpha_x");
        // From after `42;` back over `;` (other class), then `42` (word).
        assert_eq!(buf_slice(&buf, buf.word_start(17), 17), ";");
        assert_eq!(buf_slice(&buf, buf.word_start(17), 15), "");
        // Whitespace runs are crossed: from before `4` back lands before `=`,
        // and from after `=` forward lands after `42`.
        assert_eq!(buf.word_start(14), 12);
        assert_eq!(buf.word_end(13), 16);
    }

    fn buf_slice(buf: &Buffer, start: usize, end: usize) -> String {
        buf.slice_utf16(start..end)
    }

    #[test]
    fn buffer_line_helpers_round_trip() {
        let buf = Buffer::new("abc\r\ndef");
        // ide_text normalizes CRLF, so two lines.
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line_span(0), 0..3);
        assert_eq!(buf.line_span(1), 4..7);
        assert_eq!(buf.slice_utf16(0..buf.utf16_len()), "abc\ndef");
    }

    #[test]
    fn match_ranges_folds_case_and_counts_utf16() {
        assert_eq!(match_ranges("ab Ab aB AB", "ab", false), vec![0..2, 3..5, 6..8, 9..11]);
        assert_eq!(match_ranges("ab Ab aB AB", "ab", true), vec![0..2]);
        // Non-overlapping, ascending, greedy-left.
        assert_eq!(match_ranges("aaaa", "aa", true), vec![0..2, 2..4]);
        // UTF-16: the astral-plane char is two code units, so the first hit
        // starts at 2, not 1.
        assert_eq!(match_ranges("𝕊x x", "x", true), vec![2..3, 4..5]);
        // No self-overlap at the boundary; empty needle finds nothing.
        assert!(match_ranges("abc", "", true).is_empty());
        assert!(match_ranges("", "abc", true).is_empty());
    }

    #[test]
    fn text_edits_become_sorted_changes() {
        let buf = Buffer::new("hello world\nsecond line\n");
        // Out-of-order, clamped and duplicate edits normalize into one batch.
        let edits = vec![
            ide_core::TextEdit {
                range: ide_core::Range::new(ide_core::Pos::new(1, 0), ide_core::Pos::new(1, 6)),
                new_text: "SECOND".into(),
            },
            ide_core::TextEdit {
                range: ide_core::Range::new(ide_core::Pos::new(0, 6), ide_core::Pos::new(0, 11)),
                new_text: "there".into(),
            },
        ];
        let changes = text_edits_to_changes(&edits, &buf);
        assert_eq!(
            changes,
            vec![tc(6, 11, "there"), tc(12, 18, "SECOND")],
            "ascending, non-overlapping, line starts resolved"
        );
        // A no-op edit (empty range, empty insert) drops.
        let edits = vec![ide_core::TextEdit {
            range: ide_core::Range::new(ide_core::Pos::new(0, 0), ide_core::Pos::new(0, 0)),
            new_text: String::new(),
        }];
        assert!(text_edits_to_changes(&edits, &buf).is_empty());
    }

    #[test]
    fn hints_augment_a_row_inline() {
        // `repo := store.NewStore()` with a type hint after `repo ` (the
        // gopls shape: position between the name and `:=`, padding left).
        let line = "repo := store.NewStore()";
        let base = vec![plain_run(line.len(), gpui::black())];
        let hints = vec![RowHint {
            at: 5,
            label: " *store.Store".to_owned(),
            width: 90.0,
            kind: ide_lsp::types::InlayKind::Type,
        }];
        let (text, runs) = augment_with_hints(line, &hints, base);
        assert_eq!(text, "repo  *store.Store:= store.NewStore()", "the label joins at its position");
        // Three runs: the text before, the hint chip, the text after — lens
        // concatenate to the augmented string's byte length.
        assert_eq!(runs.len(), 3);
        let total: usize = runs.iter().map(|run| run.len).sum();
        assert_eq!(total, text.len());
        // A hint at the very end keeps its label (nothing after to drain).
        let hints = vec![RowHint {
            at: line.chars().map(char::len_utf16).sum(),
            label: " count:".to_owned(),
            width: 48.0,
            kind: ide_lsp::types::InlayKind::Parameter,
        }];
        let (text, runs) = augment_with_hints(line, &hints, vec![plain_run(line.len(), gpui::black())]);
        assert_eq!(text, format!("{line} count:"));
        assert_eq!(runs.len(), 2);
        let total: usize = runs.iter().map(|run| run.len).sum();
        assert_eq!(total, text.len());
    }

    #[test]
    fn display_map_collapses_folded_rows() {
        // Lines 6..10 hidden under a placeholder at line 5.
        let folds = vec![Fold { start_line: 5, end_line: 10 }];
        assert_eq!(display_count(&folds, 20), 16, "15 visible lines + 1 placeholder");
        assert_eq!(display_of_line(&folds, 5), 5);
        assert_eq!(display_of_line(&folds, 7), 6, "hidden lines clamp onto the placeholder");
        assert_eq!(display_of_line(&folds, 11), 6);
        assert_eq!(display_of_line(&folds, 19), 14);
        // Walk the whole map: rows 0-5 buffer, row 6 placeholder, 7-15 buffer.
        let resolved: Vec<_> = (0..16).map(|d| resolve_display(&folds, 20, d)).collect();
        assert_eq!(resolved[0].0, 0);
        assert_eq!(resolved[5].0, 5);
        assert_eq!(resolved[6], (5, Some(folds[0])));
        assert_eq!(resolved[7].0, 11);
        assert_eq!(resolved[14].0, 18);
        assert_eq!(resolved[15].0, 19);
        // Two folds compose: b0, b1, ph(f1), b3, b4, b5, ph(f2), b9.
        let folds = vec![Fold { start_line: 1, end_line: 2 }, Fold { start_line: 5, end_line: 8 }];
        assert_eq!(display_count(&folds, 10), 8);
        let resolved: Vec<_> = (0..8).map(|d| resolve_display(&folds, 10, d)).collect();
        assert_eq!(
            resolved.iter().map(|(l, p)| (*l, p.is_some())).collect::<Vec<_>>(),
            // Placeholders report their fold's start line.
            vec![(0, false), (1, false), (1, true), (3, false), (4, false), (5, false), (5, true), (9, false)]
        );
        // Without folds everything is identity.
        assert_eq!(display_count(&[], 7), 7);
        assert_eq!(resolve_display(&[], 7, 6), (6, None));
    }
}
