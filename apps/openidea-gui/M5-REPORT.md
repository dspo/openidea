# M5-REPORT — editor maturity (the language-agnostic base)

Branch `feat/gpui-m5-editor-depth` (based on `feat/gpui-m4-run-git`), worktree
`~/worktrees/openidea/feat-gpui-m5-editor-depth`. Per the roadmap discipline:
local commits only, no push, no PR. 12 commits this milestone (list at the
bottom). All green at each commit: `cargo fmt`, `cargo clippy --all-targets
-- -D warnings`, `cargo test` (43 tests in the crate, up from 36).

## Scope delivered

All ten roadmap items landed, organized as the design notes' seven phases
(`docs/m5-design-notes.md`, written before any code):

| Phase | Content | Commit |
|---|---|---|
| notes | design notes + clean-room log | `3caa3935` |
| 1 | language-driven indent (Tab/Shift-Tab/Enter auto-indent), vertical+horizontal scrollbars, wheel horizontal scroll | `00ad96bf` |
| 2 | semantic tokens overlay over the grammar paint; the five predicate keywords (`crate/mut/self/Self/super`) | `f130ac0c` |
| 3 | ⌥⏎ code actions (LSP codeAction → menu → workspace edit / executeCommand) and ⌘⌥L formatting (`textDocument/formatting`, `Cause::Format`) | `991b3e01` |
| 4 | signature help popup (↑/↓ cycles, closes on Esc/`)`/motion) and markdown hover cards (minimal renderer — gpui-component 0.8 has no markdown widget) | `ad7bef28` |
| 5 | live templates: IDEA-syntax port of the frontend's `features/templates/live` model, Go (13) + Rust (24) builtin sets, trigger-Tab expansion, linked tab stops | `392b4e5c` |
| 6 | ⌘F in-file find/replace bar + ⇧⌘F Find in Files dock page (`ide-search::search`, per-file groups, click-to-jump); project-wide replace deliberately not built | `3adfcfeb` |
| 7 | palette index queries off the UI thread (generation-guarded) | `e52a5b43` |

The live verification then surfaced **seven follow-up fixes**, each its own
commit (details under "What the live pass caught"):

`6cd8f579` (semantic tokens never fire without a parse) · `6c2d88f2`
(languages/go.toml: gopls needs `semanticTokens=true` initializationOptions) ·
`5b2c413d` (IME commit replaces the selection) · `69e6b401` (per-character
commits mirror as inserts into linked stops) · `b85b9407` (template
continuation lines keep the base indent) · `695037d7` (⇧⌘F focuses the query
input) · `5b730af5` (Esc hands focus back from the dock) · `6a0dd974`
(`uniform_list` needs `h_full` — run + find lists painted empty) · `8aafe0a4`
(Shift-Enter with the find bar open rewrote the buffer + overlay panic).

## Exit condition — the real loop in `fixtures/go-app`

Driven as a `/tmp/m5-go-dogfood` copy (git-inited baseline; **the fixture in
the repo is untouched**, per the dogfooding rule), gopls 0.21.1 from
`languages/go.toml`'s install hint (already installed at
`~/go/bin/gopls`), the app driven with synthetic key events (the M3/M4
driver lineage, `/tmp/m5drive.py`), evidence from the app's stderr
transcript plus window-level screenshots (`screencapture -l <windowid>`).

1. **Snippet expansion** — typed `func m5probe() string {` + Enter
   (auto-indent grew one tab unit), typed `fori`, Tab:
   transcript `[template] expanded go-fori`; screenshot
   `assets/m5-02-snippet-expanded.png`.
2. **Linked stops + Tab indent** — typed `idx` (all three linked
   occurrences updated), Tab, `total`, Tab, `return ""`, Tab past `$END$`;
   ⌘S → on disk, real tabs per `languages/go.toml`
   (`insert-spaces = false`), `cat -et` shows `^I`:
   `func m5probe() string {` / `^Ifor idx := 0; idx < total; idx++ {` /
   `^I^Ireturn ""` / `^I}`. Screenshots `m5-03-template-linked-edit.png`.
3. **⌘⌥L format** — typed `var   spaces   =  42` (wrong spacing), ⌘⌥L:
   transcript `[lsp] format 6 edits`; ⌘S → `gofmt -l` reports nothing and
   `gofmt -d` is empty (`var spaces = 42`, braces dedented); screenshot
   `assets/m5-10-after-format.png`.
4. **Semantic tokens** — `[lsp] gopls -> Ready` then
   `[lsp] semantic tokens 157 spans`; screenshots before/after pair
   `m5-00-before-semantic-grey.png` (tree-sitter-less paint, all grey) →
   `m5-01-semantic-go.png` (keywords blue, types teal, strings green,
   numbers orange, comments muted).
5. **⇧⌘F search + jump** — transcript: `[find] searching "New"` →
   `[find] "New": 3 files / 11 matches in 4 ms`; the dock page renders
   per-file groups (`internal/store/store.go 4 matches`, …) with hits
   accented, screenshot `m5-05-find-in-files-results.png`; `HandleFunc`
   search screenshot in `m5-06-find-hit-jump.png`.

**Jump-to-hit caveat (honest):** the row-click → `FindHit` →
open/jump/highlight chain is implemented, but this session's synthetic
mouse clicks never reached the app's hit-testing (keyboard events work;
neither CGEventPostToPid nor AppleScript `click at` produced a click —
M2-era click evidence predates the dock, so this may be environment, not
app). The editor-side half of the chain (highlight every hit of the
query) is verified live via ⌘F: `assets/m5-07-find-bar.png` shows the
match rects painting. **Manual check:** click a Find-in-Files row once on
an unlocked session.

## Regression (M0–M4 + M5 ①–⑤), keyboard-driven

| Behavior | Result | Evidence |
|---|---|---|
| Edit → ⌘S round trip | ✓ | snippet text on disk (above); `Saved store.go` toasts |
| ⌘Z undo / ⌘S | ✓ | probe keystrokes reverted before each save; undo granularity held (typing vs template steps) |
| ⌘V paste | ✓ | `PASTED-M5-TEXT` via pbcopy + ⌘V → on disk at the caret |
| External reload | ✓ | `echo '…' >> store.go` on disk; the clean buffer reopened showed the appended comment (watcher → `DocPatched` path) |
| Multi-cursor / column select / folds | not driven | ⌥-click/⌥-drag/gutter chevrons are mouse-only and clicks were not deliverable this session (above); M2 live evidence + unchanged code stand |
| IME gate | not driven | needs a real IME session (manual list since M0); the M5 fixes touch exactly this path — re-drive manually on an unlocked session |
| ⌘⇧O Go to File / double-⇧ Search Everywhere | ✓ | palette screenshots; `main.go` opened via Enter |
| Palette (⌃R Run variant included) | ✓ | `Run command: …` entries in the transcript |
| Palette off UI thread (phase 7) | ✓ | code path exercised on every palette keystroke this session; generation guard unit-consistent, no frozen frames observed |
| ⌃R run | ✓ | `echo` probes and a real `go test ./internal/store` in the dock — command echo, stderr in red, `exit code 1 · 0.1s`, `exited Some(1)` (the failure is the fixture's expected state mid-edit); screenshot `m5-15-run-go-test.png` |
| ⌃D diff | ✓ | overlay with `@@ -95,0 +95,2 @@ (added)`, green `+` lines; screenshot `m5-16-diff-overlay.png` |
| ⌘K commit | ✓ | `[commit] dialog open, focused=true` → typed message → ⌘Enter → `[vcs] committed 084929d…`; `git log`: `084929d M5 dogfood commit`, tree clean |
| Semantic overlay over edits | ✓ | spans recomputed live (`157 → 164 → 169 spans` across the snippet edits); degrades to empty (not crash) while the file is syntactically invalid |
| ⌘F find bar | ✓ | count `13 of 13`, Enter/Shift-Enter navigation, Esc closes; match rects paint — screenshot `m5-07-find-bar.png` |
| Signature help / hover cards / ⌥⏎ quick fixes | not driven | popup code is M5-phase-4/3 code; the drivers needed (hover = mouse move, quick fix = a caret on a diagnostic) were not deliverable this session; manual list |
| Scrollbars + horizontal scroll | ✓ | vertical + horizontal tracks visible in `m5-01`/`m5-14` screenshots (always-on, thumbs proportional) |

## Performance regression (perf HUD, debug build, Go file with semantic tokens)

F12 HUD lines from the transcript and `assets/m5-17-perf-hud.png`:

| Metric | M2 budget | M5 measured (debug) |
|---|---|---|
| key → pixel | < 16 ms (release baseline 6.4 ms) | **2.9–6.9 ms** across HUD samples (`input Some(2.98)` / `Some(3.43)` / `Some(6.87)`) |
| rows laid out per frame | viewport-bounded | 35 (viewport) |
| spans painted / total | slice of one sorted list | 56–57 / 154 (Go + semantic overlay) |
| frame interval | event-driven rendering (idle gaps are cadence, not cost — M2 note) | 359–367 ms idle gap, 0.0 ms while typing |

No measurable regression against the M2 baseline: typing latency stays in
single-digit milliseconds in a **debug** build (the M2 6.4 ms reference was
release). Semantic overlay and find rects add per-frame work only on the
visible slice (HUD's spans-painted stays viewport-bounded).

## What the live pass caught (each fixed in this milestone)

1. **Go files painted grey even with gopls Ready** — two independent
   breaks: the semantic-token request was only scheduled from a landed
   tree-sitter parse (Go has no bundled highlights query, so no parse ever
   ran), and gopls ships `semanticTokens` **disabled by default** — its
   provider advertises in capabilities but full/range requests return
   empty until the client opts in via `initializationOptions`
   (verified with a raw stdio session: 0 tokens → 301 tokens). Fixes:
   schedule the overlay on open/edits without a highlighter (`6cd8f579`)
   and `languages/go.toml` `initialization-options = { semanticTokens = true }`
   (`6c2d88f2` — a data change outside the app crate; no workspace/lock
   impact).
2. **Template stops kept their defaults** — the macOS IME commits each
   character as `insertText(range=None)`; `ime_commit` fell back to
   caret..caret instead of the pane's selection, so a stop's selected
   occurrence survived while the typed text appended after it
   (`5b2c413d`).
3. **Linked occurrences lost characters** — the mirror treated every
   later commit as a whole-occurrence replace; per-character commits now
   insert at the same occurrence-relative offset in every linked range
   (`69e6b401`). Net effect verified: `fori` → three `idx` + `total` land
   correctly on disk with tabs.
4. **Template closing braces landed at column 0** — continuation lines
   with zero body tabs lost the expansion line's base indent
   (`b85b9407`).
5. **⇧⌘F didn't focus the query input** — the gesture now means "type a
   search now" (`695037d7`).
6. **No keyboard way out of the dock's input** — Esc anywhere now returns
   focus to the editor/tree (`5b730af5`).
7. **The run and find lists painted empty** — gpui's `uniform_list` needs
   an explicit height; both panels now set `h_full`. The Run dock's
   output had the same latent defect since M4 (that milestone's
   screenshot pass was blocked by the locked session)
   (`6a0dd974`).
8. **Shift-Enter with the find bar open rewrote the buffer** — the input
   handles Enter and propagates the raw key-down; it reached the pane's
   `on_key` with `key_char "\n"` and typed into the buffer at the match
   selection. The pane's raw path now stands down while a modal input
   (rename / find bar) is up, and `highlight::overlay` clamps/drops
   out-of-range spans instead of panicking the render loop (the crash
   this caused is what surfaced the whole chain) (`8aafe0a4`).

## Deviations from the brief

- **`languages/go.toml` changed** (one `initialization-options` line).
  Everything else this milestone stayed inside `apps/openidea-gui`; the
  main workspace is untouched (`cargo metadata --no-deps` = 36 members,
  root `Cargo.lock` zero diff). The roadmap's own framing — "gopls 只是换
  一份配置" — makes the language definition the right seam; the
  alternative (special-casing gopls in the app) would have been worse.
- **Row-click jump and the replace/replace-all buttons are verified by
  code + shared-path evidence only** (see the exit-condition caveat);
  both need one manual click pass on an unlocked session.
- **gopls quirks deferred to M6** (see below) — completion and references
  probing showed gopls-specific behavior differences that belong to
  M6's "gopls 全量体验接通" scope.

## Seam intelligence for M6 (gopls vs rust-analyzer)

1. **Semantic tokens are opt-in per server.** rust-analyzer answers
   `textDocument/semanticTokens/full` unconditionally; gopls needs
   `initializationOptions: {"semanticTokens": true}` and answers empty
   (not an error) otherwise — a capability-driven target selection cannot
   detect the misconfiguration. Any future server likely needs its own
   toml tuning; consider a per-language `initialization-options` review
   pass in M6.
2. **Semantic tokens degrade to empty on invalid files.** While the
   buffer is syntactically invalid, gopls returns 0 spans (rust-analyzer
   keeps answering). The overlay path handles it (empty → grammar paint
   survives / grey for Go) and recovers on the next edit round.
3. **Format errors on invalid files.** `textDocument/formatting` returns
   an error for unparseable buffers (gofmt). The UI surfaces "Format
   unavailable" in the status bar; consider surfacing *why* (syntax
   error) in M6.
4. **Completion probed empty with gopls** at member-access positions —
   gopls logged `no completions found … package completions: invalid
   position for package completion: cursor after expression`. M3's
   completion machinery is untouched and was live-verified with
   rust-analyzer; the gopls-side investigation (position encoding?
   trigger context?) is pure M6 scope.
5. **Find-usages/rename probed without a visible result** at the probed
   caret positions (no jump list / no preview within the wait windows).
   Needs a controlled re-drive with rust-analyzer for comparison before
   blaming either side.
6. **gopls lifecycle is chatty but healthy**: `Starting → Ready → Busy →
   Ready` on every launch, `go/packages.Load` bursts on edits
   (~130 ms for the fixture), semantic answers round-trip in well under
   the 400 ms debounce.

## Known leftovers (beyond the deferred list above)

- Find-in-Files results are landed once per run (no per-file streaming
  into the dock while the walk runs) — the design note's stated M5
  shape; streaming has a seam in `FindPanel.finished`.
- The dock's Find page keeps its last results forever (no clear button);
  a re-Enter with an empty query is a no-op rather than a clear.
- `FindMatches.capped` shows `10000+` in the count label but the cap also
  stops the scan silently mid-file (documented in the bar's count only).
- Rename flow seeds the input with the current name at end-of-text (no
  pre-selection), so typing appends unless ⌘A first — minor friction vs
  IDEA, which pre-selects.
- The dock focus restoration (Esc) lands on the active editor; when the
  last tab closes while the dock holds focus, Esc focuses the tree —
  correct but unverified live.

## Manual verification checklist (needs an unlocked session / real IME)

- [ ] Click a Find-in-Files result row → cross-file jump + highlights.
- [ ] Replace / Replace all buttons in the find bar (type into the
      replace input, press each button; verify one undo step removes a
      replace-all).
- [ ] IME composition over a template stop (linked editing with Pinyin),
      and composition inside the find/rename inputs.
- [ ] ⌥-click cursor add, ⌥-drag column select, fold chevrons.
- [ ] Hover card (rest on an identifier), signature help (type `(`),
      ⌥⏎ quick fixes on a gopls diagnostic.
- [ ] Scrollbar thumb drag (vs track click).

## Commit list (oldest first)

```
3caa3935 docs(gui): M5 design notes and clean-room log (apps/openidea-gui)
00ad96bf feat(gui): language-driven indentation, scrollbars and horizontal scroll (apps/openidea-gui)
f130ac0c feat(gui): semantic tokens overlay and the five predicate keywords (apps/openidea-gui)
991b3e01 feat(gui): alt-Enter quick fixes and cmd-alt-L formatting (apps/openidea-gui)
ad7bef28 feat(gui): signature help popup and markdown hover cards (apps/openidea-gui)
392b4e5c feat(gui): live templates — trigger-Tab expansion with linked tab stops (apps/openidea-gui)
3adfcfeb feat(gui): in-file find/replace and Find in Files (apps/openidea-gui)
e52a5b43 perf(gui): palette index queries leave the UI thread (apps/openidea-gui)
6cd8f579 fix(gui): semantic tokens fire without a tree-sitter query — Go files painted grey (apps/openidea-gui)
6c2d88f2 fix(go): opt gopls into semantic tokens — the provider answers empty by default (languages/go.toml)
5b2c413d fix(gui): an IME commit replaces the selection — template stops kept their defaults (apps/openidea-gui)
69e6b401 fix(gui): per-character IME commits mirror as inserts into linked stops (apps/openidea-gui)
b85b9407 fix(gui): template continuation lines keep the base indent — block closers landed at column 0 (apps/openidea-gui)
695037d7 fix(gui): shift-cmd-F focuses the Find panel's query input (apps/openidea-gui)
5b730af5 fix(gui): Esc hands the focus back from the Find dock to the editor (apps/openidea-gui)
6a0dd974 fix(gui): uniform_list needs h_full — the run and find lists painted empty (apps/openidea-gui)
8aafe0a4 fix(gui): shift-Enter with the find bar open rewrote the buffer at the match (apps/openidea-gui)
```

Plus this report + screenshots commit.
