# M3 Report — the LSP intelligence layer

**Date:** 2026-11-04 · **Branch:** `feat/gpui-m3-insight` (based on
`feat/gpui-m2-editor`; M0–M2 branches untouched) · **Platform:** macOS 26
(arm64), app-local toolchain 1.97.1, system rust-analyzer 1.96.1.

**Verdict: M3 scope delivered.** The GPUI host gained the action/keymap layer
(M2's debt, paid first), the language-server life cycle through `ide-lsp`,
diagnostics with squiggles/gutter marks/hover, completion with an
editor-owned popup, goto-definition / find-usages / rename with preview, and
Go to File + Search Everywhere over `ide-index` with the action registry.
All phases landed as separate commits, each green (fmt + clippy `-D
warnings` + unit tests) when committed. M0–M2 behaviors were re-driven live
at the end; M3's exit criteria (edit Rust through the app: completion,
⌘B, rename all usable) were exercised on a scratch copy of `fixtures/rust-app`.

## What was built (by phase, commit per phase)

| phase | commit | content |
|---|---|---|
| docs | `c22267cb` | `docs/m3-design-notes.md` + `docs/m3-cleanroom-log.md` (written **before** any code; no Zed reads needed for M3 — see clean-room) |
| 1 | `98345669` | action/keymap layer: every editor/workbench behaviour is a gpui action (stable `"<ns>::<Name>"`), bound in `main.rs` after `gpui_component::init` (later registrations win keystroke ties); `actions.rs` carries the `REGISTRY` table (title/category/keys) — the Search Everywhere source and keymap display in one place. Printable input deliberately stays out of the action system (IME + raw fallback, unchanged from M0–M2) |
| 2 | `76823a9d` | LSP life cycle: `Host` owns an `ide_lsp::LspManager` built from `ide-language`'s registry (languages/*.toml: servers, root markers); the `DocumentObserver` feeds didOpen/didChange/didSave/didClose in version order (same shape as `ide-host`'s observer); feature requests run on the app-owned tokio runtime and answer through oneshots awaited from `cx.spawn`; quit runs a graceful `shutdown_all` |
| 3 | `7cce6ca6` | diagnostics: `IdeEvent::Diagnostics` (per-source full replacement) merged per file in the workbench, handed to the pane; wavy underlines ride the M2 run-shaping (`TextRun` underline), gutter severity marks paint next to the fold chevrons, plain-text hover tooltip parks on the diagnostic under the pointer |
| 4 | `92648669` | completion: typing schedules a debounced request (immediate on a server trigger char); the answer opens a **pane-owned overlay** (not a focus-taking widget — the editor keeps IME/keys), filtered client-side against the typed word; Enter applies `replace_range` + `additional_text_edits` through the one edit pipeline (undo, tree-sitter, server sync all see an ordinary batch) |
| 5 | `5c8d7e4c` | navigation & rename: ⌘B / ⌘-click → goto definition (one hit jumps, many open a jump list, none notifies); ⌥F7 → project-wide references; ⇧F6 → rename input overlay → server `WorkspaceEdit` preview (files + edit counts, Apply/Cancel) → applied through `Documents::apply_workspace_edit`, so open tabs reconcile via `DocPatched { workspace_edit }` |
| 6 | `c8496f60` | Go to File + Search Everywhere: `ide-index::ProjectIndex` opened per project (FNV-1a cache key under the platform cache dir, fed by the same watcher as the documents); ⇧⌘O palette (files; empty query = recents), double-⇧ palette adds Symbols + Actions; action rows come from the `REGISTRY` filtered by `src/fuzzy.rs`, a port of the repo's own TypeScript matcher (`src/ui/match.ts` + `search/common/rank.ts`), unit-tested |
| fix | `a44f2e41` | rename overlay focus: the input view unmounts when the request goes out; command() now takes the window and hands focus back to the pane (found live — a focus left on the unmounted input silenced every action, ⌘B included) |
| fix | `82553503` | double-⇧ gesture: modifier-only presses never become key events on macOS (NSFlagsChanged → ModifiersChanged), so the "shift shift" binding and the raw key-down watcher were dead code; the gesture now lives in `on_modifiers_changed` on the rising edge of a bare shift |

14 unit tests in the crate cover the query/capture mapping, fold display
map, selection math, inverse changes, the action registry invariants and the
ported fuzzy matcher (ranking order, word alignment, multi-token, indices).

## LSP wiring notes (what the GPUI host does differently)

- **One manager per open project**, exactly like the Tauri/headless hosts:
  `LspManager::new(root, languages, sink)` inside `runtime.enter()` so the
  manager captures the app runtime's handle. The runtime is a plain
  multi-thread tokio runtime built before gpui starts; `runtime.block_on`
  wraps the whole app loop, which keeps server IO driven continuously.
- **Document sync is free**: `Documents::edit` is the single funnel the M2
  editor already pushes every keystroke through, and the observer converts
  it to incremental `didChange` (version-ordered, `AppliedChange` is already
  LSP-shaped). No UI hooks were added for sync.
- **Requests**: completion/goto/references/rename spawn onto the runtime and
  answer through a `tokio::sync::oneshot`; the awaiting side is a
  `cx.spawn` future that updates the pane entity. One generation counter
  (completion) / cancel token per request; stale answers are dropped.
- **Events**: diagnostics and server status arrive as ordinary `IdeEvent`s
  on the 50 ms bus pump — the same channel the watcher uses. A missing
  binary surfaces as `ServerStatus::Missing` with the language's install
  hint in the status bar (no download flow in M3, per plan).
- rust-analyzer was already installed locally (1.96.1); no toolchain changes
  were made.

## Action registry design

- `actions!` namespaces: `workbench::*` (window-level), `editor::*` (pane),
  `palette::*` (list plumbing, deliberately not in REGISTRY).
- Bindings live in one `cx.bind_keys` block in `main.rs`; editor commands use
  the `"Editor"` key context, palette navigation uses `"Palette"` and is
  registered **after** `gpui_component::init` so it wins equal keystrokes
  against the query input's own bindings (gpui resolves in reverse
  registration order — verified live: ↑/↓ move the palette selection, not
  the input cursor).
- `REGISTRY` rows are the Search Everywhere Actions source (title, category,
  default keys) and double as the keymap display. A unit test pins name
  uniqueness and labelling.
- Execution from the palette: `cx.build_action(name, None)` +
  `window.dispatch_action(...)` **after** closing the palette and restoring
  editor focus (dispatch is deferred to the focused element; verified live —
  running "Toggle Perf HUD" from the palette toggled the editor HUD).

## Live verification (dogfooding transcript, commands + results)

Driven with synthetic CGEvents posted straight to the app PID
(`/tmp/m3drive.py`; keyboard events bypass the frontmost app, mouse events
needed the screen unlocked — see constraints below) against a scratch copy
of `fixtures/rust-app` (`/tmp/m3-dogfood`; the repo's fixture and sources
were never edited; stray edits were `rsync`-restored and are gone).

- **Completion**: typed a word prefix after `use crate::Item;` — popup
  opened at the caret with rust-analyzer's items, filtered ("parse" showed
  the `parser` module row); Enter applied the item through the edit
  pipeline; ⌘S wrote it to disk (`sed`/`grep` on the scratch file); ⌘Z
  reverted the accept (per-entry undo granularity verified). Transcript:
  `[ime] insertText` per char + disk round trip.
  `assets/m3-7-completion-popup.png`.
- **⌘B goto definition**: caret on `Item` in `use crate::Item;` →
  transcript `[lsp] opened /private/tmp/m3-dogfood/src/lib.rs` (cross-file
  jump opened the tab and didOpen'd it to the server). Same-file jumps move
  the caret (verified during the rename navigation).
- **⌥F7 find usages**: caret on `ParseError` → jump list with all 7
  project-wide references (`src/parser.rs:6/11/18/20/21/25/56`),
  first row selected. `assets/m3-7-find-usages-jump-list.png`.
- **⇧F6 rename**: input overlay prefilled with the identifier → new name →
  Enter → server workspace edit → preview panel ("Rename to `ParseFailure`
  — 7 edits in 1 files", Apply/Cancel) → Apply → **all 7 sites on disk are
  renamed** (`grep -c ParseFailure` = 7; diff vs fixture shows every site).
  `assets/m3-7-rename-input.png`, `assets/m3-7-rename-preview.png`,
  `assets/m3-7-renamed-applied.png`.
- **Diagnostics**: pristine fixture (intentional `unused_counter` warning) —
  orange squiggle under the identifier, warning-colored gutter mark on the
  line. `assets/m3-7-diagnostics-squiggle-gutter.png`. (Hover tooltip driven
  but not captured this session — see manual checklist; phase-3 evidence
  from the same branch: `assets/m3-3-hover-tooltip.png`.)
- **Go to File (⇧⌘O)**: palette with query input; "lib" lists matching
  files from the index; Enter opened lib.rs (transcript `[lsp] opened …`).
  Empty query lists recent files (recency fed by every `open_path`).
  `assets/m3-7-go-to-file.png`.
- **Search Everywhere (double ⇧)**: palette opens in everywhere mode; "open"
  lists a Files section (`.openidea/.gitignore` — the index's own match)
  and an Actions section ("Open File… ⌘O") with fuzzy highlight; Enter
  executed "Toggle Perf HUD" through `build_action`/`dispatch_action` —
  the editor's `[perf] hud on …` line in the transcript is the editor
  receiving the dispatched action. `assets/m3-7-search-everywhere.png`.
- **Fuzzy matcher**: `cargo test fuzzy` (7 tests) — ranking exact > prefix >
  contiguous > hump > spread, word-aligned rejection, multi-token, indices.

## M0–M2 regression, re-driven live at the end

- **Editing + save round trip**: typed probes reach the buffer, ⌘S writes
  through the VFS (`cat` before/after each probe; scratch restored after).
- **Undo**: ⌘Z reverts a completion accept to its pre-accept state (disk
  verified), then re-save.
- **Dirty / stale / external reload**: typed buffer → tab `●` +
  "unsaved changes" (accent); external change while dirty (`rsync` over the
  scratch file) → tab `● ⚠` + "stale — changed on disk" + status message —
  `assets/m3-7-dirty-stale-regression.png`. External change on a clean
  buffer auto-reloads through the version-based `DocPatched` path (exercised
  in the M2 sessions; the code path is unchanged in M3).
- **Highlighting**: Rust query paints on every open (all screenshots);
  HUD shows per-keystroke incremental re-parse (1.4 ms @v2/v3).
- **Folding**: chevrons render on all shots; click-to-fold needs a precise
  mouse hit — re-verified in the M2 sessions, code untouched in M3.
- **⌘O / ⌘W / IME-in-tab**: ⌘W verified live (tab closes, document closes —
  insertText stops). ⌘O panel and real-IME composition (Squirrel) are
  manual-checklist items this session (the IME needs the machine's input
  source; synthetic keys bypass it by design).
- **Performance**: HUD counters unchanged (key→px 5.3–6.4 ms during the
  session; parse 1.4 ms per batch) — no regression against the M2 budgets.

## Deviations from the brief

- **⌘-click moved off multi-cursor**: the brief requires ⌘-click = goto
  definition (IntelliJ's macOS mapping). M2's click-add-cursor migrated to a
  bare **⌥-click** (⌥-drag keeps the column selection) so the capability
  stays. ⌥-click was driven but not visually confirmed this session —
  manual checklist.
- **prepareRename is not called** (design notes' fallback path): ⇧F6 uses a
  locally computed word range and lets the server's `rename` validate.
  Server round trip for the range is an M4 refinement.
- **Jump list rows show `rel_path:line`** (accent) without the matched
  line's text — kept minimal; line text is an M4 polish.
- **The completion popup is pane-owned** (deviation from "gpui-component
  list popup" in the task text, decided in the design notes): the editor
  must keep focus for IME/typing/motion, so the popup is a self-drawn
  overlay that follows ↑/↓/Enter/Esc through `command()`.
- **Rename Enter timing quirk (found, worked around in driving)**: the very
  first Enter after the input receives synthetic select-all + typing can be
  swallowed by the input's IME state; a second Enter confirms. Human typing
  was not affected in earlier sessions; noted for M4.
- **Undo does not close the completion popup** (arrows keep navigating it
  after ⌘Z). Minor; M4 polish.

## Manual verification checklist (not scriptable here)

- [ ] Hover tooltip: rest the pointer on the `unused_counter` squiggle —
      plain-text warning tooltip appears below the line (phase-3 evidence
      `assets/m3-3-hover-tooltip.png`; this session drove the mouse to the
      token but could not confirm the tooltip render — needs eyes).
- [ ] Real IME composition inside a tab (Squirrel/Pinyin): marked range,
      candidate window, commit — M0/M2 verified; synthetic keys bypass the
      IME so this session could not re-run it.
- [ ] ⌥-click adds a cursor at the point; ⌥-drag column select; Esc
      collapses (M2 visual evidence exists for the original ⌘-click; the
      migrated ⌥-click needs one human pass).
- [ ] Fold chevron click folds/unfolds (M2 verified; chevrons visible in
      every M3 screenshot).
- [ ] ⌘O native open dialog end to end.
- [ ] Input-source switch to ABC for scripted typing, back to Pinyin.

## Leftovers / known limitations (M4 seams)

- `prepare_rename` round trip; rename preview per-edit before/after lines
  (currently files + counts only).
- Completion `completionItem/resolve` for documentation/detail columns;
  snippet support (snippets are filtered out today).
- Semantic token coloring (optional M3 item — not started; `line_runs` is
  the seam where spans would stack).
- Hover markdown rendering (plain text only); code actions / quick fix
  (⌥⏎); formatting; in-editor find; debugger (M4).
- Paste (⌘V) still unimplemented (M2 leftover; no clipboard wiring in the
  gpui host yet).
- Palette: no scroll/cursor-into-view for long lists (max 420 px panel);
  index queries run on the UI thread (fixture scale answers in ms — M4
  moves them behind the request channel); `find_files` ranking questions
  (recent-boost can outrank weaker matches — IDEA-like, but worth a look).
- `undo` keeps the completion popup open (see deviations).
- Big-file policy (read-only > 5 MB) unchanged from M2.

## Engineering hygiene

- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (21 unit tests) — clean in `apps/openidea-gui`.
- Main workspace untouched: `cargo metadata --no-deps` = 36 members, root
  `Cargo.lock` diff empty on this branch. `apps/openidea-gui/Cargo.lock`
  committed with the new `ide-index`/`dirs` path/registry deps.
- Scratch discipline: all dogfooding ran against `/tmp/m3-dogfood` (a copy
  of `fixtures/rust-app`); the fixture and all repo sources are byte-identical
  to the branch (`git status` clean outside `apps/openidea-gui`).
- The automation session drove the app with CGEvents posted to the PID
  (`/tmp/m3drive.py`, throwaway); the machine's screen was locked for the
  first half (window frames froze — events still processed, which is how
  the transcript evidence was collected) and unlocked for the screenshot
  pass. No process other than this session's app instances was touched
  (PIDs recorded in `/tmp/m3-app.pid`).

## Clean-room statement

Zed (GPL-3.0) was **not read** for M3 — everything needed is openidea's own
crates (`ide-lsp`/`ide-language`/`ide-index`/`ide-workspace`) plus the
Apache-2.0 gpui-pre/gpui-component APIs and this repo's own TypeScript
matcher. `docs/m3-cleanroom-log.md` records the sources consulted (all
Apache-2.0): gpui-pre action/keymap/window registry sources, gpui-component
input/list sources (evaluated and rejected for the completion popup —
focus-taking), openidea crates, and the two TypeScript matcher files ported
to `src/fuzzy.rs`.
