# M4 Report — run surface + minimal Git, ⌘V debt paid (MVP close-out)

**Date:** 2026-11-04/05 · **Branch:** `feat/gpui-m4-run-git` (based on
`feat/gpui-m3-insight`; M0–M3 branches and worktrees untouched) ·
**Platform:** macOS 26 (arm64), app-local toolchain.

**Verdict: M4 scope delivered.** ⌘V paste rides the M2 edit funnel with
multi-cursor semantics; run configurations + a streaming output dock +
stop + exit code/elapsed work through `ide-run`; the file tree colors
git status, a ⌃D overlay shows a unified diff, and ⌘K commits checked
files through `ide-vcs` — verified against a real repo (`git log`
evidence). All phases landed as separate commits, each green (fmt +
clippy `-D warnings` + 24 unit tests) when committed. **One caveat:** the
session's screen stayed locked (`CGSSessionScreenIsLocked: True`), so —
like M3's locked first half — keyboard-driven flows produced full
transcript + disk evidence but **window frames froze and no live
screenshots could be captured**; the capture commands and the four
states to re-drive are listed below for the unlock pass.

## What was built (by phase, commit per phase)

| phase | commit | content |
|---|---|---|
| docs | `690d6bc5` | `docs/m4-design-notes.md` + `docs/m4-cleanroom-log.md`, written **before** any code; Zed not read (see clean-room) |
| 1 | `92225330` | ⌘V paste: `editor::Paste` action (Editor context only, so gpui-component inputs keep their own clipboard), text from `cx.read_from_clipboard()`; `Cause::Paste` gives paste its own undo step, closes a stale completion popup, skips the completion follow-up; cursors map to the end of the inserted text via the same `Selection::after_inserts` math as typing. The range math was factored into the free `paste_ranges(selections)` so the unit test pins exactly what `insert_text` and paste consume |
| 2 | `3b297f07` | Run: `Host` owns an `ide_run::ProcessManager`; the ⌃R palette lists user configs (`.openidea/run.toml`) + detected ones, and its query doubles as a one-off shell command line (`Run command: …` row). Start runs on the background executor; stdout/stderr/system chunks stream through the bus into a bottom-dock `RunPanel` — gpui `uniform_list` (virtualized), follow-the-tail until the user wheels up, Stop button (SIGINT→SIGKILL process group), Clear, exit code + elapsed in the header; a 500 ms tick keeps the elapsed honest while a run is quiet |
| 3 | `05f30e6c` | Git status: `Host` discovers `ide_vcs::Repo` at open; status + branch refresh on the background executor, coalesced across `FsChanged` bursts (`git_refreshing`/`git_refresh_queued`); tree file names tint by working-tree kind — modified blue, added/renamed green, untracked olive, deleted/conflicted red (`theme::git_status_color`, hardcoded palette per plan) |
| 4 | `b38b47b4` | Diff: ⌃D opens a read-only overlay for the changed file at hand (tree's selected row when it carries a git change, else the active tab; a dirty buffer diffs its own text, else disk). `Repo::line_hunks` build on the background executor into `diff_view::from_hunks` (pure, unit-tested): `@@` headers, `-` red, `+` green, hunks only, no syntax highlighting, Esc/click-outside closes |
| 5 | `300df87c` | Commit: ⌘K opens the commit overlay — every changed file as a checkbox row (checked = staged for this commit), a multi-line message (`gpui_component` `Textarea`), Commit/Cancel. Confirm = unstage the unchecked (so the commit is exactly the checkmarks), stage the checked, `repo.commit` — all on the background executor, the new short id lands via the bus and the status refresh re-colors the tree |
| 6 | `bada9914` | Status bar: `on <branch>` next to the project name; the dock's run status (running · elapsed / exit code · elapsed) on the right; language-server rows and save state unchanged |
| fix | `6e78a4a0` | ⌘Enter commit: the Textarea's own keymap binds `secondary-enter` nearer the focus than any `"Commit"` context of ours, so a `cmd-enter` binding never won; ⌘Enter now arrives as `InputEvent::PressEnter { secondary: true }` and the workbench subscribes. Adds `workbench::StopRun` (⌃⌘R) so the live run is stoppable from the keymap, and `[vcs]`/`[commit]` transcript logging |

24 unit tests (M3's 22 + `paste_ranges_cover_carets_and_selections` +
2 diff-view tests) — `cargo test` green.

## ide-run wiring notes (what the GPUI host does)

- **One `ProcessManager` per open project**, exactly like `ide-host`'s
  `Host::processes`. `ProcessManager::start` forks the process and spawns
  its own reader/waiter/pump threads (8 ms output batching), so the UI
  only calls it from `cx.background_executor()` and receives:
  - output through the plain `on_output` callback → `BusMessage::RunOutput`
    → `RunPanel::append` (chunk boundaries may sit mid-line; the trailing
    fragment carries into the next chunk),
  - exit through `IdeEvent::RunExited { run, code }` on the shared sink —
    the same bus pump as everything else.
- **Stop is ide-run's cancellation path**: SIGINT to the process group,
  SIGKILL after a 3 s grace (verified live: `sleep 30` → stop → exit code
  137, no leftover process). Both the dock's Stop button and ⌃⌘R reach
  `ProcessManager::stop`.
- Configs: `ide_run::load_user_configs` + `ide_run::detect` (user ids
  shadow detected ones), cached once per palette open (the detector reads
  package.json/Cargo.toml/…; fixture-scale work — moving the refresh
  behind a request channel is a leftover seam).
- One run at a time in the MVP dock; a start while one is live warns
  ("A run is already active — stop it first").

## ide-vcs wiring notes

- `Repo::discover(root)` at project open → `Option<Arc<Repo>>`; reads
  (status/head/line_hunks) are gitoxide in-process and re-check on-disk
  state per call, so the long-lived handle never goes stale; writes
  (stage = `git add -A`, unstage = `restore --staged`, commit) shell out
  to git so hooks behave like the CLI.
- Refresh triggers: first render, every `FsChanged` batch, and after a
  commit. Coalescing: one in-flight read; if more bursts arrive it
  re-runs once when the read lands.
- Encoding: UTF-8 constant (`encoding_rs::UTF_8`) through
  `Repo::line_hunks` — matching M0–M3's documents; ide-vcs's full
  encoding detection stays a Tauri-host feature.

## Live verification (dogfooding transcript, commands + results)

Driven with synthetic CGEvents posted to the app PID (`/tmp/m4drive.py`,
M3's driver with a new PID file) against `/tmp/m4-dogfood` (a fresh copy
of `fixtures/rust-app`, `git init`ed with a baseline commit). Scratch
committed afterwards is clean; the repo fixture untouched.

- **⌘V paste (caret)**: `printf 'PASTED-M4-TEXT' | pbcopy` → ⌘⇧O "lib"
  ⏎ (focuses the editor) → ⌘V → ⌘S → `tail` shows `PASTED-M4-TEXT` at
  the caret, on disk. Transcript: `[lsp] opened …`, save round trip.
- **⌘V paste (selection replace)**: `REPLACED-VIA-SELECTION` →
  ⇧⌘← (select to line start) → ⌘V → ⌘S → the line now reads
  `REPLACED-VIA-SELECTION` on disk.
- **Undo granularity**: ⌘Z → ⌘S → the line reverts to the earlier paste
  (`PASTED-M4-TEXT`) — paste is its own undo step (Cause::Paste), undo
  did not overshoot into the typing before it.
- **Run + exit code**: ⌃R → `cargo test` ⏎ → transcript
  `[run] starting cargo test` / `[run] started … (run 1)` /
  `[run] exited RunId#1 code Some(101)` — 101 is the fixture's
  intentional failing test (exit-code + duration path exercised with a
  real compile + test output stream). `echo hello-m4` → `code Some(0)`.
- **Stop**: ⌃R `sleep 30` ⏎ → ⌃⌘R → `[run] exited RunId#3 code Some(137)`
  (SIGINT, then the 3 s grace SIGKILL); `pgrep sleep 30` finds nothing.
- **Git status**: baseline-clean tree → `echo … >> src/lib.rs` on disk →
  transcript `[vcs] status refresh: 1 files, branch Ok(Some("main"))` —
  the same read feeds tree coloring, and the external change auto-reloads
  the clean buffer (M1 behavior intact).
- **Diff**: ⌃D → transcript `[diff] /private/tmp/m4-dogfood/src/lib.rs —
  2 lines` (the probe line + paste line as one modified hunk + additions).
- **Commit**: ⌘K → transcript `[commit] dialog open, focused=true` →
  typed the message into the Textarea → ⌘Enter →
  `[commit] confirm: message "M4 dogfood commit" (18 chars), 1 files` →
  `[vcs] committed 1ad9c1d7c7031…` →
  `git -C /tmp/m4-dogfood log`: **`1ad9c1d M4 dogfood commit`** on top of
  the baseline; `git status` clean; the follow-up refresh reports
  `0 files` (tree colors reset after commit).
- **M0–M3 regression, keyboard-driven**: editing + ⌘S round trip ✓
  (paste/undo/save above), ⌘Z ✓, external reload ✓ (above), ⌘⇧O Go to
  File ✓ (used repeatedly), rust-analyzer lifecycle ✓ (`Starting → Ready`
  on every launch; LSP sync rides the untouched `Documents::edit`),
  run-picker typing ✓. The M4 editor diff touches only the paste cause
  arm and a read-only `buffer_text` accessor; completion/goto/rename
  paths are code-identical to M3 (their M3 live evidence stands; the
  visual re-drive below is on the manual list because of the lock).

## Screenshots (blocked by the locked session — capture commands)

The screen stayed locked the whole session (`CGSSessionScreenIsLocked:
True`): keyboard CGEvents reach the app (all evidence above is from
`/tmp/m4-app.log`), but window frames freeze, so `screencapture -l`
returns stale frames. When the screen is unlocked, re-drive and capture
(window id via the CGWindowList one-liner below; app launched with
`OPENIDEA_GUI_PROJECT=/tmp/m4-dogfood OPENIDEA_GUI_OPEN=src/lib.rs`):

- [ ] Output dock running: ⌃R `sleep 60` ⏎ → screenshot the dock
      (`Run` header, command echo, spinner-less elapsed line, Stop/Clear)
      → ⌃⌘R → screenshot the exit-code header.
- [ ] Diff overlay: dirty a file → ⌃D → screenshot (unified +/- colors).
- [ ] Commit flow: ⌘K → screenshot (checked file list, message Textarea,
      Commit/Cancel) → Esc.
- [ ] Tree coloring: dirty two files → screenshot the tree (modified
      blue names) — and the status bar `on main` + run indicator crop.

```bash
# window id of the running app:
python3 -c "import Quartz;print([w['kCGWindowNumber'] for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly,Quartz.kCGNullWindowID) if 'openidea' in (w.get('kCGWindowOwnerName') or '').lower()][0])"
screencapture -l <windowid> apps/openidea-gui/assets/m4-1-run-dock.png   # etc.
```

## Deviations from the brief

- **Commit confirmation moved from a keymap binding to the input's own
  event** (commit `6e78a4a0`): ⌘Enter is bound by gpui-component's input
  keymap (`secondary-enter`, nearer the focus than our `"Commit"`
  context) and arrives as `InputEvent::PressEnter { secondary: true }`.
  Same UX, implemented at the seam the widget actually exposes.
- **One run at a time** (the brief didn't forbid it): the dock is
  single-run; a second start warns instead of queueing. Multi-run tabs
  are a leftover seam.
- **Stop got a keymap entry (⌃⌘R)** beyond the required button — needed
  for scripted verification, and the keymap is the app's discoverability
  surface (REGISTRY row included).
- **Paste undo groups as its own step** (Cause::Paste) rather than
  riding typing's 800 ms group — closer to IDE behavior; the brief was
  silent on granularity.
- **Diff overlay is hunks-only** (no context lines, no syntax
  highlighting) — the brief explicitly allowed "最简".
- **`detect()` reads on the UI thread at palette open** — file-count
  scale work (a handful of manifests), same trade-off M3 made for index
  queries; documented as a seam, not a blocker.

## Manual verification checklist (not scriptable here — needs unlock / real IME)

- [ ] The four screenshot states above (dock running + exit, diff,
      commit dialog, tree colors + status bar).
- [ ] Multi-cursor paste: ⌥-click two cursors → ⌘V — each caret inserts
      a copy, selections replace (`paste_ranges` unit test pins the math;
      the live check needs mouse, which needs the unlocked screen).
- [ ] Wheel-up stops follow-the-tail in the Run dock; the ↓ button
      re-enables it.
- [ ] Checkbox deselect in ⌘K leaves that file out of the commit
      (verified in code: unstage-first ordering; click needs mouse).
- [ ] Completion popup closes when pasting over it (Cause::Paste closes
      it — needs a live popup to observe).
- [ ] Real IME composition still commits through the pipeline
      (untouched M0 path; ⌘V during composition is gated by the same
      `marked` check as every command).

## Leftovers / known limitations (post-MVP seams)

- **Integrated terminal** (user-deferred): the dock is the seam — add a
  "Terminal" tab beside `Run`; `ide-terminal` + the steer-ghostty route
  is written in the M2/M3 reports. `RunPanel`'s uniform_list + PTY
  writer pattern carries over directly.
- **Run**: config editing UI, test tree, multi-run tabs, dock
  hide/show toggle, `emulate_terminal` (the PTY path exists in ide-run
  but the console renders raw bytes as text today).
- **Git**: branch/remote/history/blame UI, per-hunk staging (the data
  path exists: `Repo::stage_hunks`/`stage_patch` + `diff_view`), partial
  commit checkbox behavior needs a mouse pass to verify live, diff of
  staged-vs-HEAD (`against_index=true`), conflict surface.
- **Status bar**: ahead/behind counts (`Repo::branch_info`), VCS
  operation indicator (merge/rebase in progress).
- **Palette**: queries still on the UI thread (M3 seam, still open);
  run-config cache doesn't refresh while the palette is open.
- **Diff**: no syntax highlighting, no context lines, no word-wrap
  (truncate only), no staged/unstaged switcher.
- Dock close/hide button (the dock stays from the first run onward).

## Engineering hygiene

- `cargo fmt --all --check` clean; `cargo clippy --all-targets -- -D
  warnings` clean; `cargo test` 24/24 green in `apps/openidea-gui`.
- Main workspace untouched: `cargo metadata --no-deps` = **36 members**,
  root `Cargo.lock` diff empty on the branch. App-crate
  `Cargo.lock` committed with the new `ide-run`/`ide-vcs`/`encoding_rs`
  deps.
- Scratch discipline: all dogfooding ran against `/tmp/m4-dogfood`
  (a `git init`ed copy of `fixtures/rust-app`); the fixture and repo
  sources are byte-identical to the branch. The app instance was started
  and stopped by PID (`/tmp/m4-app.pid`); only its own processes were
  killed; the `sleep` runs were stopped through the app's own stop path
  and verified gone via `pgrep`.

## Clean-room statement

Zed (GPL-3.0) was **not read** for M4 — see
`docs/m4-cleanroom-log.md`. Everything is built on openidea's own
crates (`ide-run`, `ide-vcs`, `ide-core`) plus the Apache-2.0 gpui-pre /
gpui-component APIs (clipboard, `uniform_list`, `Textarea`), with the
design fixed in `docs/m4-design-notes.md` before coding.

## MVP close-out checklist (M0–M4 deliverables index)

| milestone | branch | tip commits (phase per commit) | report | screenshots |
|---|---|---|---|---|
| M0 — self-drawn element editor, element-level IME | `spike/gpui-ime-m0` | `77191be9` (GO decision) | `apps/openidea-gui/M0-REPORT.md` | `m0-1…m0-3` (normal / composition marked / CJK committed) |
| M1 — workbench shell: tree, tabs, status bar; IO + events via `ide-workspace`/`ide-vfs` | `feat/gpui-m1-workbench` | `0183ba2e` | `M1-REPORT.md` | `m1-1…m1-5` (shell, IME, stale, reload, ⌘O) |
| M2 — rope editor: incremental tree-sitter highlight, multi-cursor/column select/undo, folding, virtualization, perf budgets | `feat/gpui-m2-editor` | `420bd87d` | `M2-REPORT.md` + `docs/m2-design-notes.md`/`m2-cleanroom-log.md` | `m2-1…m2-8` (highlight, IME+HUD, stale, big-file, multi-cursor, ⌘O, release HUD) |
| M3 — actions/keymap; LSP lifecycle, diagnostics, completion, ⌘B/⌥F7/⇧F6; Go to File / Search Everywhere | `feat/gpui-m3-insight` | `43106744` (8 phase commits beneath) | `M3-REPORT.md` + `docs/m3-*` | `m3-3…m3-7` (squiggle, gutter, hover, completion, usages, rename ×3, goto-file, everywhere) |
| M4 — run configs + output dock, git status/diff/commit, status bar, ⌘V | `feat/gpui-m4-run-git` | `690d6bc5…6e78a4a0` (8 commits, table above) | `M4-REPORT.md` (this file) + `docs/m4-*` | blocked by the locked session — capture commands above |

Post-MVP deferred items (user decision): integrated terminal,
debugger UI, profiler/coverage, DB/HTTP/Docker tools, GitHub PR flow,
plugin UI, remote development, spell check, vim mode, themes/i18n —
each with its seam named in the M4/M3 leftovers.
