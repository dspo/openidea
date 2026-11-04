# M1 Report — GPUI workbench shell + real backend wiring

**Date:** 2026-11-04 · **Branch:** `feat/gpui-m1-workbench` (based on
`spike/gpui-ime-m0`, the M0 GO) · **Platform:** macOS 26 (arm64), toolchain
1.97.1 (app-local), Squirrel (Rime) as the live Pinyin IME.

**Verdict: M1 scope delivered.** The window is now a workbench — left dock
file tree, central editor tab stack, thin status bar — backed in-process by
the same UI-agnostic crates the Tauri host drives (`ide-workspace` documents,
`ide-vfs` IO + watching), with backend events delivered to the gpui main loop
over a tokio channel. The open → edit → save → external-change closed loop
was verified live against real files with shell-side `cat` evidence, and the
M0 IME chain demonstrably still works inside a tab (an unplanned but genuine
composition session was captured in both screenshot and protocol trace).

## What was built

`apps/openidea-gui` grew from the M0 spike to ~1.8k lines across eight modules:

- `host.rs` — the backend slice. `ide_workspace::workspace::Workspace::load`
  as the open-project gate (a damaged manifest blocks, a plain folder is
  valid — same as `ide-host`), `ide_workspace::Documents` for open / edit /
  save / dirty / external-change semantics, `ide_vfs::read_dir` for the tree
  and `ide_vfs::Watcher` whose batch callback feeds
  `Documents::on_disk_changed` exactly like `crates/ide-host/src/project.rs`
  does. A tiny extension-map `detect_language` stands in for
  `LanguageRegistry` (M3 territory). All fallible operations return
  `IdeResult<T>`; the UI never touches `std::fs` directly.
- `bus.rs` — the `IdeEvent` bus prototype. Backend crates emit through a
  `SharedSink` closure onto a tokio unbounded channel; a foreground pump
  (`cx.spawn` + timer, Steer's tray-pump pattern) drains it between frames
  into `Workbench::handle_bus`. Carries `IdeEvent`s plus the one UI-native
  message (the ⌘O dialog result).
- `workbench.rs` — the shell: gpui-component `TabBar`/`Tab` (with per-tab
  close buttons and dirty `●` / stale `⚠` marks), the file-tree dock,
  `StatusBar`-based status line (project · file · save state · last message),
  and the action handlers for ⌘O / ⌘S / ⌘W.
- `file_tree.rs` — lazily-loaded directory model over `ide_vfs::read_dir`
  (dirs first, `ide_vfs::natural_cmp` order), expand/collapse, selection,
  keyboard navigation (↑/↓/Enter), refreshed from `FsChanged`.
- `editor.rs` — the M0 hand-drawn element grown from one line to a viewport:
  multi-line shaping with a `LineIndex`, wheel scrolling with
  scroll-follows-caret, Enter inserts `\n`, caret/IME geometry now
  line-aware. The buffer and the `InputHandler` (NSTextInputClient face) are
  the same M0 code paths; the M0-format stderr trace is kept for regression
  comparison. Still no rope / tree-sitter / line numbers — that is M2.
- `theme.rs` / `strings.rs` — palette and user-visible text as plain
  constants (no theme system / i18n in M1, per scope).

Wiring facts: path dependencies `ide-core` / `ide-vfs` / `ide-workspace` from
`apps/openidea-gui` into `../../crates/*`; `tokio` (features `sync` only —
channel, no runtime) and `rfd 0.15` for the native dialog; `gpui-kit-assets
0.8.0` supplies the icon set the workbench icons resolve through. The root
workspace is untouched: `cargo metadata --no-deps` still reports 36 members
with no `gpui*` and no `openidea-gui`; the root `Cargo.lock` has no diff.

## Closed loop, as verified live

App launched with `OPENIDEA_GUI_PROJECT` pointing at this repo's worktree
(the env var wins; without it the app walks up from the cwd to the nearest
`.git`). All captures are window-scoped (`screencapture -l <windowid>`; no
fullscreen pixels anywhere near the repo):

| evidence | shows |
|---|---|
| `assets/m1-1-workbench-tree.png` | tree dock + tab stack + status bar; a tree double-click opened `.m1-scratch.md` in a new tab |
| `assets/m1-2-ime-composition.png` | Pinyin composition **inside the tab**: marked range with accent underline (`地rtyeditabc` mid-composition), tab dirty `●`, status "unsaved changes" |
| `assets/m1-3-external-change-stale.png` | tab `.m1-scratch.md ⚠` and status "stale — changed on disk" after the file changed on disk while the buffer had unsaved edits |
| `assets/m1-4-clean-reload.png` | clean buffer auto-reloaded an external append (`external-appended-line` appears without any user action), tab stays clean |
| `assets/m1-5-cmd-o-dialog.png` | the native NSOpenPanel opened by ⌘O, starting in the project directory |

Shell-side proof of the save round-trip (file content before/after, same
session as `m1-1`):

```console
$ cat .m1-scratch.md                      # after typing + ⌘S in the app
M1 scripted verification file
ascii original line
可以typed-by-script                       # IME-committed CJK + scripted ASCII
$ git status --porcelain .m1-scratch.md
?? .m1-scratch.md                         # untracked scratch, written by the app
```

The live sequence that produced these:

1. Created `.m1-scratch.md` from the shell **while the app was running** —
   it appeared in the tree within ~2 s (watcher → sink → bus `FsChanged` →
   tree refresh). Live proof of the whole event pipeline.
2. Double-clicked the row (cliclick) — new tab, buffer seeded from the
   `DocumentInfo` text, caret at the trailing newline.
3. Typed into the editor (scripted keystrokes + the operator's live Pinyin
   IME — they interleaved, which is visible in `m1-2`), pressed ⌘S.
4. `cat` confirmed the bytes on disk, CJK included — the write went through
   `Documents::save` → `ide_vfs::save_text` (original encoding / BOM / line
   ending), never a bare `std::fs::write`.
5. External appends from the shell: clean tab auto-reloaded (`m1-4`); dirty
   tab went stale (`m1-3`). ⌘W closed the tab and returned to the empty
   state hint.

Two real defects were found by this live run and fixed in this branch:

- **Reload echoed as an edit.** After a backend reload (`DocPatched` →
  `set_text`) the pane's edit counter restarts at 0 while the tab's observed
  baseline stayed at the old value, so the observer immediately re-marked the
  freshly reloaded tab dirty. Fixed by re-baselining `last_seen_edits` after
  `set_text`.
- **⌘O dead at startup, and the dialog never appeared when it did fire.**
  Two layers: with no tab open nothing held focus, so the action was never
  dispatched (fixed: the tree takes the initial focus); and rfd's *sync*
  dialog cannot show an `NSOpenPanel` from a background thread, while its
  async dialog future must be *created* on the main thread (AppKit rule) even
  though it is awaited on the background executor. Fixed with
  `AsyncFileDialog` created in the action handler, awaited via
  `background_executor`.

## ide-workspace / ide-vfs API notes (for M2)

What worked well:

- `Documents` is exactly the right in-process seam for an editor host:
  `open` returns the full `DocumentInfo` (text, encoding, version, dirty),
  `save` handles encoding / BOM / line endings, `on_disk_changed` implements
  the whole clean-reload / dirty-warn policy, and the sink events
  (`DocDirty`, `DocPatched`, `Notify`) are precisely what a UI needs. M1
  wrote **no** dirty-tracking logic of its own beyond mirroring events.
- `Watcher::start(root, excludes, sink, on_change)` mirrors ide-host's
  wiring one-to-one; the debounce / ignore machinery is invisible to the
  consumer.
- `IdeError { kind, message }` converts cleanly at the edge
  (`e.message` straight to the status line); `From<io::Error>` covers the
  VFS layer's needs.

Frictions worth remembering for M2:

- `Documents::replace_all` is the only sane edit primitive for the simple
  buffer (the incremental `ChangeSet` / `TextEdit` paths need versioned
  bookkeeping M1 avoids). M2's rope editor should move to `apply_edits` —
  `DocPatched`'s `TextChange` vec already anticipates it.
- `DocumentInfo.text` is a full copy per open; fine for M1, and M2 will want
  the `TextSnapshot` path instead.
- `DocPatched` also fires for the document's own `replace_all` — a UI that
  pushes buffer text before saving receives its own echo. M1 skips it by
  comparing text; M2 should suppress by version instead.
- `ide_vfs::read_dir` returns `IpcPath` entries (serde-facing); a native
  consumer immediately unwraps `.0`. Harmless but noisy.
- The watcher emits `FsChanged` for the app's own saves too — the UI's stale
  logic keys off the tab's dirty flag to stay correct (clean + own-save ⇒ no
  stale), and `Documents` ignores its own mtime/len on reload. Works, but M2
  may want an explicit "own write" marker in `FsBatch`.

## Deviations from the task brief

- **`bus.rs` uses a tokio channel + foreground pump** as instructed, but the
  pump polls with `background_executor().timer(50ms)` (Steer's tray-pump
  pattern) because gpui has no cross-thread wake primitive. Events therefore
  reach the UI within ~50–100 ms — fine for file events; M2 streams will
  need a channel whose receiver is gpui-executor-native.
- **The file tree shows dotfiles** (`.agents`, `.git`, …) with the default
  excludes list (`**/.git`, `**/target`, `**/node_modules`, `**/dist`) in
  place of the settings-store-driven `file_excludes()`. A `.openidea`
  workspace manifest is honored by the open gate but workspace member roots
  are not merged into the tree view yet.
- **Stale semantics** follow the backend's documented behavior: a stale tab
  keeps its local buffer; ⌘S overwrites the external changes (the status
  warning says so). No diff/merge prompt — later milestone material.
- The M0 `OPENIDEA_M0_DEMO_MARKED` seeding hook and the ctrl+m synthetic
  composition were dropped; the real IME was captured live this time.

## Manual verification checklist (not scriptable from this shell)

- [ ] ⌘O → pick a file inside the NSOpenPanel → its tab opens. The panel
      itself, its initial directory and the ⌘O action are verified
      (`m1-5`); the pick's final click was left to the human — scripted
      panel navigation lost to the machine's active Squirrel IME (the
      go-to-folder path text composes into CJK).
- [ ] Pinyin composition in a tab: `ni hao` → Space commits `你好`, Esc
      cancels, candidate window follows the caret (M0 checklist, now inside
      a tab). `m1-2` plus the `[ime]` stderr trace show the full
      setMarkedText → firstRect → insertText round-trip already working
      in-tab, including commit replacing the marked range.
- [ ] Wheel-scroll a file taller than the viewport; caret-line scroll
      follow after Enter at the bottom edge.
- [ ] ⌘W on the last tab returns to the empty state (verified scripted);
      ⌘W with unsaved changes discards them by design (no prompt in M1).
- [ ] Tree keyboard: ↑/↓ move the selection, Enter opens a file / toggles a
      directory (focus the tree by clicking a row first).

## Engineering hygiene

- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings` —
  clean (only the upstream `block v0.1.6` future-incompat notice from the
  gpui stack, as in M0).
- Main workspace: `cargo metadata --no-deps` — 36 members, no `gpui*`, no
  `openidea-gui`; root `Cargo.lock` untouched (`git diff` empty).
- The app crate's own `Cargo.lock` is committed with the new path deps.
- Processes: the app instances used for verification were started and
  stopped by PID; no broad kills.

## Leftovers / known limitations

- No scrollbars (wheel + trackpad only); no horizontal overflow clipping —
  long lines paint past the window edge.
- IME trace (`[ime]` lines) still logs to stderr on every callback — kept
  from M0 for regression comparison; gate behind a flag in M2.
- Status messages have no auto-dismiss timer; the last message stays until
  replaced.
- The stale tab's ⚠ carries no "reload from disk" affordance (closing and
  reopening the tab, or saving, are the only resolutions in M1).
- Opening a second window or switching projects at runtime is out of M1
  scope; `initial_project()` is start-up only.

## How to reproduce

```sh
cd apps/openidea-gui
OPENIDEA_GUI_PROJECT=/path/to/project cargo run   # or plain cargo run inside a repo
# ⌘O open · double-click tree rows · type (Pinyin IME works) · ⌘S · ⌘W
# touch a file from another shell → watch clean reload / stale marking
```
