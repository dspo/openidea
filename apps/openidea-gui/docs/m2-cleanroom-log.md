# M2 clean-room log — Zed reading log

Zed (GPL-3.0) at `/Users/chenzhongrun/projects/github/zed` was used **only as
a design reference**. openidea-gui is Apache-2.0; no Zed code, type
definitions, literals, or line-by-line translations were copied into this
branch. Implementation was written against these notes plus openidea's own
crate APIs (`ide-text`, `ide-syntax`, `ide-workspace`) and the gpui-pre
public API (Apache-2.0, fine to use).

| when (2026-11-04) | file(s) | question it answered |
|---|---|---|
| ~16:05 | `crates/text/src/selection.rs` (Selection struct region) | What state does one selection carry? (start/end/reversed/goal — confirmed the shape my `Selection` needs: anchor+head+reversed+goal column) |
| ~16:06 | `crates/editor/src/display_map.rs`, `display_map/fold_map.rs` (struct names only) | How does Zed structure buffer→display mapping for folds? (concept only: a chain of transforms over rows; M2 collapses this to one sorted fold list walked per frame) |
| ~16:08 | `crates/language/src/syntax_map.rs` (top doc/struct region) | How are syntax trees kept in sync with edits? (snapshots tagged with parse version, background re-parse, drop off the main thread — M2: tree.copy() + background parse, version-tagged swap) |
| ~16:10 | `crates/editor/src/columnar_selection.rs` (fn listing only) | Entry points for columnar selection (confirmed: build selections per visual row from an x-range) |
| ~16:11 | `crates/editor/src/input.rs` (replace_text_in_range region) | How does IME input interact with multiple selections? (text applied through the ordinary per-selection edit pipeline; M2 differs deliberately: composition only at the primary cursor, others frozen, per the task spec) |

Files **not** read: any Zed highlighting implementation details beyond the
syntax_map layout above, undo operation internals, wrap/inlay/tab maps
(M2 has no soft wrap, no inlays, no tab rendering settings).

Commit-by-commit claim: every implementation commit after this file was
written without Zed sources open; where a question arose mid-implementation
it was answered from gpui-pre / tree-sitter / ropey documentation and
sources (Apache/MIT), not from Zed.
