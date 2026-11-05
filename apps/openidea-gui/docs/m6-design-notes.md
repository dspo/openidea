# M6 design notes — Go development experience depth

Written before any M6 code, from openidea's own crates (`ide-lsp`, `ide-core`,
`ide-workspace`, `ide-index`), the M0–M5 reports' seam notes, and the app's own
editor/workbench sources. Zed (GPL-3.0) was **not** read for M6 — see
`m6-cleanroom-log.md`. gopls behavior is probed directly against a raw stdio
session replicating the client capabilities we advertise (`caps.rs`), the same
method M5 used for the semanticTokens opt-in discovery.

One constraint shapes the whole milestone: **`crates/` stays at zero diff**
(the brief hardens the roadmap's "main workspace unaffected" rule; M0–M5 never
touched it either). Everything below is therefore `apps/openidea-gui` work plus
at most a `languages/*.toml` data change, and the one place the LSP data layer
cannot reach (diagnostic related information) is recorded as a leftover with
its exact seam, not worked around.

Eight phases, one commit each, each green (fmt + clippy `-D warnings` + unit
tests) when committed.

## 1. Completion root cause + the Go completion face

M5's probe: gopls logged `no completions found … package completions: invalid
position for package completion: cursor after expression` and the popup never
opened at member access.

Two layers to separate before touching anything:

- **Server layer** (probe first): a raw stdio session with our exact initialize
  params, a valid Go module, completion requests after `fmt.` — items expected.
  The `package completions: invalid position` line is expected to be gopls's
  *auxiliary* unimported-package completion path logging a harmless miss, not
  the reason the list was empty.
- **Client layer** (the suspect): we advertise `snippetSupport: true`, so gopls
  answers member-access completions almost entirely as snippet-format items
  (`insertTextFormat: 2`). The M3 popup **filters every `is_snippet` item out**
  (`CompletionPopup::filtered`) and `completion_ready` closes the popup when
  *all* items are snippets — which for gopls is every member access. The same
  popup over rust-analyzer showed *something* because rust-analyzer mixes plain
  items in. The "empty completion" was our filter, not gopls.

Fix, in the app only:

- `templates.rs` grows a pure `lsp_snippet_to_expansion(body) -> Expansion`:
  the LSP snippet grammar (`$1`, `${1:name}`, `${1|a,b|}`, choice lists,
  `\` escapes) parsed into the same `Expansion { text, stops }` model the live
  templates already drive, so accepting a gopls completion lands with the
  existing **linked tab stops** (Tab walks, typing mirrors). Unit tested.
- `CompletionPopup::filtered` keeps snippet items; `completion_accept` routes a
  snippet item through the template session path (one `Cause::Template` batch,
  linked occurrences) and a plain item through the old path. `Esc`/motion
  behavior unchanged.
- Popup polish for Go comes free through data already converted by `ide-lsp`:
  `labelDetails.description` (`fmt` on `Println`) and `detail` rows render
  since M3; `additionalTextEdits` (the import addition for an unimported
  package suggestion) already applies in `completion_accept`.

**References/rename re-drive**: the M5 miss was a probe artifact to re-check,
not a known defect — `ide-lsp`'s references/rename already widen beyond the
server's scope via the candidates hook; the app's `host.rs` installs the
apply-edit hook but **no candidates hook**, so widening silently degrades to
the server's own scope. gopls's scope is the whole module (fine for Go), but
the hook is one `ProjectIndex`-worded closure; wire it and re-drive
cross-file usages + rename on the scratch project (rust-analyzer comparison
included).

## 2. Organize imports on save

The roadmap names `willSave`/`waitUntil`; our client capabilities (frozen in
`caps.rs`) advertise `willSave: false`. The client-driven shape achieves the
same UX without those caps and is what we implement: on ⌘S for a dirty Go file
with a running server,

1. `code_actions(doc, whole-document range, only: ["source.organizeImports"])`
2. `resolve_code_action` (gopls defers the edit), apply through
   `Documents::apply_workspace_edit` (the one edit funnel; open tabs reconcile
   via `DocPatched`), then
3. `save_document` — disk receives the organized text; unused imports are
   whatever gopls's organizer drops (its behavior, not ours).

Runs on the app runtime; a generation guard makes a second ⌘S during the round
trip a plain save; any server error falls back to the plain save (saving is
never blocked by the organizer). A new `BusMessage::SaveFinished` lands the
dirty/toast update the synchronous path used to do inline. The same machinery
gets an explicit `editor::OrganizeImports` action (⌃⌥O) for manual use on any
Go file.

## 3. Inlay hints

`ide-lsp::LspManager::inlay_hints(doc, range)` exists; gopls ships its provider
**disabled by default** — the M5 lesson (semanticTokens) generalizes:
`languages/go.toml` `initialization-options` gains
`{ semanticTokens = true, hints = { assignVariableTypes = true,
parameterNames = true, functionTypeParameters = true } }` (the gopls settings
doc's names), as a separate shared-config commit with the rationale stated.

App side:

- **Viewport-driven scheduling** aligned with the M2 highlight policy: on
  open, after each edit round (debounced), after scroll, and after save,
  request `inlay_hints` over the visible display-line range widened by a
  viewport of slack. A generation counter drops stale answers; a request whose
  range is already covered by the previous answer is skipped (scrolling back is
  free). Edits invalidate the whole cache (positions moved).
- **Rendering, inline with reflow**: the hint label is inserted into the row's
  shaped content at its position — the row string becomes
  `before + hint + after`, runs split at the seams, hint runs styled muted
  (parameter kind) / accent (type kind), with the server's padding flags
  honored. Every x-mapping site must agree, so a single per-frame
  `RowHints` map (line → ordered hints with shaped widths) feeds:
  - the prepaint row assembly (augmented content + runs),
  - a `hint_shift(line, utf16_index)` helper the caret/selection/find-rect/
    completion-anchor/IME-bounds sites add for offsets past an insert,
  - the inverse in `offset_at` (a click inside a hint resolves to the hint's
    position; text past it subtracts the widths).
  `max_line_w` uses the augmented width automatically (it measures the shaped
  rows), so the horizontal scrollbar stays honest.
- Hint state invalidates on: edits, folds (a hidden line's hints drop from the
  visible map), diagnostics changing the line (visual noise), and toggle-off.
  `editor::ToggleInlayHints` (⌥⌘I? — decide: **⌥⌘I** free in the Editor
  context) flips the default-on state; the HUD gains a hint count line so the
  perf measurement can see the cost.

## 4. Quick fixes — the gopls data face

M5's ⌥⏎ chain (codeAction → menu → resolve → edit/command → apply) is the
carrier; M6 verifies it against gopls's own fix families and closes the gaps:

- `gopls.add_import` (unused/undefined package), `fill_struct`,
  `simplify` — all arrive as quickfix code actions, some command-only;
  `finish_code_action` already applies the edit then executes the command via
  `workspace/executeCommand`, whose edit answers ride the apply-edit hook.
- The menu row gains the action kind label (already) and prefers the server's
  `is_preferred` first row (server order kept).
- Organize-imports-as-action joins the registry (phase 2's explicit action).
- Live drive: diagnostics → ⌥⏎ → fix applied, on the scratch project; the
  command-only path logged in the transcript.

## 5. Diagnostics upgrade (within the crates/ freeze)

`ide-core::Diagnostic` has no related-information field and `ide-lsp`'s
`convert::diagnostic` drops `relatedInformation` — both frozen. The roadmap's
multi-hop list therefore lands as a **documented leftover** (its exact seam:
one `related: Vec<DiagnosticRelated>` field on the core type + a passthrough
in convert; a two-crate change for a future milestone that may touch crates/).
What ships instead, app-side:

- **Source classes**: gopls stamps each diagnostic's `source` (`syntax`,
  `compile`, analyzer names like `go vet`/`staticcheck`/`unused`). The gutter
  mark keeps its severity color; the hover tooltip gains a producer line
  (`gopls · syntax`) so a parse error is visually distinct from a vet warning.
- **Suggestion one-click apply**: the diagnostic hover tooltip requests the
  diagnostics-range code actions and renders each quickfix as a row; a click
  applies it (the ⌥⏎ keyboard path remains for keyboard-only flows).

## 6. Go to Symbol — `workspace/symbol` in the palette

Search Everywhere's Symbols group currently lists the text index's symbols.
M6 adds the language server's `workspace/symbol` answer to the same group:
`refresh_picker` (Everywhere, non-empty query) also spawns
`LspManager::workspace_symbols(query, cancel)` on the app runtime behind the
existing generation guard, converts each `WorkspaceSymbol` (name, kind,
container, location) into the palette's `SymbolMatch` shape (score 0, no
indices), dedupes against the index rows by `(path, position)`, and both rows
jump through the existing `OpenLocation` path. The empty-query state (recent
files only) does not query the server.

## 7. Cross-package correctness + the indexing status bar

- **Day-test suite** (live, scripted on the scratch project): rename an
  exported symbol across packages (capitalization respected), rename an
  unexported one (no illegal crossings), usages through an **embedded struct's
  promoted field**, and an **interface method's** implementations. Each driven
  through the real rename flow (prepare → input → preview → apply) with the
  workspace edit's file count as the transcript assertion.
- **Progress**: `ide-lsp` already translates `$/progress` into
  `IdeEvent::Task { title, message, fraction, done }` (gopls announces
  `go/packages load` / `indexing` bursts); the workbench's bus handler currently
  drops them. Keep a small live-task list, render `name: title message` in the
  status bar while any task is unfinished, clear on `done`. The server row's
  Ready/Busy continues to work as before.

## 8. Regression + performance + report

M0–M5 keyboard re-drive (edit/save/undo/paste, highlight, folds, IME gate,
palette, run/git, find, completion, navigation, rename, format, semantic
tokens), perf HUD sampling with hints on (key→pixel, completion first frame),
`M6-REPORT.md`, PR body at `/tmp/m6-pr-body.md`. The dogfood loop runs on a
`/tmp` scratch project (`go mod init`, two sub-packages, embedded struct,
interface) and on a copy of `fixtures/go-app` — the repo fixture untouched.

## What M6 deliberately does not do

Test tree UI, debugger, terminal, build chain, tool families, project-wide
replace (roadmap boundaries); diagnostic related-information multi-hop (blocked
by the `crates/` freeze — leftover with seam); LSP `willSave`/`waitUntil`
capabilities (frozen caps; the client-driven organize-on-save covers the UX);
gopls `staticcheck` enabling beyond a `languages/go.toml` settings suggestion
(analyzer availability depends on the local gopls build).
