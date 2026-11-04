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

use gpui::{
    App, Bounds, Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, InputHandler, InspectorElementId,
    InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, Render, ScrollWheelEvent, ShapedLine, SharedString, Styled, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, fill, point, px, relative, rgba, size,
};
use ide_core::{ChangeSet, Diagnostic, DocId, Severity, TextChange};
use ide_workspace::DocumentInfo;
use tree_sitter::Tree;

use crate::actions as acts;
use crate::highlight::{Highlighter, Kind, ParseResult, input_edit};
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

/// What a press started: a normal anchor drag, or a ⌥ column selection.
#[derive(Clone, Copy)]
enum Drag {
    Normal { anchor: usize },
    Columnar { from: Point<Pixels> },
}

/// Motions the keymap maps onto. Word/line variants are chosen by modifier.
#[derive(Clone, Copy, PartialEq, Eq)]
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
#[derive(Clone, Copy)]
enum Command {
    Move(Motion, bool),
    Delete { forward: bool, word: bool },
    Newline,
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
    /// The buffer line the pointer was on (the tooltip anchors under it).
    line: u32,
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
    buf: Buffer,
    focus: FocusHandle,
    geometry: Cell<Option<Geometry>>,
    /// Top visible line, in lines (clamped on use).
    scroll: Cell<f32>,
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
    spans: Vec<(Range<usize>, Kind)>,
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
    pub fn new(host: Arc<Host>, info: DocumentInfo, cx: &mut gpui::Context<Self>) -> Self {
        let language = info.language.clone();
        let mut pane = Self {
            host,
            doc_id: info.id,
            buf: Buffer::new(&info.text),
            focus: cx.focus_handle(),
            geometry: Cell::new(None),
            scroll: Cell::new(0.0),
            focused_once: Cell::new(false),
            highlighter: None,
            language,
            large: info.large,
            read_only: info.read_only,
            tree: None,
            spans: Vec::new(),
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
            hud: Hud::default(),
            created_at: std::time::Instant::now(),
            first_paint_done: Cell::new(false),
        };
        pane.start_syntax(cx);
        pane
    }

    /// Starts syntax highlighting when the language has a bundled grammar and
    /// the file is not flagged large.
    fn start_syntax(&mut self, cx: &mut gpui::Context<Self>) {
        if self.large {
            return;
        }
        let Some(highlighter) = Highlighter::for_language(&self.language) else { return };
        self.highlighter = Some(highlighter.clone());
        self.schedule_parse(highlighter, cx);
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
            self.spans = spans;
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

    /// Applies byte-coordinate edits to the syntax tree, shifts the folded
    /// ranges with the line deltas, and schedules an incremental re-parse.
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
        }
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

        let selections_before = std::mem::take(&mut self.selections);
        let selections_after = match cause {
            Cause::Typing => Selection::after_inserts(&changes),
            Cause::Delete => Selection::after_inserts(&changes),
            // Mechanical batches keep the caller's cursors; they are restored
            // (undo/redo) or clamped (IME, patches) afterwards.
            Cause::ImeCommit | Cause::Mechanical => selections_before.clone(),
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
        self.after_edits(&byte_edits, cx);
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

    /// Inserts `text` at every cursor (replacing each selection), bottom-up
    /// in one `ChangeSet` so the backend sees a single version bump.
    fn insert_text(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        let mut ranges: Vec<(usize, usize)> = self.selections.iter().map(|s| s.normalized()).collect();
        ranges.sort_unstable();
        ranges.dedup();
        let changes: Vec<TextChange> = ranges
            .into_iter()
            .map(|(start, end)| TextChange { from: start as u32, to: end as u32, insert: text.to_owned() })
            .collect();
        self.apply_edits(changes, Cause::Typing, cx);
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
            // The old tree and its queued edits describe text that no longer
            // exists anywhere; the in-flight parse (if any) is invalidated.
            self.parse_generation += 1;
            self.tree = None;
            self.spans.clear();
            self.pending_edits.clear();
            self.fold_candidates.clear();
            self.folds.clear();
            if let Some(highlighter) = self.highlighter.clone() {
                self.schedule_parse(highlighter, cx);
            }
        }
    }

    /// Replaces the pane's diagnostics (the workbench merges the producers).
    pub fn set_diagnostics(&mut self, merged: Vec<Diagnostic>, cx: &mut gpui::Context<Self>) {
        self.diagnostics = merged;
        self.hover.set(None);
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
        let caret = self.selections.first().map_or(0, |s| s.head);
        let base = range_utf16.clone().or_else(|| self.buf.marked.clone()).unwrap_or(caret..caret);
        let (inserted, change) = self.buf.replace(base, text);
        self.buf.marked = None;
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

    fn command(&mut self, cmd: Command, cx: &mut gpui::Context<Self>) {
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
        match cmd {
            Command::Move(motion, extend) => self.move_cursors(motion, extend, cx),
            Command::Delete { forward, word } => self.delete(forward, word, cx),
            Command::Newline => self.insert_text("\n", cx),
            Command::Cancel => self.collapse_cursors(cx),
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
        if ks.modifiers.platform || ks.modifiers.alt || ks.modifiers.control {
            return;
        }
        if let Some(text) = ks.key_char.as_ref() {
            self.insert_text(text, cx);
        }
    }

    // -- mouse -------------------------------------------------------------

    /// Buffer offset for a window point (its row's line, clamped into it).
    fn offset_at(&self, window: &Window, geometry: &Geometry, position: Point<Pixels>) -> usize {
        let (line, _) = self.resolve_display(self.row_at(geometry, position.y));
        let span = self.buf.line_span(line);
        let line = self.buf.slice_utf16(span.clone());
        let col = utf16_index_at_x(window, &line, position.x - geometry.origin.x, theme::px_font_size());
        span.start + col
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
            // ⌥-drag: column selection.
            self.drag.set(Some(Drag::Columnar { from: ev.position }));
            self.selections = self.columnar_selections(window, &geometry, ev.position, ev.position);
            cx.notify();
            return;
        }
        let offset = self.offset_at(window, &geometry, ev.position);
        if ev.modifiers.platform && !ev.modifiers.shift {
            // ⌘-click toggles a cursor at the point.
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
            _ => self.update_hover(ev.position, window, cx),
        }
    }

    /// Parks (or clears) the diagnostic under the pointer for the tooltip.
    /// Only a change notifies — moves within one diagnostic re-render nothing.
    fn update_hover(&mut self, position: Point<Pixels>, window: &Window, cx: &mut gpui::Context<Self>) {
        let next = self.geometry.get().and_then(|geometry| {
            let offset = self.offset_at(window, &geometry, position);
            let line = self.buf.line_of(offset);
            self.diagnostics
                .iter()
                .find(|d| {
                    let range = self.diag_range(d);
                    // An empty range (bare caret position) still shows on hover.
                    (range.start..range.end.max(range.start + 1)).contains(&offset)
                })
                .map(|d| HoverDiag { message: d.message.clone(), severity: d.severity, line })
        });
        if self.hover.replace(next.clone()) != next {
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _ev: &MouseUpEvent, _window: &mut Window, _cx: &mut gpui::Context<Self>) {
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
            let span = self.buf.line_span(line);
            let text = self.buf.slice_utf16(span.clone());
            let c0 = utf16_index_at_x(window, &text, x_left - geometry.origin.x, theme::px_font_size());
            let c1 = utf16_index_at_x(window, &text, x_right - geometry.origin.x, theme::px_font_size());
            let start = span.start + c0;
            let end = span.start + c1;
            selections.push(if down {
                Selection { anchor: start, head: end, reversed: false, goal_col: None }
            } else {
                Selection { anchor: end, head: start, reversed: true, goal_col: None }
            });
        }
        selections
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
        let max = self.display_count().saturating_sub(1) as f32;
        self.scroll.set(self.scroll.get().clamp(0.0, max));
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
            // Overlapping: keep the union as a plain selection.
            let last = out.last_mut().expect("checked above");
            let (last_start, _) = last.normalized();
            let (_, end) = selection.normalized();
            *last = Selection { anchor: last_start, head: end, reversed: false, goal_col: None };
        } else if out.last() != Some(&selection) {
            out.push(selection);
        }
    }
    out
}

impl Render for EditorPane {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if !self.focused_once.get() {
            self.focused_once.set(true);
            window.focus(&self.focus, cx);
        }
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
            .on_action(cx.listener(|this, _: &acts::TogglePerfHud, _, cx| this.command(Command::ToggleHud, cx)))
            .on_action(cx.listener(|this, _: &acts::Cancel, _, cx| this.command(Command::Cancel, cx)))
            .on_action(cx.listener(|this, _: &acts::Newline, _, cx| this.command(Command::Newline, cx)))
            .on_action(cx.listener(|this, _: &acts::DeleteBackward, _, cx| {
                this.command(Command::Delete { forward: false, word: false }, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::DeleteForward, _, cx| {
                this.command(Command::Delete { forward: true, word: false }, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::DeleteWordBackward, _, cx| {
                this.command(Command::Delete { forward: false, word: true }, cx)
            }))
            .on_action(cx.listener(|this, _: &acts::DeleteWordForward, _, cx| {
                this.command(Command::Delete { forward: true, word: true }, cx)
            }))
            .on_action(
                cx.listener(|this, _: &acts::CursorLeft, _, cx| this.command(Command::Move(Motion::Left, false), cx)),
            )
            .on_action(
                cx.listener(|this, _: &acts::CursorRight, _, cx| this.command(Command::Move(Motion::Right, false), cx)),
            )
            .on_action(
                cx.listener(|this, _: &acts::CursorUp, _, cx| this.command(Command::Move(Motion::Up, false), cx)),
            )
            .on_action(
                cx.listener(|this, _: &acts::CursorDown, _, cx| this.command(Command::Move(Motion::Down, false), cx)),
            )
            .on_action(cx.listener(|this, _: &acts::CursorWordLeft, _, cx| {
                this.command(Command::Move(Motion::WordLeft, false), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorWordRight, _, cx| {
                this.command(Command::Move(Motion::WordRight, false), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorLineHome, _, cx| {
                this.command(Command::Move(Motion::LineHome, false), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorLineEnd, _, cx| {
                this.command(Command::Move(Motion::LineEnd, false), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::CursorDocStart, _, cx| {
                this.command(Command::Move(Motion::DocStart, false), cx)
            }))
            .on_action(
                cx.listener(|this, _: &acts::CursorDocEnd, _, cx| {
                    this.command(Command::Move(Motion::DocEnd, false), cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &acts::SelectLeft, _, cx| this.command(Command::Move(Motion::Left, true), cx)),
            )
            .on_action(
                cx.listener(|this, _: &acts::SelectRight, _, cx| this.command(Command::Move(Motion::Right, true), cx)),
            )
            .on_action(cx.listener(|this, _: &acts::SelectUp, _, cx| this.command(Command::Move(Motion::Up, true), cx)))
            .on_action(
                cx.listener(|this, _: &acts::SelectDown, _, cx| this.command(Command::Move(Motion::Down, true), cx)),
            )
            .on_action(cx.listener(|this, _: &acts::SelectWordLeft, _, cx| {
                this.command(Command::Move(Motion::WordLeft, true), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectWordRight, _, cx| {
                this.command(Command::Move(Motion::WordRight, true), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectLineHome, _, cx| {
                this.command(Command::Move(Motion::LineHome, true), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectLineEnd, _, cx| {
                this.command(Command::Move(Motion::LineEnd, true), cx)
            }))
            .on_action(cx.listener(|this, _: &acts::SelectDocStart, _, cx| {
                this.command(Command::Move(Motion::DocStart, true), cx)
            }))
            .on_action(
                cx.listener(|this, _: &acts::SelectDocEnd, _, cx| {
                    this.command(Command::Move(Motion::DocEnd, true), cx)
                }),
            )
            // Hover tooltip: a plain-text bubble under the hovered diagnostic.
            .children(self.render_hover_tooltip(cx))
            // Navigation/refactoring handlers arrive in later M3 phases.
            .child(EditorElement { view: cx.entity() })
    }
}

impl EditorPane {
    /// The hover tooltip for the diagnostic under the pointer, positioned
    /// under its line from the last frame's geometry.
    fn render_hover_tooltip(&self, _cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let hover = self.hover.replace(None);
        self.hover.set(hover.clone());
        let hover = hover?;
        let geometry = self.geometry.get()?;
        let row = self.display_of_line(hover.line);
        let y = geometry.origin.y + geometry.line_height * ((row as f32 - geometry.scroll) + 1.0) + px(4.0);
        let x = geometry.origin.x + px(24.0);
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
                .text_color(theme::fg())
                .child(SharedString::from(hover.message))
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

/// Error squiggles win over warnings, warnings over info hints.
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
                    window.text_system().shape_line(content.into(), font_size, &runs, None)
                }
            };
            let folded_here = placeholder.is_some();
            let foldable_here = !folded_here && pane.fold_candidates.iter().any(|(start, _)| *start == line);
            if folded_here || foldable_here {
                chevrons.push((display, folded_here));
            }
            rows.push(DisplayedRow { display, line: placeholder.is_none().then_some(line), shaped });
        }

        // Carets (one per selection head) and selection rectangles for the
        // visible rows (positions go through the display map).
        let mut carets = Vec::with_capacity(pane.selections.len());
        let mut selection_rects = Vec::new();
        for selection in &pane.selections {
            let head_line = pane.buf.line_of(selection.head);
            let head_display = pane.display_of_line(head_line);
            if (first..last).contains(&head_display) {
                let span = pane.buf.line_span(head_line);
                let line_text = pane.buf.slice_utf16(span.clone());
                let head_x = x_at_utf16(window, &line_text, selection.head - span.start, font_size);
                let head_y = origin.y + line_height * (head_display as f32 - scroll);
                carets.push(Bounds::new(point(origin.x + head_x, head_y), size(px(2.0), line_height)));
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
                let x0 = if row > start_line {
                    px(0.0)
                } else {
                    x_at_utf16(window, &line_text, start - span.start, font_size)
                };
                let x1 = if row < end_line {
                    shaped_width(window, &line_text, font_size)
                } else {
                    x_at_utf16(window, &line_text, end - span.start, font_size)
                };
                let y = origin.y + line_height * (display as f32 - scroll);
                selection_rects.push(Bounds::new(point(origin.x + x0, y), size((x1 - x0).max(px(0.0)), line_height)));
            }
        }

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
            height: bounds.size.height,
        }));

        PrepaintState {
            rows,
            carets,
            selection_rects,
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
            // Gutter: right-aligned line numbers, the caret's row brighter,
            // separated from the text by the border color; fold chevrons at
            // the left edge.
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

            // Selections under the text, then the rows, then the carets.
            for rect in &prepaint.selection_rects {
                window.paint_quad(fill(*rect, rgba_of(theme::SELECTION_BG)));
            }
            for row in prepaint.rows.drain(..) {
                let y = origin.y + line_height * (row.display as f32 - geometry.scroll);
                let _ = row.shaped.paint(point(origin.x, y), line_height, TextAlign::Left, None, window, cx);
            }
            for caret in &prepaint.carets {
                window.paint_quad(fill(*caret, rgba_of(theme::ACCENT)));
            }

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
                        "cursors {} · folds {}",
                        self.view.read(cx).selections.len(),
                        self.view.read(cx).folds.len()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(from: u32, to: u32, insert: &str) -> TextChange {
        TextChange { from, to, insert: insert.into() }
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
