//! The conflict-marker model (M13): the blocks a conflicted file's text
//! carries (git's `<<<<<<<` / `=======` / `>>>>>>>` markers, with the
//! optional diff3 `|||||||` section), plus the navigation and side-
//! application faces the editor's conflict strip drives. Pure and
//! unit-tested; the stage versions stay upstream's (`Repo::merge_versions`,
//! `Repo::stage_and_continue`).

/// One conflict block: the marker lines included (the side application
/// replaces the whole range).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictBlock {
    /// The `<<<<<<< …` line (0-based).
    pub start_line: u32,
    /// One past the `>>>>>>> …` line (0-based).
    pub end_line: u32,
    /// The lines between the start marker and the mid/base marker.
    pub ours: Vec<String>,
    /// The diff3 base section, when the file carries `|||||||`.
    pub base: Vec<String>,
    /// The lines between the mid marker and the end marker.
    pub theirs: Vec<String>,
}

/// The marker prefixes (a `=======` line is seven equals, nothing else).
const START: &str = "<<<<<<<";
const MID: &str = "=======";
const BASE: &str = "|||||||";
const END: &str = ">>>>>>>";

/// Parses the conflict blocks of `text` in order. Unterminated blocks (the
/// file mid-edit) are skipped rather than guessed.
pub fn parse(text: &str) -> Vec<ConflictBlock> {
    let lines: Vec<&str> = text.split_inclusive('\n').map(|l| l.strip_suffix('\n').unwrap_or(l)).collect();
    let mut blocks = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        if !lines[index].starts_with(START) {
            index += 1;
            continue;
        }
        // Ours runs to the bare-mid marker — or, with a diff3 base section,
        // to the `|||||||` marker.
        let mut ours_end = None;
        let mut base_start = None;
        for (offset, line) in lines[index + 1..].iter().enumerate() {
            let at = index + 1 + offset;
            if line.starts_with(BASE) {
                base_start = Some(at);
                ours_end = Some(at);
                break;
            }
            if line.starts_with(MID) && line.trim_end() == MID {
                ours_end = Some(at);
                break;
            }
        }
        let Some(ours_end) = ours_end else { break };
        // With a base section: the base runs to its own bare-mid marker.
        let (base, mid) = match base_start {
            Some(base_at) => {
                let mut mid_at = None;
                for (offset, line) in lines[base_at + 1..].iter().enumerate() {
                    if line.starts_with(MID) && line.trim_end() == MID {
                        mid_at = Some(base_at + 1 + offset);
                        break;
                    }
                }
                let Some(mid_at) = mid_at else { break };
                (lines[base_at + 1..mid_at].iter().map(|l| l.to_string()).collect::<Vec<_>>(), mid_at)
            }
            None => (Vec::new(), ours_end),
        };
        // Theirs runs from the mid marker to the end marker.
        let mut end = None;
        for (offset, line) in lines[mid + 1..].iter().enumerate() {
            if line.starts_with(END) {
                end = Some(mid + 1 + offset + 1);
                break;
            }
        }
        let Some(end) = end else { break };
        blocks.push(ConflictBlock {
            start_line: index as u32,
            end_line: end as u32,
            ours: lines[index + 1..ours_end].iter().map(|l| l.to_string()).collect(),
            base,
            theirs: lines[mid + 1..end - 1].iter().map(|l| l.to_string()).collect(),
        });
        index = end;
    }
    blocks
}

/// The block covering `line` (a marker or body line), if any.
pub fn block_containing(blocks: &[ConflictBlock], line: u32) -> Option<usize> {
    blocks.iter().position(|block| line >= block.start_line && line < block.end_line)
}

/// The first block starting after `line` (the "next conflict" walk: a caret
/// already inside a block moves past it; one above every block lands in the
/// first).
pub fn next_block(blocks: &[ConflictBlock], line: u32) -> Option<usize> {
    blocks.iter().position(|block| block.start_line > line)
}

/// The last block the caret is not inside, starting before `line` (the
/// "previous conflict" walk: a caret inside a block moves out of it).
pub fn prev_block(blocks: &[ConflictBlock], line: u32) -> Option<usize> {
    let inside = block_containing(blocks, line);
    (0..blocks.len()).rev().find(|index| Some(*index) != inside && blocks[*index].start_line < line)
}

/// Which side a resolution keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

/// The text with the block's range replaced by the kept side's lines.
pub fn apply_side(text: &str, block: &ConflictBlock, side: Side) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = block.start_line as usize;
    let end = (block.end_line as usize).min(lines.len());
    let kept: Vec<String> = match side {
        Side::Ours => block.ours.clone(),
        Side::Theirs => block.theirs.clone(),
    };
    let mut out = String::new();
    out.push_str(&lines[..start].concat());
    for line in &kept {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&lines[end..].concat());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE: &str = "top\n\
<<<<<<< HEAD\nours one\nours two\n=======\ntheirs one\n>>>>>>> feature\nbottom\n";

    #[test]
    fn parse_reads_the_three_way_markers() {
        let blocks = parse(MERGE);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(block.start_line, 1);
        assert_eq!(block.end_line, 7);
        assert_eq!(block.ours, vec!["ours one", "ours two"]);
        assert_eq!(block.theirs, vec!["theirs one"]);
        assert!(block.base.is_empty());
    }

    #[test]
    fn parse_reads_the_diff3_base_section() {
        let text = "a\n<<<<<<< HEAD\nours\n||||||| merged-common-ancestor\nbase\n=======\ntheirs\n>>>>>>> x\nb\n";
        let blocks = parse(text);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(block.base, vec!["base"]);
        assert_eq!(block.ours, vec!["ours"]);
        assert_eq!(block.theirs, vec!["theirs"]);
        assert_eq!(block.end_line, 8);
    }

    #[test]
    fn unterminated_and_seven_equals_lookalikes_are_handled() {
        // An unterminated block (mid-edit) is skipped, not guessed.
        assert!(parse("<<<<<<< HEAD\nours only\n").is_empty());
        // A `=======` line inside a side's body is content, not a marker —
        // only a bare seven-equals line splits.
        let text = "<<<<<<< HEAD\nrule =======\n=======\ntheirs\n>>>>>>> x\n";
        let blocks = parse(text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours, vec!["rule ======="]);
    }

    #[test]
    fn navigation_walks_forward_and_backward() {
        let text = "<<<<<<< HEAD\none\n=======\nfirst theirs\n>>>>>>> x\nmid\n<<<<<<< HEAD\nsecond ours\n=======\nsecond theirs\n>>>>>>> y\ntail\n";
        let blocks = parse(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].start_line, 0);
        assert_eq!(blocks[0].end_line, 5);
        assert_eq!(blocks[1].start_line, 6);
        assert_eq!(blocks[1].end_line, 11);
        // Forward: a caret above every conflict lands in the first; inside
        // one, it moves past it.
        assert_eq!(next_block(&blocks, 0), Some(1));
        assert_eq!(next_block(&blocks, 3), Some(1));
        assert_eq!(next_block(&blocks, 6), None);
        // Backward: from the tail the last block; inside one, the earlier.
        assert_eq!(prev_block(&blocks, 11), Some(1));
        assert_eq!(prev_block(&blocks, 9), Some(0));
        assert_eq!(prev_block(&blocks, 0), None);
        // A body line finds its own block.
        assert_eq!(block_containing(&blocks, 3), Some(0));
        assert_eq!(block_containing(&blocks, 9), Some(1));
    }

    #[test]
    fn apply_side_replaces_the_whole_block() {
        let blocks = parse(MERGE);
        let ours = apply_side(MERGE, &blocks[0], Side::Ours);
        assert_eq!(ours, "top\nours one\nours two\nbottom\n");
        let theirs = apply_side(MERGE, &blocks[0], Side::Theirs);
        assert_eq!(theirs, "top\ntheirs one\nbottom\n");
        // Resolving the last block leaves no markers behind.
        assert!(parse(&theirs).is_empty());
    }
}
