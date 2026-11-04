# M0 Report — GPUI IME gate (go/no-go)

**Date:** 2026-11-04 · **Branch:** `spike/gpui-ime-m0` · **Platform:** macOS 26
(arm64), Xcode 27.2, toolchain 1.97.1, Squirrel (Rime) as the live Pinyin IME.

**Verdict: GO** — a canvas-style custom GPUI element drives the full
NSTextInputClient protocol correctly: composition starts/evolves with the
Pinyin IME, the candidate window anchors to the caret, commit replaces the
marked range (both converted CJK and raw Latin), CJK glyphs render, and the
stack pins compile in isolation from the main workspace. No doc(hidden) API
was needed anywhere. See "Verdict details" for what this does and doesn't
claim.

## What was built

`apps/openidea-gui` — one window, one hand-drawn single-line text element, no
workbench chrome, no stock input component:

- `src/editor.rs` — a UTF-16 buffer (caret + inline marked range, the
  NSTextInputClient meaning of "marked": in the document, not committed), a
  paint-only GPUI `Element` (plan in prepaint, draw + register the input
  handler in paint), and the `InputHandler` implementation. Marked text
  renders with an accent underline and a tinted background; the caret is a
  hand-painted 2px accent bar positioned by prefix shaping.
- `src/main.rs` — window bootstrap (`gpui_platform::application()` +
  `cx.open_window`), a `cmd-q` keybinding (no menus in M0).
- Exact pins copied from manox-app: `gpui-pre 0.3.9`, `gpui-pre-platform
  0.3.9` (`font-kit`, `runtime_shaders`), `gpui-component 0.8.0` (compiled
  for pin validation, nothing used from it), app-local `rust-toolchain.toml`
  (1.97.1).
- Isolation: root `Cargo.toml` gains `apps/openidea-gui` in `exclude`; the
  app has an empty `[workspace]` table and its own lockfile. `cargo metadata`
  confirms gpui never enters the root build graph; root `Cargo.lock` is
  untouched.

## IME protocol round-trip, as observed

The app logs every input-handler callback to stderr. Real trace
(Pinyin IME active; first excerpt is a human typing, second is a scripted
key injection, both against the custom element):

```text
[ime] setMarkedText range=None text="n" selected=Some(1..1)
[ime] firstRect 29..30 -> Bounds { origin: 285.89px, 24px, 9.03×22.5 }
[ime] setMarkedText range=None text="ni" selected=Some(2..2)
[ime] setMarkedText range=None text="ni h" selected=Some(4..4)
[ime] setMarkedText range=None text="ni hao" selected=Some(6..6)
[ime] insertText range=None text="你好"          <- Space picked the candidate
```

```text
[ime] setMarkedText range=None text="o" selected=Some(1..1)
[ime] firstRect 28..29 -> ...
[ime] setMarkedText range=None text="o x" selected=Some(3..3)
[ime] insertText range=None text="哦嚯"          <- CJK commit, raw-Latin commit
[ime] setMarkedText range=None text="ni ha" ...
[ime] insertText range=None text="nihao"         <- also seen (raw commit)
```

Every step worked first try: `setMarkedText` maps to
`replace_and_mark_text_in_range` (with the platform's selected range relative
to the marked text), candidate anchoring maps to `bounds_for_range` in
window coordinates (the platform converts via `firstRectForCharacterRange`),
commit maps to `replace_text_in_range` replacing the marked range. `Esc`
during composition arrives as `setMarkedText ""` on the same path — covered
by the handler's cancel branch; the scripted run couldn't capture it safely
(see "Automated-run constraints"), so it stays on the manual checklist.

## Screenshots

In `assets/` (window-scoped captures; the fullscreen capture from the same
session was kept out of the repo deliberately):

| file | state | provenance |
|---|---|---|
| `m0-1-normal.png` | committed ASCII + caret + status line | scripted `ab` injection through the IME passthrough commit path |
| `m0-2-composition-marked.png` | marked range `28..33` with accent underline + tinted background, caret inside composition | **synthetic** — seeded at startup via the same `set_marked` path (`OPENIDEA_M0_DEMO_MARKED=1`); the in-flight composition is too short-lived to screenshot reliably |
| `m0-3-committed-cjk.png` | `你好` committed, `marked=[none]` | **human** — typed by hand with the Pinyin IME during the session |

## gpui IME API notes (0.3.9, measured against Steer's experience)

- `InputHandler` is a 1:1 NSTextInputClient exposure and fully public —
  nothing our element needs is doc(hidden). Steer's `PrepaintCaretGeometry`
  concern doesn't apply: the caret here is painted by hand, no caret
  geometry is taken from the framework.
- `window.handle_input(&focus, handler, cx)` is paint-phase only and
  focus-gated; the handler must be re-registered every frame. Geometry needed
  between frames (candidate anchoring) is published by the element into the
  view during prepaint.
- `marked_text_range()` is load-bearing beyond rendering: gpui's mac
  keydown router derives `is_composing` from it. Returning `Some(0..0)`
  instead of `None` when idle would permanently route every key into the IME.
- `prefers_ime_for_printable_keys` must return `true` for this use case. It
  defaults to `false`; with it false, the first Pinyin keystroke is inserted
  as plain ASCII instead of starting a composition (the router consults it
  only when a non-ASCII input source is active — plain ABC typing still goes
  through the direct keydown path, which is what inserts text in our
  handler).
- While composing, *all* keys route through the IME first
  (`inputContext handleEvent`); unhandled ones come back through
  `doCommandBySelector` and are re-dispatched as ordinary keydowns. Esc/
  Enter/arrow semantics during composition are therefore the IME's, not
  ours — the editor only needs correct marked-range bookkeeping.
- `apple_press_and_hold_enabled = false` keeps raw key-repeats instead of
  the macOS press-and-hold accent popup.
- The gpui stack carries a transitive `block v0.1.6` future-incompat warning
  — same item Steer's 0.8 adoption notes recorded; upstream, not ours.
- `runtime_shaders` on `gpui-pre-platform` avoids the build-time Metal
  compiler (Xcode 27 no longer ships the Metal Toolchain by default; the
  offline path fails with `xcodebuild -downloadComponent MetalToolchain`).
  This is the same feature set manox-app runs.

## Automated-run constraints (honest notes)

- The scripted driving (`osascript keystroke`) worked, but the machine was
  in active use during the run: focus racing ate part of one injected
  sequence and briefly captured two keystrokes of the operator's own typing
  into the spike window ("哦嚯" in the trace above — a real CJK commit,
  just not the intended one). Keystroke injection was stopped rather than
  risk more interference; the remaining gap (Esc cancel) is covered by the
  manual checklist below.
- TIS input-source switching was not automatable from this shell (ABC
  layout never appeared in `TISCreateInputSourceList` queries), so the
  direct-ASCII path was exercised via the IME passthrough commit and by the
  operator's normal typing instead.

## Manual IME verification checklist (~5 minutes)

Run `cd apps/openidea-gui && cargo run`, click into the window (or it
self-focuses), then:

1. Type `ni hao` with the Pinyin input source.
   - Expect: letters appear inline with an accent underline and a tinted
     background; the status line shows `marked=[28..33]` (growing as you
     type); the Squirrel candidate window sits at the caret, following it.
2. Press Space.
   - Expect: `nihao` is replaced by `你好` (accent styling disappears,
     `marked=[none]`, caret right after 好).
3. Type `zhongwen`, then press Esc.
   - Expect: the composition text disappears entirely (`marked=[none]`,
     caret back where it was before composing), no residue.
4. Type `abc` with the same CJK input source, then with the ABC input source.
   - Expect: both insert `abc` directly (raw-Latin commit vs. direct keydown
     path); no swallowed keys.
5. Backspace across the `你` boundary, arrow Left/Right across it, click
   before/after it.
   - Expect: caret moves one *character* at a time across the 2-UTF-16-unit
     glyph; rendering stays aligned.
6. (already verified by hand during the spike session, repeat optionally)
   `ni hao` + Space again — matches `m0-3-committed-cjk.png`.

## Main-workspace isolation evidence

- `cargo metadata --no-deps` (worktree root): no `gpui*` package and no
  `openidea-gui` in the workspace package list (36 members, unchanged).
- Root `Cargo.lock` shows no diff on this branch.
- `cargo check -p ide-core` in the worktree root: passes (17.5s, warm).
- App crate gates: `cargo fmt --check` clean; `cargo clippy --all-targets
  -- -D warnings` clean (only the upstream `block v0.1.6` future-incompat
  notice from the gpui stack).

## Verdict details

**Why GO:**

1. The blocking question — can a custom-drawn element host editor-grade CJK
   IME on macOS — is answered yes, end to end, with a real Pinyin IME, a
   real human input session, and a protocol trace showing every
   NSTextInputClient callback firing correctly.
2. The whole protocol surface we need is public gpui API; no doc(hidden)
   dependencies, no framework patching, no fork.
3. The exact-pinned stack (`gpui-pre 0.3.9` + `gpui-component 0.8.0`) builds
   cleanly in an isolated crate, and the isolation mechanism (root
   `exclude`) keeps CI untouched.

**What this verdict does not claim** (next-gate material, none of it
IME-related):

- Multi-line text, selection rendering, mouse-drag selection, undo — the M0
  buffer is deliberately single-line and selection-less; marked-range
  bookkeeping is the only inline-edit state the IME contract requires.
- Other IMEs (Korean Jamock-style instant commit, Japanese roma-ji with
  nested reconversion) and Windows/Linux IME stacks are unverified — macOS
  was the M0 scope.
- Performance (frame pacing under the GPUI renderer, large-buffer shaping)
  and the M1 in-process integration with `ide-core`/`ide-text` are separate
  measurements.
- Candidate-window *visuals* are the IME's own (Squirrel's), not ours —
  what M0 proves is that gpui hands it the right anchor rect.

## How to reproduce

```sh
cd apps/openidea-gui
cargo run               # window opens, self-focused
# Pinyin: ni hao → Space → 你好 committed; Esc cancels composition
OPENIDEA_M0_DEMO_MARKED=1 cargo run   # static composition state for screenshots
```
