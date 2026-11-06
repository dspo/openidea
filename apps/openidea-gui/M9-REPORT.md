# M9-REPORT — first upstream sync + the keymap & navigation habit layer

Branch `feat/gpui-m9-habits` (based on `feat/gpui-m8-daily-driver`), worktree
`~/worktrees/openidea/feat-gpui-m9-habits`. Per the roadmap discipline: local
commits only, no push, no PR. Every phase commit is green (`cargo fmt`,
`cargo clippy --all-targets -- -D warnings`, unit tests — 112 in the crate at
closeout, up from M8's 72). `script/verify.sh` runs all three stages green at
closeout (evidence below); the main workspace compiles and its tests pass
(with one pre-existing upstream flake, see the regression section).

**Fork hygiene at closeout** — the exit bar of the fork principle:

```
$ git diff upstream/main..HEAD --stat -- crates/ languages/ src/ src-tauri/ wit/ Cargo.lock
(no output: zero diff)
$ git diff upstream/main..HEAD --stat -- Cargo.toml
 Cargo.toml | 4 +++-
```

The only backend-tree change is the root `Cargo.toml` exclude line (with its
two comment lines) — exactly the structural exception the fork principle
allows. `languages/` is back to upstream-exact; the gopls opt-ins the fork
used to carry there now live in the app (phase 0).

## Scope delivered

| Phase | Content | Commit |
|---|---|---|
| 0 | **The first upstream sync**: `git merge upstream/main` (275 commits, fork base `16df2ae4` → `6b0dc939`, v0.1.35 → v0.1.37; 1040 files, +85k lines). Conflict surface as predicted: root `Cargo.toml` (ours kept) and `languages/go.toml` (ours dropped — take upstream). The app compiled against the merged tree with **zero adapter changes** — the upstream drift in `ide-vcs`/`ide-lsp`/`ide-workspace`/`ide-vfs`/`ide-index` was additive. The gopls two data lines move app-side: a `LspManager::set_config_hook` overlay in `host.rs` fills `semanticTokens` + the three `hints` kinds before every gopls launch, only when nobody else set options (unit-tested, panic-free parse). | `d486b565` (merge), `cfba6a26` |
| 1 | **Keymap system**: `keymap_data.rs` generated from the upstream frontend's presets (`script/gen-keymap-data.py` — intellij-macos 357 rows, intellij-windows 357, vscode-macos 359, vscode-windows 41; data only, per the clean-room rules). `keymap.rs` holds every default binding in one table (`BINDINGS`), each row carrying its preset id; the IDEA→gpui keystroke converter implements the macOS physical-key conventions (keys named from the key **code** with no modifiers — so ⌥-combos match whatever ⌥ produces; shift+punctuation arrives as the shifted character with shift dropped: ⌘⇧[ binds as `cmd-{`, ⌃⇧/ as `ctrl-?`; the literal minus is gpui's trailing dash). User overrides persist in the settings store (IDEA notation, empty list unbinds) and replace rows at registration. The modifier-only chords (`Shift Shift` / `Alt Alt+Up` / `Ctrl Ctrl`) move into a headless-tested `Chord` tracker fed by the modifiers events plus a new root key listener for the follow-ups. | `831330e2` |
| 2 | **Edit habits** (the preset's 编辑段): ⌘D duplicate line/selection, ⌘/ and ⌘⌥/ (+ the preset's ⌃⇧/ face) comments, ⌥↑/⌥↓ extend/shrink selection (smallest tree-sitter node strictly containing the selection, word/line fallback, growth stack), ⌥⇧↑/⌥⇧↓ move lines (multi-caret, blocks ride their carets), ⌘⇧↑/⌘⇧↓ move-statement rows ride the line move (see deviations), ⌘⌫ delete line, ⇧Enter/⌘⌥Enter new line below/above, ⌃⇧J join, ⌘⇧U toggle case, ⌘- / ⌘= / ⌘⇧- / ⌘⇧= folds, ⌃L scroll to center, ⌘R replace bar, ⌘G/⌘⇧G find next/previous (with nothing to walk they find the word at the caret), ⌃Space completion, ⌘P parameter info, ⌥⌥↑/⌥⌥↓ clone caret via the chord gestures. The transforms are pure `Buffer` generators pinned headless (10 tests); the live drive walks them end to end. | `2f1c8f19` |
| 3 | **Navigation habits**: `navigation.rs` (pure, unit-tested) — the ⌘[/⌘] `NavStack` (bounded linear history, push truncates forward), the ⌘⇧E edit-location MRU, the persistent `Bookmarks`, the ⌘L parser. The workbench routes every jump through `jump_with_nav`/`jump_to` (departures recorded at tab switches, the editor's `OpenLocation` jumps, and each popup entry), keeps the ⌘E file MRU, records ⌘⇧E entries when a tab leaves or a save lands, aggregates diagnostics into F2/⇧F2 error walks (severity Error, sorted, wrapping), and adds six palette modes: ⌘E recent files, ⌘⇧E locations, ⌘F3 bookmarks, ⌘F12 structure (LSP outline, flattened, generation-guarded), ⌘L, ⌃⇥ switcher (tabs in MRU order + dock pages, cycling on repeat presses). ⌘⌥B/⌘⇧B ride the editor's jump list with `GotoKind::Implementation/TypeDefinition`; ⌘⇧⌫ jumps to the newest edit among open tabs (already-there walks back through a memory slot). | `70e41d66` |
| 4 | **Placeholders + the reconciliation table**: `reconcile_data.rs` generated (`script/gen-reconcile-data.py`) — all preset ids, three states. ⌘⇧A **Find Action** joins the palette: implemented actions execute, placeholders render muted with their IDEA keys and toast "is not yet available" on trigger. | `43925a5a` |

Plus this report and the evidence screenshots (`assets/m9-*.png`, 9 files,
window-scoped only).

## The 221-list reconciliation

The roadmap's "221 条" was already stale when written: the preset carried 352
entries at the fork base and the upstream sync grew it to **357**. The file is
the authority (`src/platform/keymaps/intellij-macos.ts`), so the table covers
all 357. The state lives in `src/reconcile_data.rs` (generated by
`script/gen-reconcile-data.py` — M14 re-runs it after each sync); the
generator's curation rules are committed with it.

**Counts: done 61 · placeholder 160 · na 136.**

- **Done (61)** — every id the keymap binds (read live from `keymap.rs`, so
  the table cannot drift from the real bindings) plus the two clone-caret
  rows and `runAnything.show` (their chords complete; the run-anything action
  itself toasts as not-yet-available). This is the M0–M8 face (⌘B, ⇧⌘O, ⌘F,
  ⇧⌘F, ⌃R, ⌘K, ⌘1/⌘4, ⌘,/ …) plus the whole M9 set above. Notes: ⌘⇧↑/⌘⇧↓
  move-statement is the **line-level move** (a real batch with caret riding,
  not a syntax-statement move — the note lives here, the row still counts
  Done only for its line behavior); ⌃D stays Show Diff (run.debug's ⌃D row is
  Placeholder until M11); F12 is the perf HUD (IDEA's jump-to-last-tool-window
  keys coincide — the preset agreement test lists the divergence explicitly);
  ⌘⇧T reopen-closed-tab has no IDEA default (the app keeps its M7 key,
  likewise an explicit exception).
- **Placeholder (160)** — ids Find Action shows in the "Not yet available"
  section with their preset keys; triggering toasts. Curated list in the
  generator: the goto-family (class/symbol/type hierarchy/call hierarchy),
  surround-with/live-template/complete-statement, fold levels 1–5 and
  recursive folds, multi-caret occurrences, the next-tab/split family,
  quick documentation/definition, the refactor extract family, the VCS
  push/branch/annotate family beyond commit, run/debug configs, the editor
  font-size keys, inline completion, zen, and the rest of the deferred
  editor machinery.
- **Na (136)** — the out-of-scope families by prefix: `db.*`, `debug.*`,
  `coverage.*`, `github.*`, `agents.*`, `http.*`/`graphql.*`, `terminal.*`,
  `markdown.*`, `image.*`, plus the popup-scoped rows (`find.popup.*`,
  `find.field.*`, `usages.*`) whose key-when surfaces the app does not have.

Exit condition "221 条对账表落地": met against the authoritative file's real
count (357), with the discrepancy documented here rather than frozen at a
stale number.

## The upstream sync — what it brought, what the app did

Sync: `16df2ae4` (v0.1.35) → `6b0dc939` (v0.1.37), 275 upstream commits.

Highlights the fork inherits (white-box benefits for the coming milestones):
the ide-vcs log/history rework (log index, block history, cancellable log —
M13's data plane), ide-lsp's watched-files/file-watch-roots hooks and
config/servers hooks (the app already rides `set_config_hook`), ide-index's
dir search, ide-vfs's rename validation, Maven/Gradle/Unity/PHP/Ruby build
families (out of the app's path), the plugin marketplace, and the keymap
preset growth (352 → 357 rows — the generator re-run picked it up).

**App adaptation list: empty.** The consumed API surface (`Workspace::load`,
`Documents` + `DocumentObserver`, `Watcher::start`, `LspManager` +
`did_*`/hooks, `ProjectIndex`, `ProcessManager`, `Repo`, `files_with_word`)
all kept their shapes; the drift was additive (new fns, widened signatures on
paths the app does not call). `cargo check` on the merged tree passed before
any edit. The two data lines the fork used to carry in `languages/go.toml`
became the `host.rs` config-hook overlay with the same payload
(`semanticTokens = true`, `hints = { assignVariableTypes, parameterNames,
functionTypeParameters }`), gated on gopls and on options being unset —
unit-tested both ways.

**Sync mechanism established for the coming milestones**: regenerate
`keymap_data.rs` and `reconcile_data.rs` after every merge (both generators
committed; each asserts its own completeness — the keymap test re-checks
bound keys against the preset, the reconcile test pins the table to the
preset one-to-one), then run `script/verify.sh`.

## Regression record

`script/verify.sh` at closeout (exit 0, evidence in `/tmp/m9-verify`):

```
== stage 1: engineering gate (fmt + clippy + tests)
running 112 tests … test result: ok. 112 passed; 0 failed
== stage 2: screenshot smoke           (5/5 states captured)
== stage 3: budget gate (Go module scenario, debug + release)
gate-debug    key_to_px                   15.2   16  max 20.2 (n=12)   PASS
gate-debug    completion_first_frame      22.1  500  max 22.1 (n=1)    PASS
gate-debug    find_in_files_first_response 12.0 1500 max 12.0 (n=1)    PASS
gate-debug    scroll_frame_during_index   14.8    50  max 15.5 (n=146) PASS
gate-debug    open_first_paint           113.0   400  max 113.0 (n=1)   PASS
gate-release  key_to_px                    2.8     8  max 5.9 (n=12)    PASS
gate-release  find_in_files_first_response 7.0   800  max 7.0 (n=1)     PASS
gate-release  scroll_frame_during_index    0.3    25  max 0.3 (n=146)  PASS
gate-release  open_first_paint            76.0   200  max 76.0 (n=1)    PASS
== verify: all stages green
```

(completion_first_frame release is n/a in this run's sampling, as recorded in
M8; debug 22.1 ms vs the 500 ms budget.) M0–M8 behavior rides the harness's
own smoke (open → HUD → find → settings → welcome) plus the unit suite's M7
Esc/focus chain and M8 fixes — all green.

Main workspace: `cargo test --workspace` compiles everything and passes all
suites with one **pre-existing upstream flake** — `ide-http`
`client::tests::localhost_reaches_ipv4_and_ipv6_servers` fails when the whole
workspace runs in parallel (loopback bind contention), passes in isolation
(verified twice). Not our diff: the backend tree is zero-diff and the app
crate is excluded from that workspace.

### The live drives (transcripts + shots under /tmp/m9-edit, /tmp/m9-nav)

Edit habits (CGEvent channel, clipboard-paste discipline, 10/10 PASS):
⌘D duplicate → undo → ⌘/ comment → ⌘/ uncomment → ⌥⇧↓ ×2 → ⌥⇧↑ ×2 (restore)
→ ⌘⌫ delete → undo → ⌘⇧U toggle case → ⌃⇧J join — every step asserted on the
saved file content. Screens: `assets/m9-edit-comment.png`.

Navigation (transcript `[nav]` lines + shots):
- ⌘L jump → ⌘[ back (`[nav] back …main.go:10` in the transcript).
- F3/⌘F3 bookmark toggles (`[nav] bookmark +/- helper.go:5`) and the
  bookmarks list — `assets/m9-bookmarks.png`.
- F2 error walk (`[nav] error 0/2: expected ';', found _`) —
  `assets/m9-error-jump.png`.
- ⌘F12 structure popup with gopls's outline — `assets/m9-file-structure.png`.
- ⌃⇥ switcher (tabs in MRU order + the Find dock page) —
  `assets/m9-switcher.png`.
- ⌘E recent files — `assets/m9-recent-files.png`; ⌘⇧E recent locations —
  `assets/m9-recent-locations.png`.
- ⌘⇧⌫ last-edit jump — `assets/m9-lastedit.png` flow (n4 shot).

Find Action (⌘⇧A): implemented actions execute; typing "surround" surfaces
the placeholder section — `assets/m9-find-action-placeholders.png` ("Surround
With — not yet available  Meta+Alt+T"); Enter toasts —
`assets/m9-find-action-toast.png`.

## Fixed on the way (found by the harnesses)

1. **Find Action's first keystroke crashed the app** (index out of bounds):
   the section bounds used the raw placeholder list while `picker_rows()`
   re-filters it by the query — bounds now end at `rows.len()`. Found by the
   ⌘⇧A probe, not by a unit test (the render path is window code).
2. **Comment-toggle byte/UTF-16 mix**: `comment_line_edits` added the token's
   UTF-16 length to a byte offset before slicing the line string — a latent
   CJK-line panic (an uppercase token stays ASCII so tests passed; the fix is
   byte-domain arithmetic).
3. **Move-down into the phantom last line**: a document ending in `\n` has an
   empty final line; moving a block down swapped with it and produced a
   stray blank line. The guard now requires the neighbor to have a trailing
   newline.

## Deviations from the brief

- **"221 条" is 357 条.** Documented above; the roadmap's number was stale
  against its own authoritative file. The table covers the real file.
- **⇧⌘↑/⇧⌘↓ (move statement) is the line-level move.** The roadmap itself
  says "⌘⇧↑/⌘⇧↓ 移动行"; a true statement move needs the parse-driven
  statement model (deferred list). The reconcile table keeps both moveLine
  and moveStatement rows Done-at-line-level with this note.
- **The doc-edge selects lost ⇧⌘↑/⇧⌘↓** (they were the M2 bindings of
  SelectDocStart/SelectDocEnd). IDEA's move-statement habit wins the keys;
  the actions stay registered but unbound. A mild M2-muscle-memory regression,
  deliberate.
- **⌃D stays Show Diff** (an explicit divergence from run.debug's ⌃D, M11
  retargets it — the keymap test carries the exception with the reason).
- **F12 is the perf HUD**, not jump-to-last-tool-window (the preset row's
  keys coincide; M10's tool-window work retargets). Listed as a keymap-test
  exception.
- **vscode/eclipse presets**: intellij-windows/vscode-macos/vscode-windows
  ported as data (备用); eclipse presets were not in the brief's list and are
  not ported (the generator can add them in one line if M14 wants them).
- **The settings keymap table still shows the app's registry** (M7 shape).
  The preset-wide keymap page with the recorder is M14's; the reconcile table
  is its data source.

## Known leftovers / deferred

- **Intermittent IME-composition crash (P2, needs a frame).** Twice during
  ~10 scripted sessions the app died with `end byte index 19 is not a char
  boundary; it is inside '是'` — the user's live Chinese IME composed into a
  popup/editor input during rapid synthetic keystrokes, and something slices
  the composition string at a byte index. The panic hook recorded both
  crashes (crash.log; "crashes are never silent" worked as designed). Not
  reproduced with `RUST_BACKTRACE=1` across four targeted attempts; the
  minimal ⌘L/⌘F3/⌘E sequences pass. Suspect: upstream input/IME byte-slicing
  (gpui-component input preedit) triggered by M9's additional popup surfaces;
  the saved files were always clean (marked text never committed). Manual
  check: type through the real IME into ⌘L/⌘⇧A inputs; if it recurs, capture
  the frame.
- Statement-level move (needs the parse-driven statement model) — deferred
  list.
- Bookmarks render only in the ⌘F3 list; gutter markers are M10/M12 surface.
- ⌘E/switcher list closed files/tabs by session order only; the index's
  recency (⌘⇧O empty query) stays the other list.
- The settings keymap table (M7) is unchanged; the M14 Settings keymap page
  owns the preset view + overrides editor + recorder.

## Manual verification checklist (needs the user's hands)

- [ ] IME composition inside the ⌘L / ⌘⇧A / ⌘F inputs (the synthetic channel
      bypasses the IME; if the intermittent crash reproduces, capture the
      backtrace and file it against gpui-component).
- [ ] ⌥⌥↑/⌥⌥↓ clone-caret with the real modifier cadence (the chord window is
      600 ms; held-then-arrow works headless and in the drives).
- [ ] ⌘[ / ⌘] across real goto-definition jumps (the drives exercised
      ⌘L/⌘E/switcher histories; goto rides the same OpenLocation hook).
- [ ] ⌘F12 on a Rust file (rust-analyzer's outline shape; the drive used
      gopls).
- [ ] The keymap override file: edit `settings.json`
      `keymap_overrides` by hand (e.g. `"file.save": ["Ctrl+S"]`), relaunch,
      confirm ⌃S saves and ⌘S does not; empty list unbinds.
- [ ] F2/⇧F2 wrap-around at the last/first error with mixed warnings
      (the walk is errors-only by design).
- [ ] Switcher on a many-tab session (the drive used 2 tabs + 2 docks).

## Seam intelligence for M10 (workbench structure)

- The picker now hosts seven modes with per-mode section headers — the
  tool-window/Find Action surface machinery M10's menus need is `open_picker`
  + `picker_rows` + section bounds; adding a mode is one enum arm + rows fn.
- `navigation.rs` is the place M10's status-bar widgets read (can_back /
  can_forward are there for exactly that) and where M12's test-tree jumps
  should push history (`push_nav`/`jump_to` are `pub(crate)`).
- Bookmarks: `Bookmarks::lines_of` was removed as dead in M9 — M10's gutter
  markers should re-add it (the persistence shape is in place:
  `settings.bookmarks`).
- The chord tracker (`keymap::Chord`) is generic over the `GESTURES` table —
  M10's tool-window chords (e.g. any new double-strike) are one row + one
  `run_gesture` arm.
- The upstream merge landed tool-window stripe work in the Tauri frontend
  (`src/workbench/toolwindows/`) — semantic reference for M10's stripe,
  clean-room rules apply (no copying).
- `keymap::bind` panics on unknown action names at startup — the headless
  `bindings_register_against_the_action_registry` test guards it; add rows
  there when M10 registers new actions.

## Commit list (oldest first, this milestone)

```
d486b565 Merge remote-tracking branch 'upstream/main' into feat/gpui-m9-habits
cfba6a26 feat(gui): the gopls opt-ins move app-side — a config-hook overlay restores what go.toml used to carry (apps/openidea-gui)
831330e2 feat(gui): the keymap layer — IDEA presets as data, user overrides, double-strike chords, physical key matching (apps/openidea-gui)
2f1c8f19 feat(gui): the edit habits — the IDEA editor chord set lands (apps/openidea-gui)
70e41d66 feat(gui): the navigation habits — history, recents, bookmarks, errors, structure, switcher (apps/openidea-gui)
43925a5a feat(gui): Find Action and the reconciliation table — every preset row accounted for (apps/openidea-gui)
(+ this report and the evidence screenshots)
```
