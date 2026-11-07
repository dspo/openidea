//! A deterministic process for tests: the test drives both directions —
//! feeding output into the terminal and reading what the "user" typed.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use terminal_ghostty::{ProcessOutput, TerminalProcess};

#[derive(Default)]
struct TestState {
    out: Mutex<Option<ProcessOutput>>,
    /// Everything `write` was called with (the keystroke stream).
    written: Mutex<Vec<u8>>,
    resized: Mutex<Vec<(u16, u16)>>,
    killed: AtomicBool,
}

/// The process half: hand to [`crate::GhosttyView::spawn_with`].
pub struct TestProcess {
    state: std::sync::Arc<TestState>,
}

/// The test half: drive the terminal from the test.
#[derive(Clone)]
pub struct TestProbe {
    state: std::sync::Arc<TestState>,
}

/// Build the pair. `TestProcess` goes into the view; `TestProbe` stays in
/// the test.
pub fn test_process() -> (TestProcess, TestProbe) {
    let state = std::sync::Arc::new(TestState::default());
    (TestProcess { state: state.clone() }, TestProbe { state })
}

impl TestProbe {
    /// Simulate process output (what the shell/agent wrote).
    pub fn feed_output(&self, bytes: &[u8]) {
        if let Some(out) = self.state.out.lock().unwrap().as_ref() {
            out.output(bytes);
        }
    }

    /// Simulate the process exiting.
    pub fn exit(&self, code: Option<u32>) {
        if let Some(out) = self.state.out.lock().unwrap().as_ref() {
            out.exited(code);
        }
    }

    /// The keystroke bytes the terminal sent to the process.
    pub fn written(&self) -> Vec<u8> {
        self.state.written.lock().unwrap().clone()
    }

    pub fn resized(&self) -> Vec<(u16, u16)> {
        self.state.resized.lock().unwrap().clone()
    }

    pub fn killed(&self) -> bool {
        self.state.killed.load(Ordering::SeqCst)
    }
}

impl TerminalProcess for TestProcess {
    fn attach(self: std::sync::Arc<Self>, out: ProcessOutput) {
        *self.state.out.lock().unwrap() = Some(out);
    }

    fn write(&self, bytes: &[u8]) {
        self.state.written.lock().unwrap().extend_from_slice(bytes);
    }

    fn resize(&self, cols: u16, rows: u16) {
        self.state.resized.lock().unwrap().push((cols, rows));
    }

    fn kill(&self) {
        self.state.killed.store(true, Ordering::SeqCst);
    }
}
