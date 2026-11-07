//! The duplicate-code inspection (M12): `ide-analysis`'s token-based clone
//! detection over the project's source files, run on the background
//! executor and rendered in the Duplicates tool window. The file walk rides
//! the host's VFS listing (the excludes apply per level); the loader
//! prefers an open tab's dirty buffer (the find-in-files face).

use std::path::{Path, PathBuf};

use ide_analysis::{DuplicateGroup, DuplicatesReport, SourceFile};

/// One member row of the window (the report's fragment, project-relative).
#[derive(Clone, Debug)]
pub struct MemberRow {
    pub path: PathBuf,
    pub rel_path: String,
    pub line: u32,
    pub col: u32,
    /// The fragment's source (capped by the crate at 32 KiB).
    pub text: String,
}

/// One group of the window's tree: the fragments that match each other.
#[derive(Clone, Debug)]
pub struct DuplicateGroupRow {
    pub tokens: u32,
    pub lines: u32,
    pub language: String,
    pub members: Vec<MemberRow>,
}

/// The window's face: the groups plus the run's shape.
#[derive(Clone, Debug, Default)]
pub struct DuplicatesFace {
    pub groups: Vec<DuplicateGroupRow>,
    pub files: u32,
    pub truncated: bool,
}

impl DuplicatesFace {
    /// The report's project-relative face (fragments carry IpcPaths).
    pub fn from_report(report: DuplicatesReport) -> Self {
        let groups = report
            .groups
            .into_iter()
            .map(|group: DuplicateGroup| DuplicateGroupRow {
                tokens: group.tokens,
                lines: group.lines,
                language: group.language.as_str().to_owned(),
                members: group
                    .fragments
                    .into_iter()
                    .map(|f| MemberRow {
                        path: f.path.0.clone(),
                        rel_path: f.rel_path.clone(),
                        line: f.range.start.line,
                        col: f.range.start.col,
                        text: f.text,
                    })
                    .collect(),
            })
            .collect();
        Self { groups, files: report.files, truncated: report.truncated }
    }
}

/// The project's source files: a depth-first walk through the host's VFS
/// listing (the excludes skip `target`/`node_modules`/… at every level).
pub fn collect_files(host: &crate::host::Host) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![host.root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = host.read_dir(&dir) else { continue };
        for entry in entries {
            // read_dir flags (not filters) the exclude matches; the walk
            // skips them the way the file tree does.
            if entry.ignored || entry.excluded {
                continue;
            }
            let path = entry.path.0;
            match entry.kind {
                ide_vfs::EntryKind::Dir => stack.push(path),
                _ => out.push(path),
            }
        }
    }
    out.sort();
    out
}

/// The report's source-file list: paths joined with their project-relative
/// faces (`ide-analysis` wants both).
pub fn source_files(root: &Path, paths: &[PathBuf]) -> Vec<SourceFile> {
    paths
        .iter()
        .map(|path| SourceFile {
            path: path.clone(),
            rel_path: path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The face maps the report into window rows (the parse face the render
    /// reads; the real report comes from ide-analysis's own tests).
    #[test]
    fn the_face_maps_the_report_groups() {
        let report = DuplicatesReport::default();
        let face = DuplicatesFace::from_report(report);
        assert!(face.groups.is_empty());
    }
}
