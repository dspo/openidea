# M11 design notes — terminal + debugger (+ Linux IME phase 0.5)

Written before the milestone's code (the clean-room discipline). Part 1 is the
LINUX-GATE-REPORT §2 follow-up: the Zed comparison notes and the double-write
attribution, per the gate report's "下一步" item 2. The terminal and debugger
sections fix the design the later phases implement. Zed (GPL) was read for the
IME notes only; nothing is copied from it (see the clean-room log).

## Part 1 — Linux IME: the double-write, attributed (clean-room reading)

### What the gate report left open

Two defects, attribution undetermined (three suspects: fcitx5 waylandim's
pass-through design / gpui-pre consuming both channels / sway 1.11 +
wlroots 0.19.2 relay behavior). Evidence: with fcitx5 attached, every
character key produced a raw key event AND a commit_string insertion
("nniihhaaoo"); separately, composition (preedit/candidates) never engaged and
fcitx5's own `key_trace=5` stayed empty.

### What upstream Zed does (read, not copied)

Zed's Wayland client (both current master `dd34370114` and our snapshot
gpui-pre-linux 0.3.9 = zed@93d7456 — the two handlers are byte-equivalent in
structure):

- `zwp_text_input_v3::Event::CommitString { text }`:
  `state.composing = false`, then — deliberately — a **single-character**
  commit is NOT delivered as IME insertText. It is re-synthesized as a plain
  `KeyDown` (no modifiers, `key = key_char = commit_text`). The in-code reason:
  IBus commits plain ASCII letters even when composition never engaged, and
  those keys must reach Zed's editor as *keys* (vim mode); a CJK/label text
  (`か`, multi-char) rides `ImeInput::InsertText` instead.
- `PreeditString` sets `composing = true` and stashes the preedit; `Done`
  converts the stash into `SetMarkedText` (or `DeleteText` when empty).
- The raw `wl_keyboard` key handler **never filters on `composing`** — every
  press dispatches a real `KeyDown` alongside whatever the text-input channel
  does. `composing` is only consumed by the mouse-press handler (click resets
  the IME).

So the platform contract Zed's client assumes: **the IME commit and the raw
key are mutually exclusive per keystroke** — the compositor/IME forwards the
key OR commits its text, never both. GNOME+Mutter+ibus honors this (ibus
forwards key events explicitly via `forward_key_event` only when it did not
consume them).

### Attribution (evidence-based, refined from the report's three suspects)

Our double-write is the client consuming both channels — but the *cause* of
both channels firing sits below the app:

1. **fcitx5 waylandim pass-through** (primary suspect, now with a mechanism):
   when composition never engages (pinyin never activated — the report's
   defect 1), fcitx5's waylandim both commits the character per key AND the
   key still reaches the app's `wl_keyboard`. On sway the keys also never
   enter fcitx5's processing pipeline (empty key_trace), yet commits flowed —
   consistent with waylandim emitting a commit per plain key without running
   its input context engine (or a relay oddity in wlroots 0.19.2's
   input-method grab path, suspect 3; can only be separated on the Fedora
   bench with fcitx5-side tracing).
2. **gpui-pre-linux is faithful to upstream** (suspect 2 resolved): the
   snapshot behaves exactly like current Zed — single-char commit → KeyDown.
   Zed would see the same double input on this exact desktop stack; the
   divergence is the desktop side, not the snapshot's drift.
3. The app (us) has no guard on the second arrival because by the time it
   reaches `EditorPane::on_key` it *is* a `KeyDown` with `key_char` —
   indistinguishable from a real keypress except by timing.

### The fix this milestone lands (client-side dedup, platform-agnostic)

Since the second write arrives as an ordinary key event, the app dedupes at
its own input funnel. A pure `ImeEchoDeduper` (unit-tested, no clock reads —
timestamps injected) classifies a key/commit insert as **echo** when:

- the text is identical to the previous raw insert,
- it arrives within `ECHO_WINDOW` (50 ms — one Wayland dispatch batch; the
  fastest intentional repeat typing is ≥100 ms apart, and held-key repeats
  carry `is_held: true` and never enter the dedup),
- no marked text was set between the two inserts (a real composition's
  commits must never be dropped — the composing state disarms the dedup),
- the event is not held.

Applied at both surfaces that write text on the keystroke path: the editor's
`on_key` (key_char) and its non-composing `ime_commit` (defensive, covers a
future snapshot that reverts to IME-channel delivery), and the terminal
view's keystroke encoder (Phase 1). The multi-char commit path keeps a
dedup-check too, keyed the same way.

Residual risk, documented: an IME that legitimately commits the exact
just-typed character within 50 ms without ever marking text (no known macOS
IME does; GNOME+ibus does not — it either forwards or commits) would lose
one character. The window is a constant; the Fedora bench can lengthen it if
its traces show a wider echo gap.

### Composition-never-engaged (defect 1): candidate directions, NOT implemented here

Ranked, all app-side-overlay-red-line compliant (no gpui-pre edits from this
fork — vendor a patched crate only if a fix must live below the app):

1. **Fedora-side confirmation first** (the report's item 1, the user's 2
   minutes): GNOME real desktop + ibus. If clean, the defect set is
   sway+fcitx5-specific and the dedup above is the entire client-side fix;
   the preedit path (text-input-v3 → `SetMarkedText` → `ime_set_marked`) is
   already implemented and M0-verified.
2. If GNOME shows it too, the suspect moves into gpui-pre-linux's
   enable/commit negotiation (`text_input.enable` timing vs the compositor's
   Enter event). Next probe: log `Enter/Leave/Enable` round-trips in
   ime-probe.sh against a minimal client (no app) to isolate whether
   `enable` is issued before the first key.
3. Worst case (compositor relay): sway 1.11 + wlroots 0.19.2 input-method
   grab behavior — upstream sway/wlroots issues, not ours; the app's only
   lever is the dedup.

`script/ime-probe.sh` and the smoke suite stay untouched this milestone
(regression-checked in Phase 0.5); the probe's isolation harness is the
right vehicle for any Fedora-side re-drive of items 2–3.

## Part 2 — Terminal: libghostty-vt + self-drawn GPUI element

The technical route is fixed by the roadmap: libghostty-vt (MIT) as the
emulation core, a self-drawn GPUI element for the surface, architecture
mirroring manox-app's `steer-ghostty` (the production implementation of this
route; user-owned, reference/reuse allowed). No Zed reading involved.

### Crate layout

New path-dependency sub-crate `apps/openidea-gui/terminal-ghostty/` (the root
`exclude` already covers the whole `apps/openidea-gui` tree — unchanged).
Crate contents, each transcribed from the pinned Ghostty rev, mirroring
steer-ghostty's file set:

- `build.rs` — pin Ghostty rev + codeload tarball sha256; resolution order
  `GHOSTTY_SRC` (dev loop) → pinned tarball; zig via `GHOSTTY_ZIG` → PATH
  ≥ 0.17 → pinned toolchain; the (rev, zig-identity) stamped artifact cache
  under `STEER_GHOSTTY_CACHE` or `OUT_DIR`. Same `-Dcpu=baseline` +
  ReleaseFast + `lib-static` export discipline (the dylib-beside-archive
  linker trap).
- `src/sys.rs` + `src/key_consts.rs` — hand-transcribed FFI against the
  pinned rev's `include/ghostty/vt/*.h` (terminal/render/key/paste/style
  headers). Discipline: rev and bindings move together; `key_consts` stays
  regenerable, never hand-edited.
- `src/terminal.rs` — the safe wrapper: `Terminal` (new/vt_write/resize/
  scroll/title/pwd/paste/default colors), `RenderState` snapshots into an
  owned `Frame` (cells with resolved colors + style bits), `KeyEncoder`
  (sync-from-terminal + encode_press).
- `src/pty.rs` — portable-pty transport (writer queue thread, reader thread,
  polled reaping, SIGTERM→SIGKILL descendant sweep). Here it implements the
  app's own `TerminalProcess` seam (the trait stays in this crate; the
  brief's "PTY via ide-terminal" seam = the same portable-pty substrate
  ide-run uses, kept crate-local because `ide-terminal` is a main-workspace
  crate and the fork forbids depending on/altering it beyond path use).
- `src/input.rs` — GPUI keystroke → ghostty key consts + mods; with the
  M11 echo-deduper applied at the write-to-pty boundary.
- `src/view.rs` — `GhosttyView` (Focusable, EventEmitter) + `GhosttyElement`
  (paint-only: prepaint snapshots the frame under the lock, groups same-style
  runs, shapes with per-cell force-width, paints bg quads → runs → cursor →
  IME handler registration). Render-driven resize. OSC title/pwd mirrored
  through the event pump. `Theme { fg, bg }` supplied by the app's palette
  (theme.rs bridge — the crate owns no colors).

### Workbench integration

- The terminal tool window flips `lit` in `toolwindow.rs` (the data row
  already exists: bottom anchor, OnUse stripe, `SquareTerminal` icon). The
  brief's ⌘5 face: ⌘5 is the **Debug** slot in the table (`slot 5`, IDEA's
  Meta+5); the terminal lights with its IDEA-accurate face — no slot collision,
  the slot map test keeps pinning 5→debug. Terminal opens via its stripe
  button (OnUse semantics like Find) and the action palette.
- A `DockPage::Terminal` arm renders the terminal surface in the bottom dock;
  the dock header becomes the "Local × + ▾" face: session label (OSC title
  falls back to "Local"), × close, + new tab, ▾ overflow. Multi-tab:
  `Vec<Entity<GhosttyView>>`, new tab inherits the front session's live cwd
  (OSC 7) else the project root, at the project root otherwise.
- Each open terminal registers in the Services surface (Part 4).

### Semantic acceptance (the reason ghostty is the core)

vim (full-screen alt-screen enter/leave), htop (redraw + function keys),
less (scroll modes), 24-bit color (`COLORTERM=truecolor` + palette passthrough),
OSC 8 hyperlinks, live resize reflow. Automated where deterministic (the
TestProcess pair drives the terminal from tests without a PTY); the
window-level checks run live with window-scoped captures.

## Part 3 — Debugger (DAP)

Design fixed before implementation:

- Adapters strictly from `languages/*.toml` `[debugger]` declarations as they
  stand (go=delve, rust=codelldb, python=debugpy, ts/js) — **zero toml
  changes**. Missing binaries install locally per the toml install-hint
  (machine-level, not repo).
- A minimal DAP client (stdout/stdin JSON, the same `tokio::process` shape
  ide-lsp uses app-side) behind `debug.rs`: initialize → launch/attach →
  setBreakpoints → configurationDone, then stopped/continued/output events.
- Breakpoints: gutter marker (⌘F8 toggle), persisted per session, sent per
  file on configurationDone; conditional breakpoint via the gutter context
  (condition string rides `breakpoints` request `condition`).
- Debug tool window (slot ⌘5 lights with `lit`): Variables (scopes → rows),
  Watches (user expressions, evaluated on stop), Call Stack (frames, click
  jumps to the file:line), Console (DAP output + REPL-lite: evaluate on the
  stopped frame).
- Stepping: F7 step-into, F8 step-over, F9 continue/resume, plus the current
  execution line highlight (the stopped frame's file:line, a full-width
  marker row painted by the editor element when the frame matches).
- launch/attach joins the run-configuration system: a debug start resolves
  the language's `[debugger]` adapter, builds the DAP launch config from the
  run config's program/args/cwd/env.

## Part 4 — Services tool window (minimal face)

`lit` flip + one `render_window_body` arm: the session list (run sessions and
debug sessions in start order, name + state badge) and a stop button per
session. No start-from-here, no grouping — M12's test tree inherits the row
model.
