# M3 clean-room log

Continues `m2-cleanroom-log.md`. Zed (GPL-3.0, `~/projects/github/zed`) may be
read **before** implementation as a design reference only; every M3 read is
listed here with the question it answered. No Zed code, types, literals or
line-level structure is copied; the implementation is written from these notes
plus openidea's own Apache-2.0 crates and the Apache-2.0 gpui-pre /
gpui-component API.

## 2026-11-04 (M3, before any M3 code)

No Zed files were read for M3. The reasons:

- LSP lifecycle, document sync, diagnostics, completion, navigation and rename
  are fully encapsulated in openidea's own `ide-lsp` (the same manager the
  Tauri and headless hosts drive) — the GPUI host is a wiring problem, not a
  protocol one.
- The action/keymap layer maps onto gpui-pre's own action system
  (`actions!`, `cx.bind_keys`, `on_action`, `cx.all_action_names`,
  `cx.build_action` — Apache-2.0 registry sources listed below).
- The fuzzy matcher for actions is a port of this repo's own TypeScript
  (`src/ui/match.ts`, `src/features/search/common/rank.ts`); files/symbols use
  openidea's own `ide-index` engine.

## Apache-2.0 sources consulted (non-Zed)

- gpui-pre 0.3.9 sources in the local cargo registry:
  `src/action.rs` (action registry: names, `build_action`,
  `all_action_names`), `src/keymap/binding.rs` + `src/keymap/keymap.rs`
  (binding matching, keystroke sequences), `src/window.rs`
  (`dispatch_action`, `on_action` registration).
- gpui-component 0.8.0 sources: `menu/popup_menu.rs`, `list/*` (evaluated for
  the completion popup; rejected because the popup must not take focus from
  the editor — see design notes §4), `status_bar.rs`, `tab.rs` (already used
  by M1/M2).
- openidea crates: `ide-lsp/src/lib.rs` (+ `types.rs`, `convert.rs` usage),
  `ide-language/src/registry.rs`, `ide-workspace/src/documents.rs`
  (`DocumentObserver`, `apply_workspace_edit`), `ide-core/src/event.rs`,
  `ide-core/src/text.rs`, `ide-index/src/lib.rs` + `matching.rs`,
  `ide-host/src/project.rs` (the host wiring sample: LspObserver, index open,
  cache-dir keying).
- This repo's TypeScript frontend: `src/ui/match.ts`,
  `src/features/search/common/rank.ts` (ported to Rust for action filtering).
- `~/projects/dspo/manox-app` (the user's own gpui-kit consumer, same pins):
  Cargo.toml pins checked; no code copied.
