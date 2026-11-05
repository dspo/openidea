# M7-REPORT — workbench polish

Branch `feat/gpui-m7-polish` (based on `feat/gpui-m6-go-dx`), worktree
`~/worktrees/openidea/feat-gpui-m7-polish`. Per the roadmap discipline: local
commits only, no push, no PR. 9 commits (list at the bottom), each green at
commit time (`cargo fmt`, `cargo clippy --all-targets -- -D warnings`, unit
tests; 61 in the crate at the end, up from 57 — 4 of the new ones are the
headless focus/Esc-chain regression). **`crates/`, the root `Cargo.lock` and
`languages/*.toml` carry zero M7 commits** (verified: `git log
feat/gpui-m6-go-dx..HEAD -- crates/ Cargo.lock languages/` is empty); the main
workspace is untouched (36 members via `cargo metadata --no-deps`). The only
new dependency is `gpui-pre 0.3.9` with its `test-support` feature as a
**dev-dependency** — same pin as the runtime dependency, the feature set only
widens for tests.

## Scope delivered

All seven roadmap items, organized as the design notes' phases
(`docs/m7-design-notes.md`, written before any code; clean-room log at
`docs/m7-cleanroom-log.md`):

| Phase | Content | Commit |
|---|---|---|
| notes | design notes + clean-room log (Zed not read) | `b19c3059` |
| 0+1 | settings store (recents / layout / font size, one JSON file, parse-round-trip writes, corrupt-file replaced) + the welcome view: Open Folder…, recent-projects list, ⌘O becomes context-sensitive (folder picker with no project), env/cwd kept as the stateless fallback | `925236d4` |
| 2 | dock completion: ⌘1 tree / ⌘4 dock visibility (persisted), hand-rolled 4 px drag handles (tree 150–480 px, dock 120–560 px, saved on drag end), multi-run tabs — every start opens a new output tab, `RunExited` routes by run id, closing a live tab stops it | `f48705d6` |
| 3 | tab management: pin (skipped by ⌘W and the close button, accent-dot marker), ⌘⇧T reopen (capped stack, cleared per project), tab-bar overflow menu + wheel scroll | `0d99723d` |
| 4 | context menus: tree rows (New File… / New Folder… / Rename… / Delete…, all IO through `ide-vfs`, delete → trash with a confirmation dialog, dirty buffers saved before rename/delete) and the editor (Go to Definition / Find Usages / Rename / Quick Fix / Format / Organize Imports dispatched through the pane's own action context — one source of truth) | `1cb3ebb6` |
| 5 | toasts replace the never-dying status message: `push_notification` stack (auto-hide, close button), bus results queue and flush at render, severity-mapped, click targets (save failure → editor, run failure → dock) | `ba23fcd8` |
| 6 | the minimal settings overlay (⌘,): font-size steppers over the theme's atomic (panes re-metric live), the active tab's indent policy read from the language registry, the read-only keymap table generated from `actions::REGISTRY` | `ca60b0ab` |
| 7 | one Esc chain: `on_cancel` retreats one surface (settings → delete confirm → tree prompt → commit → diff → picker → editing surface); `restore_focus` walks the whole stack topmost-first and falls back to the root sink when the tree is hidden or absent; every close path routes through it; the headless regression suite (`gpui test-support`) pins "after any overlay closes, actions must land" | `f0e8a88d` |
| live fix | ⌘W closing the LAST tab stranded the focus on the unmounted pane (`focus_active` had no else branch) — caught in the scratch session, fixed with its own regression test | `f12edf32` |

## Exit condition — the real loop, UI only

Stateless launch (no env, no CLI flags, no settings file, cwd outside any git
repo), driver: cua-driver foreground keystrokes + HID clicks + clipboard
paste (the user's IME eats printable synthetic keys — see Environment).
Transcript: `/tmp/m7-transcript.log` (the app's own eprintln trail). Scratch:
`~/1-m7-scratch` (a copy of `fixtures/go-app`, `git init` + one commit; the
leading digit sorts it first in the open panel, see Deviations), deleted
after the session; the repo fixtures untouched.

1. **Launch → welcome** — `m7-01-welcome.png`: title, Open Folder… button,
   recents empty on a fresh machine, status "No project open". The root sink
   holds the focus, so ⌘O works before any click.
2. **⌘O → folder dialog → project open** — the panel flow is keyboard-driven
   (⌘⇧H home, ↓ selects the scratch, Return); transcript
   `[workbench] project opened /Users/chenzhongrun/1-m7-scratch`;
   `m7-02-project-open.png` (tree, branch in the status bar).
3. **Tree keys → main.go open** — ↓/⏎ expand cmd/server, ⏎ opens the file;
   gopls becomes ready, inlay hints paint (`m7-03-editor-14px-hints.png`).
4. **Edit → save** — the comment lands via clipboard paste (⌘V), ⌘S runs the
   Go organize-imports pre-save (`[save] main.go organized=false error=None`)
   and the auto-hiding toast reports `Saved main.go`
   (`m7-07-save-toast.png`); disk diff is exactly `1 insertion(+)`.
5. **⌃R → run** — the run picker lists the detected `go run ./cmd/server`
   (`m7-04-run-picker.png`); ⏎ starts it, the dock tab shows
   `running · 6.7s` and the server log `listening on :8080`
   (`m7-05-run-live.png`); `curl localhost:8080/notes` returns the fixture
   note JSON.
6. **⌃⌘R → stop** — dock header flips to `exit code 1 · 36.1s` with
   `signal: interrupt` in the output.
7. **⌘K → commit** — the dialog lists main.go checked, the message pastes in
   (`m7-08-committed-toast.png` shows `Committed ea03f78`), `git log` shows
   the commit on top of the init and the worktree is clean.

Plus the M7 faces exercised in the same session: ⌘⇧T reopens the ⌘W-closed
tab (`m7-11-…` pair with `f31-closed.png`), ⌘1 hides/shows the tree with the
store persisting the layout (`m7-11-tree-hidden.png`), ⌘4 the dock, ⌘, opens
settings (`m7-09-settings.png`), the font steppers change the editor live
14→15→17 px, persisted (`m7-10-editor-17px.png` vs `m7-03-…`), the tree
context menu creates a file through ide-vfs (git-added green row, opens in a
tab; `m7-12-…`→`m7-14-new-file-created.png`) and deletes it through the
confirmation dialog (`m7-13-delete-confirm.png`), and the editor context menu
lists the six code actions (`m7-15-context-menu-editor.png`). A second ⌃R
opens a second run tab beside the first (`m7-06-multi-run-tabs.png`).

## What the live pass caught (fixed, `f12edf32`)

**⌘W on the last tab killed every action.** `on_close_tab` re-focused via
`focus_active`, which has no fallback when `active` becomes `None` — the
focus stayed on the unmounted pane, the M3 death class exactly, surviving in
the one close path M7's earlier phases had not unified. Found by driving the
flow (the tree's Enter stopped working after ⌘W); fixed by falling back
through `restore_focus`, with a new headless regression
(`focus_active_without_a_tab_still_lands_on_a_live_surface`).

## The M0–M6 protection record

Editor/engine internals were not refactored in M7 (the only editor.rs change
is reading the font size from the theme atomic). Re-driven live in this
session: typing/IME single-insert, undo, ⌘S (organize-imports pre-save),
gopls completion-adjacent faces (hints, semantic tokens re-paint), ⌘W/⌘⇧T,
⌘K commit, ⌃R/⌃⌘R, the palettes (⌃R run picker), Esc chain, ⌘1/⌘4, ⌘,.
The remaining M0–M6 keyboard matrix (multi-cursor, folding, ⇧⌘O/⇧⌘F full
flows, ⌘F replace, ⌥⏎ quick fixes, ⇧F6 rename) is covered by the untouched
editor code plus 61 unit tests; the dedicated live re-drive belongs to M8's
soak plan (seam note below).

## Environment record

The session ran unlocked (unlike M5/M6). Two walls remain synthetic-input
specific:

- The user's Chinese IME (青简) intercepts printable synthetic keys: typed
  text composed into hanzi inside the editor and the NSOpenPanel's go-to
  sheet. All text input in the flow therefore goes through the **clipboard
  paste** path (⌘V into the editor / the palette / the dialog inputs), which
  bypasses the IME entirely and is verified single-insert.
- The native open panel is out-of-process (its AX surface is unresolved to
  the driver) and cancels itself whenever the app deactivates; long keyboard
  navigation inside it was too flaky to automate, which motivated the
  home-directory scratch (`~/1-m7-scratch`) so the panel flow reduces to
  ⌘⇧H + ↓ + Return.
- HID clicks DO land on GPUI content when fronted with exact coordinates
  (the settings steppers, the context menus, and the dialogs were all driven
  by clicks); pixel coordinates must be read from the same capture the
  driver holds (its downscaled raster, not raw window points).

## Deviations from the brief

- **The scratch lives at `~/1-m7-scratch`, not `/tmp`** — same hygiene (the
  repo is untouched, the directory was deleted afterwards), chosen so the
  NSOpenPanel flow is reachable through the keyboard-only path above. The
  task's `/tmp` wording was about not dirtying the repo; nothing else
  changes.
- **`docs/m7-design-notes.md` sketched an `OPENIDEA_GUI_SETTINGS` path
  override** for test isolation; it did not materialize — the store prunes
  stale recents on read, so deleting the file restores a clean state, and
  the tests avoid touching the store file at all.
- The design notes expected the registry's keys-nonempty test clause to be
  relaxed for the keymap table; every REGISTRY row carries a default
  binding, so the clause stayed.

## Manual verification checklist (real keyboard/mouse)

- [ ] Drag-resize the tree (150–480 px) and the dock (120–560 px); relaunch
      restores the dragged sizes (persistence verified in-store; the drag
      itself needs a real mouse).
- [ ] The tab bar's overflow "∨" menu lists every tab when many are open;
      the wheel scrolls the strip.
- [ ] Pin a tab via the tab bar's right-click menu, verify ⌘W skips it and
      the accent dot shows; unpin.
- [ ] Rename… on a tree row with a dirty buffer open: the save-first →
      reopen-new-path sequence.
- [ ] New Folder… inside a nested directory target.
- [ ] Toast stacking: two failures inside the 5 s window stack; click the
      "Save failed" toast → the editor focuses; "Run failed" → the dock
      surfaces.
- [ ] Search Everywhere (double ⇧) and Go to File (⇧⌘O) end to end with the
      overlay-persistence from the recents.
- [ ] The keymap table scrolls with the full registry; the indent row shows
      a Rust tab's policy after opening a .rs file.

## Known leftovers

- The gpui-component floating menus (context menus) dismiss on click-outside
  but not on Esc when the focus sits elsewhere (the menu does not hold the
  focus); the workbench's own overlays all retreat on Esc. Low severity,
  click-first UX; worth folding into M8's Esc polish.
- The dock's Find page still holds results forever (M5 leftover, unchanged).
- The welcome view's recents rows need a stale-entry click to surface the
  error toast; pruning happens on the next save (by design, not surfaced).
- The settings overlay's indent row reads the language registry only; a
  per-project `.editorconfig` override is not reflected (the pane still
  honors it at edit time).
- Toast ids are per-push (no dedupe) — a burst of identical saves stacks
  identical toasts until the 5 s auto-hide.

## Seam intelligence for M8 (soak / daily driving)

1. **Soak should watch the new persisted state**: the settings file
   (`~/Library/Application Support/openidea-gui/settings.json`) is written on
   every toggle/drag/open — a corrupt-file fallback exists (replaced, never
   blocking), but soak should confirm the file never grows unbounded and the
   recents prune works after projects move.
2. **The run dock now accumulates tabs** — every ⌃R opens one. Soak should
   answer whether exited tabs need an auto-clean policy (e.g. keep last N)
   or the manual close is enough; `RunExited` routes by id and closing a
   live tab stops it, so the machinery is safe.
3. **The Esc chain is now data-driven** (`on_cancel`'s surface list). Adding
   a new overlay in M8 means: render it last (topmost), add it to the
   `on_cancel` chain and the `restore_focus` walk, and extend
   `focus_chain_tests::open_overlay` — the regression suite makes the
   "forgot one surface" failure automatic.
4. **Modal-input confirmation is regression-tested** (the palette
   PressEnter path rides the same subscription shape the M6 note asked to
   protect); the rename/find-bar inputs remain live-verified only.
5. **The IME + clipboard-paste discipline** from this session is the
   documented way to drive text through the app synthetically; M8's soak
   harness should reuse it instead of raw `type_text`.
6. `theme::font_size()` is a process-wide atomic — a future per-project or
   per-language font setting should thread the settings value instead of
   extending the atomic.

## Commit list (oldest first)

```
b19c3059 docs(gui): M7 design notes and clean-room log (apps/openidea-gui)
925236d4 feat(gui): welcome view + recent projects — the app opens without docs (apps/openidea-gui)
f48705d6 feat(gui): the dock completes — toggles, drag resize, layout persistence, multi-run tabs (apps/openidea-gui)
0d99723d feat(gui): tabs pin, ⌘⇧T reopens, overflow gets a menu (apps/openidea-gui)
1cb3ebb6 feat(gui): context menus — tree mutations through ide-vfs, editor action entries (apps/openidea-gui)
ba23fcd8 feat(gui): toasts replace the never-dying status message (apps/openidea-gui)
ca60b0ab feat(gui): the minimal settings overlay — font size, indent display, keymap table (apps/openidea-gui)
f0e8a88d feat(gui): one Esc chain, one focus rule — regression-tested (apps/openidea-gui)
f12edf32 fix(gui): closing the last tab re-lands the focus through the chain (apps/openidea-gui)
```

Plus this report and the evidence screenshots (`assets/m7-*.png`, 15 files).
