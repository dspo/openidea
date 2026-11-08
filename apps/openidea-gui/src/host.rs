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
use ide_language::{LanguageRegistry, LanguageServerConfig, Syntax};
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

/// The app's LSP server options overlay (M9's fork move): the gopls opt-ins
/// this fork used to carry as two data lines in `languages/go.toml`. gopls
/// ships semantic tokens disabled (`semanticTokens` defaults to false and the
/// provider answers empty without it — gopls docs / settings.md); inlay hints
/// are the same story, the provider advertises but answers empty until the
/// wanted `hints` kinds turn on (M6 probes: 5 hints on main.go for
/// assignVariableTypes + parameterNames). They ride the LspManager's config
/// hook now, so `languages/` stays at zero diff with upstream.
const GOPLS_INITIALIZATION_OPTIONS: &str = "semanticTokens = true\n[hints]\nassignVariableTypes = true\nparameterNames = true\nfunctionTypeParameters = true\n";

/// The vtsls / tsserver inlay-hint opt-in (M14a): the provider advertises but
/// answers null until the wanted kinds turn on — the gopls story repeating —
/// and upstream's `typescript.toml`/`javascript.toml` carry none. The shape
/// is VSCode's `typescript.inlayHints.*` settings tree (M14a stdio probes:
/// the preferences-form `includeInlay*` keys are ignored by vtsls; this form
/// answers). The kinds mirror the Go baseline's default-on faces: types,
/// parameter names, return types, enum values.
const TS_INLAY_HINTS: &str = r#"
[typescript.inlayHints.parameterTypes]
enabled = true
[typescript.inlayHints.parameterNames]
enabled = "all"
suppressWhenArgumentMatchesName = false
[typescript.inlayHints.functionLikeReturnTypes]
enabled = true
[typescript.inlayHints.variableTypes]
enabled = true
suppressWhenTypeMatchesName = false
[typescript.inlayHints.propertyDeclarationTypes]
enabled = true
[typescript.inlayHints.enumMemberValues]
enabled = true
[javascript.inlayHints.parameterTypes]
enabled = true
[javascript.inlayHints.parameterNames]
enabled = "all"
suppressWhenArgumentMatchesName = false
[javascript.inlayHints.functionLikeReturnTypes]
enabled = true
[javascript.inlayHints.variableTypes]
enabled = true
suppressWhenTypeMatchesName = false
[javascript.inlayHints.propertyDeclarationTypes]
enabled = true
[javascript.inlayHints.enumMemberValues]
enabled = true
"#;

/// Merges `layer` under `base` (deep, tables union; leaves from `layer`).
/// Mirrors ide-lsp's own settings merge, app-side (the fork's options live
/// here, never in the tomls).
fn merge_toml(base: &mut toml::Value, layer: toml::Value) {
    match (base, layer) {
        (toml::Value::Table(base), toml::Value::Table(layer)) => {
            for (key, value) in layer {
                match base.get_mut(&key) {
                    Some(existing) => merge_toml(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, layer) => *base = layer,
    }
}

/// Fills server-config gaps before each launch. Deliberately conservative:
/// the gopls opt-ins only when nobody set options, and the vtsls hints as a
/// merge under whatever exists (a user's own inlayHints kind wins).
fn apply_server_overrides(cfg: &mut LanguageServerConfig) {
    if cfg.name == "gopls" && cfg.initialization_options.is_none() {
        // Static text, unit-tested to parse; `.ok()` keeps the launch path
        // panic-free even if it ever drifts.
        cfg.initialization_options = toml::from_str(GOPLS_INITIALIZATION_OPTIONS).ok();
    }
    if cfg.name == "vtsls"
        && let Ok(hints) = toml::from_str::<toml::Value>(TS_INLAY_HINTS)
    {
        // The existing options merge OVER the hints tree: the toml's (or a
        // user's) own kinds win, the defaults fill the gaps.
        let existing = cfg.initialization_options.take().unwrap_or_else(|| toml::Value::Table(Default::default()));
        let mut merged = hints;
        merge_toml(&mut merged, existing);
        cfg.initialization_options = Some(merged);
    }
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
        // The app-side server options overlay (see GOPLS_INITIALIZATION_OPTIONS)
        // applies to every server config before launch.
        lsp.set_config_hook(Box::new(|cfg, _root| apply_server_overrides(cfg)));
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
        // References/rename widening (M6): project files mentioning the symbol
        // name as a whole word are opened on the server for a second pass —
        // the word-index stand-in `ide-host` wires the same way.
        {
            let hook_documents = documents.clone();
            let hook_languages = languages.clone();
            lsp.set_candidates_hook(Box::new(move |q, cancel| {
                let query = ide_search::WordQuery {
                    word: q.word.clone(),
                    dir: q.root.clone(),
                    exclude: Vec::new(),
                    limit: q.limit,
                };
                let filter = |rel: &str| q.languages.contains(&hook_languages.detect(&q.root.join(rel), None));
                match ide_search::files_with_word(
                    &q.root,
                    &query,
                    Some(&filter),
                    &|p| hook_documents.search_text_of(p),
                    cancel,
                ) {
                    Ok(found) => ide_lsp::CandidateFiles { files: found.files, limit_hit: found.limit_hit },
                    Err(_) => ide_lsp::CandidateFiles::default(),
                }
            }));
        }
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
        // The log window's cache (M13): upstream's disposable, content-
        // addressed log index outside the worktree, keyed per repository.
        if let Some(repo) = &repo
            && let Some(cache) = dirs::cache_dir()
        {
            repo.enable_log_index(&cache.join("openidea-gui").join("log"));
        }
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

    // -- tree mutations (M7: the file tree's context menu; all IO is ide-vfs)

    /// Creates an empty file (and missing parents); never truncates.
    pub fn create_file(&self, path: &Path) -> IdeResult<()> {
        ide_vfs::create_file(path)
    }

    /// Creates a directory (and missing parents).
    pub fn create_dir(&self, path: &Path) -> IdeResult<()> {
        ide_vfs::create_dir(path)
    }

    /// Renames / moves; refuses to overwrite an existing target.
    pub fn rename(&self, from: &Path, to: &Path) -> IdeResult<()> {
        ide_vfs::rename(from, to)
    }

    /// Moves to the OS trash (recoverable); permanent only without a trash.
    pub fn delete(&self, path: &Path) -> IdeResult<()> {
        ide_vfs::delete(path)
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

    /// Captures `edit`'s preconditions (disk + dirty-buffer fingerprints) at
    /// the moment the user is shown it — the rename preview. Without the
    /// capture, Apply rejects files the server versioned but the editor never
    /// opened (gopls cross-file renames) as stale: `apply_workspace_edit`
    /// trusts preconditions precisely for those.
    pub fn prepare_workspace_edit(&self, edit: &mut ide_core::WorkspaceEdit) -> IdeResult<()> {
        self.documents.prepare_workspace_edit(edit)
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

    // -- git log / branches (M13) -------------------------------------------

    /// One page of the log (call on a background thread): `query` carries the
    /// branch/author/path/text filters and `skip`/`limit` pagination; the
    /// commits arrive newest-first with upstream's graph connections.
    pub fn git_log(&self, query: &ide_vcs::LogQuery) -> IdeResult<Vec<ide_vcs::Commit>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.log(query)
    }

    /// Files changed by `commit` relative to its first parent.
    pub fn git_commit_files(&self, commit: &str) -> IdeResult<Vec<ide_vcs::ChangedFile>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.commit_files(commit)
    }

    /// Added/deleted lines per file between two revisions.
    pub fn git_diff_numstat(&self, from: &str, to: &str) -> IdeResult<Vec<ide_vcs::FileLineStats>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.diff_numstat(from, to, &[])
    }

    /// Content of `path` at `rev` (default HEAD); `None` if absent there.
    pub fn git_file_at(&self, path: &Path, rev: Option<&str>) -> IdeResult<Option<String>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.file_at(path, rev, encoding_rs::UTF_8)
    }

    /// Local and remote branches, the current first.
    pub fn git_branches(&self) -> IdeResult<Vec<ide_vcs::Branch>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.branches()
    }

    /// Merge / rebase / cherry-pick in progress, if any.
    pub fn git_operation(&self) -> IdeResult<Option<String>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.operation()
    }

    /// Creates branch `name` at HEAD, optionally switching to it.
    pub fn git_create_branch(&self, name: &str, checkout: bool) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.create_branch(name, None, checkout)
    }

    /// Switches to `branch` as IDEA does: local changes in the way are
    /// reported (the outcome names them), remote branches get a tracking
    /// branch. `None`'s semantics live upstream ([`Repo::checkout_with`]).
    pub fn git_checkout(&self, branch: &str) -> IdeResult<ide_vcs::CheckoutOutcome> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.checkout_with(branch, false, &ide_vcs::CheckoutOptions::default())
    }

    /// Deletes a local branch without asking; the answer reports one that
    /// was not fully merged (IDEA's restore notification data).
    pub fn git_delete_branch(&self, branch: &str) -> IdeResult<ide_vcs::DeletedBranch> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.delete_branch_restorable(branch)
    }

    /// One blame entry per line of `current_text` (or the file on disk);
    /// uncommitted lines carry the all-zero commit id.
    pub fn git_blame(&self, path: &Path, current_text: Option<&str>) -> IdeResult<Vec<ide_vcs::BlameLine>> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.blame(path, current_text, None, encoding_rs::UTF_8)
    }

    /// Merges `branch` into HEAD; conflicts leave the repository mid-merge
    /// (the error says so) for the conflict surface.
    pub fn git_merge(&self, branch: &str) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.merge(branch)
    }

    /// Rebases the current branch onto `onto`; conflicts leave the
    /// repository mid-rebase for the conflict surface.
    pub fn git_rebase(&self, onto: &str) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.rebase(onto)
    }

    /// The project's changelists (M13): upstream's per-project
    /// `.openidea/changelists.json` (the Tauri host's face), loaded with the
    /// module's normalization.
    pub fn changelists_load(&self) -> ide_vcs::ChangelistState {
        ide_vcs::changelists::load(&ide_core::project_dir::path(&self.root, "changelists.json"))
    }

    /// Persists the changelists; the per-user entries stay out of version
    /// control like IDEA's `.idea/.gitignore` (upstream's helper).
    pub fn changelists_save(&self, state: &ide_vcs::ChangelistState) -> IdeResult<()> {
        ide_vcs::changelists::ensure_ignored(
            &ide_core::project_dir::path(&self.root, ""),
            &["/shelf/", "/changelists.json"],
        )
        .ok();
        ide_vcs::changelists::save(&ide_core::project_dir::path(&self.root, "changelists.json"), state)
    }

    /// Continues (or aborts) the merge/rebase in progress.
    pub fn git_continue_operation(&self) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.continue_operation(&ide_core::CancellationToken::new(), &|_| {})
    }

    pub fn git_abort_operation(&self) -> IdeResult<()> {
        let repo = self.repo.as_ref().ok_or_else(|| IdeError::not_found("the project is not a git repository"))?;
        repo.abort_operation(&ide_core::CancellationToken::new(), &|_| {})
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry row gopls gets from `languages/go.toml` (now upstream's,
    /// without this fork's old two data lines).
    fn gopls_config() -> LanguageServerConfig {
        toml::from_str(
            r#"
name = "gopls"
command = "gopls"
root-markers = ["go.work", "go.mod"]
"#,
        )
        .expect("gopls row parses")
    }

    #[test]
    fn gopls_options_fill_semantic_tokens_and_hints() {
        let mut cfg = gopls_config();
        assert!(cfg.initialization_options.is_none(), "upstream's go.toml carries no options");
        apply_server_overrides(&mut cfg);
        let options = cfg.initialization_options.expect("gopls options filled");
        assert_eq!(options["semanticTokens"].as_bool(), Some(true));
        let hints = options["hints"].as_table().expect("hints table");
        for kind in ["assignVariableTypes", "parameterNames", "functionTypeParameters"] {
            assert_eq!(hints[kind].as_bool(), Some(true), "hint kind {kind} opted in");
        }
    }

    #[test]
    fn overrides_leave_other_servers_and_set_options_alone() {
        let mut other: LanguageServerConfig =
            toml::from_str("name = \"rust-analyzer\"\ncommand = \"rust-analyzer\"\n").expect("row parses");
        apply_server_overrides(&mut other);
        assert!(other.initialization_options.is_none(), "non-gopls untouched");

        let mut preset: LanguageServerConfig = toml::from_str(
            "name = \"gopls\"\ncommand = \"gopls\"\ninitialization-options = { semanticTokens = false }\n",
        )
        .expect("preset row parses");
        apply_server_overrides(&mut preset);
        assert_eq!(
            preset.initialization_options.expect("kept")["semanticTokens"].as_bool(),
            Some(false),
            "an existing option is never clobbered"
        );
    }

    /// The upstream vtsls rows carry rename/import preferences; the overlay
    /// merges the hints under them without touching either.
    fn vtsls_config() -> LanguageServerConfig {
        toml::from_str(
            r#"
name = "vtsls"
command = "vtsls"
[initialization-options.typescript.preferences]
useAliasesForRenames = false
"#,
        )
        .expect("vtsls row parses")
    }

    #[test]
    fn vtsls_options_gain_the_inlay_hint_tree() {
        let mut cfg = vtsls_config();
        apply_server_overrides(&mut cfg);
        let options = cfg.initialization_options.expect("options present");
        // The toml's own preference survives the merge.
        assert_eq!(
            options["typescript"]["preferences"]["useAliasesForRenames"].as_bool(),
            Some(false),
            "existing preferences survive"
        );
        let hints = &options["typescript"]["inlayHints"];
        assert_eq!(hints["parameterTypes"]["enabled"].as_bool(), Some(true));
        assert_eq!(hints["parameterNames"]["enabled"].as_str(), Some("all"));
        assert_eq!(hints["variableTypes"]["suppressWhenTypeMatchesName"].as_bool(), Some(false));
        assert_eq!(
            options["javascript"]["inlayHints"]["functionLikeReturnTypes"]["enabled"].as_bool(),
            Some(true),
            "the javascript section rides along"
        );
    }

    #[test]
    fn vtsls_options_merge_without_options_and_kept_kinds() {
        // No toml options: the hints tree is the whole payload.
        let mut bare: LanguageServerConfig =
            toml::from_str("name = \"vtsls\"\ncommand = \"vtsls\"\n").expect("bare row parses");
        apply_server_overrides(&mut bare);
        let hints = &bare.initialization_options.expect("filled")["typescript"]["inlayHints"];
        assert_eq!(hints["enumMemberValues"]["enabled"].as_bool(), Some(true));

        // A project's own kind is never clobbered.
        let mut preset: LanguageServerConfig = toml::from_str(
            "name = \"vtsls\"\ncommand = \"vtsls\"\n[initialization-options.typescript.inlayHints.parameterTypes]\nenabled = false\n",
        )
        .expect("preset row parses");
        apply_server_overrides(&mut preset);
        assert_eq!(
            preset.initialization_options.expect("kept")["typescript"]["inlayHints"]["parameterTypes"]["enabled"]
                .as_bool(),
            Some(false),
            "an existing hint kind wins"
        );
    }
}
