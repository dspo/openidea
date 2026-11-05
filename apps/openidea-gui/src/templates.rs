//! Live templates: the IDEA-syntax model ported from the Tauri frontend's
//! `features/templates/live/model.ts` (Apache-2.0 project code) — `$NAME$`
//! variables (repeated occurrences are linked), `$END$` (final caret), `$$`
//! (a literal dollar). Body leading tabs after a newline are indent units
//! relative to the expansion line. The builtin sets are the frontend's own
//! Go and Rust groups; template management UI stays out of M5.

/// One builtin template. `languages` holds language ids (`"go"`, `"rust"`).
pub struct LiveTemplate {
    pub id: &'static str,
    pub abbrev: &'static str,
    /// Kept for a future abbreviation popup (no template UI in M5).
    #[allow(dead_code)]
    pub description: &'static str,
    pub body: &'static str,
    pub languages: &'static [&'static str],
}

/// Variable defaults (`$NAME$` fills with this when no user text was typed
/// there yet) — the frontend's `variables: [v("I", undefined, '"i"')]`.
pub fn builtin_defaults(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "go-fori" => &[("I", "i")],
        "go-forr" => &[("K", "_"), ("V", "v")],
        "rust-test" | "rust-tmod" => &[("NAME", "it_works")],
        "rust-derive" => &[("TRAITS", "Debug, Clone")],
        _ => &[],
    }
}

/// The builtin sets (the frontend's `builtins.ts` Go and Rust groups).
pub const BUILTIN_TEMPLATES: &[LiveTemplate] = &[
    LiveTemplate {
        id: "go-iferr",
        abbrev: "iferr",
        description: "if err != nil { return … }",
        body: "if err != nil {\n\treturn $RET$err\n}$END$",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-fori",
        abbrev: "fori",
        description: "Indexed for loop",
        body: "for $I$ := 0; $I$ < $N$; $I$++ {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-forr",
        abbrev: "forr",
        description: "for range loop",
        body: "for $K$, $V$ := range $X$ {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-func",
        abbrev: "func",
        description: "Function",
        body: "func $NAME$($PARAMS$) $RET$ {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-meth",
        abbrev: "meth",
        description: "Method",
        body: "func ($R$ $TYPE$) $NAME$($PARAMS$) $RET$ {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-main",
        abbrev: "main",
        description: "main function",
        body: "func main() {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-test",
        abbrev: "test",
        description: "Test function",
        body: "func Test$NAME$(t *testing.T) {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-bench",
        abbrev: "bench",
        description: "Benchmark function",
        body: "func Benchmark$NAME$(b *testing.B) {\n\tfor i := 0; i < b.N; i++ {\n\t\t$END$\n\t}\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-st",
        abbrev: "st",
        description: "Struct type",
        body: "type $NAME$ struct {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-inf",
        abbrev: "inf",
        description: "Interface type",
        body: "type $NAME$ interface {\n\t$END$\n}",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-p",
        abbrev: "p",
        description: "fmt.Println",
        body: "fmt.Println($END$)",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-pf",
        abbrev: "pf",
        description: "fmt.Printf",
        body: "fmt.Printf(\"$FMT$\\n\", $END$)",
        languages: &["go"],
    },
    LiveTemplate {
        id: "go-go",
        abbrev: "go",
        description: "goroutine",
        body: "go func() {\n\t$END$\n}()",
        languages: &["go"],
    },
    LiveTemplate {
        id: "rust-fn",
        abbrev: "fn",
        description: "Function",
        body: "fn $NAME$($PARAMS$) {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-pfn",
        abbrev: "pfn",
        description: "Public function",
        body: "pub fn $NAME$($PARAMS$) {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-afn",
        abbrev: "afn",
        description: "Async function",
        body: "async fn $NAME$($PARAMS$) {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-fnr",
        abbrev: "fnr",
        description: "Function with return type",
        body: "fn $NAME$($PARAMS$) -> $RET$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-main",
        abbrev: "main",
        description: "main function",
        body: "fn main() {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-test",
        abbrev: "test",
        description: "Test function",
        body: "#[test]\nfn $NAME$() {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-tmod",
        abbrev: "tmod",
        description: "Test module",
        body: "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn $NAME$() {\n\t\t$END$\n\t}\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-impl",
        abbrev: "impl",
        description: "impl block",
        body: "impl $TYPE$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-implt",
        abbrev: "implt",
        description: "Trait implementation",
        body: "impl $TRAIT$ for $TYPE$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-st",
        abbrev: "st",
        description: "Struct",
        body: "struct $NAME$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-en",
        abbrev: "en",
        description: "Enum",
        body: "enum $NAME$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-tr",
        abbrev: "tr",
        description: "Trait",
        body: "trait $NAME$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-match",
        abbrev: "match",
        description: "match expression",
        body: "match $EXPR$ {\n\t$PAT$ => $END$,\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-ifl",
        abbrev: "ifl",
        description: "if let",
        body: "if let $PAT$ = $EXPR$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-whilel",
        abbrev: "whilel",
        description: "while let",
        body: "while let $PAT$ = $EXPR$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-for",
        abbrev: "for",
        description: "for loop",
        body: "for $ITEM$ in $ITER$ {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-loop",
        abbrev: "loop",
        description: "loop",
        body: "loop {\n\t$END$\n}",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-let",
        abbrev: "let",
        description: "let binding",
        body: "let $NAME$ = $END$;",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-letm",
        abbrev: "letm",
        description: "let mut binding",
        body: "let mut $NAME$ = $END$;",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-derive",
        abbrev: "derive",
        description: "#[derive(...)]",
        body: "#[derive($TRAITS$)]",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-p",
        abbrev: "p",
        description: "println!",
        body: "println!(\"$END$\");",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-pd",
        abbrev: "pd",
        description: "println! with Debug",
        body: "println!(\"{:?}\", $END$);",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-ep",
        abbrev: "ep",
        description: "eprintln!",
        body: "eprintln!(\"$END$\");",
        languages: &["rust"],
    },
    LiveTemplate {
        id: "rust-mod",
        abbrev: "mod",
        description: "Module",
        body: "mod $NAME$ {\n\t$END$\n}",
        languages: &["rust"],
    },
];

/// The templates whose language set contains `language`.
pub fn for_language(language: &str) -> impl Iterator<Item = &'static LiveTemplate> {
    BUILTIN_TEMPLATES.iter().filter(move |t| t.languages.contains(&language))
}

/// A body piece after parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Var(String),
    End,
}

/// `$NAME$` variables, `$END$`, `$$` escapes; a lone `$` stays literal.
pub fn parse_body(body: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut text = String::new();
    let flush = |out: &mut Vec<Segment>, text: &mut String| {
        if !text.is_empty() {
            out.push(Segment::Text(std::mem::take(text)));
        }
    };
    let mut i = 0;
    let bytes = body.as_bytes();
    while i < bytes.len() {
        if body[i..].starts_with('$') {
            if body[i + 1..].starts_with('$') {
                text.push('$');
                i += 2;
                continue;
            }
            if let Some(close) = body[i + 1..].find('$') {
                let name = &body[i + 1..i + 1 + close];
                if is_var_name(name) {
                    flush(&mut out, &mut text);
                    if name == "END" {
                        out.push(Segment::End);
                    } else {
                        out.push(Segment::Var(name.to_owned()));
                    }
                    i += close + 2;
                    continue;
                }
            }
        }
        let ch = body[i..].chars().next().expect("non-empty suffix");
        text.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut out, &mut text);
    out
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// One tab stop: the variable's name (`""` for `$END$`) and its occurrence
/// ranges in the expanded text (UTF-16 offsets, ascending).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub name: String,
    pub ranges: Vec<(usize, usize)>,
}

/// The expansion of one template: concrete text plus its tab stops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    /// First variable stop first, `$END$` last (empty name).
    pub stops: Vec<Stop>,
}

/// Expands `template` at a line whose indentation is `base_indent`
/// (verbatim, may be empty); the document's indent unit replaces body tabs.
/// Variable occurrences fill with their defaults; repeated occurrences share
/// one stop (linked), `$END$` closes the session. Newlines only come from
/// body text segments (the builtins never put a variable at a line start),
/// so re-indentation is a per-piece transform inside one segment.
pub fn expand(template: &LiveTemplate, unit: &str, base_indent: &str) -> Expansion {
    let segments = parse_body(template.body);
    let defaults = builtin_defaults(template.id);
    let mut text = String::new();
    let mut stops: Vec<Stop> = Vec::new();
    let mut utf16 = 0usize; // cursor in the produced text
    let utf16_of = |s: &str| s.chars().map(char::len_utf16).sum::<usize>();
    for segment in segments {
        match segment {
            Segment::Text(body_text) => {
                for (piece_index, piece) in body_text.split('\n').enumerate() {
                    if piece_index > 0 {
                        text.push('\n');
                        utf16 += 1;
                    }
                    // Leading body tabs are indent units relative to the
                    // expansion line; every continuation line carries the
                    // expansion line's own indentation too (zero body tabs
                    // keeps the base indent — the closing brace of a block
                    // body aligns with its opener).
                    let tabs = piece.chars().take_while(|c| *c == '\t').count();
                    let rest = &piece[tabs..];
                    let indent = format!("{base_indent}{}", unit.repeat(tabs));
                    if piece_index > 0 && !indent.is_empty() {
                        text.push_str(&indent);
                        utf16 += utf16_of(&indent);
                    }
                    text.push_str(rest);
                    utf16 += utf16_of(rest);
                }
            }
            Segment::Var(name) => {
                let value = defaults.iter().find(|(n, _)| *n == name).map_or("", |(_, v)| v);
                let start = utf16;
                text.push_str(value);
                utf16 += utf16_of(value);
                match stops.iter_mut().find(|stop| stop.name == name) {
                    Some(stop) => stop.ranges.push((start, utf16)),
                    None => stops.push(Stop { name, ranges: vec![(start, utf16)] }),
                }
            }
            Segment::End => {
                stops.push(Stop { name: String::new(), ranges: vec![(utf16, utf16)] });
            }
        }
    }
    // $END$ is always the session's last stop (appended when the body lacks
    // one, so Tab past the variables lands the caret at the end).
    if !matches!(stops.last(), Some(stop) if stop.name.is_empty()) {
        stops.push(Stop { name: String::new(), ranges: vec![(utf16, utf16)] });
    }
    Expansion { text, stops }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(id: &'static str, body: &'static str) -> LiveTemplate {
        LiveTemplate { id, abbrev: "x", description: "", body, languages: &["go"] }
    }

    #[test]
    fn parse_handles_vars_end_and_escapes() {
        let got = parse_body("for $I$ := 0; $I$ < $N$; $$x $y$");
        assert_eq!(
            got,
            vec![
                Segment::Text("for ".into()),
                Segment::Var("I".into()),
                Segment::Text(" := 0; ".into()),
                Segment::Var("I".into()),
                Segment::Text(" < ".into()),
                Segment::Var("N".into()),
                Segment::Text("; $x ".into()),
                Segment::Var("y".into()),
            ]
        );
        assert_eq!(parse_body("plain"), vec![Segment::Text("plain".into())]);
        assert_eq!(parse_body("$END$"), vec![Segment::End]);
    }

    #[test]
    fn expand_links_repeats_and_fills_defaults() {
        let t = template("go-fori", "for $I$ := 0; $I$ < $N$; $I$++ {\n\t$END$\n}");
        let got = expand(&t, "    ", "");
        // `$N$` has no default: an empty tab stop (the frontend's model too).
        assert_eq!(got.text, "for i := 0; i < ; i++ {\n    \n}");
        assert_eq!(got.stops.len(), 3);
        // Linked occurrences share one stop; ranges are UTF-16 into the text.
        assert_eq!(got.stops[0].name, "I");
        assert_eq!(got.stops[0].ranges, vec![(4, 5), (12, 13), (18, 19)]);
        assert_eq!(got.stops[1].name, "N");
        assert_eq!(got.stops[1].ranges, vec![(16, 16)]);
        assert_eq!(got.stops[2], Stop { name: String::new(), ranges: vec![(28, 28)] });
    }

    #[test]
    fn expand_indents_with_tab_unit_and_base() {
        let t = template("go-func", "func $NAME$($PARAMS$) $RET$ {\n\t$END$\n}");
        let got = expand(&t, "\t", "\t");
        assert_eq!(got.text, "func ()  {\n\t\t\n\t}", "empty defaults leave both parameter gaps");
        // The placeholder line carries the base indent plus one unit.
        assert_eq!(got.stops.last().unwrap().ranges, vec![(13, 13)]);
    }

    #[test]
    fn expand_appends_end_when_body_lacks_one() {
        let t = template("x", "let $NAME$ = ;");
        let got = expand(&t, "\t", "");
        assert_eq!(got.text, "let  = ;");
        assert_eq!(got.stops.len(), 2, "the $END$ caret appends after the variable");
        assert_eq!(got.stops.last().unwrap().ranges, vec![(8, 8)]);
    }

    #[test]
    fn language_filter_scopes_sets() {
        assert!(for_language("go").all(|t| t.languages.contains(&"go")));
        assert_eq!(for_language("go").count(), 13);
        assert_eq!(for_language("rust").count(), 24);
        assert_eq!(for_language("plaintext").count(), 0);
    }
}
