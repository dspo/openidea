//! The smallest markdown renderer the hover popup needs (gpui-component 0.8
//! ships no markdown widget): fenced code blocks, headings, list items,
//! paragraphs, and inline `code` / **bold** runs. Everything else renders as
//! plain text. gpui elements are built by the caller from [`block`] /
//! [`inline`]; this module only parses.

/// One inline run: text plus the style it paints with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Plain(String),
    Code(String),
    Bold(String),
}

/// One block of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A fenced code block's lines (the fence and language tag stripped).
    Code(Vec<String>),
    /// A heading with its level (1–6).
    Heading(u8, String),
    /// A list item (the marker stripped).
    ListItem(String),
    /// Any other non-empty line.
    Paragraph(String),
}

/// Splits the document into blocks. Blank lines separate paragraphs; a
/// ` ``` ` fence swallows everything until the closing fence (or the end).
pub fn block(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut in_code = false;
    let mut code: Vec<String> = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                out.push(Block::Code(std::mem::take(&mut code)));
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        if in_code {
            code.push(line.to_owned());
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let heading = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&heading) && trimmed[heading..].starts_with(' ') && heading <= trimmed.len() {
            out.push(Block::Heading(heading as u8, trimmed[heading..].trim().to_owned()));
            continue;
        }
        if let Some(item) =
            trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ "))
        {
            out.push(Block::ListItem(item.to_owned()));
            continue;
        }
        // "1. " style markers.
        let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && digits + 2 <= trimmed.len() && trimmed[digits..].starts_with(". ") {
            out.push(Block::ListItem(trimmed[digits + 2..].to_owned()));
            continue;
        }
        out.push(Block::Paragraph(trimmed.to_owned()));
    }
    if in_code && !code.is_empty() {
        out.push(Block::Code(code));
    }
    out
}

/// Splits one line into styled runs: `` `code` `` and `**bold**` spans.
/// Unterminated markers stay literal text.
pub fn inline(src: &str) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    // Appends a run, merging plain text into the previous plain run.
    let push = |out: &mut Vec<Inline>, kind: Inline| {
        if let (Some(Inline::Plain(existing)), Inline::Plain(extra)) = (out.last_mut(), &kind) {
            existing.push_str(extra);
            return;
        }
        out.push(kind);
    };
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    while i < bytes.len() {
        let rest = &src[i..];
        // (run, where the marker started, where scanning resumes).
        let mut styled: Option<(Inline, usize, usize)> = None;
        if rest.starts_with('`')
            && let Some(end_rel) = src[i + 1..].find('`')
        {
            let content = &src[i + 1..i + 1 + end_rel];
            if !content.is_empty() {
                styled = Some((Inline::Code(content.to_owned()), i, i + end_rel + 2));
            }
        } else if rest.starts_with("**")
            && let Some(end_rel) = src[i + 2..].find("**")
        {
            let content = &src[i + 2..i + 2 + end_rel];
            if !content.is_empty() {
                styled = Some((Inline::Bold(content.to_owned()), i, i + end_rel + 4));
            }
        }
        match styled {
            Some((run, run_start, next_i)) => {
                if run_start > text_start {
                    push(&mut out, Inline::Plain(src[text_start..run_start].to_owned()));
                }
                push(&mut out, run);
                i = next_i;
                text_start = i;
            }
            None => i += src[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if text_start < src.len() {
        push(&mut out, Inline::Plain(src[text_start..].to_owned()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_split_fences_headings_and_lists() {
        let src = "# Title\n\nSome *text*.\n\n```go\nfmt.Println(1)\n```\n\n- alpha\n1. beta\n## Sub";
        let got = block(src);
        assert_eq!(
            got,
            vec![
                Block::Heading(1, "Title".into()),
                Block::Paragraph("Some *text*.".into()),
                Block::Code(vec!["fmt.Println(1)".into()]),
                Block::ListItem("alpha".into()),
                Block::ListItem("beta".into()),
                Block::Heading(2, "Sub".into()),
            ]
        );
    }

    #[test]
    fn unterminated_fence_swallows_the_tail() {
        let got = block("text\n```rust\nlet x = 1;\nlet y = 2;");
        assert_eq!(
            got,
            vec![Block::Paragraph("text".into()), Block::Code(vec!["let x = 1;".into(), "let y = 2;".into()])]
        );
    }

    #[test]
    fn inline_runs_split_code_and_bold() {
        let got = inline("a `func (x int)` b**old** tail `x`");
        assert_eq!(
            got,
            vec![
                Inline::Plain("a ".into()),
                Inline::Code("func (x int)".into()),
                Inline::Plain(" b".into()),
                Inline::Bold("old".into()),
                Inline::Plain(" tail ".into()),
                Inline::Code("x".into()),
            ]
        );
        // Unterminated markers stay literal.
        assert_eq!(inline("a `bc"), vec![Inline::Plain("a `bc".into())]);
        assert_eq!(inline("plain"), vec![Inline::Plain("plain".into())]);
        assert!(inline("").is_empty());
    }
}
