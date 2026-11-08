# M14A-REPORT — five languages at the Go baseline + the performance debt cleared

Branch `feat/gpui-m14a-langdx`, worktree
`~/worktrees/openidea/feat-gpui-m14a-langdx`, based on `origin/Native`
(614d9f07, the M13 tip) and **rebased onto upstream/main** in phase 0.
Per the roadmap discipline: local commits only, **no push, no PR**; every
phase commit green (`cargo fmt`, `cargo clippy --all-targets -- -D
warnings`, unit tests — 186 in the app crate at closeout, up from M13's
180).

**Milestone commits** (oldest first, this milestone):

```
06bd39d6 chore(gui): the app lockfile follows the rebased workspace versions
c69180cf perf(gui): the status bar becomes its own entity — caret and doc events stop re-rendering the shell
b0374811 perf(gui): the paint loop fetches visible lines in one sequential pass — the keystroke frame was rope-bound
2068f98b feat(gui): python/ts/js language servers come up at the Go baseline — the vtsls inlay-hint overlay and the organize-imports sub-kind widening
c590e532 fix(gui): organize imports on save generalizes past Go — python/ts/js ride the same funnel
6b8960c2 fix(gui): the inlay-hint request range clamps to the last real line — rust-analyzer rejected every request
6002146e chore(gui): the reconciliation table advances — the M11–M13 five-language faces are Done
```

**No merge commits**: `git log --merges --oneline upstream/main..HEAD` is
empty; the fork chain stays linear on upstream/main (160 commits ahead of
upstream, all replayed or authored).

**M14b owns the rest of M14**: TODO view, scratch, viewers, welcome
alignment, the keymap settings page, the three-party reconciliation
closeout and the deferred-items recovery plan.

## Phase 0 — the rebase discipline

`git fetch upstream`: upstream/main moved f07627ad → 99100e48 (**25
commits**, v0.1.39 era: editor semantic-token scheme styles, VCS worktree
fixes, keymap/notification/terminal fixes, run/menu fixes). The **128**
fork commits replayed with **zero conflicts** (the fork's content lives
under `apps/openidea-gui/`).

**Tree-invariance proof** (the acceptance bar):

```
$ BASE=$(git merge-base m14a-pre-rebase-backup upstream/main)   # f07627ad
$ diff <(git diff --name-only m14a-pre-rebase-backup..HEAD | sort) \
       <(git diff --name-only $BASE..upstream/main | sort)
FILE SETS IDENTICAL (164 files)
$ diff <(git diff m14a-pre-rebase-backup..HEAD) <(git diff $BASE..upstream/main)
363c363  < index e0352b85..2f13c3eb 100644   → blob-hash context line
366-367  exclude = ["sdk", "plugins", "apps/openidea-gui"] vs ["sdk", "plugins"]
375c375  @@ -47,6 vs @@ -45,6                     → hunk offsets in root Cargo.toml
```

i.e. the post-rebase tree delta **is** the new upstream delta; the only
differing lines are the two cosmetic classes M10/M11 already documented
(blob-hash context lines and hunk offsets shifted by the fork's own
`exclude = ["apps/openidea-gui"]` line in the root Cargo.toml). Backup
branch `m14a-pre-rebase-backup` (at 614d9f07) retained.

Upstream bumped the workspace version to 0.1.39, so the app lockfile
followed (`06bd39d6`, the M11/M12 precedent — 13 workspace-crate entries).

## ① The performance debt cleared — and what it actually was

The brief's prescription: the status bar as its own entity, and
`gate-debug key_to_px` back inside the 16 ms budget (M13 closeout 16.3).

**A/B, same machine, same session, same scenario** (`script/verify.py
perf` on the 100-package Go module, medians of 12 paste+HUD rounds):

| build | key_to_px medians (two runs) | pooled |
|---|---|---|
| baseline (M13 chain, pre-fix) | 16.0 / 15.5 | 15.8 (n=24, max 20.9) |
| + status-bar entity only | 15.9 / 16.1 | no change |
| + the paint-loop fix (final) | **8.3 / 7.7** | — (max sample 13.9) |

The status-bar entity (its own `StatusBarView` with one diffed `sync` per
pump/render; caret, dirty, diagnostics, server rows and `$/progress`
tasks notify only that small view — the 300 ms caret throttle and its
render-time flush are gone) is the prescribed structural fix and stands,
but the A/B proved the **frame cost was never the shell**: `key_to_px`
measures the pane's input→prepaint, and `sample` during a typing storm
showed **ropey's offset scans topping the profile**. The row loop called
`line_span` + `slice_utf16` + `line_to_byte` per visible row (four-plus
rope walks each, ~150 per frame) and the HUD's spans counter added two
more per row. `Buffer::line_block` (unit-tested against the per-line face
it replaced: CRLF tails, CJK double units, no-trailing-newline, empty and
phantom final lines) fetches each contiguous line run with two offset
conversions and one copy. Frame cost halved; every budget sample now
inside the budget with ~8 ms of headroom (see ⑦ for the in-suite gate).

Two more finds landed with it:

- **`apply_workspace_edit` rejected cross-file renames** ("…changed;
  request the refactoring again", live toast): the rename preview never
  captured preconditions, and `apply_workspace_edit` trusts preconditions
  precisely for files the server versioned but the editor never opened.
  The preview now captures them (`host.prepare_workspace_edit`), and the
  go live rename (below) applies its 2 edits in 2 files.
- **Organize-imports-on-save was gated to Go** (`detect(...) == "go"`):
  python/ts/js silently took the plain save. The gate is now the language
  set whose ecosystem organizes imports on save (go, python via ruff's
  sub-kind, ts/js via tsserver's); rust stays on the explicit ⌘⌥L
  rustfmt.

## ② The four languages come up (zero toml changes)

Servers installed per the tomls' install hints (machine tools, no repo
change): `uv tool install basedpyright` (1.40.2), `uv tool install ruff`
(0.17.4), `npm i -g @vtsls/language-server` (0.3.0) and
`typescript-language-server` with **typescript@6** — the hint's pin is
load-bearing: with the npm-default typescript@7 the fallback server
advertises nothing. gopls and rust-analyzer were already installed.

App-side seams (`languages/*.toml` untouched — fork hygiene holds):

- **vtsls inlay hints** (`2068f98b`): the provider advertises but answers
  null until the VSCode-shape `typescript.inlayHints.*` /
  `javascript.inlayHints.*` tree turns the kinds on (the
  preferences-form `includeInlay*` keys are ignored by vtsls — probed).
  The overlay merges under the toml's existing options; a set kind wins
  (unit-tested both ways).
- **organize-imports sub-kind widening**: the M6 filter kept only the
  exact `source.organizeImports` kind, so ruff's `.ruff` and the
  tsserver family's `.ts` were dropped and python had no save-organize at
  all. Sub-kinds match by prefix (unit-tested: parent, `.ruff`, `.ts`,
  sibling kinds, lookalikes, disabled).
- **inlay range off-by-one** (`6b8960c2`): the M6 range end was one past
  the last line; gopls reads that as "to the end", rust-analyzer
  validates strictly and rejected **every** rust hint request. Clamped,
  with a bounded retry for requests that race the server's reanalysis.

rust re-verified: semantic tokens, hints (live in the screenshot),
completion, rename, format — all answering (see ③).

## ③ The exit condition — one real loop per language, live

Each drive: open a scratch project (under `/tmp/m14a-dogfood` and
`~/tmp-scratch`, the repo untouched) → add a function → module-qualified
completion → save (organize where the ecosystem does it) → inlay hints
on → cross-file rename → find usages. Transcripts:
`/tmp/m14a-evidence/m14a-<lang>-transcript.log`; screenshots in
`assets/m14a-*.png` (window-scoped only).

| lang | server ready | hints painted | completion | save | rename applied | evidence |
|---|---|---|---|---|---|---|
| go | gopls ✓ | ✓ on open + after edits | `fmt.` 22.7 ms | organized=false (nothing to organize) | `New`→`Open`, 2 edits in 2 files (main.go + internal/store/store.go), `go build` clean | `m14a-go-hints.png`, `-rename-preview`, `-usages` |
| python | basedpyright ✓ (+ ruff ✓) | ✓ (10–12 hints: `g: Any`, `text: Any`, `obj=`, `name=`) | `json.` 379 ms | **organized=true** (ruff's organize through the M6 funnel; toast "Saved · Organized imports main.py") | `render`→`draw`, 2 edits in 1 file | `m14a-python-completion.png`, `-usages` |
| ts | vtsls ✓ | ✓ (the overlay's first live pay-off) | `store.` painted | **organized=true** (tsserver) | `makeItem`→`buildItem` across main.ts + store.ts, import alias updated in place (the toml's `useAliasesForRenames=false`) | `m14a-ts-hints.png`, `-rename-preview`, `-usages` |
| js | vtsls ✓ | ✓ | `store2.` painted | **organized=true** | `makeItem`→`buildItem` at the declaration + `module.exports` (the CJS boundary below) | `m14a-js-completion.png`, `-usages` |
| rust | rust-analyzer ✓ | ✓ on open + after edits (`: Counter`, `: Range<Counter>`) | `counter.` 31.7 ms | plain save (rust's ecosystem face is ⌘⌥L) | `Counter`→`Tally`, **3 sites** incl. the pasted `crate::Tally::new()` | `m14a-rust-hints.png`, `-rename-preview`, `-usages` |

Format legs: go `[lsp] format 1 edits`, rust `2 edits`, python `1 edit`
(ruff); ts/js answered empty (the scratch was already clean) — all
through the ⌘⌥L face.

The rename preview's mouse-click Apply is used by the drives (the
synthetic Enter into the modal surface is the M6/M12-documented flake;
the click coordinates track the preview's file-row count).

## ④ The rename-correctness matrix (protocol level, the app's funnel)

| case | answer | boundary |
|---|---|---|
| go exported, cross-package (M6 matrix re-verified live) | all sites | — |
| python same-file variable/def (`text`, `render`) | all sites | — |
| python cross-file from the DECLARATION (greeter.py) | both files | — |
| python cross-file from a USE of another module's symbol | **null** ("Rename produced no edits") | basedpyright face |
| python `import json` alias rename | **null** | pyright family: import-statement renames unsupported |
| python dict key vs variable | prepareRename on the key returns the string's own range; a variable rename never touches same-named keys | keys are independently renamable literals |
| ts export consumed cross-file | both files, the import alias in place | — |
| **.js consuming a .ts export (allowJs + checkJs)** | **both files** (the .js consumer's call site updates) | allowJs is enough for the ESM face |
| js CJS: `require()` namespace property accesses | declaration + `module.exports` follow; the property accesses do **not**, even with checkJs | tsserver face; ESM-style imports track |
| rust in-crate (healthy analysis) | all 3 sites incl. the path-qualified `crate::Tally::new()` | — |
| rust cross-crate (path dep in a workspace) | both crates' files | — |
| rust with a broken crate (the probe's own bad path, pre-fix) | syntax-level rename only (declaration + impl, uses missed), hints 0 | RA degrades to syntax when analysis is broken — the scratch was the bug, not RA |

## ⑤ The per-language behavior register (the M6 format)

**gopls** (0.21.x, the M6 baseline, re-verified): the semanticTokens/hints
opt-ins still ride the app's config hook; **clamps** out-of-range request
ranges — which silently masked the app's inlay off-by-one for four
milestones (gopls tolerated what RA rejects).

**rust-analyzer** 1.96.1: semantic tokens always on (15–18 spans live).
Inlay hints answer only when the workspace **loaded** — a crate with an
error degrades to the syntax face (rename loses path-qualified uses,
hints 0) while tokens keep answering; the error is visible in cargo, and
the app can't tell "unloaded" from "no hints" (the server answers
empty-not-error — the gopls pattern, third instance). Range validation
strict (`Invalid offset`), no clamping. Constructor-name type hints ARE
rendered in 1.96.1 (`let counter = Counter::new()` hints `: Counter`).
Labels are plain strings. Format = rustfmt ✓. No organize-imports code
action (the ecosystem face is the explicit format).

**basedpyright** 1.40.2: semantic tokens on by default; **inlay hints on
by default** (type + parameter + named-arg hints — no overlay needed,
unlike gopls/vtsls). Server-to-client `workspace/configuration` requests
serialize the whole dispatch until answered (the M6 gopls lesson, second
server — raw peers must answer or starve). `typeCheckingMode=standard`
via the toml's initialization-options works; violations flag live.
Rename boundaries above. No formatting, no organize (ruff's halves).

**ruff server** 0.17.4: formatting, organize imports
(`source.organizeImports.ruff`), pull diagnostics, quickfixes; **no**
completion, semantic tokens, inlay hints or rename. `require-root-marker`
honored (`pyproject.toml#tool.ruff`). Its organize answer includes
`source.fixAll.ruff` under an organize-only `only` — clients filter.

**vtsls** 0.3.0: semantic tokens on by default. Inlay hints advertised
but **null until configured**, and only in the VSCode
`typescript.inlayHints.*` shape (the `preferences.includeInlay*` shape
the tsserver ecosystem documents is ignored — probed). Labels arrive as
`InlayHintLabelPart[]` with locations (the M6 conversion handles both).
Completion: plain items only. Organize answers `only` loosely (refactor
kinds leak in) — clients filter by kind. Rename honors the toml's
`useAliasesForRenames=false` (the alias updates in place). The toml's
initialization-options ride both `initialize` and `workspace/configuration`
(section `""`).

**typescript-language-server** 6.0.3: dead with typescript@7 (advertises
nothing), fully alive with typescript@6 — the toml install hint's `@6`
pin is load-bearing. Its organize kind is `source.organizeImports.ts`.
It is the fallback for vtsls and only starts when vtsls is missing.

## ⑥ The reconciliation table advances

The rebase's upstream delta grew the preset **357 → 362 rows**; both
generators re-ran (`keymap_data.rs` + `reconcile_data.rs`). Flips —
EXTRA_DONE, each with a live face from the named milestone:
`terminal.new` (⌃⌘T, M11), `debug.evaluate` (the Debug console, M11),
`run.runContext` (the run gutter's one-test click, M12), `run.rerun`
(the Run window's header/per-node rerun, M12), `vcs.branches` (⌃⌘G,
M13), `vcs.annotate` (⌃⌘A, M13) — plus the rows the keymap binds, which
read Done automatically (`run.debug` ⌃D, `toolwindow.terminal`,
`find.showUsages`, `vcs.newBranch`, the `debug.step*` set).

**Counts: done 84 · placeholder 150 · na 128** (M10's last regen: 69 /
158 / 130).

## ⑦ Verification

- Engineering: `cargo fmt --all --check` clean; `cargo clippy
  --all-targets -- -D warnings` clean; app tests **186/186** (180 pre-M14a
  + 6: the status tuple equality, the line-block face ×2, the organize
  sub-kind filter, the vtsls overlay ×2).
- `M8_VERIFY_OUT=/tmp/m14a-verify bash script/verify.sh` — the full
  three-stage gate — runs at closeout; the stage-3 record is in the PR
  body (`/tmp/m14a-pr-body.md`). The A/B numbers above are the budget
  leg's evidence: **key_to_px 8.3/7.7 medians vs the 16 budget**, every
  sample under budget.
- Fork hygiene at closeout:

```
$ git log --merges --oneline upstream/main..HEAD      → (empty)
$ git diff --stat upstream/main..HEAD -- crates/ languages/ src/ src-tauri/ wit/ Cargo.lock fixtures/
                                                      → (empty: zero diff)
$ git diff upstream/main..HEAD -- Cargo.toml          → the exclude line + its 2 comment lines only
```

- Tree-invariance: the phase-0 proof, re-verified against closeout HEAD —
  the non-`apps/` delta is byte-identical to the upstream delta modulo
  the two cosmetic classes shown in phase 0.
- Scratch discipline: all dogfooding under `/tmp/m14a-dogfood`,
  `/tmp/m14a-matrix`, `~/tmp-scratch` and `/tmp/m14a-budget`; the repo's
  fixtures untouched. Only this session's processes were killed.

## Deviations from the brief

- **The "status bar alone will fix the budget" prescription was tested
  and falsified** — the honest A/B is in ①. The entity landed (it is the
  prescribed structural fix and kills the per-keystroke shell frames),
  but the budget went green through the rope-bound frame fix, which the
  profiling demanded. `perf/budgets.json` baselines are unchanged; the
  gate judged against the same 16/8 ms budgets.
- **The run/debug/test column's re-verification rides the M11/M12 live
  evidence** plus this milestone's engineering gate; the drives here
  re-verified the editor/LSP column (the brief's loop) and the debug
  faces were not re-driven end-to-end (M11's python leg + its manual
  checklist stand).
- The rename preview's **Apply is mouse-driven in the drives** — the
  synthetic Enter into the modal surface is the documented flake (M6/M12;
  human keyboards behave, M3 evidence).
- js's live rename used a `jsconfig.json` scratch (added — a machine
  scratch, not a repo file); without it tsserver answers no edits for
  CJS property renames (recorded in ④).

## Manual verification checklist (needs the user's hands)

- [ ] `script/verify.sh` on an uncontended session (the HID-tap scroll
      leg and the F12 HUD sampling prefer the front).
- [ ] The rename preview's **Enter** Apply with a real keyboard (the
      synthetic channel's known flake; M3 verified Enter-apply live).
- [ ] IME composition into the popup/inputs (the standing M9/M10 item).
- [ ] The rust hints on a warm daily-driver workspace (the milestone's
      live pass ran on a fresh scratch; the openidea repo itself answers
      6 hints at the protocol level).

## Seam intelligence for M14b

- `StatusBarView::sync(Pushed)` is the one door every status-bar datum
  passes through — new widgets (TODO counts, git状态 chips) add a field
  to `Pushed`, not a notify site.
- `Buffer::line_block` is the shape any per-line batch consumer should
  use (the future TODO/structure windows walk rows the same way).
- The organize-on-save gate (`Self::organizes_imports_on_save`) is where
  format-on-save would slot in if M14b wants IDEA's reformat-on-save.
- The rename preview's `prepare_workspace_edit` pattern — capture
  fingerprints when the user is shown a workspace edit — generalizes to
  any future refactoring-preview face (M12's seam note, now proven live).
- The server quirks register (⑤) is the checklist for any new language
  toml: negotiate `workspace/configuration` answering, expect
  empty-not-error opt-ins, probe the rename boundaries, filter code
  actions by prefix.
- `script/gen-reconcile-data.py` + `script/gen-keymap-data.py` must both
  re-run after every sync — upstream grew the preset under us this time.

## Known leftovers

- The synthetic-channel flakes are unchanged (rename Enter, HID taps
  under front-app contention) — all human-reachable, manual checklist.
- rust hints on a cold scratch need the crate to compile (a broken crate
  degrades RA to the syntax face); the app surfaces the diagnostics, but
  a "workspace not loaded" state is not distinguishable from "no hints".
- `typescript-language-server` remains the fallback and needs the
  typescript@6 pin (the toml hint is correct; npm's default @7 breaks it).
- The python live loop's scratch accumulated probe copies across runs
  (restored at closeout); nothing in the repo was touched.

## Evidence set (window-scoped, in `assets/`)

| face | file |
|---|---|
| go inlay hints + completion accepted (the loop) | `m14a-go-hints.png` |
| python completion popup over `json.` | `m14a-python-completion.png` |
| ts inlay hints (the vtsls overlay's pay-off) | `m14a-ts-hints.png` |
| js completion over `store2.` | `m14a-js-completion.png` |
| rust inlay hints (`: Counter`, `: Range<Counter>`) | `m14a-rust-hints.png` |
| go rename preview (2 edits in 2 files) | `m14a-go-rename-preview.png` |
| ts rename preview (cross-file, alias in place) | `m14a-ts-rename-preview.png` |
| go / python usages windows | `m14a-go-usages.png`, `m14a-python-usages.png` |

Transcripts: `/tmp/m14a-evidence/*-transcript.log`; the A/B measured
JSONs and HUD shots: `/tmp/m14a-ab/`.
