# M2 design notes — the GPUI code editor

Written up-front from a design-only pass over Zed's editor architecture
(see `m2-cleanroom-log.md` for what was read and why). Everything here is my
own restatement of the concepts; no Zed code, types or literals are copied.
Implementation targets openidea's own crates (`ide-text`, `ide-syntax`) and
the gpui-pre API surface M0/M1 already proved. Where M2 needs less than Zed
builds, we build less: no soft wrap, no injections, no anchors, no
per-cursor local histories.

## 1. Buffer and edits (Phase 1)

The pane holds a **local copy** of `ide_text::Document` (rope-backed,
`\n`-normalized, UTF-16 coordinate conversions built in). The backend
`Documents` registry stays the owner of dirty / reload / save semantics; the
pane mirrors it version-for-version.

- **User edit**: build a `ChangeSet` (`TextChange { from, to, insert }`,
  UTF-16 offsets of the pre-edit text, sorted, non-overlapping), then
  1. apply it to the local `Document` via `apply_detailed` — the returned
     `ByteEdit` vec is exactly tree-sitter's `InputEdit` feed (Phase 3 uses it),
  2. send the same `ChangeSet` to the backend via `Documents::edit(doc,
     base_version, changes)`.
  Both sides bump their version by one per applied change set, so they stay
  in lockstep. If the backend rejects (`OutOfSync`), the pane resyncs from
  `Documents::snapshot` (full text + version).
- **Echo suppression by version**: backend-originated patches arrive as
  `IdeEvent::DocPatched { doc, base_version, version, changes }`. `edit()`
  does *not* emit `DocPatched` (only save/reload/`apply_edits` do), so in
  practice every `DocPatched` is foreign; the version check is still the
  rule: apply `changes` only when the pane is exactly at `base_version`
  (the reload case: clean buffer, pane and backend agree); anything else
  falls back to a full `snapshot()` resync instead of trying to guess.
  This replaces M1's whole-text comparison.
- **Dirty flags** come from the backend (`DocDirty` through the bus) since
  every user edit now reaches it synchronously; the pane's own edit counter
  and the workbench's observer bookkeeping (`last_seen_edits`) disappear.
  ⌘S becomes `Documents::save(doc)` alone — the backend already has the text.

## 2. Viewport virtualization (Phase 2)

- Line geometry comes from the rope's own metadata: `len_lines()` is O(1),
  `line_to_byte(i)` / `line(i)` are O(log n) — no eager line index is built
  for a 50 MB file. The M0 `LineIndex` (full text scan per frame) dies here.
- The element shapes **only the visible rows** (plus one row of buffer above
  and below). Scroll position is a `f32` row offset; total height is
  `line_count * line_height`, painted as an empty spacer of that size with
  the shaped rows drawn into the clipped viewport (`Window::paint_layer`).
- Fold state (Phase 5) turns *buffer rows* into *display rows*: a sorted list
  of folded ranges maps display row ⇄ buffer row by walking the list — folds
  are expected to be few, so a linear walk per frame is fine. Each folded
  range renders as **one placeholder display row** (a head glyph plus a
  short count label); clicking it unfolds. Line numbers gutter shows the
  buffer row of each display row, so numbers skip during a fold.

## 3. Incremental highlighting (Phase 3)

- One `tree_sitter::Tree` per pane plus the parsed version. On every edit,
  first `tree.edit(InputEdit::from(ByteEdit))` (cheap, keeps the tree usable),
  then schedule an incremental re-parse on the background executor:
  `tree.copy()` as the old tree, text read directly from a `ropey::Rope`
  clone (chunks slide under the parser's read callback — no full-text copy),
  new tree swapped in on the main thread when it lands, tagged with the
  version it parsed.
- Highlight spans come from a highlights query run over the tree. Strategy:
  **viewport-first** — the query runs with a byte-range filter covering the
  visible rows (with slack), producing `(byte range, kind)` spans the paint
  code converts per line; a full-document span set is never materialized.
  Scroll just recomputes for the new window (the query is cheap relative to
  a 50 MB parse; measured in the perf HUD). Files flagged `large` (or
  without a grammar) skip parsing/highlighting entirely.
- Token kinds (`SyntaxHighlightKind`-style classes) map onto a hardcoded
  palette in `theme.rs`; unknown kinds fall back to the default foreground.
- While a re-parse is in flight, the stale tree still colors correctly on
  average; spans carry the parsed version and are recomputed when the new
  tree lands or the viewport moves.

## 4. Multi-cursor (Phase 4)

- A selection is `{ anchor, head, reversed, goal_col }` in UTF-16 offsets.
  `reversed` records which end moved; `goal_col` preserves the wanted column
  across vertical moves (shorter lines clamp but do not destroy the goal).
- Cursors live in a `Vec`, sorted by offset. Invariants: distinct cursors
  may overlap; when an edit would make two cursors' ranges overlap, the
  survivors are merged (an edit that deletes across two cursors fuses them).
  Primary cursor is index 0 (owns the IME, see §5).
- **Typing** applies one edit per cursor: all edits are computed against the
  pre-edit offsets, applied bottom-up (descending start offset) so earlier
  offsets stay valid, and submitted as ONE `ChangeSet` (the backend requires
  sorted, non-overlapping changes; bottom-up == sorted). Each cursor's new
  position derives from its own edit: head lands at
  `start + inserted_len`; anchors before the edit are untouched, anchors
  inside/after shift by the edit's delta.
- **Backspace/Delete**: with a non-empty selection, delete it; empty cursor
  deletes one char boundary left/right (grapheme-ish: one `char`, never a
  surrogate half). Deleting across cursors can merge them.
- **Movement** (arrows, ⌘←→ line ends with smart-home toggle, ⌥←→ word,
  ⌘↑↓ document ends, ↑↓ vertical with goal column): applied per cursor;
  Shift extends `head` only (tail stays). ⌘-click adds a cursor at the
  point; ⌥-drag (column select) builds one cursor per visual row between the
  drag's start/end x, each spanning the x-range as mapped per line; Esc
  collapses to the primary cursor. Click collapses everything to one cursor.
- **Undo / redo — one timeline**: every edit batch (one keystroke round, one
  paste, one IME commit, one fold-preserving replace) pushes
  `{ base_version, changes, inverse, cursors_before, cursors_after }`.
  Consecutive single-line typing batches merge into one entry (same line,
  contiguous insertion/deletion). Undo pops the entry, applies `inverse`
  through the *same* edit pipeline (local + backend, one version bump), and
  pushes it to the redo stack; redo is symmetric. Cursor positions are
  restored from the entry, then clamped.

## 5. IME inside the multi-cursor editor (Phase 6)

M0 proved the protocol for one caret; M2 lifts it into the pane's world:

- The **primary cursor is the composition site**. `setMarkedText` inserts the
  composition text inline at the primary cursor (replacing its selection, or
  the previous marked range on the next callback) — exactly the
  NSTextInputClient contract M0 verified; the marked range is tracked in
  buffer UTF-16 offsets and renders with underline + tint (the M0 visual).
- Commit (`insertText`) replaces the marked range; cancel (`setMarkedText ""`)
  removes it. Both go through the ordinary edit pipeline (they are edits: the
  backend sees composition keystrokes too, like every mainstream editor).
- While a composition is live, the **other cursors freeze**: movement and
  editing commands short-circuit (the macOS IME routes keys itself; anything
  that still reaches the key handler during composition is passed through
  untouched, per the M0 finding). Vertical scroll-follow tracks the marked
  range's end.
- Candidate anchoring: `bounds_for_range` resolves the marked range's rect
  from the pane's published geometry; off-viewport ranges clamp to the
  primary cursor's display row.
- `marked_text_range()` returning `None` when idle stays load-bearing (the
  mac key router derives `is_composing` from it — M0 lesson).

## 6. Folding (Phase 5)

- Foldable ranges come from the syntax tree: any named node spanning more
  than one line whose first line is not the same as its last (bracketed
  constructs, comment runs later). Sorted, nested folds are allowed but M2
  folds only **top-level non-overlapping** ranges per line — folding a range
  hides all nested folds inside it.
- Gutter shows a chevron on foldable first lines; clicking folds/unfolds.
  Folded ranges render as one placeholder display row (`⋯ 12 lines`) that
  unfolds on click (§2). The display map (§2) is the single source of row
  mapping for paint, mouse mapping, and caret geometry.
- Edits landing inside a folded range keep it folded while its range still
  spans >1 line; ranges that collapse to one line unfold automatically.
- ⌥-click on a chevron folds all sibling ranges of the same node kind
  (optional; only if cheap).

## 7. Performance HUD (Phase 7)

- F12 toggles an overlay drawn by the editor element: rolling frame interval
  (p50/max), rows laid out last frame, span count painted, last incremental
  parse duration + version, and the **key→pixel latency** (Instant captured
  in the key handler, diffed when the same frame paints the editor).
- Measurement method goes into M2-REPORT: keys typed by script, scroll by
  wheel events, open latency by instrumenting the tab-open path; numbers are
  read from the HUD's own counters (logged to stderr on toggle-off).
