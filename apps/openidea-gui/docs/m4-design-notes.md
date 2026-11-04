# M4 design notes — run surface, minimal Git, ⌘V

Written before any M4 code, in my own words, from openidea's own crates
(`ide-run`, `ide-vcs`, `ide-core`) and the Apache-2.0 gpui-pre / gpui-component
APIs. Zed (GPL-3.0) was **not** read — see `m4-cleanroom-log.md`.

## 0. ⌘V paste (the M2 debt, paid first)

The M2 edit pipeline already has the exact shape paste needs:
`insert_text(text)` builds one `ChangeSet` replacing every normalized
selection range with the text (bare carets insert, selections replace) and
`Cause::Typing` re-maps every cursor to the end of its inserted text
(`Selection::after_inserts`). That is precisely multi-cursor paste semantics,
and it flows through `Documents::edit` (LSP sync, undo, tree-sitter all see an
ordinary batch).

- New `editor::Paste` action, bound to `cmd-v` in the `"Editor"` context only —
  gpui-component inputs (palette query, rename) keep their own clipboard
  handling and must not see this binding.
- Text comes from `cx.read_from_clipboard()` → `ClipboardItem::text()`
  (gpui-pre platform API; macOS NSPasteboard under the hood). Empty clipboard
  or non-text item = no-op.
- While the IME is composing or a modal flow (rename) owns the pane, `command()`
  already gates everything — paste rides the same gate.
- Undo granularity: paste rides `Cause::Typing`, so a paste right after typing
  groups into the same undo step. IDEs usually give paste its own step; MVP
  accepts the grouping (noted as a leftover).
- Unit test: a pure helper `paste_ranges` (selections in, replacement ranges
  out) is tested against caret/selection/multi-cursor cases — the same math
  `insert_text` consumes, so the test pins the semantics without a gpui
  context.

## 1. Run: configurations + output panel

`ide_run` is the whole backend: `RunConfig` (id/name/kind/command/args/cwd/env,
`detect(root)` from package.json/Cargo.toml/…, user configs in
`.openidea/run.toml`), `ProcessManager` (`start(root, config, options, sink,
on_output) → RunInfo`, `stop(run_id)` = SIGINT→SIGKILL on the process group,
output as `OutputChunk::{Stdout,Stderr,System}` through a plain callback, exit
as `IdeEvent::RunExited { run, code }` through the sink). `ide-host` just owns
a `ProcessManager` per project — the GPUI host does the same.

- **Host**: `Host` gains `run: ProcessManager`. Start runs on the app runtime
  (`spawn_blocking` — spawn() does fork/exec and file IO), answered through a
  oneshot awaited from `cx.spawn`, exactly the M3 request shape.
- **Choosing a config**: a run picker over the existing palette pattern — one
  list of detected + user configs (`name — command`), Enter starts it. The
  picker's query doubles as creation: Enter on no-match builds a `shell` config
  with the query as the command line (`cargo test`, `ls -la` …), so "new" and
  "select" are one surface. `ctrl-r` opens it (workbench action, in REGISTRY).
- **Output panel**: a bottom dock in the workbench (between editor area and
  status bar, collapsible). State lives in a `RunDock` struct on the workbench
  (same choice as `Picker`: plain struct, not an entity — one instance, owned
  by the one window):
  - lines: `Vec<(Stream, String)>` capped (oldest lines dropped past
    MAX_LINES); rendered with gpui's `uniform_list` (virtualized — a noisy
    `cargo build` renders only the visible window) behind a
    `UniformListScrollHandle`; follow-the-tail via `scroll_to_item(last,
    Bottom)` while the user hasn't scrolled up (a wheel-up sets `follow =
    false`; a Jump-to-end button re-enables).
  - colors: stdout = foreground, stderr = the error tone, system lines = muted.
  - header: config name, live status (running → elapsed timer refreshed by the
    bus pump's 50 ms notify; exited → "exit code N · Xs"), Stop button while
    running, Clear always.
- **Data flow**: `on_output` closure → `BusMessage::RunOutput { run, chunk }`
  → workbench appends into the dock (text batched by ide-run's 8 ms pump, so
  chunk rate is modest). `RunExited` arrives as an ordinary `IdeEvent` on the
  bus sink; the workbench routes it to the dock and the status bar.
- **Stop**: button + a `workbench::StopRun` action → `ProcessManager::stop`
  (its own thread; graceful, whole process group) — the CancellationToken
  semantics of the brief, already built into ide-run.

## 2. Git minimal surface (via `ide_vcs`)

`ide_vcs::Repo` is discovered once at project open (`Repo::discover(root)` →
`Option<Arc<Repo>>`); reads are gitoxide in-process, writes shell out to `git`
(hooks/credentials behave like the CLI). Everything the phases need exists:
`status()` → `FileStatus { rel_path, staged, unstaged }`, `head()` → branch
name, `line_hunks(path, current_text, against_index=false, encoding)` →
`LineHunk { kind, start, lines, old_start, old_lines, old_text }`,
`stage(&[PathBuf])`, `unstage`, `commit(message, amend=false, sign_off=false)`
→ new HEAD id.

- **Refresh model**: git reads run on the background executor and answer
  through the bus (`BusMessage::GitStatus { statuses, branch }`). Triggers:
  project open, every `FsChanged` batch (same debounce the tree already rides),
  and after stage/commit (the write paths also emit `VcsChanged` — either
  route refreshes).
- **Tree coloring** (phase 3): the workbench keeps
  `git_status: HashMap<PathBuf, ChangeKind>` (working-tree kind wins; staged
  kind when clean in the tree). Tree rows tint the file name: added/renamed =
  green tone, modified = blue tone, deleted = red tone, untracked = olive
  tone, conflicted = error red. Hardcoded constants in `theme.rs` (the git
  palette is explicitly allowed to leave the token family, like the syntax
  palette already does).
- **Diff view** (phase 4): a read-only overlay (the rename-preview overlay
  pattern: centered modal, scrollable, Esc/Close). Target: the selected tree
  row if it has working-tree changes, else the active tab's file; `ctrl-d`
  opens (`workbench::ShowDiff`). Data: `line_hunks` against HEAD with the
  current text (open dirty buffer wins, else the file on disk); rendered as a
  unified view — per hunk a `@@ -old +new @@` header, `- ` lines from
  `old_text` in the red tone, `+ ` lines from the current text in the green
  tone, context in muted. No syntax highlighting in the diff (per plan).
  Untracked files show as one all-added hunk (what `line_hunks` already
  reports).
- **Commit flow** (phase 5): `ctrl-k` (`workbench::Commit`) opens a commit
  overlay: the changed-file list (checkbox rows, click toggles, default =
  every changed path), a multi-line message input (gpui-component
  `TextareaState`), Commit / Cancel. Commit = `stage(selected)` →
  `commit(message)` → refresh status + notify with the new short id. The
  staging is real (`git add` semantics through ide-vcs), so partial commits
  work by deselecting files.

## 3. Status bar (phase 6)

Left keeps project/file/save state; adds the git branch (`Repo::head()`, from
the same refresh). Right adds the run indicator: spinner-ish "running · Xs"
while a run is live (elapsed refreshed by the pump), "exit N · Xs" after, from
the dock's state. Language-server rows stay.

## 4. Threading and state rules (carried from M0–M3)

- No git/run work on the UI thread: `Repo` reads and `ProcessManager::start`
  go through the background executor / app runtime; UI updates only through
  bus messages and `cx.spawn` continuations.
- One `Repo` per project, `Arc`ed; ide-vcs re-reads on-disk state per call, so
  a long-lived handle never goes stale.
- All new user-visible strings go in `strings.rs`; all new colors in
  `theme.rs`.

## 5. What M4 deliberately does not do

Configuration editing UI, debugger, test tree, integrated terminal, git
branch/remote/history/blame UI, GitHub — each named in the brief as deferred;
seams they will need: the dock takes more tabs (terminal beside Run), the
picker pattern generalizes to config editing, `Repo` already exposes every
write the branch UI would need.
