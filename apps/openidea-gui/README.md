# openidea-gui

The future GPUI frontend host for OpenIDEA. **Status: M0 gate (go/no-go
spike)** — this crate exists to answer one question before any workbench work
starts: can editor-grade CJK IME input (compose, candidate-window anchoring,
commit, cancel) work against a hand-drawn GPUI element on macOS?

Read [`M0-REPORT.md`](./M0-REPORT.md) for the answer and the evidence.

## Run

```sh
cd apps/openidea-gui   # has its own rust-toolchain.toml and Cargo.lock
cargo run
```

Switch the macOS input source to Pinyin (or any CJK IME), type `ni hao`, watch
the composition underline and the candidate window follow the caret, press
Space to commit, Esc to cancel. `Cmd+Q` quits.

`OPENIDEA_M0_DEMO_MARKED=1 cargo run` seeds a static composition state through
the same code path the IME protocol drives (for deterministic screenshots).

## Layout

- `src/main.rs` — window bootstrap only.
- `src/editor.rs` — the single-line buffer (UTF-16 caret + marked range), the
  hand-painted element, and the gpui `InputHandler` (NSTextInputClient
  semantics) that drives it from the platform IME.

## Isolation from the main workspace

This crate is deliberately **outside the root workspace**: the root
`Cargo.toml` lists it in `exclude`, and this directory carries an empty
`[workspace]` table plus its own lockfile. The gpui stack never enters the
main build graph, so `cargo nextest run --workspace` / `cargo clippy
--workspace` in CI are unaffected.

## Stack pins

The GPUI stack is exact-pinned to the versions manox-app (Steer) locks, so the
two repos share one tested snapshot:

- `gpui-pre 0.3.9` (gpui, aliased) + `gpui-pre-platform 0.3.9` with
  `font-kit` + `runtime_shaders` (runtime Metal shader compilation avoids
  requiring the separately-downloadable Xcode Metal Toolchain)
- `gpui-component 0.8.0` — pinned for stack validation only; nothing from it
  is used yet (M0 renders everything by hand)

No dependency on `zed-industries/zed` directly — the same review-enforced rule
manox-app applies.

## License

This crate is Apache-2.0, like the rest of OpenIDEA. Its GPUI dependencies
(`gpui-pre` / `gpui-pre-platform` — Longbridge's repackaged snapshots of
zed's gpui — and `gpui-component` / `gpui-base`) are Apache-2.0; their
license texts ship inside the published crates and their upstream notices
apply unchanged.
