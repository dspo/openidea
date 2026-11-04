//! File tree state for the workbench's left dock. A lazily-loaded directory
//! model over `ide-vfs::read_dir` (ignore rules applied by the backend), kept
//! fresh by the workbench from `FsChanged` events. No UI code here — the
//! workbench renders `rows()` and drives the mutation methods.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ide_core::IdeResult;
use ide_vfs::{EntryKind, FileEntry};

use crate::host::Host;

/// One flattened, indented row of the visible tree.
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub depth: usize,
    pub expanded: bool,
}

pub struct FileTree {
    expanded: HashSet<PathBuf>,
    /// `read_dir` results per listed directory, keyed by its canonical path.
    cache: HashMap<PathBuf, Vec<FileEntry>>,
    /// Canonical path of the highlighted row.
    pub selected: Option<PathBuf>,
    /// The project root, the walk's entry point.
    root: Option<PathBuf>,
}

impl FileTree {
    pub fn new() -> Self {
        Self { expanded: HashSet::new(), cache: HashMap::new(), selected: None, root: None }
    }

    /// Seeds (or reseeds) the tree for `root`, expanded.
    pub fn open(&mut self, host: &Host) -> IdeResult<()> {
        self.cache.clear();
        self.expanded.clear();
        self.selected = None;
        self.root = Some(host.root.clone());
        self.expanded.insert(host.root.clone());
        self.list(host, &host.root)?;
        Ok(())
    }

    /// Expands / collapses a directory (entries are read through the host on
    /// first expand).
    pub fn toggle(&mut self, host: &Host, dir: &Path) {
        if self.expanded.remove(dir) {
            return;
        }
        self.expanded.insert(dir.to_path_buf());
        if !self.cache.contains_key(dir) {
            let _ = self.list(host, dir);
        }
    }

    /// Re-lists the root and every expanded directory after `FsChanged`.
    pub fn refresh(&mut self, host: &Host) {
        let Some(root) = self.root.clone() else { return };
        let _ = self.list(host, &root);
        for dir in self.expanded.clone() {
            if dir != root {
                let _ = self.list(host, &dir);
            }
        }
    }

    fn list(&mut self, host: &Host, dir: &Path) -> IdeResult<()> {
        let entries = host.read_dir(dir)?;
        self.cache.insert(dir.to_path_buf(), entries);
        Ok(())
    }

    pub fn select(&mut self, path: PathBuf) {
        self.selected = Some(path);
    }

    /// The visible rows, depth-first: directories first, then files, both in
    /// natural order (the order `ide_vfs::natural_cmp` gives digits).
    pub fn rows(&self) -> Vec<Row> {
        let Some(root) = &self.root else { return Vec::new() };
        let mut rows = Vec::new();
        self.walk(root, 0, &mut rows);
        rows
    }

    fn walk(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let Some(entries) = self.cache.get(dir) else { return };
        let mut entries = entries.clone();
        entries.sort_by(|a, b| {
            let dir_of = |e: &FileEntry| e.kind == EntryKind::Dir;
            dir_of(b).cmp(&dir_of(a)).then_with(|| ide_vfs::natural_cmp(&a.name, &b.name))
        });
        for entry in entries {
            let is_dir = entry.kind == EntryKind::Dir;
            let expanded = is_dir && self.expanded.contains(entry.path.0.as_path());
            rows.push(Row { path: entry.path.0.clone(), name: entry.name.clone(), kind: entry.kind, depth, expanded });
            if expanded {
                self.walk(entry.path.0.as_path(), depth + 1, rows);
            }
        }
    }

    /// Moves the selection over the visible rows (keyboard navigation); the
    /// moved-to row when the tree has rows, `None` otherwise.
    pub fn move_selection(&mut self, delta: isize) -> Option<PathBuf> {
        let rows = self.rows();
        if rows.is_empty() {
            return None;
        }
        let current =
            self.selected.as_ref().and_then(|selected| rows.iter().position(|row| &row.path == selected)).unwrap_or(0);
        let next = (current as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.selected = Some(rows[next].path.clone());
        self.selected.clone()
    }
}
