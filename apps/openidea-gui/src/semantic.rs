//! Semantic-token overlay: LSP `textDocument/semanticTokens` decoded by
//! `ide-lsp` (absolute `[line, utf16col, utf16len, typeIdx, mods]` quintuples
//! with the server's legend) turned into the editor's byte-range span list,
//! merged over the tree-sitter spans in [`crate::highlight::overlay`].

use std::ops::Range;

use ide_lsp::types::SemanticTokens;
use ropey::Rope;

use crate::highlight::Kind;

/// The token types this editor paints, mapped onto the syntax palette.
/// Unknown types (variables, namespaces, punctuation, …) keep the default
/// foreground, so the overlay only ever adds color the grammar query would
/// not.
fn kind_for_token_type(name: &str) -> Option<Kind> {
    let kind = match name {
        "keyword" | "controlKeyword" | "modifier" | "selfKeyword" | "selfTypeKeyword" | "boolean" => Kind::Keyword,
        "comment" => Kind::Comment,
        "string" | "character" => Kind::String,
        "escapeSequence" | "escape" | "formatSpecifier" => Kind::Escape,
        "number" | "integer" | "byte" | "float" => Kind::Number,
        "function" | "method" => Kind::Function,
        "type" | "class" | "struct" | "enum" | "interface" | "trait" | "union" | "typeAlias" | "builtinType"
        | "typeParameter" => Kind::Type,
        "property" | "enumMember" => Kind::Property,
        "macro" | "attribute" | "builtinAttribute" | "decorator" => Kind::Macro,
        "operator" => Kind::Operator,
        "constant" | "const" | "static" => Kind::Constant,
        _ => return None,
    };
    Some(kind)
}

/// Byte offset of the UTF-16 column `col16` within `line` of the rope
/// (clamped into the line).
fn byte_col_in_line(rope: &Rope, line: usize, col16: usize) -> Option<usize> {
    let line_start = rope.try_line_to_byte(line).ok()?;
    let line_end = rope.try_line_to_byte(line + 1).unwrap_or(rope.len_bytes());
    let mut units = 0usize;
    let mut at = line_start;
    for ch in rope.slice(line_start..line_end).chars() {
        if units >= col16 {
            return Some(at);
        }
        units += ch.len_utf16();
        at += ch.len_utf8();
    }
    Some(line_end)
}

/// The decoded tokens as byte-range spans, ascending and non-overlapping
/// (the protocol emits ascending, non-overlapping tokens). Tokens whose
/// legend type has no palette mapping, or whose range falls outside the
/// rope (a stale answer), are dropped.
pub fn token_spans(tokens: &SemanticTokens, rope: &Rope) -> Vec<(Range<usize>, Kind)> {
    let mut out = Vec::new();
    for chunk in tokens.data.chunks_exact(5) {
        let [line, col, len, type_idx, _mods] = chunk else { continue };
        let Some(name) = tokens.token_types.get(*type_idx as usize) else { continue };
        let Some(kind) = kind_for_token_type(name) else { continue };
        if *len == 0 {
            continue;
        }
        let Some(start) = byte_col_in_line(rope, *line as usize, *col as usize) else { continue };
        let Some(end) = byte_col_in_line(rope, *line as usize, *col as usize + *len as usize) else { continue };
        if end > start {
            out.push((start..end, kind));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope_of(text: &str) -> Rope {
        Rope::from_str(text)
    }

    fn tokens(types: &[&str], data: &[u32]) -> SemanticTokens {
        SemanticTokens {
            data: data.to_vec(),
            token_types: types.iter().map(|s| s.to_string()).collect(),
            token_modifiers: Vec::new(),
        }
    }

    #[test]
    fn token_types_map_onto_kinds() {
        assert_eq!(kind_for_token_type("keyword"), Some(Kind::Keyword));
        assert_eq!(kind_for_token_type("selfKeyword"), Some(Kind::Keyword));
        assert_eq!(kind_for_token_type("method"), Some(Kind::Function));
        assert_eq!(kind_for_token_type("struct"), Some(Kind::Type));
        assert_eq!(kind_for_token_type("variable"), None, "variables keep the default color");
        assert_eq!(kind_for_token_type("punctuation"), None);
    }

    #[test]
    fn tokens_map_to_byte_ranges() {
        // "fn main() {\n\tlet x = 1;\n}\n"
        // line 1 starts at byte 12; "let" sits at utf16 col 1 (after the tab)
        // → bytes 13..16.
        let rope = rope_of("fn main() {\n\tlet x = 1;\n}\n");
        let got = token_spans(&tokens(&["keyword", "variable"], &[0, 0, 2, 0, 0, 1, 1, 3, 0, 0, 1, 5, 1, 1, 0]), &rope);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (0..2, Kind::Keyword));
        assert_eq!(got[1], (13..16, Kind::Keyword), "utf16 col maps through the rope's line bytes");
    }

    #[test]
    fn stale_and_unknown_tokens_drop() {
        let rope = rope_of("abc\n");
        // Type index beyond the legend, zero length, and a line past the end.
        let got = token_spans(&tokens(&["keyword"], &[0, 0, 3, 7, 0, 0, 0, 0, 0, 0, 5, 0, 3, 0, 0]), &rope);
        assert!(got.is_empty());
    }
}
