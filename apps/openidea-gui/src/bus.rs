//! The backend → UI event bridge — the GPUI host's counterpart of the Tauri
//! `ide://event` forwarding, and the prototype of the app-wide `IdeEvent` bus.
//!
//! Backend crates emit `IdeEvent`s into a `SharedSink` (see
//! `ide-core::event`); the sink here is a closure that pushes onto a tokio
//! unbounded channel. The watcher threads produce events, gpui has no
//! cross-thread wake primitive, so the foreground side is a pump task (the
//! same pattern Steer's tray pump uses): a `cx.spawn` loop that drains the
//! channel between frames and hands each message to the workbench entity.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, WeakEntity};
use ide_core::{IdeEvent, IdeResult, Pos, SharedSink};
use ide_run::{OutputChunk, RunInfo};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::workbench::Workbench;

/// One message for the gpui main loop: a backend `IdeEvent`, or the result of
/// the native ⌘O dialog (which runs on its own thread).
pub enum BusMessage {
    Ide(IdeEvent),
    Picked(Option<PathBuf>),
    /// The welcome view's (or projectless ⌘O's) folder pick: the project to
    /// open. `None` = the user cancelled.
    PickedFolder(Option<PathBuf>),
    /// The editor asks the workbench to open (or activate) a file and place
    /// the caret at `pos` (goto definition / find usages across files).
    OpenLocation {
        path: PathBuf,
        pos: Pos,
    },
    /// A run configuration start attempt finished (M4; spawned onto the
    /// background executor). `index` is the dock tab the start created, so
    /// concurrent starts route to their own panels.
    RunStarted {
        index: usize,
        name: String,
        result: IdeResult<RunInfo>,
    },
    /// One output chunk of a run (ide-run batches at ~8 ms), routed to the
    /// tab that started it; exits carry the run id through
    /// `IdeEvent::RunExited`.
    RunOutput {
        index: usize,
        chunk: OutputChunk,
    },
    /// A git status + branch refresh finished (M4; the reads ran on the
    /// background executor).
    GitStatus {
        statuses: Vec<ide_vcs::FileStatus>,
        branch: Option<String>,
    },
    /// A file's diff against HEAD finished building (M4; hunks render as a
    /// unified read-only view).
    DiffReady {
        path: PathBuf,
        title: String,
        result: IdeResult<Vec<crate::diff_view::DiffLine>>,
    },
    /// ⇧⌘F (M5): the Find panel asked for a project search; the workbench
    /// snapshots the dirty open buffers (their text wins over the disk) and
    /// runs `ide_search::search` on the background executor.
    FindInFilesStart {
        request: u32,
        query: String,
    },
    /// A Find-in-Files run finished; `request` lets the panel drop stale
    /// answers (a newer Enter supersedes an older run).
    FindInFilesDone {
        request: u32,
        files: Vec<ide_search::FileMatches>,
        summary: ide_search::SearchSummary,
    },
    /// A Find-in-Files row was clicked: open (or activate) the file, place
    /// the caret at the hit, and highlight every hit of the query in the
    /// buffer (the find bar stays closed).
    FindHit {
        path: PathBuf,
        pos: Pos,
        query: String,
    },
    /// ⌘S's organize-imports-then-save round trip finished (M6; the code
    /// action + write ran on the app runtime). `organized` says whether the
    /// server actually changed the imports; the error text (when set) failed
    /// the plain save.
    SaveFinished {
        doc: ide_core::DocId,
        name: String,
        organized: bool,
        error: Option<String>,
    },
    /// A tree creation finished (M7; the ide-vfs call ran on the background
    /// executor). A created file opens in a tab; both refresh the tree.
    FsCreated {
        path: PathBuf,
        is_dir: bool,
        error: Option<String>,
    },
    /// A tree rename finished; the renamed file's tab reopens at `to`.
    FsRenamed {
        from: PathBuf,
        to: PathBuf,
        error: Option<String>,
    },
    /// A tree delete (trash move) finished; the tree refreshes either way.
    FsDeleted {
        error: Option<String>,
    },
    /// The editor's floating context menu just opened (M8): the pane does
    /// not know the workbench, so the menu's entity rides the bus for the
    /// Esc chain (a focus-orphaned menu must still close on Esc).
    MenuOpened {
        menu: gpui::WeakEntity<gpui_component::menu::PopupMenu>,
    },
    /// The editor's references answer landed (M10): the Find Usages window's
    /// rows (the pane keeps its single-hit direct jump; multi-hit results
    /// leave the old in-pane jump list for the tool window).
    UsagesFound {
        path: PathBuf,
        symbol: String,
        locations: Vec<ide_core::Location>,
    },
    /// The Commit window's diff preview finished building (M10; the read ran
    /// on the background executor like the ⌃D overlay's).
    CommitDiffReady {
        path: PathBuf,
        result: IdeResult<Vec<crate::diff_view::DiffLine>>,
    },
    /// The commit flow finished: the new HEAD commit id, or the failure —
    /// with the message, so the Commit window's history keeps it (M10).
    CommitDone {
        result: IdeResult<String>,
        message: String,
    },
    /// The active pane's caret moved (M10's line:col status widget). The
    /// pane dedupes to one message per (line, col) change.
    CaretMoved {
        doc: ide_core::DocId,
        line: u32,
        col_utf16: u32,
    },
}

/// The UI end of the event bus. Cheap to clone; `send` never blocks
/// (unbounded — an event storm may queue, the pump drains at frame rate).
#[derive(Clone)]
pub struct Bus {
    tx: UnboundedSender<BusMessage>,
}

impl Bus {
    pub fn channel() -> (Self, UnboundedReceiver<BusMessage>) {
        let (tx, rx) = unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, message: BusMessage) {
        // A closed channel only means the pump (and window) is gone.
        let _ = self.tx.send(message);
    }

    /// The `IdeEvent` face the backend crates consume (watcher, documents).
    pub fn sink(&self) -> SharedSink {
        let bus = self.clone();
        Arc::new(move |event: IdeEvent| bus.send(BusMessage::Ide(event)))
    }

    /// The foreground drain: forwards every queued message into the workbench,
    /// polling between frames. Polling here is *not* busy-waiting on the work:
    /// the watcher already debounces on its own threads.
    pub fn spawn_pump(self, mut rx: UnboundedReceiver<BusMessage>, workbench: WeakEntity<Workbench>, cx: &mut App) {
        const POLL: Duration = Duration::from_millis(50);
        cx.spawn(async move |cx| {
            loop {
                let mut batch = Vec::new();
                match rx.try_recv() {
                    Ok(first) => {
                        batch.push(first);
                        // Drain what queued behind it in the same pass.
                        while let Ok(message) = rx.try_recv() {
                            batch.push(message);
                        }
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
                }
                for message in batch {
                    let _ = workbench.update(cx, |workbench, cx| workbench.handle_bus(message, cx));
                }
                cx.background_executor().timer(POLL).await;
            }
        })
        .detach();
    }
}
