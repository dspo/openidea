//! Incremental tree-sitter highlighting for the editor pane.
//!
//! The [`Highlighter`] owns the highlights query for one language. The pane
//! edits its `tree_sitter::Tree` in place with every buffer change (the
//! `ByteEdit` vec from `ide_text` maps 1:1 onto tree-sitter's `InputEdit`),
//! then schedules a parse job on the background executor: text is
//! materialized from the rope off the main thread, parsed incrementally from
//! the edited tree, and the whole-document span list is produced by running
//! the query once — the paint path then only slices those spans per visible
//! line (no per-frame query, no per-frame text copy).
//!
//! The grammar comes from the repo's bundled set through
//! `ide_language::Syntax::ts_language` (its documented seam for engines that
//! parse on their own); the highlights query below is written for the
//! tree-sitter-rust node names. Files flagged `large` are never parsed.

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use ide_core::LanguageId;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};

/// What a captured span paints. Mapped onto the hardcoded palette in
/// `theme::syntax_color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    String,
    Escape,
    Number,
    Constant,
    Comment,
    Function,
    Type,
    Property,
    Macro,
    Operator,
}

/// Capture-name prefixes; a capture paints by its longest known prefix
/// (`function.call` → `Function`). Unknown names and private `@_` captures
/// paint nothing.
const CAPTURES: &[(&str, Kind)] = &[
    ("string.escape", Kind::Escape),
    ("escape", Kind::Escape),
    ("boolean", Kind::Constant),
    ("constant", Kind::Constant),
    ("comment", Kind::Comment),
    ("function", Kind::Function),
    ("keyword", Kind::Keyword),
    ("macro", Kind::Macro),
    ("number", Kind::Number),
    ("operator", Kind::Operator),
    ("property", Kind::Property),
    ("string", Kind::String),
    ("type", Kind::Type),
];

fn kind_for(name: &str) -> Option<Kind> {
    if name.starts_with('_') {
        return None;
    }
    let mut prefix = name;
    loop {
        if let Some((_, k)) = CAPTURES.iter().find(|(n, _)| *n == prefix) {
            return Some(*k);
        }
        prefix = &prefix[..prefix.rfind('.')?];
    }
}

/// A finished parse hands the pane its tree, its span list, the foldable
/// line ranges and the parsed version.
pub type ParseResult = (Tree, Vec<(Range<usize>, Kind)>, Vec<(u32, u32)>, u32);

/// At most this many fold ranges are collected (huge generated files).
const MAX_FOLDS: usize = 4096;

/// `(start line, end line)` pairs of foldable syntax nodes: named nodes that
/// span several lines, outermost first (nested candidates are dropped when
/// their containing range is kept). The root node itself is not a candidate —
/// folding the whole document from it would swallow every other range.
pub fn fold_candidates(tree: &Tree) -> Vec<(u32, u32)> {
    let root = tree.root_node();
    let mut all: Vec<(u32, u32)> = Vec::new();
    let mut stack: Vec<tree_sitter::Node> = Vec::new();
    for i in 0..root.child_count() {
        if let Some(child) = root.child(i) {
            stack.push(child);
        }
    }
    while let Some(node) = stack.pop() {
        let (start, end) = (node.start_position().row as u32, node.end_position().row as u32);
        if node.is_named() && end > start {
            all.push((start, end));
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    // Outermost wins: longest ranges first, then keep what does not overlap
    // an already-kept range (the kept list stays ascending by construction).
    all.sort_by_key(|(start, end)| (*start, std::cmp::Reverse(*end)));
    let mut kept: Vec<(u32, u32)> = Vec::new();
    for (start, end) in all {
        if kept.len() >= MAX_FOLDS {
            break;
        }
        match kept.last() {
            Some(&(_, last_end)) if start <= last_end => {}
            _ => kept.push((start, end)),
        }
    }
    kept
}

/// Highlights for one language: the query plus each capture's kind.
pub struct Highlighter {
    language: tree_sitter::Language,
    query: Query,
    kinds: Vec<Option<Kind>>,
}

impl Highlighter {
    /// The bundled grammar + highlights for `language`, or `None` without a
    /// grammar or a valid query.
    pub fn for_language(language: &LanguageId) -> Option<Arc<Self>> {
        static SYNTAX: OnceLock<ide_language::Syntax> = OnceLock::new();
        let syntax = SYNTAX.get_or_init(ide_language::Syntax::new);
        let (_, ts_language) = syntax.ts_language(language)?;
        let query = Query::new(&ts_language, RUST_HIGHLIGHTS).ok()?;
        let kinds = query.capture_names().iter().map(|n| kind_for(n)).collect();
        Some(Arc::new(Self { language: ts_language, query, kinds }))
    }

    /// Parses `text` (reusing `old_tree` for an incremental parse) and runs
    /// the highlights query over it. Runs on a background thread; the pane
    /// hands over its edited tree and only keeps the result if the document
    /// is still at `version`.
    pub fn parse(&self, text: &str, old_tree: Option<&Tree>, version: u32) -> Option<ParseResult> {
        let mut parser = Parser::new();
        parser.set_language(&self.language).ok()?;
        let tree = parser.parse(text, old_tree)?;
        let spans = self.spans(&tree, text);
        let folds = fold_candidates(&tree);
        Some((tree, spans, folds, version))
    }

    /// Sorted, non-overlapping `(byte range, kind)` spans of the whole text:
    /// a smaller capture wins over the node containing it, for the same node
    /// the later pattern wins, and the predicate keywords re-mark themselves
    /// as keywords on top.
    fn spans(&self, tree: &Tree, text: &str) -> Vec<(Range<usize>, Kind)> {
        let mut captures: Vec<(Range<usize>, Kind)> = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), text.as_bytes());
        while let Some(m) = matches.next() {
            for capture in m.captures {
                if let Some(kind) = self.kinds[capture.index as usize] {
                    let range = capture.node.byte_range();
                    if range.start < range.end {
                        captures.push((range, kind));
                    }
                }
            }
        }
        captures.extend(predicate_keyword_spans(tree, text));
        // Painted largest first, then by pattern order (later overwrites), so
        // inner nodes and winning patterns end up visible.
        captures.sort_by_key(|(range, _)| std::cmp::Reverse(range.len()));
        resolve(captures, text.len())
    }
}

/// Folds captured (possibly nested, possibly overlapping) ranges into a flat,
/// sorted, non-overlapping paint list over `0..len`.
fn resolve(captures: Vec<(Range<usize>, Kind)>, len: usize) -> Vec<(Range<usize>, Kind)> {
    let mut painted: Vec<Option<Kind>> = vec![None; len];
    for (range, kind) in &captures {
        painted[range.clone()].fill(Some(*kind));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < len {
        let v = painted[i];
        let start = i;
        while i < len && painted[i] == v {
            i += 1;
        }
        if let Some(kind) = v {
            out.push((start..i, kind));
        }
    }
    out
}

/// Merges two sorted, non-overlapping span lists with `extra` winning where
/// they overlap (semantic tokens paint over the grammar capture, at any
/// length). Two paint passes over one byte grid, compacted like [`resolve`]
/// — only called when a source lands, never per frame.
pub fn overlay(base: &[(Range<usize>, Kind)], extra: &[(Range<usize>, Kind)], len: usize) -> Vec<(Range<usize>, Kind)> {
    let mut painted: Vec<Option<Kind>> = vec![None; len];
    for (range, kind) in base {
        painted[range.clone()].fill(Some(*kind));
    }
    for (range, kind) in extra {
        painted[range.clone()].fill(Some(*kind));
    }
    let mut out: Vec<(Range<usize>, Kind)> = Vec::new();
    let mut i = 0;
    while i < len {
        let start = i;
        let v = painted[i];
        while i < len && painted[i] == v {
            i += 1;
        }
        if let Some(kind) = v {
            out.push((start..i, kind));
        }
    }
    out
}

/// The Rust keywords the grammar does not (fully) capture for this pane's
/// query runner: `crate`/`self`/`super` are anonymous tokens no named-node
/// pattern matches, `mut` rides `mutable_specifier`, and `Self` is a
/// `type_identifier` the official query separates with a `#eq?` predicate.
/// A tree walk re-marks all of them as keywords (the M2 leftover).
const PREDICATE_KEYWORDS: &[&str] = &["crate", "mut", "self", "Self", "super"];

const PREDICATE_KINDS: &[&str] = &["crate", "self", "super", "mutable_specifier", "identifier", "type_identifier"];

/// `(byte range, Keyword)` spans for the keyword-shaped nodes whose text is
/// one of [`PREDICATE_KEYWORDS`].
fn predicate_keyword_spans(tree: &Tree, text: &str) -> Vec<(Range<usize>, Kind)> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    let mut done = false;
    while !done {
        if PREDICATE_KINDS.contains(&cursor.node().kind()) {
            let range = cursor.node().byte_range();
            if range.start < range.end && PREDICATE_KEYWORDS.contains(&&text[range.clone()]) {
                out.push((range, Kind::Keyword));
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        if cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                done = true;
                break;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
    out
}

/// Byte positions of an edit, as tree-sitter wants them (`ide_text::ByteEdit`
/// is documented to map 1:1).
pub fn input_edit(edit: &ide_text::ByteEdit) -> InputEdit {
    InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: Point { row: edit.start.row, column: edit.start.col },
        old_end_position: Point { row: edit.old_end.row, column: edit.old_end.col },
        new_end_position: Point { row: edit.new_end.row, column: edit.new_end.col },
    }
}

/// Highlights for Rust, written against the tree-sitter-rust grammar's node
/// names. Plain identifiers paint the default foreground, so only the
/// specific captures below exist.
const RUST_HIGHLIGHTS: &str = r#"
(line_comment) @comment
(block_comment) @comment

(string_literal) @string
(raw_string_literal) @string
(char_literal) @string
(escape_sequence) @escape

(integer_literal) @number
(float_literal) @number
(boolean_literal) @constant

(type_identifier) @type
(primitive_type) @type

(function_item name: (identifier) @function)
(function_signature_item name: (identifier) @function)

(call_expression function: (identifier) @function)
(call_expression function: (field_expression field: (field_identifier) @function))
(field_identifier) @property

(mutable_specifier) @keyword

(macro_invocation macro: (identifier) @macro)
(macro_definition name: (identifier) @macro)

[
  "as" "async" "await" "break" "const" "continue"
  "dyn" "else" "enum" "extern" "fn" "for" "if" "impl" "in"
  "let" "loop" "match" "mod" "move" "pub" "ref"
  "return" "static" "struct" "trait"
  "type" "union" "unsafe" "use" "where" "while"
] @keyword

[
  "+" "-" "*" "/" "%" "=" "==" "!=" "<" ">" "<=" ">="
  "&&" "||" "!" "&" "|" "^" "<<" ">>" "+=" "-=" "*="
  "/=" "%=" "->" "=>"
] @operator
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> Arc<Highlighter> {
        Highlighter::for_language(&LanguageId::new("rust")).expect("rust grammar + query")
    }

    fn spans(src: &str) -> Vec<(String, Kind)> {
        let hl = rust();
        let mut parser = Parser::new();
        parser.set_language(&hl.language).unwrap();
        let tree = parser.parse(src, None).unwrap();
        hl.spans(&tree, src).into_iter().map(|(r, k)| (src[r].to_owned(), k)).collect()
    }

    fn kind_of(got: &[(String, Kind)], text: &str) -> Option<Kind> {
        got.iter().find(|(s, _)| s == text).map(|(_, k)| *k)
    }

    #[test]
    fn query_compiles_and_captures_map() {
        let hl = rust();
        assert!(hl.kinds.contains(&Some(Kind::Keyword)));
    }

    #[test]
    fn query_error_debug() {
        let syntax = ide_language::Syntax::new();
        let (_, lang) = syntax.ts_language(&LanguageId::new("rust")).expect("grammar");
        match tree_sitter::Query::new(&lang, RUST_HIGHLIGHTS) {
            Ok(_) => {}
            Err(e) => panic!("query error: {e}"),
        }
    }

    #[test]
    fn rust_tokens_get_kinds() {
        use Kind::*;
        let src = "// note\nfn main() {\n    let s = \"hello\";\n    let t = \"a\\nb\";\n    println!(\"{s}\");\n    let n: u32 = 42;\n}\n";
        let got = spans(src);
        assert_eq!(kind_of(&got, "// note"), Some(Comment));
        assert_eq!(kind_of(&got, "fn"), Some(Keyword));
        assert_eq!(kind_of(&got, "main"), Some(Function));
        assert_eq!(kind_of(&got, "let"), Some(Keyword));
        assert_eq!(kind_of(&got, "\"hello\""), Some(String));
        // The escape splits its string: only the escape run matches that text.
        assert_eq!(kind_of(&got, "\\n"), Some(Escape));
        // `println!` is a macro invocation, not a function call.
        assert_eq!(kind_of(&got, "println"), Some(Macro));
        assert_eq!(kind_of(&got, "u32"), Some(Type));
        assert_eq!(kind_of(&got, "42"), Some(Number));
    }

    #[test]
    fn plain_identifiers_paint_nothing() {
        let src = "fn main() {\n    let value = other;\n}\n";
        let got = spans(src);
        assert_eq!(kind_of(&got, "value"), None, "bare identifiers keep the default color");
        assert_eq!(kind_of(&got, "other"), None);
    }

    #[test]
    fn resolve_splits_nesting_and_truncates_overlap() {
        use Kind::Comment as C;
        use Kind::Keyword as K;
        use Kind::String as S;
        let comment = 0..30;
        // A shorter capture nested inside the comment splits it in two.
        let got = resolve(vec![(comment.clone(), C), (10..16, K)], 30);
        assert_eq!(got, vec![(0..10, C), (10..16, K), (16..30, C)]);
        // An overlapping (not nested) capture truncates the open range.
        let got = resolve(vec![(0..30, C), (20..40, S)], 40);
        assert_eq!(got, vec![(0..20, C), (20..40, S)]);
        // Later capture replaces an identical range.
        let got = resolve(vec![(5..9, Kind::Property), (5..9, Kind::Function)], 9);
        assert_eq!(got, vec![(5..9, Kind::Function)]);
        // No capture: empty.
        assert!(resolve(Vec::new(), 10).is_empty());
    }

    #[test]
    fn input_edit_maps_fields() {
        let edit = ide_text::ByteEdit {
            start_byte: 1,
            old_end_byte: 3,
            new_end_byte: 5,
            start: ide_text::BytePoint { row: 0, col: 1 },
            old_end: ide_text::BytePoint { row: 0, col: 3 },
            new_end: ide_text::BytePoint { row: 1, col: 2 },
        };
        let mapped = input_edit(&edit);
        assert_eq!(mapped.start_byte, 1);
        assert_eq!(mapped.new_end_position, Point { row: 1, column: 2 });
    }

    #[test]
    fn predicate_keywords_paint_as_keywords() {
        // The five grammar-predicate keywords the plain query misses (M2
        // leftover). `mut` rides `mutable_specifier`; the identifier-shaped
        // ones come from the tree walk.
        let src = "use crate::foo;\nfn f(&mut self) -> Self { super::g() }\n";
        let got = spans(src);
        for word in ["crate", "self", "Self", "super", "mut"] {
            assert_eq!(kind_of(&got, word), Some(Kind::Keyword), "`{word}` must paint as a keyword");
        }
        // Regular identifiers stay plain.
        assert_eq!(kind_of(&got, "foo"), None);
    }

    #[test]
    fn overlay_lets_extra_win() {
        use Kind::{Comment as C, Function as F, Keyword as K};
        let base = vec![(0..10, C), (10..20, F)];
        // A shorter extra span carves out of the base; an identical range wins.
        let got = overlay(&base, &[(12..16, K)], 20);
        assert_eq!(got, vec![(0..10, C), (10..12, F), (12..16, K), (16..20, F)]);
        // An extra span wider than the base swallows it.
        let got = overlay(&base, &[(0..15, K)], 20);
        assert_eq!(got, vec![(0..15, K), (15..20, F)]);
        // Disjoint extras append in order.
        let got = overlay(&base, &[(20..24, K)], 30);
        assert_eq!(got, vec![(0..10, C), (10..20, F), (20..24, K)]);
        // Empty extra returns the base.
        assert_eq!(overlay(&base, &[], 20), base);
    }
}
