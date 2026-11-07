//! The process seam behind a terminal surface: byte source, input sink,
//! resize, teardown. gpui-free — hosts whose process is owned elsewhere
//! implement [`TerminalProcess`] without pulling a GUI stack (the built-in
//! [`crate::pty::Pty`] is one implementation; tests use the `test-support`
//! probe).

use std::sync::Arc;

/// The feedback handle a [`TerminalProcess`] writes into. Thread-safe by
/// construction: the fold closure locks the terminal, the exit closure marks
/// the shared state and wakes the pump.
#[derive(Clone)]
pub struct ProcessOutput {
    output: OutputFold,
    exited: ExitReport,
}

/// Folds process output bytes into the terminal (thread-safe).
pub type OutputFold = Arc<dyn Fn(&[u8]) + Send + Sync>;
/// Marks a real process exit (code when known).
pub type ExitReport = Arc<dyn Fn(Option<u32>) + Send + Sync>;

impl ProcessOutput {
    pub fn new(output: OutputFold, exited: ExitReport) -> Self {
        Self { output, exited }
    }

    /// Fold process output into the terminal (thread-safe).
    pub fn output(&self, bytes: &[u8]) {
        (self.output)(bytes);
    }

    /// Report a real process exit (code when known). Terminating a process
    /// from the host side does not report through here.
    pub fn exited(&self, code: Option<u32>) {
        (self.exited)(code);
    }
}

/// The backing process of one terminal.
///
/// All methods are `&self`/`Arc<Self>`: implementations own their interior
/// mutability. The view holds the process in an `Arc` and kills it on drop.
pub trait TerminalProcess: Send + Sync + 'static {
    /// Begin feeding output into `out` and watching for exit. The view calls
    /// this once after construction; implementations spawn their own threads.
    fn attach(self: Arc<Self>, out: ProcessOutput);
    /// Keystroke-grade bytes into the process stdin.
    fn write(&self, bytes: &[u8]);
    fn resize(&self, cols: u16, rows: u16);
    /// Close semantics: terminate the process (its whole group where
    /// possible). Runs on tab close, session close, and view drop.
    fn kill(&self);
}
