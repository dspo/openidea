//! The diff view model (M4): a unified, +/- colored read-only view of one
//! file's changes against HEAD, built from `ide_vcs::Repo::line_hunks` (the
//! same hunks the gutter markers use). Pure data + builder here; the
//! workbench renders the overlay. M12 adds the expected-vs-actual builder
//! the test tree's failure face rides ([`from_texts`]).

use ide_vcs::{HunkKind, LineHunk};

/// One rendered diff line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    /// `@@ -old +new @@` header.
    Hunk(String),
    /// A removed line (`- `).
    Del(String),
    /// An added line (`+ `).
    Add(String),
}

/// Unified diff lines from the hunks of `current_text` against HEAD. Context
/// lines are skipped (hunks only) — the MVP diff is about what changed.
pub fn from_hunks(hunks: &[LineHunk], current_text: &str) -> Vec<DiffLine> {
    let lines: Vec<&str> = current_text.split_inclusive('\n').collect();
    let mut out = Vec::new();
    for hunk in hunks {
        out.push(DiffLine::Hunk(format!(
            "@@ -{},{} +{},{} @@{}",
            hunk.old_start + 1,
            hunk.old_lines,
            hunk.start + 1,
            hunk.lines,
            match hunk.kind {
                HunkKind::Added => " (added)",
                HunkKind::Deleted => " (deleted)",
                HunkKind::Modified => "",
            }
        )));
        // The old lines each carry their terminating `\n`; the display form
        // trims it.
        for line in hunk.old_text.split_inclusive('\n') {
            out.push(DiffLine::Del(line.trim_end_matches('\n').to_owned()));
        }
        for line in lines.get(hunk.start as usize..(hunk.start + hunk.lines) as usize).into_iter().flatten() {
            out.push(DiffLine::Add(line.trim_end_matches('\n').to_owned()));
        }
    }
    out
}

/// The expected-vs-actual face (M12's failure diff): an LCS over the two
/// texts' lines renders only what differs (context skipped, like
/// [`from_hunks`]) — two single-line values show as one `-`/`+` pair.
pub fn from_texts(expected: &str, actual: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = actual.lines().collect();
    // LCS table (the texts are the runner's summary faces — small).
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, ai) in a.iter().enumerate() {
        for (j, bj) in b.iter().enumerate() {
            table[i + 1][j + 1] = if ai == bj { table[i][j] + 1 } else { table[i][j + 1].max(table[i + 1][j]) };
        }
    }
    let mut out = Vec::new();
    let mut i = a.len();
    let mut j = b.len();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
            out.push(DiffLine::Add(b[j - 1].to_owned()));
            j -= 1;
        } else {
            out.push(DiffLine::Del(a[i - 1].to_owned()));
            i -= 1;
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_vcs::diff_hunks;

    #[test]
    fn hunks_render_as_unified_lines() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nd\ne\n";
        let lines = from_hunks(&diff_hunks(old, new), new);
        assert_eq!(
            lines,
            vec![
                DiffLine::Hunk("@@ -2,1 +2,1 @@".into()),
                DiffLine::Del("b".into()),
                DiffLine::Add("B".into()),
                DiffLine::Hunk("@@ -5,0 +5,1 @@ (added)".into()),
                DiffLine::Add("e".into()),
            ]
        );
    }

    #[test]
    fn texts_render_the_expected_actual_pair() {
        let lines = from_texts("5", "4");
        assert_eq!(lines, vec![DiffLine::Del("5".into()), DiffLine::Add("4".into())]);
        // Common lines are context (skipped); the changed tail shows alone.
        let lines = from_texts("a\nb\n", "a\nB\n");
        assert_eq!(lines, vec![DiffLine::Del("b".into()), DiffLine::Add("B".into())]);
        assert!(from_texts("", "").is_empty());
    }

    #[test]
    fn deleted_tail_and_untracked_file() {
        // Deletion at the end: `lines == 0` and `start` is the line that now
        // follows the removed block (ide-vcs's convention), so +2,0 for a
        // one-line file; no added lines render.
        let old = "a\nb\n";
        let new = "a\n";
        let lines = from_hunks(&diff_hunks(old, new), new);
        assert_eq!(lines, vec![DiffLine::Hunk("@@ -2,1 +2,0 @@ (deleted)".into()), DiffLine::Del("b".into())]);
        // An untracked file is one all-added hunk (ide-vcs's shape).
        let added = vec![LineHunk {
            kind: HunkKind::Added,
            start: 0,
            lines: 2,
            old_start: 0,
            old_lines: 0,
            old_text: String::new(),
        }];
        let lines = from_hunks(&added, "x\ny\n");
        assert_eq!(
            lines,
            vec![
                DiffLine::Hunk("@@ -1,0 +1,2 @@ (added)".into()),
                DiffLine::Add("x".into()),
                DiffLine::Add("y".into())
            ]
        );
    }
}
