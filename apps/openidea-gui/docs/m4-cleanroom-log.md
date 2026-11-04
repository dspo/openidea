# M4 clean-room log

Record of sources consulted before / while writing the M4 code, with their
licenses. Zed (`~/projects/github/zed`, GPL-3.0) was **not read** for M4: the
run and git surfaces are entirely openidea's own backend crates
(`ide-run`, `ide-vcs`), and the UI shapes (dock, overlay, palette reuse) were
already established by this repo's own M1–M3 code.

## Read before coding

- **openidea repo (Apache-2.0)** — the implementation basis:
  - `crates/ide-run/src/lib.rs`, `process.rs` — `RunConfig`, `detect`,
    `load_user_configs`, `ProcessManager::start/stop`, `OutputChunk`,
    exit reported as `IdeEvent::RunExited` through the sink; ide-run's own
    8 ms output batching pump.
  - `crates/ide-host/src/ops/run.rs`, `lib.rs` — the ProcessManager-per-project
    wiring sample; `ops/vcs/commands.rs` — the `Repo` call shapes.
  - `crates/ide-vcs/src/lib.rs` — `Repo::discover/status/head/line_hunks/
    stage/unstage/commit`, `FileStatus`, `LineHunk`, `ChangeKind`;
    reads = gitoxide, writes = git CLI.
  - `apps/openidea-gui/src/{host,bus,workbench,editor,file_tree,theme,strings}.rs`
    — the M1–M3 host this builds on (bus pump, picker, edit pipeline,
    `insert_text` / `Selection::after_inserts`, overlay patterns).
  - `apps/openidea-gui/M3-REPORT.md` — the leftover list that scoped M4
    (paste debt, run/git milestone).

## API verification (Apache-2.0 / MIT crates, registry sources)

- `gpui-pre-0.3.9/src/platform.rs` — `Platform::read_from_clipboard /
  write_to_clipboard`, `ClipboardItem::text()`; `App::read_from_clipboard`.
- `gpui-pre-0.3.9/src/elements/uniform_list.rs` — `uniform_list(id, count, f)`,
  `UniformListScrollHandle::scroll_to_item(ix, ScrollStrategy::Bottom)` for the
  output dock's virtualized follow-the-tail.
- `gpui-component-0.8.0/src/input/textarea.rs`, `input/mod.rs` —
  `Textarea` / `TextareaState` (multi-line input) for the commit message.

## During coding

Only the sources above were revisited (API shape checks). No GPL code was
read at any point; everything implemented here derives from the notes in
`m4-design-notes.md`.
