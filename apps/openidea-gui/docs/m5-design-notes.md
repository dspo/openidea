# M5 design notes — editor maturity (the language-agnostic base)

Written before any M5 code, in my own words, from openidea's own crates
(`ide-lsp`, `ide-search`, `ide-language`, `ide-text`, `ide-workspace`), the
M0–M4 reports' seam notes, the app's own editor/highlight sources, and the
Apache-2.0 gpui-pre / gpui-component APIs. The live-template model is a port
of this repo's own TypeScript (`src/features/templates/live/model.ts`,
Apache-2.0 project code). Zed (GPL-3.0) was **not** read for M5 — see
`m5-cleanroom-log.md`.

Seven phases, one commit each, each green (fmt + clippy `-D warnings` + unit
tests) when committed.

## 1. Language indent + scrollbars + horizontal scroll

- **Indent policy** `{ tab_size, insert_spaces }` resolves once at pane
  creation: `DocumentInfo.editorconfig` (`indent_style`/`indent_size`/
  `tab_width`) wins, else the language's `indent` from `languages/*.toml`
  through `LanguageRegistry::get` (`go.toml` declares tab-size 4,
  insert-spaces false), else the default (4 spaces). One pure helper, unit
  tested.
- **Tab** becomes an action (`editor::TabIndent`, bound in the Editor
  context): insert one indent unit per cursor (the unit string is `"\t"` or
  `" "` × tab_size — Go finally gets real tabs). **Shift-Tab**
  (`editor::OutdentLine`) removes up to one unit from each cursor's line
  start, one batch. Phase 5 hooks snippet expansion into the same Tab action.
- **Enter** upgrades `Command::Newline` to auto-indent: per cursor, the new
  line keeps the current line's leading whitespace plus one extra unit when
  the current line's trimmed text ends with `{`, and removes one unit when
  the *new* text after the caret starts with `}` (typing `}`-closing keeps
  the closing brace aligned). One change per cursor, one batch — the same
  `Cause::Typing` funnel, so undo grouping and server sync are unchanged.
- **Horizontal scroll**: `scroll_x: Cell<f32>` on the pane, carried in
  `Geometry` alongside the vertical `scroll`. Rows, selections, carets, find
  highlights, popups and the IME `bounds_for_range` shift by `-scroll_x`;
  `offset_at` / columnar selection add it back. Wheel x-delta scrolls
  horizontally; clamp is `0..max(0, widest_line + right_pad - viewport_w)`.
  The document's widest line is not measured eagerly (that would shape every
  line): a `max_line_w` cell keeps the **observed max** over the rows laid
  out so far — it converges as the user scrolls and is exact for any
  viewport-sized file. Resync resets it.
- **Scrollbars**: painted in the element's paint phase — a vertical track
  (right edge, 8 px) and a horizontal track (bottom edge, 8 px), thumb
  proportion = viewport / total, min thumb 24 px, subtle colors from
  `theme.rs`. Thumb drags are a new `Drag::Scroll` variant (grab offset kept
  so the thumb does not jump); track clicks page toward the click. Mouse
  handlers short-circuit on scrollbar hits before any caret/edit logic.
  Vertical scroll changes also notify (the HUD numbers stay comparable).

## 2. Semantic tokens overlay

- `LspManager::semantic_tokens(doc)` returns absolute
  `[line, utf16col, utf16len, typeIdx, mods]` quintuples with the server's
  legend. A pane-side helper maps each token onto a byte range (rope:
  `line_to_byte` + UTF-16 → char → byte walk) and onto the existing
  `highlight::Kind` palette via a conservative token-type table (keyword,
  string, comment, number, function/method, type-ish, property, macro,
  operator, regexp→string, decorator→macro); unknown types drop.
- **Overlay semantics**: semantic spans are merged over the tree-sitter
  spans with a pure `overlay(base, extra) -> merged` (extra wins on
  overlap, output sorted non-overlapping). The pane stores **one merged
  list** in `spans`, recomputed when either source lands (parse swap,
  semantic answer) — paint keeps slicing one sorted list, so per-frame cost
  is unchanged and the HUD counters keep meaning what they meant.
- **Lifecycle**: one request per edit round, debounced (400 ms) on the app
  runtime, generation-guarded like completion; fired on open, after edits,
  and after save. A missing/failed server simply leaves the tree-sitter
  paint (the overlay is additive). Edits while a request is in flight
  bump the generation, so stale answers drop.
- **The five predicate keywords** (`crate`, `mut`, `self`, `Self`, `super`):
  the tree-sitter-rust grammar models them as identifier nodes that the
  official query distinguishes with `#eq?` predicates, which this pane's
  runner does not evaluate. Fix in `highlight.rs`: after the query pass, a
  tree walk collects `identifier`/`type_identifier` nodes whose **text** is
  one of the five and appends `(range, Keyword)` spans (shorter ranges are
  processed later in `resolve`, so they overwrite any enclosing capture —
  `Self` over its `@type` paint included). Pure, unit tested, grammar
  untouched.

## 3. Code actions (⌥⏎) + format (⌘⌥L)

- `editor::CodeActions` (alt-enter): requests `code_actions` over the
  caret's word range, opens a popup reusing the completion popup's
  skeleton (kind badge + title, ↑/↓/Enter/Esc through `command()`).
- Applying: `edit` present → `Host::apply_workspace_edit` (the M3 rename
  path — this file's edits come back as `DocPatched { workspace_edit }`
  and the pane reconciles); else `resolve_code_action` first (an edit may
  arrive), then either apply that edit or `execute_command` for
  command-only actions. Stale popups (edits moved the caret) close.
- `editor::FormatDocument` (cmd-alt-l): `lsp.format(doc, None,
  FormatOptions { tab_size, insert_spaces, … })` → the returned `TextEdit`s
  convert to ascending non-overlapping `TextChange`s applied through the
  one edit pipeline under a new `Cause::Format`: its own undo step
  (a whole-file rewrite must not merge into typing), selections kept
  (clamped) rather than re-mapped to insertion ends, no completion
  follow-up. Transcript logs `[lsp] format N edits`.

## 4. Signature help + hover markdown

- **signatureHelp**: after a typing edit whose committed text ends with
  `(` or `,` (per cursor: the primary cursor decides), the pane requests
  `signature_help` (immediate, generation-guarded). The answer renders in
  a popup above the caret using the completion popup skeleton: the active
  signature's label with the active parameter's span accented
  (`Signature.parameters` are UTF-16 ranges into the label), other
  signatures dimmed, ↑/↓ cycle signatures. Closes on Esc, newline, `)`,
  caret motion, or when the completion popup opens (completion wins).
- **Hover markdown**: the pointer-rest path (M3's diagnostic hover) gains
  an LSP hover branch — when no diagnostic covers the offset, a debounced
  (300 ms) `hover` request for that offset parks `Hover { contents: Markup
  { kind, value } }`. Rendering is a minimal markdown walk (gpui-component
  0.8 has no markdown widget — checked, so we build the smallest one):
  fenced code blocks as monospace rows on a raised background; headings
  (#…###) medium-weight; list items bulleted and indented; inline `code`
  in the accent tone and **bold** as medium runs; everything else plain.
  Plain-text markup keeps the M3 single-bubble shape. Moving the pointer
  away replaces/cancels like the diagnostic hover.

## 5. Live templates / snippets

- New `templates.rs`: the IDEA-syntax model ported from the frontend's
  `features/templates/live/model.ts` — `parse_body` ($NAME$ / $END$ /
  $SELECTION$ / $$ escape), variable defaults (expressions are *not*
  ported: the builtins M5 ships only use defaults), and expansion to
  concrete text with **tab stops**.
- Indent-aware: body `\t`s render as the document's indent unit;
  continuation lines carry the expansion line's own indentation plus the
  body's relative units. `$SELECTION$` is out of scope (no surrounders in
  M5).
- A `TemplateSession` on the pane: the stop list (each stop = variable
  name + its occurrence ranges), the active index, and the expansion's
  undo entry (`Cause::Template`, one step — Esc or undo removes the whole
  expansion). Typing at the active stop replaces **all linked occurrences**
  of that variable in one batch (ranges recomputed after each edit); Tab /
  Shift-Tab walk stops (the active stop's ranges select, so typing
  replaces); the final `$END$` is a bare caret; Tab past the last stop or
  Esc ends the session.
- **Trigger**: the Tab action from phase 1 — if no session is active and
  the word before the caret equals an abbrev for the document's language,
  expand it (the abbrev deletes, the body inserts). No template management
  UI; the builtin sets are `const` tables: Go (iferr, fori, forr, func,
  meth, main, test, bench, st, inf, p, pf, go) and Rust (fn, pfn, afn, fnr,
  main, test, tmod, impl, implt, st, en, tr, match, ifl, whilel, for, loop,
  let, letm, derive, p, pd, ep, mod) — the frontend's own Go/Rust groups.
- LSP snippet-format completion items stay filtered out (M3 behavior, the
  `is_snippet` seam stays open for M6).

## 6. In-editor find/replace (⌘F) + Find in Files (⇧⌘F)

- **Find bar** (`editor::Find`, cmd-f): an overlay under the editor's top
  edge (the rename-input pattern — the pane keeps focus, the bar's input
  is rendered active). State: query input, case-sensitivity toggle,
  matches (UTF-16 starts, recomputed on query change and after every edit
  batch while open, capped), current index, count "n of m", prev/next
  (Enter / Shift-Enter), a replace toggle (⌥⌘F or the arrow button) with
  replace / replace-all. Navigation selects the match and scrolls it into
  view; replace-all is one `Cause::Replace` batch (its own undo step).
  Matches paint as translucent rects under the text (current match
  stronger). Esc closes.
- **Find in Files** (`workbench::FindInFiles`, shift-cmd-f): a new
  `FindPanel` entity in the bottom dock (a page beside Run — the dock
  header grows two tabs when both exist; M4's single-panel slot becomes a
  one-of-two page switch, `RunPanel` itself untouched). Query input +
  Enter starts `ide_search::search` on the background executor (plain
  pattern, default excludes; dirty open buffers pass through a snapshot
  overlay taken at search start). `on_file` callbacks accumulate into a
  shared buffer; the summary lands through the bus
  (`BusMessage::FindInFilesDone`). Rows: file header (rel path + count) +
  line rows (line number, line text with matched ranges accented); click
  → `OpenLocation` (open tab, caret, find-all highlight of the hit).
  Project-wide replace stays built-later (per plan).

## 7. Palette queries off the UI thread (M3 debt) + regression

- `refresh_picker` moves the index calls (`find_files`, `find_symbols`,
  `recent_files`) onto the background executor behind a generation
  counter: a query change bumps it, spawns the search, and the answer
  lands through `cx.spawn` only if the generation is current — stale
  answers drop, the UI never blocks on the index. Action filtering (the
  tiny registry scan) stays synchronous.
- Regression: M0–M4 behaviors re-driven live (edit/save/undo/paste,
  highlight, multi-cursor, folds, IME gate, ⌘O/⌘W, external reload,
  completion/⌘B/usages/rename, palette, run/git, HUD budgets).

## Threading (unchanged invariants)

| work | thread |
|---|---|
| buffer edits, popups, paint, find-in-buffer | gpui main thread |
| tree-sitter parse, semantic-token byte mapping, file search, palette index queries | background executor / app runtime |
| LSP requests | app tokio runtime → oneshot → `cx.spawn` |
| semantic/format/code-action/edit answers | same request shape as M3 |

## What M5 deliberately does not do

Template management UI, surrounders/postfix templates, LSP snippet
completions, regex search, project-wide replace, search streaming into the
dock per-file (results land once per run), scrollbar minimap, soft wrap.
Each has a seam named in the report.
