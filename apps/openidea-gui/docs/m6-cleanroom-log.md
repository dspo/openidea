# M6 clean-room log

Per the milestone discipline: Zed (GPL-3.0, `~/projects/github/zed`) was **not
read** this milestone — not even before coding. Every design decision above
came from:

- this repo's own crates: `ide-lsp` (LspManager feature APIs: completion,
  codeAction/resolve, executeCommand, inlayHint, workspaceSymbol, references
  widening, `$/progress` → `IdeEvent::Task`, `convert.rs` diagnostics),
  `ide-core` (Diagnostic / Pos / WorkspaceEdit / IdeEvent), `ide-workspace`
  (Documents edit/save/apply_workspace_edit), `ide-index` (SymbolMatch),
  `ide-language` (`languages/*.toml` schema),
- the app's own M0–M5 sources (`editor.rs` popup/element pipeline,
  `workbench.rs` bus/palette/status, `host.rs` wiring, `semantic.rs` overlay,
  `templates.rs` model),
- the M0–M5 reports and design notes (seam intelligence, especially M5's
  gopls-vs-rust-analyzer notes),
- the LSP 3.17 specification shape of the features (inlayHint, completion
  itemDefaults, executeCommand) as already encoded in `ide-lsp`'s types,
- live probes against the local gopls binary (`gopls version` 0.21.1) with
  raw stdio JSON-RPC sessions replicating `caps.rs` client capabilities —
  the same empirical method M5 used to discover the semanticTokens opt-in,
- `languages/go.toml` hints settings: gopls's own settings documentation
  (`gopls settings hints` via `gopls help` / its published settings.md),
  consulted as the server's user documentation, not as editor source code.

No gpui-component or gpui-pre API beyond what M0–M5 already used was needed.
