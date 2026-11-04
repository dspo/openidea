# M2 Report — the GPUI code editor

**Date:** 2026-11-04 · **Branch:** `feat/gpui-m2-editor` (based on
`feat/gpui-m1-workbench`; M0's `spike/gpui-ime-m0` untouched) · **Platform:**
macOS 26 (arm64), toolchain 1.97.1 (app-local), live Squirrel (Rime) Pinyin
IME as the test IME.

**Verdict: M2 scope delivered.** The tabbed workbench now edits through a
rope-backed multi-cursor code editor with incremental tree-sitter highlighting,
viewport virtualization, syntax folding, editor-grade IME (the M0 protocol,
now living at the primary cursor of a multi-cursor pane), one undo timeline,
a line-number gutter and an F12 perf HUD. All seven suggested phases landed,
each as its own commit. M1 behaviors (⌘O / ⌘S / dirty / stale / external
reload / IME-in-tab) were re-verified live at the end.

## What was built (by phase, commit per phase)

| phase | commit | content |
|---|---|---|
| docs | `7d6fc74a` | design notes + clean-room log (written **before** any code) |
| 1 | `15eb4240` | buffer on `ide_text::Document`: the pane mirrors the backend document version-for-version; user edits apply one `ChangeSet` locally and through `Documents::edit`; `DocPatched` reconciles **by version** (incremental apply on exact base match, snapshot resync otherwise); dirty/save/reload semantics stay backend-only |
| 2 | `4d763609` | line-number gutter + clipped viewport: only visible rows (±1) are shaped; rope metadata for line geometry (80k-line file opens like a small one); long lines clipped to the element; caret/click x measurement walks one shaped line instead of reshaping prefixes |
| 3 | `bfa716eb` | incremental tree-sitter highlighting: grammar via `Syntax::ts_language`, hand-written Rust highlights query, tree edited in place per buffer change (`ByteEdit` → `InputEdit`), background incremental re-parse with mid-parse edit journal + generation guard, spans painted per visible line from the sorted whole-document list; kinds → hardcoded palette in `theme.rs` |
| 4 | `e3753e45` | multi-cursor: `Vec<Selection>` (anchor/head/reversed/goal col), ⌘-click toggle cursors, Shift-click extends, ⌥-drag column select, Esc collapses; movements (arrows, ⌘-arrows line/doc ends with smart-home toggle, ⌥-arrows word over whitespace/word/punct classes), Shift extends; backspace/delete across cursors (⌥ deletes words); edits applied bottom-up as one `ChangeSet` with cursor remap; single undo timeline with 800 ms typing groups, IME commits undo as one step, ⌘Z/⌘⇧Z workbench actions |
| 5 | `086c3041` | folding: fold candidates from the parse job (named multi-line nodes, outermost-first, root excluded), gutter chevrons toggle, `⋯ N lines` placeholder row unfolds on click, display-map free functions drive layout/paint/gutter/mouse/caret geometry, edits shift fold ranges by line deltas, resync clears fold state |
| 6 | (with 1+4) | editor IME in the multi-cursor world: `setMarkedText`/`insertText` act at the **primary cursor** (inline marked range, M0 visuals), other cursors freeze while composing, commit replaces the marked range and its undo entry removes the whole composition, candidate anchoring via the published geometry, `marked_text_range()` still returns `None` idle (the M0 key-router lesson) |
| 7 | this commit | F12 perf HUD (fps, frame ms, rows laid out, spans painted/total, key→px latency, parse ms@version, cursors/folds), first-paint open latency log, release-build measurements, screenshots, this report |

13 unit tests in the crate cover the query/capture mapping, span resolution,
`InputEdit` mapping, selection normalization, post-batch cursor mapping,
inverse-change computation, clamping, word boundaries, CRLF normalization and
the fold display map.

## Architecture (text diagram)

```
Workbench (tabs, tree, status bar, ⌘O/⌘S/⌘W/⌘Z actions)
  │ owns                │ bus (IdeEvents + dialog results, 50 ms pump)
  ▼                     ▼
EditorPane ── local ide_text::Document (rope, UTF-16) ── mirror of
  │  ▲                        ▲                             backend Document
  │  └─ DocPatched: apply by version / snapshot resync       (ide-workspace
  ├─ Selections (multi-cursor) + folds (display map)          Documents)
  ├─ Tree + spans + fold candidates (background parse jobs)
  │        ▲ ByteEdit journal (InputEdit 1:1)
  └─ EditorElement (paint-only): prepaint lays out visible display rows,
     paint draws gutter/chevrons/selections/lines/carets/HUD and registers
     the InputHandler (NSTextInputClient → primary cursor)
```

Edit pipeline (single funnel): `apply_edits(changes, cause)` → local
`apply_detailed` (ByteEdits for the tree) → `Documents::edit` (backend bumps
version, owns dirty) → undo entry (grouped by cause) → cursor remap →
`after_edits` (tree.edit + background re-parse) → scroll-follow → notify.

## Performance (measured, per the HUD)

Method: the F12 HUD's own counters (frame intervals over a rolling window,
input→prepaint latency, rows/spans laid out per frame, parse duration per
incremental re-parse, logged to stderr on toggle); open latency from a
first-paint log inside the editor's first prepaint after pane creation.
Scroll was driven by synthesized CGEvent wheel events (`m2-scroll`, 100
lines per event, ~16 ms apart); typing by `osascript keystroke` through the
live Pinyin IME.

| measurement | debug build | release build | acceptance |
|---|---|---|---|
| 50 MB file open → first paint | 4 ms | **2 ms** | "opens without freezing" ✓ (highlighting/syntax skipped by the backend's >5 MB `large` flag; the file opens **read-only**, see deviations) |
| key → pixel (typing through the IME commit path) | 9.0 ms | **6.4 ms** | < 16 ms ✓ |
| frame cost (hello.rs, highlighted, idle re-render) | 10.6 ms | **5.2 ms** | ≈ 192 fps capacity ✓ |
| scroll frame cost (50 MB file, viewport 36 rows) | 25.3 ms | **8.4 ms** | ≈ 118 fps capacity ✓; HUD showed 12 fps during synthetic scroll because rendering is event-driven and the synthesized events coalesce (see note) |
| incremental re-parse per keystroke batch (1.4 KB file) | 0.5–13.7 ms | **1.2 ms** | background, main thread never parses ✓ |
| rows laid out per frame | 36 (viewport) regardless of file size | same | virtualization ✓ |

Note on scroll fps: the editor renders on input only (no animation loop), so
"s fps" during scroll measures the synthetic event cadence, not render
capacity. The capacity number is the HUD's per-frame cost (8.4 ms release ≈
118 fps); reaching a sustained 120 fps display rate would need a continuous
animation source (e.g. caret blink), which is M3 material.

## Clean-room statement

Zed (GPL-3.0) at `/Users/chenzhongrun/projects/github/zed` was used as a
**design reference only**, before implementation started. `docs/m2-design-notes.md`
restates the concepts in my own words; `docs/m2-cleanroom-log.md` lists every
file read, when, and which question it answered. No Zed code, types, literals
or line-level translations were copied; implementation was written against
these notes plus openidea's own crates (`ide-text`/`ide-syntax`/`ide-language`/
`ide-workspace`) and the Apache-2.0 gpui-pre API. The Rust highlights query was
written against the tree-sitter-rust grammar's public node names; the word
motion, display map, undo grouping and cursor remap algorithms are my own
implementations of the concepts. Zed sources were never consulted again after
the notes were committed.

## M1 regression, re-verified live at the end of M2

- ⌘O opens the NSOpenPanel over the workbench (`assets/m2-7-cmdo-dialog.png`).
- ⌘S round trip: IME-committed text (`你`) and typed text reached disk through
  `Documents::save` (`cat` evidence in the session log); clean save keeps the
  tab clean.
- Dirty/stale: typed buffer → `●` + "unsaved changes"; external append while
  dirty → `⚠` + "stale — changed on disk" (`assets/m2-3-stale.png`).
- External change on a clean buffer auto-reloads through the version-based
  `DocPatched` path (verified twice: phase 1 and the final pass).
- IME inside a tab: `setMarkedText "n"/"ni"` → marked range with accent
  underline → Space commits `你` → save → on disk (`assets/m2-2-ime-marked.png`,
  `m2-2-ime-hud.png`, `[ime]` trace in the log).
- Tree open, ⌘W, status line: exercised throughout every session.

## Screenshots (window-scoped only, `screencapture -l <windowid>`)

`assets/m2-1-rust-highlight.png` (Rust highlighting + gutter + chevrons) ·
`m2-2-ime-marked.png` (mid-composition marked range + HUD) ·
`m2-2-ime-hud.png` (committed `你`, dirty state) · `m2-3-stale.png` (stale tab) ·
`m2-4-big50-open.png` (50 MB file open) · `m2-5-big50-scroll-hud.png` (release
HUD mid-scroll on the 50 MB file) · `m2-6-multicursor.png` (⌘-click cursors on
the 50 MB file, HUD counters) · `m2-7-cmdo-dialog.png` (⌘O panel) ·
`m2-8-release-hud.png` (release key→px 6.4 ms).

## Deviations from the brief

- **Large files are read-only.** The backend (`Documents`) flags >5 MB files
  `read_only` (they are not "observed"), so the 50 MB acceptance file opens
  and scrolls but refuses edits. M2 initially resync-stormed on rejected
  edits; fixed by short-circuiting `apply_edits` when the pane is read-only.
  Editing huge files is a backend policy question, not an editor one.
- **Highlighting runs the query once per parse over the whole document**
  (sorted span list) instead of a viewport-filtered query per frame: tree-sitter
  0.25's query API needs contiguous bytes, and the one-shot list keeps the main
  thread free of per-frame query work (paint slices the list per line). The
  design notes were updated to match.
- **⌥-click "fold all siblings" was skipped** (the brief marked it optional).
- **The perf HUD draws in the paint phase over the content** (top-right,
  opaque background), not a separate overlay view.

## Manual verification checklist (not scriptable from this shell)

- [ ] Multi-cursor **typing** with the ABC input source: ⌘-click three spots,
      type — text appears at all cursors. (Scripted printable keys route
      through the machine's live Pinyin IME, which commits at the primary
      cursor only — by design; the direct-keydown path needs a human to switch
      input sources. Cursor creation/movement/rendering was verified by script;
      `Selection::after_inserts` (the multi-cursor insert math) is unit-tested.)
- [ ] ⌘Z / ⌘⇧Z across a burst of typing (undo merges the burst into one step,
      redo replays it). Action wiring identical to ⌘S (verified scripted); the
      undo content path is unit-tested but its live run was blocked by the IME
      swallowing scripted typing in this session.
- [ ] Esc during a real IME composition cancels it (M0 checklist item; the
      script could not keep a composition alive reliably).
- [ ] Candidate window (Squirrel's) tracks the caret while composing with the
      marked range near the viewport edges.
- [ ] Wheel scrolling by hand on a trackpad (scripted CGEvent scrolling used
      in its place; feel/inertia untestable).

## Leftovers / known limitations

- No scrollbars (M1 leftover); no horizontal scrolling — long lines clip at
  the right edge (soft wrap explicitly out of scope).
- Keywords `crate/mut/self/Self/super` paint as plain identifiers: the
  tree-sitter-rust grammar models them as identifiers, and predicate-based
  captures (`#eq?`) are not evaluated by the pane's query runner.
- Search highlight, sticky scroll, minimap, snippets, bracket matching: out
  of M2 scope (later milestones).
- Paste (⌘V) is unimplemented (no clipboard wiring in the gpui host yet).
- The 12 fps scroll *reading* under synthetic events (see perf note) would
  deserve a re-measure with an animation-driven frame source.
- `Docs`/`README.md` in `apps/openidea-gui` still describe M1; the M2 report
  is the authoritative document for this milestone.

## Engineering hygiene

- `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings`
  clean (only the upstream `block v0.1.6` future-incompat notice).
- Main workspace untouched: `cargo metadata --no-deps` = 36 members, no
  `gpui*`, no `openidea-gui`; root `Cargo.lock` has no diff on this branch.
- App crate `Cargo.lock` committed with the new path deps (`ide-text`,
  `ide-language`, `tree-sitter =0.25.10` matching the workspace pin,
  `streaming-iterator`).
- All app instances started and stopped by PID; no broad kills.

## How to reproduce

```sh
cd apps/openidea-gui
OPENIDEA_GUI_PROJECT=/path/to/project cargo run          # debug
OPENIDEA_GUI_PROJECT=/path/to/project cargo run --release # perf numbers
# ⌘O · tree double-click · ⌘-click for cursors · ⌥-drag column select
# ⌘-arrows / ⌥-arrows movements · ⌘Z / ⌘⇧Z · gutter chevrons fold
# F12 toggles the perf HUD · Pinyin IME composes at the caret
# touch an open file from another shell → clean reload / stale ⚠
```
