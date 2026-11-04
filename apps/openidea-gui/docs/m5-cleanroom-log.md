# M5 clean-room log

Continues `m4-cleanroom-log.md`. Zed (GPL-3.0, `~/projects/github/zed`) may be
read **before** implementation as a design reference only; every M5 read is
listed here with the question it answered. No Zed code, types, literals or
line-level structure is copied; the implementation is written from these
notes plus openidea's own Apache-2.0 crates and the Apache-2.0 gpui-pre /
gpui-component API.

## 2026-11-04 (M5, before any M5 code)

No Zed files were read for M5. The reasons:

- Semantic tokens, code actions, formatting, signature help and hover are
  LSP protocol surfaces fully encapsulated in openidea's `ide-lsp`
  (`semantic_tokens`, `code_actions`/`resolve_code_action`/`execute_command`,
  `format`, `signature_help`, `hover` — the same manager the Tauri and
  headless hosts drive). The GPUI host's work is rendering and wiring.
- Scrollbars and horizontal scroll extend the M2 paint-only element whose
  geometry math is this repo's own (`Geometry`, `offset_at`, the display
  map); a scrollbar is a quad plus a drag mapping on top of the existing
  `scroll` cell, not a new architecture.
- The live-template model is ported from this repo's own TypeScript
  (`src/features/templates/live/model.ts` + the Go/Rust groups in
  `live/builtins.ts` — Apache-2.0 project code), like M3's fuzzy matcher
  was.
- Markdown rendering: gpui-component 0.8.0 ships no markdown widget
  (checked: no `markdown` module in the crate sources), so the hover popup
  gets the smallest renderer this app can own — headings, lists, bold,
  inline and fenced code — written directly against gpui text primitives.
- Find in Files is openidea's own `ide-search::search` (the same engine the
  Tauri host's search drives), streamed through a callback; the dock page
  reuses the M4 RunPanel slot pattern.
- Palette off-loading moves the existing `ProjectIndex` calls behind the
  background executor — a threading change, not a search change.

## Apache-2.0 sources consulted (non-Zed)

- openidea crates: `ide-lsp` (feature requests, `types.rs`, `convert.rs` —
  token decode/legend, `FormatOptions`, `CodeAction`, `SignatureHelp`),
  `ide-search` (`SearchQuery`, `FileMatches`, `search`), `ide-language`
  (`IndentConfig`, `LanguageRegistry::get`), `ide-text` (`Document`
  offsets), `ide-workspace` (`DocumentInfo`, `EditorConfigProps`),
  `ide-index` (`find_files`/`find_symbols` — for the palette move).
- This repo's TypeScript: `src/features/templates/live/model.ts` (body
  parse + tab stops), `src/features/templates/live/builtins.ts` (Go/Rust
  builtin groups).
- gpui-pre / gpui-component 0.8.0 crate sources (registry cache): confirmed
  no markdown widget exists; reused `InputState`, `Button`, `TabBar`
  (dock tabs), scroll-event deltas, quad painting.
