# M6-REPORT — Go development experience depth

Branch `feat/gpui-m6-go-dx` (based on `feat/gpui-m5-editor-depth`), worktree
`~/worktrees/openidea/feat-gpui-m6-go-dx`. Per the roadmap discipline: local
commits only, no push, no PR. 12 commits (list at the bottom), each green at
commit time (`cargo fmt`, `cargo clippy --all-targets -- -D warnings`, 47 unit
tests in the crate, up from 43). **`crates/` and the root `Cargo.lock` carry
zero M6 commits** (verified: `git log feat/gpui-m5-editor-depth..HEAD -- crates/
Cargo.lock` is empty); the main workspace is untouched (36 members; the
crates/ delta `main..branch` is upstream's own post-base commits, not ours).
The only shared-config change is `languages/go.toml` (its own commit, below).

## Scope delivered

All eight roadmap items, organized as the design notes' phases
(`docs/m6-design-notes.md`, written before any code):

| Phase | Content | Commit |
|---|---|---|
| notes | design notes + clean-room log | `0998b74d` |
| 1 | completion root cause: LSP snippet items expand through the template-session machinery (linked tab stops); references/rename gain the candidates widening hook | `2f2d94ca` |
| — | probe findings recorded (root cause settled empirically; locked-session record) | `42abd2cf` |
| 2 | organize imports before save: `source.organizeImports` → resolve → apply through Documents → write; explicit ⌃⌥O action | `42368ceb` |
| 3 | inlay hints: viewport-driven requests, painted **inline with reflow**, every x-mapping shifted; ⌥⌘I toggle; HUD counter | `57034be4` |
| — | `languages/go.toml`: opt gopls hints on (the semanticTokens story repeating) | `feed2021` |
| 4 | the ⌥⏎ menu sorts fixes before intentions, drops disabled suggestions | `7c123b58` |
| 5 | the diagnostic tooltip classifies its producer (`compiler · error`, `syntax · error`, analyzer names) and offers one-click fixes | `f4799cf4` |
| 6 | `workspace/symbol` joins Search Everywhere's Symbols group | `080d61f9` |
| 7 | `$/progress` tasks render in the status bar (indexing no longer reads as a hang) | `a6d2d94b` |
| 8 | live-pass fixes: hint answers version-guarded (stale answers dropped + re-requested), resync clears hints, ⌥⏎ anchors at the caret line's diagnostic | `40396017` |

## The M5 handoff items — settled

1. **Member-access completion returned empty.** Root cause found by probing a
   raw stdio session with our exact client capabilities: after `fmt.` gopls
   answers **29 items, all 29 snippet-format** (`insertTextFormat: 2`); even
   plain keyword completions arrive as snippets. The M3 popup dropped every
   `is_snippet` item and closed when all were — i.e. at every gopls member
   access. rust-analyzer only ever looked healthy because it mixes plain items
   in. The M5 `package completions: invalid position` warning was a red
   herring (gopls's auxiliary unimported-package path logging a miss). Fix:
   LSP snippets parse into the live-template expansion model
   (`templates::lsp_snippet_to_expansion`), so accepting a completion lands
   with **linked tab stops**; popup no longer filters snippets. Live
   evidence: `assets/m6-01-completion-member.png` — the open popup over
   `fmt.` inside `main` with the fmt members and their signatures.
2. **References/rename probes saw no results.** The same probe answers
   `NewStore` references cross-package (declaration in store.go + usage in
   main.go) and `prepareRename` with a placeholder — the server face is
   healthy; the M5 miss was the driving. One real gap found and fixed: the
   app never installed `set_candidates_hook`, so references/rename silently
   lost the beyond-scope widening the Tauri host has. Wired in `host.rs`.

## Exit condition — the real loop on the dogfood copies

Driven on `/tmp/m6-go-dogfood` (a git-inited copy of `fixtures/go-app`; the
repo fixture untouched) and on `/tmp/m6-scratch` (`go mod init`, two
sub-packages, an embedded struct, an interface), gopls 0.21.1:

- **Add a function** — typed `func m6probe() { … }` into main.go live
  (auto-indent per `languages/go.toml`, real tabs; M5 loop intact).
- **Completion with package-qualified suggestions** — `assets/m6-01-completion-member.png`
  (popup open over `fmt.`, member signatures rendered); unimported-package
  suggestions verified at the protocol level (`labelDetails` +
  `additionalTextEdits` carrying the import) and `additionalTextEdits` apply
  in `completion_accept`.
- **Save with import auto-organize** — ⌘S on a dirty Go file runs
  `source.organizeImports` first: transcript `[lsp] organize imports on save`
  → `[save] main.go organized=true` (gopls also **added** the missing
  `slices` import through it); any failure falls back to the plain save
  (`organized=false` lines), one flow at a time.
- **Inlay hints always on** — default on; every dogfood screenshot shows them
  (`s *store.Store :=`, `pattern:`, `handler:`, `title:`, `body:`, `error:`,
  `code:`, `name:`, …) painted inline with the line reflowed.
- **Cross-package rename** — protocol-level matrix over the scratch module
  (the app's exact funnel): exported `NewStore` → store.go 2 + main.go 1;
  unexported `find` stays in-package; the embedded `Tag.Label` rename reaches
  the promoted usage in helper.go (store.go 2 + helper.go 1); an interface
  method renames exactly its declaration (Go semantics); gopls refuses a
  rename while the package has errors (surfaces through the rename error).
- **⇧⌘F over the change** — `assets/m6-08-find-in-files.png` (dock page, per-file
  groups, hit accented; M5 surface intact).

## What the live pass caught (each fixed, `40396017`)

1. **Stale hint answers**: gopls computes hints against its own document
   state, which can lag the pane's edits; the answer landed late and painted
   every label one line off (visible after an organize-imports patch).
   Answers now carry the requesting buffer version; stale answers drop and
   re-request. The resync path (out-of-band buffer replacement) clears the
   painted set too.
2. **⌥⏎ missed the quickfix**: a bare-caret request range overlaps no
   diagnostics, so gopls answered only source actions. The request anchors at
   the caret line's diagnostic — `Add import: "errors"` then topped the menu
   and applied live (the `errors` import landed in the dogfood import block).

## gopls vs rust-analyzer behavior record (extends M5's list)

1. Snippet-format answers are the norm for gopls (`snippetSupport: true` in
   our caps); rust-analyzer mixes plain items. Clients must handle snippets or
   lose Go completion entirely.
2. Feature opt-ins are per server and silent: semanticTokens (M5) and now
   inlay hints both answer **empty, not error**, without initializationOptions.
3. gopls errors on out-of-line ranges ("column is beyond end of line") instead
   of clamping — request ranges must be clamped to the line.
4. `source.organizeImports` arrives with the edit attached (no resolve round
   trip); quickfixes like `gopls.add_import` need the line's diagnostics in
   the request context.
5. gopls's rename refuses while the package has compile errors — the message
   is user-actionable and surfaces through the rename flow's error path.
6. Server-to-client requests (e.g. `window/workDoneProgress/create`) must be
   answered by any raw-protocol peer: gopls's jsonrpc2 dispatch serializes
   behind an unanswered request (probe-side lesson).

## Performance (perf HUD, debug build, dogfood main.go with 31–33 hints painted)

| Metric | M2 budget | M6 measured (debug) |
|---|---|---|
| key → pixel | < 16 ms (release baseline 6.4 ms) | **5.18 ms** with hints on (HUD `input Some(5.18)`), single sample this session |
| rows laid out per frame | viewport-bounded | 35 (viewport) |
| spans painted / total | viewport slice | 46/76 |
| hint request cadence | — | debounce 250 ms, skip-when-covered (transcript: requests only on range/edits changes) |

The inline-with-reflow rendering costs one shape per hinted label per frame on
the visible slice only; the HUD's hints counter exposes it. No regression
against the M2/M5 numbers observed; release-mode sampling stays a manual item
(`pnpm perf` needs the full workspace toolchain and an unlocked session).

## Environment record

This milestone's session ran mostly with `CGSSessionScreenIsLocked = Yes`
(same wall M5 hit for clicks). Synthetic keyboard events were fenced for long
spans (navigation keys dropped; printable keys double-inserted through both
the IME and the raw fallback); window-scoped captures work whenever the
display is awake (`caffeinate -u` first; the sandboxed shell also needs the
capture to run unsandboxed). During the unlocked windows the full dogfood
loop was driven live; the rest was verified through raw-protocol probes
(`/tmp/m6probe_completion.py`, `m6probe_organize.py` lineage) that replicate
`caps.rs` exactly.

## Deviations from the brief

- **`languages/go.toml` changed again** (hints opt-in, own commit
  `feed2021`) — same rationale as M5's semanticTokens line: the language
  definition is the seam, the change is data-only.
- **Diagnostic related-information multi-hop is NOT delivered.** `ide-core::
  Diagnostic` has no related field and `ide-lsp::convert::diagnostic` drops
  `relatedInformation` — both frozen by this brief's `crates/ 零 diff` rule.
  The seam is documented (one `related: Vec<DiagnosticRelated>` field + a
  convert passthrough) for a milestone that may touch crates/. Everything
  else in the diagnostics-upgrade item shipped (producer classes, severity
  wording, one-click fixes).
- **`willSave`/`waitUntil` not used** for organize-on-save: the client caps
  (`caps.rs`, frozen) advertise `willSave: false`; the client-driven
  code-action-then-save shape delivers the same UX without the capability.
- **go vet / staticcheck analyzers**: delivered as the *classification*
  (producer names in the tooltip: `compiler`, `syntax`, analyzer names) —
  enabling specific analyzers beyond gopls's defaults is a settings exercise,
  not an integration one.
- The live rename-preview drive (⇧F6 → input → preview) was blocked this
  session by the synthetic-key flakes above (the input opened and accepted
  typed text; Enter-confirmation didn't land synthetically). The flow is
  M3-live-verified and the correctness matrix ran at the protocol level.

## Manual verification checklist (unlocked session, real keyboard/mouse)

- [ ] Completion popup: type `fmt.` inside a function, walk with ↑/↓, accept
      with Enter, walk the linked tab stops (placeholder args) with Tab.
- [ ] An unimported package suggestion (e.g. `strings.Repeat`): accept and
      verify the import lands via `additionalTextEdits`.
- [ ] ⇧F6 rename end to end (input → preview → Apply) on an exported symbol
      with cross-package usages; undo removes the whole rename.
- [ ] ⌥⏎ on a diagnostic line → pick `Add import: …` → verify the import.
- [ ] Diagnostic hover: producer line + one-click fix rows (mouse).
- [ ] Search Everywhere: type a symbol only gopls knows (e.g. an embedded
      field), confirm the Symbols group lists it and Enter jumps.
- [ ] Perf HUD (F12) with hints on: key→px under load, release-mode numbers
      via `pnpm perf` on the full workspace.
- [ ] gopls indexing a large repository: the status-bar task line appears
      (`go packages load …`) and clears on completion.

## Known leftovers

- Related-information multi-hop (blocked; seam documented above).
- Inlay hints: no `inlayHint/resolve` round trip (labels are complete from
  gopls; tooltips resolve lazily when a future UI wants them).
- The organize-on-save flow serializes with the plain save (a second ⌘S while
  the organizer runs saves plainly) rather than queueing — deliberate.
- `hints_by_line` is rebuilt per frame (one shape per visible hint); a
  per-hint width cache is the obvious optimization if the HUD ever shows it.
- The scratch/dogfood transcripts live under `/tmp` (`/tmp/m6-scratch`,
  `/tmp/m6-go-dogfood`, `/tmp/m6-transcript.log`); the repo fixture is
  untouched.

## Seam intelligence for M7 (workbench polish)

1. The status bar's right side is now four stacked producers (run, server
   rows, **live tasks**, message). M7's toast/notification system should
   replace the single `message` slot and give tasks their own surface.
2. The palette's `Picker` has a third content slot (`lsp_symbols`) beside
   files/symbols/actions — the same pattern extends to any future provider;
   the sections math in `render_picker` must grow with it.
3. Esc/enter handling through modal inputs (rename, find bar) is the M7
   focus-chain item the live pass kept hitting: synthetic Enter into a modal
   input did not confirm; real keyboards behave (M3 evidence) — but the
   defensive regression M7 plans should cover modal-input confirmation.
4. The dock's Find page still holds results forever (M5 leftover, unchanged).
5. `hints_by_line` (pane) and the geometry publish are the two per-frame
   handoffs between the element and the entity; anything M7 paints inside
   rows should follow the same shape (prepaint fills, helpers read).

## Commit list (oldest first)

```
0998b74d docs(gui): M6 design notes and clean-room log (apps/openidea-gui)
2f2d94ca fix(gui): gopls completions land — snippet items expand through template stops (apps/openidea-gui)
42abd2cf docs(gui): M6 probe findings — completion root cause settled, locked-session record (apps/openidea-gui)
42368ceb feat(gui): organize imports before save — the Go DX cmd-S (apps/openidea-gui)
feed2021 fix(go): opt gopls inlay hints on — the provider answers empty by default (languages/go.toml)
57034be4 feat(gui): inlay hints — viewport-driven, painted inline with reflow (apps/openidea-gui)
7c123b58 feat(gui): the quick-fix menu sorts fixes before intentions (apps/openidea-gui)
f4799cf4 feat(gui): the diagnostic tooltip classifies its producer and offers one-click fixes (apps/openidea-gui)
080d61f9 feat(gui): workspace/symbol joins Search Everywhere's Symbols group (apps/openidea-gui)
a6d2d94b feat(gui): server progress lands in the status bar — indexing reads as work (apps/openidea-gui)
40396017 fix(gui): inlay hints stay honest across server patches; code actions anchor at the line's diagnostic (apps/openidea-gui)
```

Plus this report + evidence commit.
