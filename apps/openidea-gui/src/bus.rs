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
use ide_core::{IdeEvent, Pos, SharedSink};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::workbench::Workbench;

/// One message for the gpui main loop: a backend `IdeEvent`, or the result of
/// the native ⌘O dialog (which runs on its own thread).
pub enum BusMessage {
    Ide(IdeEvent),
    Picked(Option<PathBuf>),
    /// The editor asks the workbench to open (or activate) a file and place
    /// the caret at `pos` (goto definition / find usages across files).
    OpenLocation {
        path: PathBuf,
        pos: Pos,
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
