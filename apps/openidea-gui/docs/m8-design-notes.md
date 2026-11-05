# M8 design notes — daily-driver closeout

Written before any code, per the milestone discipline. Scope: ROADMAP §M8
(stability, state restore, perf budgets, soak, leftover triage, regression
harness, closeout report). Inputs triaged: M5/M6/M7 REPORTs (Known leftovers +
manual checklists), M7's "seam intelligence for M8", the existing F12 HUD
(`editor.rs` `Hud` cells + the `[perf] hud` transcript line), `settings.rs`
(one JSON store, parse-round-trip writes, corrupt-file replace).

Constraints carried forward: local commits only (no push, no PR); `crates/`,
the root `Cargo.lock` and `languages/*.toml` stay zero-diff; the main
workspace untouched; scratch and soak data live under `/tmp`; every phase
commit is green (fmt + clippy `-D warnings` + unit tests); clean-room (no Zed
sources — only this repo's crates and the Apache gpui-pre/gpui-component
snapshots).

## Phase 1 — stability

### Panic hook

A panic in a gpui app dies quietly by default (stderr only, terminal gone for
a launcher-launched app). Design:

- `main.rs` installs `std::panic::set_hook` before the runtime starts. The
  hook: (1) appends one entry to `~/Library/Application Support/openidea-gui/
  crash.log` — timestamp, thread, payload, location, note (crash-log writes
  never panic; every fallible op is ignored); (2) `eprintln!`s the same lines
  (the scripted-session transcript keeps carrying the story); (3) marks the
  run unclean (below); (4) chains into the default hook so the OS-side report
  behavior is unchanged.
- The **last-run toast** (the brief's "最后一次 toast，不静默死"): a toast needs
  the gpui App, which a panic hook does not have (and pushing UI from inside
  unwinding is how secondary panics happen). The honest shape is: record the
  crash durably, surface it at the next launch. `settings.rs` gains a
  `last_exit.json` sidecar (`{"clean": bool, "reason": …}`) next to
  `settings.json`: written `clean=false` by the panic hook, `clean=true` by
  the graceful Quit action, flipped to "clean (running)" right after the
  startup read. Startup: an unclean marker → info toast "The previous session
  ended unexpectedly" (+ reason, truncated) pointing at the log path. Signal
  kills (SIGKILL/lldb) leave the stale unclean marker — the toast says
  "unexpectedly", which is true, and the hook-annotated crash.log disambiguates
  panics from kills.
- Unit-testable faces: the entry formatting (`crash_entry()`) and the
  last-exit read/clear round-trip (file-based tests under `/tmp` via the env
  override — see the settings note below).

### unwrap/expect/panic! audit

Full inventory (non-test code) with the classification **startup** (controllable,
process cannot continue) / **guarded** (internal invariant proven in the same
block; message carries context) / **runtime-hazard** (a poisoned lock, an
empty slice, a race — the process dies on user data):

| Site | Class | Action |
|---|---|---|
| `main.rs` runtime build | startup | keep (message) |
| `main.rs` window open | startup | keep (message) |
| `fuzzy.rs` consecutiveness `last/first unwrap` | runtime-hazard | guard empty → 0 |
| `workbench.rs` find-files `Mutex::lock expect` ×2 | runtime-hazard (poisoning) | recover from poison (`into_inner`) |
| `workbench.rs` dock `_ if find.is_some() => unwrap` | guarded shape | rewrite `if let` (no unwrap) |
| `workbench.rs` overlay-class `unreachable!` ×2 | guarded (exhaustive enum) | keep (context) |
| `editor.rs` goto "checked above" expect | guarded | rewrite `if let` |
| `editor.rs` HUD dispatch `unreachable!` | guarded (exhaustive enum) | keep (context) |
| `editor.rs` hint-merge "checked above" expect | guarded | rewrite `if let` |
| `templates.rs` suffix `chars().next() expect` ×2 | guarded (index < len) | guard with `and_then` |

Everything under `#[cfg(test)]` stays — unwrap in tests is the convention.
The audit table ships in M8-REPORT.md; the fixes are one commit with tests
where a behavior changes (fuzzy empty-input, poisoned-lock recovery).

## Phase 2 — session restore (window/tab state)

M7 persists the layout (tree/dock, sizes) and the recents; the editor side is
missing: which files were open, which was active, where the carets were.

- `settings.rs`: `session: BTreeMap<String, SessionTabs>` keyed by the
  project root (canonical path, matching the recents entries).
  `SessionTabs { tabs: Vec<TabSession>, active: Option<usize> }`,
  `TabSession { path, cursor_line, cursor_col_utf16 }`. Every field
  `#[serde(default)]` (older files keep parsing — the store's stated rule).
  Hard caps: ≤ 30 tabs per project, ≤ 20 projects (the M7 watchpoint: the
  file must never grow unbounded).
- **Write discipline** (the other M7 watchpoint — settings write
  amplification): cursor moves happen per keystroke and are kept **in memory
  only**; the store is written on the rare structural events (tab
  open/close/activate/pin) and on graceful quit / project switch, never on
  typing.
- Restore at startup: `initial_project` resolves as today; if a session entry
  exists for that root, the workbench opens the recorded paths in order
  (existing files only), re-activates the recorded active tab, and places
  each pane's caret (`EditorPane` gains `cursor_pos()` — the primary
  selection as 0-based line/UTF-16 col, the IPC convention — and
  `place_cursor(line, col)`; the existing `jump_to_offset` path covers
  scrolling the caret into view). Restore failures (file gone) skip that tab
  and shrink the session.
- `OPENIDEA_GUI_PROJECT` (the scripted-session override) disables restore so
  scripted sessions stay stateless — the same rule the env override already
  claims for project selection.

## Phase 3 — perf budgets

The HUD already measures key→px, frame interval, rows/spans, parse ms; the
transcript already carries `[perf] hud …`, `[find] … in <ms> ms`, and
`[perf] first paint <ms> after open`. Gaps: completion first-paint and an
explicit per-metric budget contract.

- `perf/budgets.json` (in-app, `include_str!`-read): each metric gets
  `budget_ms` (debug) and `budget_release_ms`, with the M2/M5/M6 baselines in
  comments-adjacent fields (`baseline_*`). Metrics: `key_to_px` (gopls+hints
  on), `completion_first_frame`, `find_in_files_first_response`,
  `scroll_frame_during_index`, `open_first_paint`.
- HUD extension: the key→px line gains the budget ("key→px 5.1 ms · budget
  16"), and a new last line shows `budget ✓/✗` for the session's best/worst —
  the HUD becomes the budget table's live face. The authoritative comparison
  is the harness script (below), which parses the transcript into a measured
  JSON and exits non-zero on breach.
- Instrumentation additions (both eprintln-shaped, transcript-driven like
  every prior milestone): `[perf] completion request v<n> t+<ms>` on the
  debounced request and `[perf] completion painted v<n> after <ms> ms` at the
  popup's first paint; `[perf] indexing on/off` around live `$/progress`
  tasks so the scroll-frame samples can be correlated with indexing.
- Scenarios: (a) this repository (multi-language, real size) — open a Rust
  file, gopls/rust-analyzer off is fine for key→px (the LSP layer is idle
  typing), on for hints where available; (b) `/tmp/m8-gomodule` — a synthetic
  ≥100-package Go module (generator script, `go.mod` + 100 dirs × 4 files,
  real cross-package references so gopls does real work). Scroll during the
  first index (status task visible) for the index-time frame metric. Debug
  and release (`cargo build --release`) each measured once; numbers land in
  M8-REPORT.md against budgets.

## Phase 4 — simulated soak

- Driver: the established synthetic-input lineage (CGEvent keystrokes for
  modifiers/navigation, **clipboard paste for all text** — the IME discipline
  from M5–M7), plus `ps` RSS sampling per minute into a CSV. Scripts live in
  `/tmp` (machine-specific driving, per the M5/M6 precedent), referenced from
  the report.
- Loop (≥ 30 min): paste-edit-⌘S cycles on the Go module, ⌘W/⌘⇧T tab churn
  (≥ 100 open/close cycles), ⌘1/⌘4 dock/tree toggles, ⌃R run start/stop
  bursts (cheap `go run` on a tiny package), palette E2E every few minutes.
  gopls stays on with inlay hints on (the daily-driver posture).
- Watchpoints (M7's list): settings.json size + write count (the file's mtime
  and a `fs_usage` spot check are enough), crash.log empty, run-tab
  accumulation (answer the policy question: manual close is fine if
  exited-tab count stays sane in the loop; else implement keep-last-N),
  RSS curve slope (flat = healthy; gpui's allocator may hold — compare
  against the M2-era behavior, judge by sustained growth only).

## Phase 5 — leftover triage

Input: the three reports' Known leftovers + manual checklists. Grades:
P1 daily-blocking / P2 real friction / P3 polish (listed only).

Fix this milestone (P1/P2):

1. **P1 — floating menus ignore Esc** (M7): a right-click menu open + Esc
   leaves it up when focus does not sit on the menu — a keyboard user is
   trapped (mouse required). Empirical first (reproduce in the live session),
   then fix at the dispatch level; candidates ranked: (a) re-focus discipline
   at open (if the component's focus-on-open is being stolen by our
   re-render, stop stealing it); (b) app-side Cancel interception: the
   workbench's `on_cancel` handler, when no overlay of ours is open, closes
   any open floating menu (public face to investigate; the ContextMenu state
   is element-scoped, so this may need a menu-open flag the workbench tracks
   via the DismissEvent subscription the component already emits).
2. **P2 — Find dock page results never clear** (M5): Enter with an empty
   query clears the results and the status hint returns; a stale query stays
   re-runnable. (`FindPanel::start_search` early-returns on empty today.)
3. **P2 — toast dedupe** (M7): identical (level, text) within the auto-hide
   window reuses the live toast instead of stacking (burst of saves currently
   stacks). gpui-component `Notification` has `id1(key)`; a stable key
   (`toast-<hash>` via the workbench) + a within-window suppression in
   `flush_toasts`.
4. **P2 — rename does not preselect** (M5; tree + symbol rename): after
   seeding, `set_selected_range(0..name.len())` (gpui-base InputState has the
   API) so typing replaces, matching IDEA.

P3 (listed, not fixed): capped Find scan stops mid-file silently; per-file
streaming into the dock; recents stale-entry toast timing; settings indent
row ignores `.editorconfig`; related-info multi-hop (blocked by the frozen
crates/, seam documented); inlayHint/resolve; organize-on-save serialization;
`hints_by_line` per-frame rebuild; run-tab auto-clean (only if soak says
manual is fine — else it upgrades to P2).

## Phase 6 — regression harness

`script/verify.sh` (in-app): one command, three stages, non-zero on any
failure:

1. `cargo fmt --all --check` + `cargo clippy --all-targets -- -D warnings` +
   `cargo test` (the crate; ~5 min cold, warm afterwards).
2. Screenshot smoke: generate the scratch project (reusing the Go-module
   generator's small mode), launch the debug app with
   `OPENIDEA_GUI_PROJECT` + `OPENIDEA_GUI_OPEN`, drive a fixed state set
   (open → tab switch → find bar → HUD → settings overlay), capture
   window-scoped (`caffeinate -u` + `screencapture -l <windowid>`, never
   full-screen) into `/tmp/m8-verify`, assert transcript markers + PNG sizes.
3. Budget gate: the perf scenario (short form) against `perf/budgets.json`,
   measured JSON into `/tmp/m8-verify`, breach → exit 1.

Local-runnable only (no CI); synthetic text rides the clipboard-paste
channel; the script kills only PIDs it started.

## Phase 7 — closeout report + full regression

`M8-REPORT.md`: scope table, exit-condition drive, the budget table (numbers
vs budgets, debug+release), the soak RSS table + conclusions, the P1/P2 fixes
with repro→fix evidence, the P3 list, the deferred-build list re-checked
against the roadmap's 边界 section, the one-week human soak guide (how to
launch, what to watch, where issues go), the M0–M7 regression matrix, the
commit list. Screenshots: `assets/m8-*.png` (window-scoped only). PR body →
`/tmp/m8-pr-body.md` (based on `feat/gpui-m7-polish`; no issue number, none
fabricated).

## Sequencing (each its own green commit)

docs → ① stability → ⑤ P1/P2 fixes → ② session restore → ③ budgets → ④ soak
(run + fixes it demands) → ⑥ harness → ⑦ report/evidence. ⑤ lands before ②
so restore code is written against the settled rename/toast surfaces; ④ runs
in the background across ⑥/⑦ work.
