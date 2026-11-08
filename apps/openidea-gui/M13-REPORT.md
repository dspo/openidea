# M13-REPORT — local VCS in full: the log graph, branches, blame, conflicts, changelists, the Find Usages tree

Branch `feat/gpui-m13-vcs`, worktree
`~/worktrees/openidea/feat-gpui-m13-vcs`, based on `origin/Native`
(a47c8c2e, the M12 tip) and **rebased onto upstream/main** in phase 0
(the sync dividend: the log graph's data plane rides upstream's new
ide-vcs log/history faces outright). Per the roadmap discipline: local
commits only, **no push, no PR**; every phase commit green (fmt +
clippy `-D warnings` + unit tests — 180 in the app crate at closeout,
up from M12's 161: the git_graph, blame, conflicts and
changelist-grouping test faces).

**Milestone commits** (oldest first, this milestone):

```
b5161ec1 feat(gui): the Git tool window (⌘9) — the log graph on upstream's new ide-vcs log API …
725ae05c feat(gui): the branches popup — ⌃⌘G / the VCS menu / the status-bar branch widget click …
0ecbe951 feat(gui): the blame dual form — VCS › Annotate (⌃⌘A, the preset's vcs.annotate face) …
6ae6df95 feat(gui): the conflict-resolution minimal UI — the pure conflicts module parses …
973ed5f2 feat(gui): the changelists — the Commit window's rows group under their changelist headers …
f039f2ae feat(gui): the Find Usages window becomes the M13 tree — the rows group by file …
```

**No merge commits**: `git log --merges --oneline upstream/main..HEAD`
is empty; the fork chain stays linear on upstream/main.

## Phase 0 — the rebase discipline (the sync dividend, realized)

`git fetch upstream`: upstream/main moved d509ed6e → f07627ad
(**98 commits** — the ide-vcs log/history/blame/changelists rebuild this
milestone was told to harvest). The **121** fork commits replayed with
**zero conflicts** (git auto-replay; the fork's content lives under
`apps/openidea-gui/`).

**Tree-invariance proof** (the acceptance bar):

```
$ diff <(git diff --name-only m13-pre-rebase-backup..HEAD | sort) \
       <(git diff --name-only d509ed6e..upstream/main | sort)
FILE SETS IDENTICAL (758 files)
$ diff <(git diff m13-pre-rebase-backup..HEAD) <(git diff d509ed6e..upstream/main)
(empty — byte-identical, third milestone running)
```

i.e. the post-rebase tree delta **is** the new upstream delta, byte for
byte. Backup branch `m13-pre-rebase-backup` (at a47c8c2e) retained.
Re-verified at closeout on the non-app paths (the app's own M13 commits
are the only delta against the proof):

```
$ diff <(git diff --name-only d509ed6e..upstream/main | grep -v '^apps/') \
       <(git diff --name-only m13-pre-rebase-backup..HEAD | grep -v '^apps/')
non-app file sets identical
$ diff <(git diff d509ed6e..upstream/main -- crates/ languages/ src/ src-tauri/ wit/ Cargo.lock fixtures/) \
       <(git diff m13-pre-rebase-backup..HEAD -- crates/ languages/ src/ src-tauri/ wit/ Cargo.lock fixtures/)
non-app byte-identical
```

The workspace version stayed 0.1.38 through this delta, so the app
lockfile needed **no** follow-up commit this time (the M11/M12
precedent's condition did not arise). `script/verify.sh` on the rebased
tree: stage 1 green, stage 2 smoke green, stage 3 all budgets **except**
`gate-debug key_to_px` 16.2 > 16 (max 18.3) — the documented M10
debug-chrome carry-over that M12 also recorded; tonight's warm
re-measures read 16.7–16.9 (session contention; M12's warm pass read
15.4). Closeout re-run and disposition at the bottom of this report.

**Fork hygiene at closeout** (the acceptance bar):

```
$ git log --merges --oneline upstream/main..HEAD   → (empty)
$ git diff --stat upstream/main..HEAD -- crates/ languages/ src/ src-tauri/ wit/ Cargo.lock fixtures/
                                                   → (empty: zero diff)
$ git diff upstream/main..HEAD -- Cargo.toml       → the exclude line + its 2 comment lines only
```

## Upstream ide-vcs API notes (what the sync bought — the M13 brief's dividend ledger)

The M13 data plane is upstream's, through the app's `host.rs` adapter
(UI never touches `ide_*` directly; the adapter grew only these thin
faces): `Repo::log(LogQuery)` (filters + `skip`/`limit` pagination +
`graph_parents`/`graph_dashed`/`on_current_branch`/`log_total` per row),
`Repo::enable_log_index` (the disposable content-addressed log cache —
opened per repository at project open, exactly the Tauri host's call),
`Repo::commit_files`, `Repo::diff_numstat`, `Repo::file_at`,
`Repo::branches`, `Repo::blame`, `Repo::branches/checkout_with/
create_branch/delete_branch_restorable/merge/rebase`, `Repo::operation`,
`Repo::continue_operation/abort_operation`, `Repo::stage`, and the
`changelists` module (`load`/`save`/`ensure_ignored` — the same
`.openidea/changelists.json` the Tauri host keeps, with its `.gitignore`
guard). Notes worth keeping:

- `Commit.graph_parents` are the **display** connections for the
  filtered set ("never use these as revision/diff parents") — the lane
  walk consumes them, the commit-detail diff uses `parents`.
- `LogQuery.path` takes an absolute path (upstream `rel_path`s it);
  branch/author/text needles are case-insensitive substrings upstream.
- `log_total` did not populate for plain (branch=None) queries in
  tonight's scratch runs — the totals line degrades to the loaded count
  (a noted upstream seam, harmless here).
- `diff_hunks(old, new)` (the free fn) + `diff_view::from_hunks` render
  any two-revision file diff — the commit-detail face rides it.
- `checkout_with(CheckoutOutcome::…)` returns structured outcomes
  (LocalChanges/UntrackedFiles/LocalBranchAhead/Conflicts) the popup
  turns into run-panel lines.
- merge/rebase leave the repository mid-operation and return `Err` —
  the popup's toast + the status chip + the conflict strip take over.

## ① The Git tool window (⌘9): the log graph

The deferred `git` window lit (`toolwindow.rs` flips to the preset's
`toolwindow.git` slot ⌘9; the pure `slot_map_is_the_preset_map` test
now pins ⌘9 → git). Data: upstream `Repo::log` on the background
executor; `git_graph::assign_lanes` (pure, **6 tests**: linear,
fork+converge, merge edges that start at their row's bottom, dashed
flags, lane reuse, the graph_parents fallback) derives each row's dot
lane, pass-through verticals and outgoing edges from the filtered
commits' `graph_parents`. Rendering: a per-row canvas (`PathBuilder::stroke`
lines, `paint_path`, dashed edges via `dash_array`), lane colors from
the 8-hue `theme::graph_lane_color` palette, HEAD/branch decoration
chips, summary/author/date columns, "Load more" pagination
(500/page, upstream-side `skip`), and the commit detail panel (message,
author, date, refs; the changed files with `+a/−r` numstat whose click
opens the parent↔commit diff in the proven ⌃D overlay — root commits
diff against git's empty tree). Filters (the brief's minimal set): the
branch-scope menu (All Branches / each local branch), the author and
message needles (Enter applies), and the Path chip (the active tab's
file, rename-following history upstream).

**Live** (`/tmp/m13-scratch` — the scripted topology: ≥3 branches, a
true merge, a rename, 4 authors): ⌘9 loads 6 commits; the screenshot
(`assets/m13-log-graph.png`) shows the topology lanes with the fork and
the merge, HEAD/main/topic/feature chips; the second screenshot
(`assets/m13-log-detail.png`) shows the detail panel on the rename
commit (`greet.go +0/−0` — the pure rename numstat upstream detected).
Transcript: `[git] log page: 6 commits`.

## ② The branches popup

⌃⌘G (app-added; the preset's `vcs.branches` row carries no keys), the
VCS menu's Git Branches… row, and **the status-bar branch widget click**
(the IDEA-convention entries the brief named — ⌘⇧N stayed untouched, it
is the preset's `file.newScratchFile`). The modal lists local and
remote branches (current first, ● checked-out, ahead/behind vs the
upstream), per-row Checkout / Merge / Rebase buttons and two-step
Delete (the checked-out branch is protected — no checkout, no delete).
⌘⌥N (the preset's `vcs.newBranch`) opens the popup with the name input
armed; ⏎ creates at HEAD **and** switches. Every operation runs through
ide-vcs's write faces on the background executor and lands its output
as a **finished Run panel tab** (the brief's "输出进 run 输出面板":
`run_panel::RunPanelSeed` gained a pre-finished form — `(exit code,
lines)` — so the git face opens already-settled, no process behind it).
The status bar carries the merge/rebase-in-progress chip (the M4
leftover's data half: `GitStatus` now also carries `operation`).

**Live**: ⌘⌥N `experiment/one` ⏎ → the scratch's branch flips
(`[vcs] status refresh: … branch Ok(Some("experiment/one"))`,
`assets/m13-branch-created.png`); the popup's row click on main
switches back (`m13-after-checkout.png`, branch `main`). The popup
itself: `assets/m13-branches.png` (current protected, the three action
buttons per row).

## ③ Blame, dual form

- **Inline blame** (on by default, the IDEA face — the persisted
  `inline_blame` setting, ⌃⌘B / VCS › Toggle Inline Blame): the caret
  line's `author (date rev)` tail a step right of the text; hovering it
  brightens and spreads the summary underneath. Tab opens seed the
  entries in the background when the setting is on; a live face
  re-blames on save (the dirty buffer wins over the disk).
- **Annotation mode** (VCS › Annotate, ⌃⌘A — the preset's
  `vcs.annotate` face): the 210px swimlane left of the gutter, one
  `author MM-DD summary-slice` label per line, colored by commit age in
  five bands (fresh → ancient). The lane is click-inert; the numbers,
  chevrons and breakpoint dots shift right of it.

The pure `blame.rs` module (**6 tests**): the age bands, the label
faces, the uncommitted all-zero-commit face ("Not Committed Yet"), the
bounds-checked line→entry mapping. Upstream `Repo::blame` feeds it
through the host adapter.

**Live**: `assets/m13-blame-inline.png` (the tail on the caret line of
farewell.go) and `assets/m13-blame-annotate.png` (the swimlane, amber
for the week-old commits); transcript `[blame] …: 12` per load. The
conflicted-file screenshot below adds the uncommitted face live
("Not Committed Yet" on a merge's marker line).

## ④ The conflict-resolution minimal UI

Detection: the status refresh reports the in-progress operation (the
chip), and the active tab's buffer is re-parsed for conflict blocks on
DocDirty / DocPatched / tab activation (the pure `conflicts.rs`,
**5 tests**: the three-marker shape, diff3 `|||||||` sections, bare
`=======` lookalikes as content, unterminated blocks skipped, the
navigation walks, side application). The strip above the editor shows
the count + the operation chip, the walk (‹ Prev / Next ›; the editor
actions ⌘⌥↓ / ⌘⌥↑, app-added keys — ⌃⌥↓ is Mission Control),
Accept Yours / Accept Theirs (the block replaced through the backend's
workspace-edit funnel, so the open tab reconciles via `DocPatched` —
the same door renames use), Mark Resolved (stages through git; the
status refresh drops the file from conflicted), and the operation's
Continue / Abort (whose git output joins the run panel like every
branch operation).

**Live**: the real `git merge conflict-demo` conflict (both sides edited
`greet.go`'s greeting) renders the strip (`assets/m13-conflict-strip.png`:
"1 conflicts · merge in progress" + all six buttons) with the conflicted
file red in the tree and "merge in progress" in the status bar; the
keyboard walk (`assets/m13-conflict-walk.png`): ⌘⌥↑ lands the caret
exactly on the `<<<<<<< HEAD` marker (status `7:1`), the gutter mark and
the "Not Committed Yet" blame tail ride the same line, and the
following ⌘⌥↓ toasts "No more conflicts in this file". The strip's
chips (Yours/Theirs/Resolved/Continue) need mouse — the synthetic
channel could not reach them tonight (contention, see the manual
checklist); their machinery is the unit-tested application face plus
the proven stage/continue adapters.

## ⑤ Changelists

The Commit window's rows group under their changelist headers (the
active list first and ●-dotted, per-group counts); headers carry
Set Active / Delete Changelist context menus (a deleted list's files
return to the active list; the default list cannot be deleted); rows
carry Move to Changelist items; the inline New Changelist input (⏎ or
the button) creates **and activates** (the next changes land there —
IDEA's face). Persistence rides upstream's `ide-vcs::changelists`
module at `.openidea/changelists.json` with its `.gitignore` guard —
byte-compatible with the Tauri host. The grouping is the pure
`changelist_groups` face (**1 test**: unassigned files ride the active
list, stale assignments to deleted lists fall back to it, active
first). The Commit window and the Git window read the one workbench
value (`changelists: ChangelistState`), so the two faces cannot drift.

**Live**: `assets/m13-changelists-default.png` — the "● Changes 2"
group over both changed files with the New Changelist input row. The
create/move click-through needs mouse (manual checklist); the
persistence module is upstream-tested.

## ⑥ The Find Usages tool window

The M10 answer becomes the brief's tree: the rows group by file
(first-hit order, per-file counts), a row's click selects it and loads
its line into the window's preview pane (the symbol bracketed; the
dirty buffer wins over the disk) while keeping the jump; and ⌘⌥F7
(the preset's `find.showUsages`) rides the new `editor::ShowUsages` —
a single hit takes the **window** (⌥F7 keeps its direct jump; the
pane's `usages_force_window` flag routes the answer).

**Live**: ⌘⌥F7 on `Farewell` (a single cross-file usage) opens the
window with the grouped tree (`farewell.go 1` → the `6:6` row) and the
preview placeholder — `assets/m13-usages-window.png`; the OnUse stripe
button lights at first use (the M10 contract). The preview-with-content
row click = manual checklist (mouse contention tonight).

## ⑦ Regression + verification summary

- Engineering: `cargo fmt --all --check` clean; `cargo clippy
  --all-targets -- -D warnings` clean; app tests **180/180**
  (161 pre-M13 + 19 new: git_graph 7, blame 6, conflicts 5,
  changelist grouping 1).
- `script/verify.sh` (the M0–M12 regression harness): closeout re-run
  captured at the bottom of this report (stage 1 + smoke green; the
  budget leg's disposition below).
- Fork hygiene + tree invariance + no merge commits: the command
  outputs are in Phase 0 above, re-verified at closeout.
- The M0–M12 surface: every M13 change is additive on the app crate
  (new modules, new actions/keys, the lit `git` window); the one
  behavior-adjacent change to existing faces is `GitStatus` gaining the
  `operation` field (a new bus-message field, both ends in-tree) and
  `RunPanelSeed` gaining the pre-finished form (default `None` keeps
  every existing construction site identical).

**Bonus find (a real bug, fixed in the blame commit):** the editor's
paint phase drained the display rows before the gutter's number loop
read them — **line numbers have been silently absent** in the M12
evidence set (compare `assets/m12-gutter-1-marks.png` on the M12
worktree: chevrons and run marks, no numbers). The fix captures the
row anchors before the drain; the numbers are back in every M13
screenshot above.

## Screenshots (window-scoped, in `assets/`)

| face | file |
|---|---|
| log graph (topology lanes + refs chips) | `m13-log-graph.png` |
| commit detail (rename commit, numstat) | `m13-log-detail.png` |
| branches popup | `m13-branches.png` |
| branch created+switched (status bar) | `m13-branch-created.png`, `m13-after-checkout.png` |
| inline blame tail | `m13-blame-inline.png` |
| blame annotation swimlane | `m13-blame-annotate.png` |
| conflict strip (merge in progress) | `m13-conflict-strip.png` |
| conflict walk (caret on the marker, toast) | `m13-conflict-walk.png` |
| changelist grouping + New Changelist input | `m13-changelists-default.png` |
| Find Usages tree + preview pane | `m13-usages-window.png` |

Transcripts: `/tmp/m13-evidence/*-transcript.log` (`[git] log page`,
`[vcs] status refresh` with branch flips and the `op Some("merge")`,
`[blame]` loads, `[conflict]` faces).

## Deviations from the brief

- **The conflict chips' click-through and the changelist create/move
  click-through are on the manual checklist** — the synthetic mouse
  channel could not reach the strip/list chips tonight (the M12
  report's identical contention; HID taps, PID posting, click-state
  and System Events all tried). The keyboard-reachable faces (the walk
  actions, the no-more toast, ⌘⌥N create+switch via the popup input,
  ⌘⌥F7) are live-verified above; the chip flows ride unit-tested
  application faces and the proven stage/workspace-edit adapters.
- **The branches popup's key is ⌃⌘G (app-added)** — the brief named the
  status-bar click and the VCS menu as the entries ("⌘⌥N 之外的惯例
  入口"); the registry requires every row to display a binding, and
  ⌘⇧N is the preset's `file.newScratchFile` row (untouched).
- **The blame swimlane truncates labels by character budget** (28
  chars) — the paint cannot clip text, so the label itself truncates
  (the pure face is unit-tested on the budget).
- **Log dates are UTC** (`git_graph::format_date_ms`, dependency-free
  civil-from-days; local time is a noted limitation).
- **The log totals line degrades gracefully** — upstream's `log_total`
  stayed `None` for plain all-branch queries in tonight's runs; the
  window shows the loaded count (an upstream seam to revisit).
- The brief's "行级 blame 交互编辑" stays out (the declared boundary);
  blame is read-only, the lane is click-inert.

## Manual verification checklist (needs the user's hands)

- [ ] `script/verify.sh` on an uncontended session (the budget gate's
      key_to_px/scroll legs need HID input; the re-run below ran
      while other applications held the front).
- [ ] The conflict chips end-to-end on the scratch repo:
      `/tmp/m13-scratch.sh /tmp/m13-scratch`, recreate the conflict
      (`git checkout -b conflict-demo; … ; git merge conflict-demo`),
      then `python3 /tmp/m13-conflict3.py` (self-contained: Accept
      Yours → Mark Resolved → Continue; asserts `git log`'s merge
      commit). Tonight the synthetic clicks never reached the strip.
- [ ] The changelist create/move click-through:
      `python3 /tmp/m13-clist.py` (clicks the inline input, types the
      name, ⏎; then a row's Move to Changelist menu), then confirm
      `.openidea/changelists.json`.
- [ ] The Find Usages preview with content: click a usage row (the
      preview pane fills with the bracketed line).
- [ ] The blame swimlane on a file with mixed commit ages (the five
      bands' colors side by side) and the inline-tail hover box.

## Leftovers / known limits

- The log list is a plain scroll (no `uniform_list` virtualization) —
  the 500-row page cap keeps a page cheap; virtualizing the rows is
  the seam if repositories need multi-thousand-row pages.
- Lane rendering caps at 12 lanes (wider topologies clip the graph
  column; the summary still reads).
- The branch filter lists local branches only (remote-tracking scopes
  are an upstream `LogQuery.branch` string away — an M14 face).
- `git_run_op` names its panels `git merge --continue`/`--abort`
  verbatim (a rebase in progress would mislabel the panel — the
  operation string is available on `self.git_operation`).
- The date column and the blame lane speak UTC.
- The usages preview highlights the symbol's FIRST occurrence in the
  line (a same-line double occurrence highlights the first).

## Seam intelligence for M14

- `git_graph::assign_lanes` + `paint_graph_row` are the whole graph
  face — a Log settings face (作者/路径/日期 columns, topo vs date
  order) only changes the `LogQuery` fields the window already builds.
- The status-bar branch widget is the branches popup's click entry —
  M14's keymap settings page can rebind `workbench::Branches`
  (⌃⌘G) like any row; the popup's action verbs (`git_run_op`) are the
  generic "run an ide-vcs write and show its output" face — stash/
  shelve UIs (the declared out-of-scope items) ride it unchanged.
- `RunPanelSeed.finished` makes any tool open a settled output tab —
  the M14 TODO/coverage faces can reuse it for one-shot commands.
- The conflict strip's blocks ride `DocPatched` reconciliation — any
  M14 refactoring-preview face that edits a buffer through workspace
  edits inherits the tab-reconcile for free.
- `editor::ShowUsages`'s `usages_force_window` pattern (a one-shot
  flag steering the answer's route) is the general shape for
  "same engine, two presentations" actions.
- The drained-rows bug class: the editor's paint phases share
  `PrepaintState` by reference — new paint-side faces must not consume
  fields a previous phase drained (the numbers bug was exactly this).

## Post-closeout verify (the recorded re-run)

```
$ M8_VERIFY_OUT=/tmp/m13-verify-closeout bash script/verify.sh
== stage 1: engineering gate (fmt + clippy + tests)     180/180 ok
== stage 2: screenshot smoke                            5/5 ok
== stage 3: budget gate (Go module scenario, debug + release)
gate-debug    key_to_px                        16.3   16   max 20.5 (n=12)   FAIL  ← the documented M10 carry-over
gate-debug    find_in_files_first_response      9.0  1500                     PASS
gate-debug    scroll_frame_during_index        13.9    50  max 14.3 (n=150)  PASS
gate-debug    open_first_paint                 92.0   400                     PASS
gate-release  key_to_px                         5.7    8   max 8.8 (n=12)    PASS
gate-release  find_in_files_first_response      6.0   800                     PASS
gate-release  scroll_frame_during_index         0.3    25  max 0.5 (n=150)   PASS
gate-release  open_first_paint                 70.0   200                     PASS
```

Disposition: the single FAIL is the debug `key_to_px` carry-over M10
introduced and M12 recorded (M12's in-suite 16.3 → its warm re-measure
15.4); tonight's warm attempts read 16.7–16.9 with other applications
holding the front — every other budget passes in-suite, and the M13
additions sit outside the measured scenario's path (the Git/Usages
windows closed, no blame lane on the scenario's file). The
uncontended re-run is the manual checklist's first item.
