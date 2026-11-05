# M7 design notes — workbench polish

Written before any M7 code, from openidea's own crates (`ide-vfs`,
`ide-language`, `ide-core`), the M0–M6 reports' seam notes (M3's focus-bug
history, M4's dock slot, M6's status-bar/palette/Esc notes), the app's own
workbench/dock/tab/status-bar/action-registry sources, and the Apache-2.0
gpui-component 0.8 / gpui-pre 0.3.9 APIs. Zed (GPL-3.0) was **not** read for
M7 — see `m7-cleanroom-log.md`. `~/projects/dspo/manox-app` (the user's own
project) supplied the notification push pattern and the settings-file
read-modify-write discipline.

Seven implementation phases after the notes, one commit each, each green
(fmt + clippy `-D warnings` + unit tests) when committed. **`crates/` stays at
zero diff**; no `languages/*.toml` change is needed this milestone (the
settings overlay only *reads* the existing indent config).

## 0. The settings store (new `settings.rs`, foundation for ①–⑥)

One JSON file per user, read-modify-written with the manox discipline (every
write round-trips through parse; a corrupt file is replaced, never allowed to
block):

- Path: platform data dir (`dirs::data_dir()`)/`openidea-gui/settings.json`.
- Content: `recent_projects: Vec<PathBuf>` (most recent first, canonicalized,
  capped at 10, pruned to existing directories on read), `layout`
  (`tree_visible`/`tree_width`/`dock_visible`/`dock_height`), `font_size`.
- Loaded once at startup into a plain struct; `Workbench` owns it and saves
  on every change (recents on project open, layout on toggle/drag-end, font
  on change). New deps: `serde` (derive) + `serde_json` — nothing else.

## 1. Welcome / project picker (the M1 leftover)

Startup order becomes: `OPENIDEA_GUI_PROJECT` env (unchanged, scripted
sessions keep working) → most recent project from the store (when it still
exists on disk) → the M1 cwd-derivation as the stateless fallback → no
project at all, which now renders a **welcome view** instead of the bare
"No project open" hint:

- The welcome takes the editor area (the tree column hides with it — an
  empty tree carries nothing): app title, an **Open Folder…** button, and the
  recent-projects list as clickable rows.
- ⌘O becomes context-sensitive: with a project it stays the file dialog;
  without one it opens the **folder** dialog (the one decision a new user
  needs).
- Opening a folder runs `rfd`'s `pick_folder` on the background executor like
  the file dialog; the answer lands as `BusMessage::PickedFolder`. The bus
  handler constructs `Host::open` inside the runtime `enter()` (the main
  thread is inside `block_on`, so `Handle::current()` is valid there — the
  same shape main.rs uses), replaces the `ProjectHost` global, reseeds the
  tree, resets the git-refresh seed, and records the recent. Focus hands to
  the tree through the existing pending-flag pattern (bus has no window).
- A closed/unopenable recent is skipped, not fatal (a stale path prunes on
  the next save).

## 2. Dock completion

- **Visibility toggles**: `workbench::ToggleTree` (⌘1, the IDEA Project
  gesture) and `workbench::ToggleDock` (⌘4, IDEA's Run). ⌘4 surfaces the
  dock with its last page when hidden; with no panel ever created it no-ops
  (the M4 contract — the dock exists from the first run/search — is kept).
- **Drag resize**: hand-rolled 4 px handles (tree/center vertical, center/dock
  horizontal) — `on_mouse_down` arms a drag state, the root div's
  `on_mouse_move`/`on_mouse_up` drive it (mouse events reach the root
  through the children). Clamps: tree 150–480 px, dock 120–560 px.
- **Layout persistence**: visibility + sizes ride the settings store;
  drag-end and toggles save. `Workbench::new` takes the loaded layout.
- **Multi-run tabs** (the M4 slot): `run_panel: Option<Entity>` becomes
  `runs: Vec<Entity<RunPanel>>` and `DockPage::Run(usize)`. Every start opens
  a *new* output tab and selects it (the "a run is already active" warning
  dies — that was the single-run MVP); each tab keeps its own header/status
  (exit code, elapsed, Stop, Clear — per-panel, M4 behavior intact).
  `RunExited` routes by run id; ⌃⌘R stops the selected run tab, else the
  first live one. Run tabs gain a close button: closing a live tab stops its
  run first. The Find page keeps its tab beside the runs.

## 3. Tab management

- **Pin**: `Tab.pinned`; a right-click menu on the tab toggles it. Pinned
  tabs are skipped by ⌘W (and by the close button) and carry a small accent
  glyph prefix, so they stay identifiable. Position stays put (protection is
  the point; pinned-first reordering would renumber every index consumer for
  no observable gain).
- **Reopen closed**: `workbench::ReopenClosedTab` (⌘⇧T) pops a path off a
  capped (20) stack of closed tab paths and reopens it through `open_path`.
- **Overflow**: the editor `TabBar` gains `.menu(true)` — the built-in
  overflow dropdown — and the strip already wheel-scrolls horizontally
  (gpui-component's own `overflow_x_scroll` + `track_scroll`).

## 4. Context menus (gpui-component `ContextMenuExt` + `PopupMenu`)

- **File tree rows** get `.context_menu(...)`: New File… / New Folder…
  (inside a dir target, beside a file target), Rename…, Delete…. The
  name-entry prompts and the delete confirmation are small workbench
  overlays (an `Input`, or a two-button confirm) — the commit overlay's
  shape. **All IO goes through `ide-vfs`** (`create_file` / `create_dir` /
  `rename` / `delete` — delete moves to the OS trash), wrapped on `Host`, run
  on the background executor, answered through the bus, and confirmed only
  by the dialog for delete. A created file opens in a tab. Renaming or
  deleting a file with a dirty open tab saves it first (rename reopens the
  new path; delete's dialog already said the content goes away).
- **Editor** gets the same menu over its surface, with the existing actions
  as entries — Go to Definition ⌘B, Find Usages ⌥F7, Rename… ⇧F6, Quick
  Fix… ⌥⏎, Format Document ⌘⌥L, Organize Imports ⌃⌥O — dispatched through
  the popup's `action_context(pane_focus)` so they land in the pane's own
  handlers (one source of truth; no second implementation).

## 5. Toasts (the M1 debt)

`self.message` and the status-bar message slot die. Every surface becomes
`window.push_notification(...)` (gpui-component's notification system:
auto-hide 5 s, stacked, close button, click handler — the manox usage is the
pattern). Handlers with a window push directly; bus-side results queue
into a pending list flushed at the next render (the established
no-window-in-bus pattern). Severity maps to `success`/`info`/`warning`/
`error`; click targets: save-failed → focus the active editor, run-launch
failed → surface the Run dock.

## 6. Minimal settings

`workbench::OpenSettings` (⌘,) opens a workbench overlay (commit-dialog
shape, Esc closes):

- **Font size**: −/+ steppers over a clamped 10–24 px range (default 14).
  `theme::px_font_size()` reads a module-level atomic set by the overlay, so
  the editor, gutter, hints, and find bar re-metric on the next frame (all
  sites already read it per frame); `cx.notify_all()` repaints.
- **Indent display**: the active tab's language policy from
  `LanguageRegistry::get(detect(path))` (e.g. "Go: 4 spaces-wide tabs"),
  read-only; no tab → "follows language defaults".
- **Keymap table**: read-only rows generated from `actions::REGISTRY`
  (title, keys) — the registry is already the keymap's display source; rows
  without a default binding render a blank keys cell (the uniqueness test
  drops its keys-non-empty clause to match).

## 7. Focus & Esc chain + the defensive regression

The M3 death (focus stranded on an unmounted input silenced every action)
and M6's live-pass note (modal-input confirmation) generalize into one rule:
**closing any overlay hands focus back to the editor (or the tree) in the
same update; bus-side closes set the pending flag**. All close paths route
through one helper. The regression test rides gpui's own headless
`TestAppContext` (gpui-pre's `test-support` feature as a dev-dependency —
the same harness gpui-component tests with): open the workbench window with
no host, open every overlay class, Esc it away, then dispatch an action and
assert it landed — "after any overlay closes, actions must fire". Plus pure
unit tests for the Esc-chain order (topmost surface wins) and the
focus-restore decision.

## 8. Regression + report

M0–M6 keyboard re-drive on `/tmp` scratch copies (the repo fixtures
untouched), before/after window screenshots, the scratch exit run (no env,
no CLI: welcome → open folder → edit → run → commit, UI only),
`M7-REPORT.md`, PR body at `/tmp/m7-pr-body.md`.

## What M7 deliberately does not do

Themes/i18n, terminal, debugger, build chains, tool families,
related-info multi-hop (still blocked by the crates/ freeze), settings
beyond the three rows (no editorconfig UI, no theme picker), dock
re-parenting/float (the drag resize is the agreed scope).
