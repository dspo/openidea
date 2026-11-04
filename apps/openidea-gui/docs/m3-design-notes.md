# M3 design notes — the LSP intelligence layer

Written before any M3 code, in my own words, from openidea's own crates
(`ide-lsp`, `ide-language`, `ide-workspace`, `ide-core`, `ide-index`) and the
Apache-2.0 gpui-pre / gpui-component APIs. See `m3-cleanroom-log.md` for what
was read.

## 1. Action & keymap layer (paying M2's debt first)

M2 hardcoded editor keys in `EditorPane::on_key`. That blocks Search Everywhere
(enumerate actions) and any rebindable keymap, so M3 starts here.

- gpui already owns the machinery: `actions!` declares typed actions with
  stable string names (`"<namespace>::<Name>"`), `cx.bind_keys` maps keystroke
  strings onto them, `on_action` handlers receive dispatched actions from the
  focused element upward, and `cx.all_action_names()` + `cx.build_action()`
  enumerate/construct them for command-palette-style UIs.
- The app adds a thin **registry table** on top: one `ActionInfo` row per
  user-invocable action — stable name (matching the gpui name), human title,
  category, and its default keystroke for display. Search Everywhere filters
  this table by title; the keymap display reads the same rows. One table, two
  consumers.
- Editor keys migrate from `on_key` branching to actions: motions (with
  modifier variants), deletions, newline, cursor collapse, escape, HUD toggle.
  **Printable input stays out of the action system** — typed characters are
  raw text, not commands; they keep flowing through the IME
  (`InputHandler`) and the `key_char` fallback in `on_key_down`, exactly as in
  M0–M2. F12 (HUD) becomes an action so it shows up in the palette too.
- Double-shift (Search Everywhere) is not a keymap primitive: gpui bindings
  match keystroke *sequences*, not timed double-taps. The workbench watches
  bare shift key-down events and dispatches the action itself when two land
  within a short window.

## 2. LSP lifecycle — one process per project slice, driven by Documents

`ide-lsp::LspManager` is the whole server story; the Tauri and headless hosts
prove the pattern. The GPUI host repeats it:

- `LanguageRegistry::load` gives the `languages/*.toml` set, including each
  language's `language-servers` (rust → `rust-analyzer`, root markers,
  install hint). `Documents` gets the registry's `detect` instead of M1's
  extension-map stand-in.
- A `DocumentObserver` feeds `did_open` / `did_change` / `did_save` /
  `did_close`. Documents calls these synchronously in version order on every
  edit, which is precisely the incremental-sync feed the servers need — no
  extra UI hooks. Large files are skipped at the observer (servers choke on
  them, same as both other hosts).
- The manager needs a tokio runtime (requests use timers; peers are tokio
  tasks). The app owns one multi-thread runtime created before the host;
  `LspManager::new` runs inside `runtime.enter()` so the manager captures that
  handle. Observer callbacks are the sync notification paths — safe off
  runtime context, as the watcher threads in `ide-host` already prove.
- Feature requests (`completion`, `hover`, `goto`, `references`,
  `prepare_rename`, `rename`) are async; the UI spawns them onto the app
  runtime and receives results over a tokio oneshot, awaited from a `cx.spawn`
  future that updates the pane entity. One cancellation token per request
  (a new keystroke cancels the previous completion).
- Diagnostics and server status arrive as ordinary `IdeEvent`s on the bus
  (`Diagnostics` is a full replacement per `(path, source)`;
  `LanguageServer` carries status transitions). A missing binary surfaces as
  `ServerStatus::Missing` with the language's install hint — shown in the
  status bar; no download flow in M3.
- Quit spawns `shutdown_all` (graceful `shutdown` → exit); the manager's
  `Drop` kills peers regardless.

## 3. Diagnostics — runs in the paint pipeline, not extra elements

The pane keeps `Vec<Diagnostic>` for its path (merged across sources by the
workbench).

- **Squiggles** ride the existing run-shaping: `TextRun` supports wavy
  underlines (the M2 marked-range underline already proves the path), so
  `line_runs` splits runs at diagnostic-range intersections and marks the
  covered runs with a severity-colored wavy underline. No extra elements, no
  per-frame re-shaping beyond what M2 already does.
- **Gutter marks**: one small bar per line with at least one diagnostic,
  severity-colored, painted in the gutter's left padding during the paint
  phase (next to the fold chevrons).
- **Hover message**: plain text tooltip. Mouse moves (when not dragging)
  resolve the buffer offset; a diagnostic covering it parks `(range, message)`
  and the pane renders a small overlay div near that line until the pointer
  leaves the range or moves on. No markdown rendering in M3.

## 4. Completion — editor-owned popup overlay

- Trigger: after a typing edit, the character before the primary caret decides
  — word character → debounced request; a server trigger character (`.`, `::`
  …) → immediate request carrying the trigger. Empty result or no server →
  nothing shows.
- Filtering happens client-side over the returned items (`filter_text` or
  `label`, fuzzy-matched against the typed prefix); `is_incomplete` lists
  re-query as the prefix changes.
- The popup is a **self-drawn overlay div inside the pane**, anchored at the
  primary caret's window position (the pane's published `Geometry` already
  gives caret↔pixel mapping for the IME candidate window — same seam), kept
  inside the viewport. Chosen over gpui-component's `PopupMenu` because that
  widget takes focus and owns key handling; the editor must keep focus (IME,
  typing, motion) while the popup merely follows ↑/↓/Enter/Esc. The deviation
  is recorded in the report.
- Accept applies the item's `replace_range` → `insert_text` (IDE coordinates,
  UTF-16 — the pane's own offset space) plus `additional_edits`, through the
  one edit pipeline (`apply_edits` → `Documents::edit`), so undo, cursor
  mapping, tree-sitter, and server sync all see an ordinary batch.

## 5. Navigation & rename

- ⌘B / ⌘-click → `goto(Definition)`; zero results → status message, one →
  jump, many → jump list in the shared overlay list. Jumping within the file
  moves the primary selection; across files opens (or activates) the tab and
  then places the selection.
- Find usages → `references(project_wide: true)`; results listed as
  `rel_path:line` rows with the line's text.
- ⇧F6 → `prepare_rename` (falls back to the word range) → a small modal with
  a text input prefilled with the current name → `rename` → the returned
  `WorkspaceEdit` is shown as a **preview** (files, edit counts, per-edit
  before/after lines) with Apply/Cancel → Apply calls
  `Documents::apply_workspace_edit`, which lands on open tabs as
  `DocPatched { workspace_edit: true }` events the M2 pane already reconciles.

## 6. Go to File & Search Everywhere — the index does the matching

`ide-index::ProjectIndex` is the backend's own fuzzy engine (nucleo-based,
IDEA-like scoring, recency, CamelHump, match indices for highlighting). The
app opens it against the project root with a per-project cache directory
(FNV-1a key of the root, same convention the headless host uses) and:

- ⇧⌘O (Go to File) → `find_files(query, limit)`; empty query lists recent
  files; opening marks recency.
- Double-shift (Search Everywhere) → one list, three sections: Files
  (`find_files`), Symbols (`find_symbols`, all kinds), Actions (the registry
  table filtered by the in-app matcher).
- Actions are not in the index, so the app carries a small matcher — ported
  from this repo's own TypeScript (`src/ui/match.ts` + the scoring in
  `src/features/search/common/rank.ts`, both Apache-2.0 project code) into
  `src/fuzzy.rs`, unit-tested. Substring → word/CamelHump → subsequence,
  scored exact > prefix > contiguous > hump > spread.
- Index queries run on the UI thread for now (fixture-scale projects answer
  in single-digit ms); moving them behind the request channel is an M4 seam.

## 7. Threading summary

| work | thread |
|---|---|
| document edits, pane state, paint | gpui main thread |
| tree-sitter parses, index scans (if moved), file IO | gpui background executor / dedicated threads (existing) |
| LSP process IO, request futures, timers | app-owned tokio runtime |
| LSP results → UI | tokio oneshot → `cx.spawn` → entity update |
| diagnostics, server status, patches | bus sink → 50 ms pump → workbench (existing) |

## 8. M4 seams left deliberately

- `Host` will expose `lsp` + `index` + `documents` — M4 (debugger) can start
  servers and observe documents without new plumbing.
- The overlay list widget (jump list / completion / panels) is one component;
  semantic-token spans can join `line_runs` the same way diagnostics do.
- `find_files` off the UI thread; keymap table → user-editable keymap;
  completion resolve (`completionItem/resolve`) for docs/detail.
