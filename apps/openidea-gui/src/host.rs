//! The backend host: one open project folder assembled in-process from the
//! same UI-agnostic crates the Tauri host (`src-tauri`) and the headless host
//! (`crates/ide-host`) drive. `crates/ide-host/src/project.rs` is the full
//! sample of this pattern — index, language servers, git, deploy — of which
//! M1 needs the document + VFS slice only:
//!
//! - `ide-workspace::workspace::Workspace::load` — the "open project" gate
//!   (a damaged `.openidea` manifest is rejected, like `Host::open_project`).
//! - `ide-workspace::Documents` — open / edit / save / dirty, external-change
//!   reload (`on_disk_changed`), and the `DocDirty` / `DocPatched` /
//!   `Notify` events it emits through the sink.
//! - `ide-vfs` — directory listing, and the debounced `Watcher` whose batch
//!   callback feeds `Documents::on_disk_changed`, exactly as `ide-host`
//!   wires it (file content goes through `Documents`, never a bare read).
//!
//! All fallible operations return `IdeResult` / `IdeError` per the repo
//! convention; the UI never sees a raw `std::io::Error`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ide_core::{ChangeSet, DocId, IdeError, IdeResult, LanguageId, SharedSink};
use ide_vfs::{FileEntry, Watcher};
use ide_workspace::{DocumentInfo, Documents};

/// M1's exclude set: the IDE defaults that matter for a Rust/JS repo. The
/// settings-store-driven list (`settings.file_excludes()`) arrives with the
/// settings UI in a later milestone.
const DEFAULT_EXCLUDES: &[&str] = &["**/.git", "**/target", "**/node_modules", "**/dist", ".DS_Store"];

fn excludes() -> Vec<String> {
    DEFAULT_EXCLUDES.iter().map(|s| (*s).to_owned()).collect()
}

/// One open project folder: documents plus the watcher that keeps them and
/// the file tree in sync.
pub struct Host {
    /// Canonicalized project root.
    pub root: PathBuf,
    /// Display name (the folder's own name).
    pub name: String,
    documents: Arc<Documents>,
    /// Kept alive for the project's lifetime; dropping it stops the watch.
    _watcher: Watcher,
}

impl Host {
    /// Opens `root` as a project: workspace-manifest gate, documents, watcher.
    /// `sink` receives every `IdeEvent` the backend produces.
    pub fn open(root: &Path, sink: SharedSink) -> IdeResult<Self> {
        let root = dunce::canonicalize(root)?;
        if !root.is_dir() {
            return Err(IdeError::not_found(format!("{} is not a folder", root.display())));
        }
        // A damaged workspace manifest blocks the open (same as ide-host); a
        // plain folder without one is a valid project.
        ide_workspace::workspace::Workspace::load(&root)?;
        let documents = Arc::new(Documents::new(sink.clone(), Box::new(detect_language)));
        let watcher = {
            let documents = documents.clone();
            Watcher::start(
                &root,
                excludes(),
                sink,
                Box::new(move |batch| {
                    // External changes reach open documents through the same
                    // door ide-host uses: clean buffers reload (the UI learns
                    // via `DocPatched`), dirty ones warn (`Notify`).
                    let changed: Vec<PathBuf> = batch.created.iter().chain(&batch.modified).cloned().collect();
                    documents.on_disk_changed(&changed);
                }),
            )?
        };
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Self { root, name, documents, _watcher: watcher })
    }

    /// Lists `dir` (VFS entries with ignore flags applied).
    pub fn read_dir(&self, dir: &Path) -> IdeResult<Vec<FileEntry>> {
        ide_vfs::read_dir(dir, &excludes())
    }

    /// Opens (or re-opens) `path` as a document; the UI seeds its buffer from
    /// the returned text.
    pub fn open_document(&self, path: &Path) -> IdeResult<DocumentInfo> {
        self.documents.open(path)
    }

    /// Applies a UI-originated change set (UTF-16 offsets of the pre-edit
    /// text) to the document model. The dirty flag follows (`DocDirty` fires
    /// through the sink); returns the new version. M2's editor mirrors the
    /// document version-for-version, so edits are incremental — no full-text
    /// pushes.
    pub fn edit(&self, doc: DocId, base_version: u32, changes: ChangeSet) -> IdeResult<u32> {
        self.documents.edit(doc, base_version, changes)
    }

    /// Full snapshot (text, version, flags) for out-of-sync recovery.
    pub fn snapshot(&self, doc: DocId) -> IdeResult<DocumentInfo> {
        self.documents.snapshot(doc)
    }

    /// Writes the document to disk through the VFS (original encoding, BOM
    /// and line ending); a clean document is not rewritten.
    pub fn save_document(&self, doc: DocId) -> IdeResult<()> {
        self.documents.save(doc)
    }

    /// Detaches the document: external changes stop reaching the UI tab.
    pub fn close_document(&self, doc: DocId) {
        self.documents.close(doc)
    }
}

/// M1 stand-in for `LanguageRegistry::detect` (M3 territory): a small
/// extension map for the languages this repo itself uses, everything else is
/// plain text — enough for `Documents` to name a language in its events.
fn detect_language(path: &Path, first_line: Option<&str>) -> LanguageId {
    const PLAIN: &str = "plaintext";
    let by_extension = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
    let id = match by_extension.as_deref() {
        Some("rs") => "rust",
        Some("ts" | "tsx" | "mts" | "cts") => "typescript",
        Some("js" | "jsx" | "mjs" | "cjs") => "javascript",
        Some("py") => "python",
        Some("go") => "go",
        Some("java" | "kt" | "kts") => "java",
        Some("c" | "h" | "cpp" | "hpp" | "cc") => "cpp",
        Some("css" | "scss") => "css",
        Some("html" | "htm") => "html",
        Some("json" | "jsonc") => "json",
        Some("toml") => "toml",
        Some("yaml" | "yml") => "yaml",
        Some("md" | "markdown") => "markdown",
        Some("sh" | "bash" | "zsh") => "shell",
        Some("xml" | "svg") => "xml",
        // Extension-less scripts: the hashbang decides.
        _ if path.extension().is_none() => by_hashbang(first_line),
        _ => PLAIN,
    };
    LanguageId::new(id)
}

fn by_hashbang(first_line: Option<&str>) -> &'static str {
    const PLAIN: &str = "plaintext";
    let Some(rest) = first_line.and_then(|l| l.strip_prefix("#!")) else {
        return PLAIN;
    };
    if rest.contains("python") {
        "python"
    } else if rest.contains("node") {
        "javascript"
    } else {
        "shell"
    }
}
