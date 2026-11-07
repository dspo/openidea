# M12-REPORT — the test tree + the inspections (+ the Duplicates window, + the OSC 8 click-through)

Branch `feat/gpui-m12-tests`, worktree
`~/worktrees/openidea/feat-gpui-m12-tests`, based on `origin/Native`
(b3fb2d54, the M11 tip) and rebased onto upstream/main in phase 0. Per the
roadmap discipline: local commits only, **no push, no PR**; every phase
commit green (fmt + clippy `-D warnings` + unit tests — 161 in the app
crate + 20 in terminal-ghostty at closeout, of which one smoke test is the
new OSC 8 lookup face against the real library).

**Milestone commits** (oldest first, this milestone):

```
ea04a8f5 chore(gui): the app lockfile follows the rebased workspace versions
175e57f1 feat(gui): the test-tree data plane — app-side parsers for the four runner families (go -json, cargo libtest/nextest, pytest -v, vitest/jest JSON), real-output fixtures, the rerun-command builder and the gutter test-item scanner
40476db9 feat(gui): the Run window's test tree — suite/case rows with state icons, durations and the totals line; failure rows jump to source and open the expected/actual diff (a from_texts LCS joins diff_view); per-node rerun replaces the tab in place; the streaming parse joins ide-run's mid-line chunks
fee54649 feat(gui): the run gutter — a green ▶ per test declaration (go/rust/python/ts-js scanners), a click on the gutter's right strip runs that one test through the project's matching test config (⌥-click the file's suite)
e1a4e76b feat(gui): the Problems window's two-level aggregate — the active file's rows first, the project's under them, severity filter chips (all/errors/warnings) with live counts, and the F2/⇧F2 walker's target highlighted as the cursor (row clicks update it too)
645eb6d1 feat(gui): the Duplicates tool window — Code › Locate Duplicates… (⌘⇧D) runs ide-analysis's clone detection over the walked project (excludes applied, dirty buffers win), the right-stripe window lists groups with member rows (click jumps) and a per-group compare chip opening the fragments in the diff overlay
1d2600b0 feat(gui): the terminal's OSC 8 click-through -- a cmd-click on a hyperlink cell resolves the URI through the libghostty grid-ref face (the click-to-pid FFI transcription verified against the pinned headers) and opens it via the platform handler; the real-library smoke pins the lookup face with the spec's empty-URI terminator
```

**No merge commits**: `git log --merges --oneline upstream/main..HEAD` is
empty; the fork chain stays linear on upstream/main.

## Phase 0 — upstream sync (the rebase discipline)

`git fetch upstream`: upstream/main moved 6efff739 → d509ed6e (**31 commits**).
The 113 fork commits replayed with **zero conflicts** (git auto-replay; the
fork's content lives under `apps/openidea-gui/`).

**Tree-invariance proof** (the acceptance bar, rebase before/after):

```
$ diff <(git diff --name-only m12-pre-rebase-backup..HEAD | sort) \
       <(git diff --name-only 6efff739..upstream/main | sort)
FILE SETS IDENTICAL (269 files)
$ diff <(git diff m12-pre-rebase-backup..HEAD) <(git diff 6efff739..upstream/main)
(empty — byte-identical, cleaner than M10/M11's cosmetic hunk-offset lines)
```

i.e. the post-rebase tree delta **is** the new upstream delta, byte for
byte; the fork carried zero drift. Backup branch `m12-pre-rebase-backup`
(at b3fb2d54) retained. The app lockfile followed the rebased workspace
versions (`ea04a8f5`, the M11 precedent).

`script/verify.sh` on the rebased tree: stage 1 (fmt + clippy + 144 tests
at that point), stage 2 smoke 5/5, stage 3 budgets — one honest note: the
first in-suite run read `gate-debug key_to_px` 16.3 > 16 (the documented
M10 debug-chrome carry-over); the standalone warm re-measure passed at
**15.4** vs budget 16, all other budgets PASS in-suite.

**Fork hygiene at closeout**:

```
$ git log --merges --oneline upstream/main..HEAD   → (empty)
$ git diff --stat upstream/main..HEAD -- crates/ languages/ src/ src-tauri/ wit/ Cargo.lock fixtures/
                                                   → (empty: zero diff)
$ git diff upstream/main..HEAD -- Cargo.toml       → the exclude line + its 2 comment lines only
```

## ①+② The test tree (data plane + the Run window's tree face)

`src/test_tree.rs` — the parsers for the four runner families, all fed by
**real runner output embedded as the unit-test fixtures** (the /tmp
samples' captured streams):

| Runner | Stream face | States covered | Nested suites | Parameterized |
|---|---|---|---|---|
| go | `go test -json` NDJSON events | pass/fail/skip + subtests + the package-build-failure case (only when no case failed — the binary's non-zero exit after failed cases is not a build failure) | `TestAdd/two-plus-two` paths flatten as cases | table-driven subtests |
| cargo | libtest lines (`test <path> ... ok/FAILED/ignored[, reason]`), the failure sections, the `test result:` counts; **nextest JSON events win per line** (`{"type":"test",...}`) | pass/fail/skip + the panic location + the `left/right` assertion pair | module paths as ids | — |
| pytest | `-v` rows + the FAILURES section at finish (location `file.py:N:`, the `E assert A == B` pair, dotted class headers matched by leaf name) | pass/fail/skip + reasons + parameterized ids as distinct cases | classes render as `file::Class::name` | `[1-1-2]` ids |
| vitest/jest | the jest-shape JSON report (`--outputFile`; vitest 5 matches, `skipped`/`pending` both skip) | pass/fail/skip + the frame location + jest's `Expected:/Received:` and vitest's `expected A to be B` | describe ancestors → suites (`math.add` ›) | `it.each` cases |

Known dialect differences (recorded, all pinned by tests):
- vitest 5 writes the report to a FILE (`--reporter=json` + `--outputFile`);
  jest needs `--json` — the derived commands carry each runner's own flags
  (`Runner::Vitest`/`Runner::Jest`), and the derived command bypasses the
  npm wrapper entirely (`npx vitest run …` / `npx jest --json …`) to avoid
  the `--` flag-forwarding dance.
- go reports failure files relative to the PACKAGE directory
  (`calc_test.go`); the workbench resolves them against the root, then over
  the project index (suffix match, then file-name match).
- pid-posted/racing chunk boundaries land mid-line: the streaming parse
  joins fragments across chunks the way the Run dock's panel does (a
  dedicated test feeds a half-split pytest stream).

The Run window's tree face: totals line (`x passed / y failed / z skipped` +
phase), suites with aggregate icons/durations, cases with leaf names,
failure messages, durations, a per-node rerun (▶ — replaces the tab in
place with the filtered command), and for failures a diff chip. A failed
row's click jumps to the failure location; the header's diff chip opens the
expected/actual overlay (`diff_view::from_texts`, an LCS over the two
texts, context skipped — the same DiffLine face ⌃D rides). The session
shows in Services like any run (M11's unified list, free).

**Live acceptance — all five languages** (each: green/red tree + failure
jump with the parsed line + the diff overlay; drive
`python3 /tmp/m12-drive.py <lang>`; captures `assets/m12-{lang}-1-tree /
2-jump / 3-diff.png`):

| Language | Tree | Jump | Diff |
|---|---|---|---|
| go (`go test -json`) | `4 passed / 1 failed / 1 skipped` | `calc_test.go:21` (the Errorf line) | `- 10 / + 9` |
| rust (`cargo test`) | `2 passed / 1 failed / 1 skipped` | `src/lib.rs:16` | `- 1 / + 0` |
| python (`pytest -v`) | `5 passed / 2 failed / 1 skipped` | `tests/test_calc.py:24` | `- 7 / + 6` |
| ts (`vitest run`) | `4 passed / 1 failed / 1 skipped` | `src/math.test.ts:13` | `- 5 / + 4` |
| js (`jest --json`) | `3 passed / 1 failed / 1 skipped` | `__tests__/math.test.js:8` | `- 5 / + 4` |

The rerun faces live: the header's RotateCw (all) re-ran the go suite
(`[run] starting go test ./...` again), and the per-node reruns carry the
runner filters (`go test -json ./... -run ^TestBroken$`,
`cargo test broken_subtraction`, `pytest -v <id>`, vitest/jest `-t`).

## ③ The run gutter

The editor gutter carries a green ▶ per test declaration — the scanner
(`test_items`) reads go `func Test*`/`Benchmark*`/`Fuzz*`, rust `#[test]`
(+ `#[tokio::test]`, `#[ignore]` tolerated between the attribute and the
fn), python `def test*` (with the enclosing class), js/ts
`it(/it.skip(/test(/test.skip(/it.each(/test.each(` with the describe
stack. A click on the gutter's right-edge strip runs that ONE test through
the project's matching test config (⌥-click the file's suite where the
runner supports it); the marks refresh on open and save.

**Live acceptance — go and rust** (the brief's two-language minimum;
captures `assets/m12-gutter-1-marks.png` — four ▶s on the go sample — and
`m12-gutter-2-rerun.png`):

```
go:   [tests] run at calc/calc_test.go:TestBroken → go test -json ./... -run ^TestBroken$
      → [tests] finished: 0 passed / 1 failed / 0 skipped
rust: [tests] run at src/lib.rs:broken_subtraction → cargo test broken_subtraction
      → [tests] finished: 0 passed / 1 failed / 0 skipped
```

## ④ The Problems window's two-level aggregate

The M10 list becomes IDEA's Problems face: the **current file's** rows
first (the section follows the active tab), the **project's** under them,
each section headed with its count; the severity filter chips
(All/Errors/Warnings) with the live `N errors · M warnings` totals; and the
F2/⇧F2 walker now highlights its landing row as the cursor (row clicks
update it too). The stripe badge still carries the error count.

**Live**: `assets/m12-problems-1-all.png` (two sections + chips + totals),
`m12-problems-2-f2.png` (the F2 cursor on the landed row, the editor on
4:2), `m12-problems-3-errors.png` (the Errors chip selected). Scratch
project `/tmp/m12-problems/go-app` (two files with real gopls errors).

## ⑤ The duplicate-code inspection (the Duplicates window)

Code › Locate Duplicates… (⌘⇧D, an app-added key — no IDEA preset row, the
⌃⌘T precedent) walks the project through ide-vfs (excludes applied,
`target`/`node_modules` skipped), prefers open tabs' dirty buffers in the
loader, and runs `ide-analysis::find_duplicates` (the token-based CPD-style
detection, defaults: ≥60 tokens, identifiers and literals anonymized) on
the background executor. The answer opens the **Duplicates window** (right
stripe, OnUse — the button earns its place at the first run): group headers
(`#N · T tokens · L lines · lang · N fragments`), member rows
(`rel_path line:col`) whose click jumps into the fragment, and a per-group
compare chip that opens the first two fragments in the diff overlay.

**Live**: `assets/m12-dup6-1-window.png` (the group `#1 · 154 tokens · 30
lines · rust · 2 fragments` with both member rows on the /tmp rust sample),
`m12-dup6-2-jump.png` (the second member row selected; the caret on line
32 — checkout_b's fragment start). Drive `python3
/tmp/m12-duplicates-drive.py /tmp/m12-problems/rust-dup`.

## ⑥ The OSC 8 click-through (the M11 seam)

The terminal resolves the OSC 8 hyperlink under a ⌘-click through
libghostty's grid-ref face (`ghostty_terminal_grid_ref` +
`ghostty_grid_ref_hyperlink_uri`, the FFI transcription verified against
the pinned headers, the sized-struct + tagged-union conventions followed)
and opens it with the platform handler (`open` / `xdg-open`). The
modifier bypasses the mouse-report programs (IDEA/ghostty's own
convention); the lookup face is pinned by a **real-library smoke test**
(`hyperlink_uri_at_reads_the_labeled_cell`) — a terminal fed a spec-form
OSC 8 sequence exposes the URI at the labeled cells and none on the plain
row (the emission's END face is the spec's empty-URI form `]8;;`; a bare
ST leaks the link onto the following cells — recorded in the test).

**Live drive: environment-blocked tonight.** The full drive
(`/tmp/m12-osc8-drive.py`: ⌃⌘T a terminal, emit the hyperlink, ⌘-click the
label, assert the `[terminal] cmd-click https://…` marker) could not
complete: the synthetic Enter stopped reaching the shell mid-milestone
(the command sits at the prompt unexecuted; the pid-posted paste still
lands, so the terminal receives one event channel but not the other), and
the **M11 baseline binary reproduces the identical behavior** —
definitively not an M12 regression. The manual checklist carries the
re-run.

## Verification summary

- `cargo fmt --all --check` clean; `cargo clippy --all-targets -- -D
  warnings` clean (app + terminal-ghostty); app tests **161/161**
  (144 pre-M12 + 17 new: the parser/filter/scanner faces), terminal-ghostty
  **20/20** (the new OSC 8 lookup smoke included).
- `script/verify.sh` stage 1 green, stage 2 smoke 5/5 green, stage 3
  budgets: **partially re-measured tonight** — the first three-stage pass
  (all budgets) ran at Phase 0 (17:47) on the rebased tree; the closeout
  re-run collected `open_first_paint` 84 ms / 66 ms and
  `find_in_files_first_response` 10 ms / 5 ms (both PASS) but the
  `key_to_px`/`scroll` samples could not be collected: the user's own
  DeepSeek Harness window took the front and swallows every HID-tap event
  (scrolls, F12, plain keys posted to HID). The M12 additions are out of
  that scenario's measured path (the Run/Problems/Duplicates windows are
  closed during it; the gutter adds no marks to a file without test
  functions), so a full budget re-run on an uncontended session is the
  manual checklist's first item.
- Five-language test-tree transcripts: `/tmp/m12-evidence/<lang>/<lang>-transcript.log`
  (`[tests] jump: …` + `[tests] diff overlay: …` per language); the gutter:
  `/tmp/m12-evidence/gutter/gutter-transcript.log` (+ `-rust`); duplicates:
  `/tmp/m12-evidence/duplicates/*-transcript.log`.
- Fork hygiene + tree-invariance + no merge commits: the command outputs
  are in Phase 0 above, re-verified at closeout.

## Deviations from the brief

- **The OSC 8 live transcript is environment-blocked tonight** (above); the
  implementation, the FFI discipline and the lookup face (real-library
  smoke) are in place — the manual checklist carries the drive.
- **Locate Duplicates gained the app-added ⌘⇧D** (the registry test
  requires every action row to display a default binding; no IDEA preset
  row exists for it — the ⌃⌘T precedent). ⌃⌥⇧D was tried first and does
  not dispatch (macOS ⌥-modified letters arrive composed; the keymap loads
  bindings without physical mapping — recorded as a keymap seam for M14's
  keymap work).
- The go test-tree counts the *subtests* as cases (IDEA shows the
  hierarchy; the two-level suite/case constraint flattens
  `TestAdd/two-plus-two` as sibling rows under the package) — the totals
  therefore count 6 cases where IDEA would show 3 tests.
- The npm-family derived test command bypasses `npm run` with
  `npx <runner>` (flag-forwarding through npm required a `--` dance that
  swallowed the reporter flags — exit 127 face) — the stored configuration
  stays untouched.
- The budget gate's closeout re-measure is partial (the front-app
  contention above) — Phase 0's full three-stage green is the recorded
  baseline, the re-run is on the manual checklist.

## Manual verification checklist (needs the user's hands)

- [ ] `script/verify.sh` on an uncontended session (the budget gate's
      key_to_px/scroll legs need HID input; the DeepSeek Harness window
      took the front tonight and swallows them). One command; everything
      else already runs green.
- [ ] The OSC 8 click-through live drive: `python3 /tmp/m12-osc8-drive.py`
      — ⌃⌘T a terminal, emit the spec-form hyperlink, ⌘-click CLICK-ME,
      assert `[terminal] cmd-click https://example.com/m12` in the
      transcript. (Tonight the synthetic Enter stopped reaching the shell —
      the M11 baseline binary reproduces it, so it is the session state,
      not the code.)
- [ ] The Duplicates window's per-group compare chip by real mouse (the
      synthetic channel kept missing the narrow chip in the narrow dock;
      the mechanism is the proven TestDiff face).
- [ ] The test-tree drives re-run from a session where the app can hold
      the front: `python3 /tmp/m12-drive.py go|rust|python|ts|js` (all five
      legs' transcripts above were captured live tonight before the
      contention; the drive scripts are self-contained).

## Seam intelligence for M13

- `test_tree.rs` is the whole data plane: the parsers are per-line
  streaming (`RunnerParser::line`) + whole-document at finish
  (`RunnerParser::finish`); a new runner family is an enum variant, a
  `runner_for` arm, a `test_run_config` arm and fixtures — M13's Find
  Usages window needs none of it, but M14's per-language test faces would.
- `TestRunShared` is SessionShared-shaped (Mutex state + feed/finish): any
  future streaming-analysis face (coverage, profilers) can clone the
  pattern, including the **mid-line chunk joining** (`pending` fragment)
  that cost this milestone its first live bug.
- `start_test_run(config, filter)` is the shared "start a test
  configuration with a filter" face — a future Coverage window or test
  re-run history rides it unchanged.
- `resolve_test_file` (root join → index suffix match → file-name match)
  is the generic "runner-relative path → project file" resolver; the
  profiler/coverage faces will need the same.
- The Duplicates window's walk + loader pattern (ide-vfs walk with
  excludes; dirty-buffer-preferring loader; background find + bus answer)
  is reusable for any project-wide analysis face (the structural search of
  `ide-analysis` rides the exact same shape).
- The interceptor's control-key pass-through fix (single-char key_char
  only) is the terminal's contract with pid-posted events — any future
  face that posts synthetic keys to the terminal must know control keys
  arrive with their key_char set.
- `diff_view::from_texts` (expected/actual LCS) is the generic two-text
  diff face — coverage diffs, the refactoring previews of M14 ride it.

## Leftovers / known limits

- The gutter ⌥-click's file-level face runs the whole configuration for
  go/rust (libtest has no file filter; go's is package-level) — the
  per-file face exists for python/ts/js.
- The cargo failure-section parser settles on the section END
  (`failures:`/`test result:`/the next header); a failure section at the
  stream's very end settles at finish ✓ (covered).
- pytest's FAILURES-section headers with spaces (module-level error
  faces) are skipped — only `Name`/`Class.name` headers map.
- jest's failure frames resolve to the first non-node_modules JS frame —
  internals-only stacks fall back to the suite file without a line.
- The Duplicates window's compare chip opens the FIRST TWO fragments (a
  group with N>2 fragments compares pairwise by re-running the analysis
  with narrower options — M14 face).
