# M7 clean-room log

Per the milestone discipline: Zed (GPL-3.0, `~/projects/github/zed`) was **not
read** this milestone — not before coding, not during. Every design decision
in `m7-design-notes.md` came from:

- this repo's own crates: `ide-vfs` (`create_file`/`create_dir`/`rename`/
  `delete` — trash semantics read from `ops.rs`), `ide-language`
  (`LanguageRegistry::get`/`detect`, `IndentConfig`), `ide-core` (event
  types), `ide-workspace`/`ide-run`/`ide-vcs` (as already wired in `host.rs`),
- the app's own M0–M6 sources (`workbench.rs` dock/palette/overlay shapes,
  `editor.rs` action funnel, `host.rs` construction order, `theme.rs`
  constants),
- the M0–M6 reports and design notes (M3's focus-bug history, M4's dock slot
  and multi-run seam, M6's status-bar/palette/Esc handoff notes),
- the Apache-2.0 **gpui-component 0.8.0** sources in the local cargo registry
  (`notification.rs` — auto-hide/stack/click notification list pushed through
  `WindowExt::push_notification`; `menu/popup_menu.rs` + `menu/context_menu.rs`
  — `ContextMenuExt` and `PopupMenu::action_context`; `tab/tab_bar.rs` —
  overflow `menu(true)` and the built-in wheel-scroll strip; `resizable.rs`
  evaluated and rejected in favor of two hand-rolled drag handles, keeping
  the M1 layout structure),
- the Apache-2.0 **gpui-pre 0.3.9** API (`TestAppContext` via its
  `test-support` feature, mouse/key listeners, `SharedString`),
- **`~/projects/dspo/manox-app`** (the user's own project, Apache-2.0):
  `crates/agent-ui/src/workspace.rs` notification usage
  (`window.push_notification(Notification::error(..).title(..))`) and
  `crates/agent-ui/src/projects.rs` settings-file discipline (parse-edit-
  serialize read-modify-write that preserves foreign keys and replaces a
  corrupt file rather than blocking on it) — adapted to a single JSON file.

No editor shell behavior was copied from any copyleft source; the welcome
view, dock toggles, drag handles, pin/reopen logic, settings overlay and
focus chain are built on the app's existing M1–M6 structures.
