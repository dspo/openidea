//! The backend host: one open project folder assembled in-process from the
//! same UI-agnostic crates the Tauri host (`src-tauri`) and the headless host
//! (`crates/ide-host`) drive. `crates/ide-host/src/project.rs` is the full
//! sample of this pattern — index, language servers, git, deploy — of which
//! this host needs documents + VFS + the M3 language-server slice:
//!
//! - `ide-workspace::workspace::Workspace::load` — the "open project" gate
//!   (a damaged `.openidea` manifest is rejected, like `Host::open_project`).
//! - `ide-workspace::Documents` — open / edit / save / dirty, external-change
//!   reload (`on_disk_changed`), and the `DocDirty` / `DocPatched` /
//!   `Notify` events it emits through the sink. Language detection goes
//!   through the `languages/*.toml` registry (M1 carried an extension map).
//! - `ide-vfs` — directory listing, and the debounced `Watcher` whose batch
//!   callback feeds `Documents::on_disk_changed`, exactly as `ide-host`
//!   wires it (file content goes through `Documents`, never a bare read).
//! - `ide-lsp::LspManager` — language servers per `(server, root)`, started
//!   lazily when a document of a matching language opens. The
//!   [`LspObserver`] feeds `did_open` / `did_change` / `did_save` /
//!   `did_close` from `Documents` (version-ordered, incremental); feature
//!   requests (`completion`, `goto`, `rename`, …) are async and must run on
//!   the app-owned tokio runtime the manager captured at construction —
//!   spawn onto [`Host::runtime`] and await the result off the UI thread.
//!
//! All fallible operations return `IdeResult` / `IdeError` per the repo
//! convention; the UI never sees a raw `std::io::Error`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ide_core::{ChangeSet, DocId, IdeError, IdeResult, RunId, SharedSink};
use ide_index::ProjectIndex;
use ide_language::{LanguageRegistry, Syntax};
use ide_lsp::LspManager;
use ide_run::{OutputChunk, ProcessManager, RunConfig, RunInfo, StartOptions};
use ide_text::AppliedChange;
use ide_vcs::{FileStatus, Repo};
use ide_vfs::{FileEntry, Watcher};
use ide_workspace::documents::DocumentObserver;
use ide_workspace::{DocumentInfo, Documents};

/// M1's exclude set: the IDE defaults that matter for a Rust/JS repo. The
/// settings-store-driven list (`settings.file_excludes()`) arrives with the
/// settings UI in a later milestone.
const DEFAULT_EXCLUDES: &[&str] = &["**/.git", "**/target", "**/node_modules", "**/dist", ".DS_Store"];

fn excludes() -> Vec<String> {
    DEFAULT_EXCLUDES.iter().map(|s| (*s).to_owned()).collect()
}

/// One open project folder: documents, the watcher that keeps them and the
/// file tree in sync, and the language-server manager fed by the same
/// documents.
pub struct Host {
    /// Canonicalized project root.
    pub root: PathBuf,
    /// Display name (the folder's own name).
    pub name: String,
    documents: Arc<Documents>,
    /// The bus sink, for UI-originated notifications from the editor (a
    /// failed navigation surfaces as a status-bar message).
    pub sink: SharedSink,
    /// Language servers (M3): started per document language, observed below.
    pub lsp: Arc<LspManager>,
    /// The `languages/*.toml` registry (M5): the editor reads each document
    /// language's `indent` policy from here (editorconfig still wins).
    pub languages: Arc<LanguageRegistry>,
    /// The project index (M3): fuzzy file / symbol search behind Go to File
    /// and Search Everywhere, kept fresh by the same watcher as the documents.
    pub index: Arc<ProjectIndex>,
    /// Run configurations and their processes (M4): the same engine the Tauri
    /// host's Run tool window drives. Output streams through a plain callback;
    /// exits arrive as `IdeEvent::RunExited` on the sink.
    pub run: ProcessManager,
    /// The project's git repository (M4), `None` outside a repo. Reads are
    /// gitoxide in-process and re-check on-disk state per call, so the handle
    /// never goes stale.
    pub repo: Option<Arc<Repo>>,
    /// The app-owned tokio runtime's handle: LSP feature requests
    /// (`completion`, `goto`, …) must spawn onto it, and it drives the
    /// servers' IO. Kept alongside the host so callers share one runtime.
    pub runtime: tokio::runtime::Handle,
    /// Kept alive for the project's lifetime; dropping it stops the watch.
    _watcher: Watcher,
}

impl Host {
    /// Opens `root` as a project: workspace-manifest gate, documents, watcher,
    /// language servers. `sink` receives every `IdeEvent` the backend
    /// produces. Must be called inside the app runtime's `enter()` context so
    /// the LSP manager captures it.
    pub fn open(root: &Path, sink: SharedSink, runtime: tokio::runtime::Handle) -> IdeResult<Self> {
        let root = dunce::canonicalize(root)?;
        if !root.is_dir() {
            return Err(IdeError::not_found(format!("{} is not a folder", root.display())));
        }
        // A damaged workspace manifest blocks the open (same as ide-host); a
        // plain folder without one is a valid project.
        ide_workspace::workspace::Workspace::load(&root)?;
        let languages = Arc::new(LanguageRegistry::load(None)?);
        let detect = {
            let languages = languages.clone();
            Box::new(move |path: &Path, first_line: Option<&str>| languages.detect(path, first_line))
        };
        let documents = Arc::new(Documents::new(sink.clone(), detect));
        let lsp = LspManager::new(root.clone(), languages.clone(), sink.clone());
        let index = {
            // Persistent index cache keyed per project (the FNV-1a convention
            // ide-host uses for its caches), under the platform cache dir.
            let cache = dirs::cache_dir()
                .map(|dir| dir.join("openidea-gui").join("index").join(project_key(&root)))
                .unwrap_or_else(|| root.join(".openidea-index"));
            ProjectIndex::open(
                &root,
                &cache,
                Vec::new(),
                excludes(),
                languages.clone(),
                Arc::new(Syntax::new()),
                sink.clone(),
            )?
        };
        documents.add_observer(Arc::new(LspObserver(lsp.clone())));
        // Server-initiated edits (`workspace/applyEdit`, the answer channel of
        // gopls commands) land through Documents like every other workspace
        // edit, so open tabs reconcile via DocPatched (the Tauri host's hook).
        let hook_documents = documents.clone();
        lsp.set_apply_edit_hook(Box::new(move |edit| hook_documents.apply_workspace_edit(&edit).is_ok()));
        let watcher_index = index.clone();
        let watcher = {
            let documents = documents.clone();
            Watcher::start(
                &root,
                excludes(),
                sink.clone(),
                Box::new(move |batch| {
                    // External changes reach open documents through the same
                    // door ide-host uses: clean buffers reload (the UI learns
                    // via `DocPatched`), dirty ones warn (`Notify`). The index
                    // hears the same batch so its search stays current.
                    let changed: Vec<PathBuf> = batch.created.iter().chain(&batch.modified).cloned().collect();
                    documents.on_disk_changed(&changed);
                    watcher_index.update(&changed, &batch.removed);
                }),
            )?
        };
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let repo = Repo::discover(&root).ok().flatten().map(Arc::new);
        eprintln!("[vcs] {} repository at {}", if repo.is_some() { "found" } else { "no" }, root.display());
        Ok(Self {
            root,
            name,
            documents,
            sink,
            lsp,
            languages,
            index,
            run: ProcessManager::new(),
            repo,
            runtime,
            _watcher: watcher,
        })
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

    /// Detaches the document: external changes stop reaching the UI tab
    /// (servers see `didClose` through the observer).
    pub fn close_document(&self, doc: DocId) {
        self.documents.close(doc)
    }

    /// Applies a server-produced workspace edit (rename) through Documents:
    /// open tabs reconcile via `DocPatched { workspace_edit: true }`.
    pub fn apply_workspace_edit(&self, edit: &ide_core::WorkspaceEdit) -> IdeResult<()> {
        self.documents.apply_workspace_edit(edit)
    }

    /// Best-effort graceful server shutdown (`shutdown` request + `exit`),
    /// spawned onto the runtime; peers are killed regardless at process end.
    pub fn shutdown_lsp(&self) {
        let lsp = self.lsp.clone();
        self.runtime.spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), lsp.shutdown_all()).await;
        });
    }

    // -- run (M4) -----------------------------------------------------------

    /// The run configurations to offer: user-defined ones (`.openidea/
    /// run.toml`) first, then the detector's (package.json scripts, Cargo
    /// targets, …) that user config with the same id does not shadow.
    pub fn run_configs(&self) -> IdeResult<Vec<RunConfig>> {
        let mut configs = ide_run::load_user_configs(&self.root)?;
        for detected in ide_run::detect(&self.root) {
            if !configs.iter().any(|c| c.id == detected.id) {
                configs.push(detected);
            }
        }
        Ok(configs)
    }

    /// Starts `config`: spawning happens on this (background) thread —
    /// `ProcessManager::start` forks the process and spawns its own reader /
    /// waiter threads; output streams through `on_output`, and the exit lands
    /// on `sink` as `IdeEvent::RunExited`.
    pub fn start_run(
        &self,
        config: &RunConfig,
        on_output: Box<dyn Fn(OutputChunk) + Send + Sync>,
    ) -> IdeResult<RunInfo> {
        self.run.start(&self.root, config, &StartOptions::default(), self.sink.clone(), on_output)
    }

    /// Graceful stop (SIGINT to the process group, SIGKILL after a grace
    /// period) — ide-run's own cancellation path.
    pub fn stop_run(&self, id: RunId) -> IdeResult<()> {
        self.run.stop(id)
    }

    // -- git (M4) -----------------------------------------------------------

    /// Working-tree status plus the current branch (reads are gitoxide;
    /// call on a background thread).
    pub fn git_status(&self) -> IdeResult<(Vec<FileStatus>, Option<String>)> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        let statuses = repo.status()?;
        let branch = repo.head()?.0;
        Ok((statuses, branch))
    }

    /// Line hunks of `current_text` against HEAD (UTF-8 like every M0–M3
    /// document; call on a background thread).
    pub fn git_line_hunks(&self, path: &Path, current_text: &str) -> IdeResult<Vec<ide_vcs::LineHunk>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.line_hunks(path, current_text, false, encoding_rs::UTF_8)
    }

    /// Stages `paths` (git add semantics) — the commit flow's first half.
    pub fn git_stage(&self, paths: &[PathBuf]) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.stage(paths)
    }

    /// Unstages `paths` — the commit dialog's unchecked files leave the index.
    pub fn git_unstage(&self, paths: &[PathBuf]) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.unstage(paths)
    }

    /// Commits the index; returns the new HEAD commit id.
    pub fn git_commit(&self, message: &str) -> IdeResult<String> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.commit(message, false, false)
    }
}

/// Stable per-project cache-directory name (FNV-1a of the root), the same
/// convention `ide-host` uses for its per-project caches.
fn project_key(root: &Path) -> String {
    let s = root.to_string_lossy();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Feeds the document life cycle to the language servers, exactly like
/// `ide-host`'s observer of the same name. Large documents are skipped
/// (servers choke on them, like IDEA). The callbacks run synchronously in
/// version order while the document is locked; `did_*` here are the
/// notification paths (no runtime context needed).
struct LspObserver(Arc<LspManager>);

impl DocumentObserver for LspObserver {
    fn opened(&self, info: &DocumentInfo) {
        eprintln!("[lsp] opened {} ({}, large={})", info.path.display(), info.language.as_str(), info.large);
        if info.large {
            return;
        }
        match &info.uri {
            Some(uri) => self.0.did_open_virtual(info.id, uri, &info.language, info.version, &info.text),
            None => self.0.did_open(info.id, &info.path, &info.language, info.version, &info.text),
        }
    }

    fn changed(
        &self,
        doc: DocId,
        _path: &Path,
        version: u32,
        changes: &[AppliedChange],
        full_text: &dyn Fn() -> String,
    ) {
        self.0.did_change(doc, version, changes, full_text);
    }

    fn saved(&self, doc: DocId, _path: &Path, text: &str) {
        self.0.did_save(doc, text);
    }

    fn closed(&self, doc: DocId, _path: &Path) {
        self.0.did_close(doc);
    }
}
