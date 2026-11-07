//! The debug adapter client (M11): DAP over the adapter processes the
//! `languages/*.toml` `[debugger]` tables declare — delve, codelldb, debugpy,
//! js-debug — reached through the run-configuration system (a debug start
//! mirrors a run start; the zero-toml rule holds, the table is data).
//!
//! The wire is DAP's base protocol (JSON with `Content-Length` framing) over
//! either a TCP socket (`tcp` transport, `${port}` substitution in the
//! adapter args) or the adapter's stdio. One session drives one process:
//! handshake → breakpoints → launch → configurationDone, then the event
//! stream (stopped/continued/output/terminated) carries the debugging face.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ide_core::{IdeError, IdeResult};
use ide_run::RunConfig;
use serde_json::{Value, json};
use tokio::io::BufReader;

use crate::bus::Bus;
use crate::bus::BusMessage;

/// One file's breakpoint set as the protocol face (the lines plus any
/// per-line conditions).
pub type FileBreakpointSet = Vec<(PathBuf, Vec<u32>, Vec<(u32, String)>)>;

// ---------------------------------------------------------------------------
// The wire codec (pure, unit-tested): DAP base-protocol framing
// ---------------------------------------------------------------------------

/// One decoded DAP message: a response, an event, or a request from the
/// adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct DapMessage(pub Value);

/// Encodes a message with DAP's `Content-Length` framing.
pub fn encode_frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_string(message).unwrap_or_default();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// Pulls one framed message off the front of `buf`; `Ok(None)` = need more
/// bytes. Bytes up to the message end are consumed either way (a malformed
/// header aborts the stream with an error).
pub fn decode_frame(buf: &mut Vec<u8>) -> Result<Option<DapMessage>, String> {
    // Find the header/body boundary.
    let sep = find_subsequence(buf, b"\r\n\r\n");
    let Some(sep) = sep else {
        // Guard against a header that never terminates (garbage stream).
        if buf.len() > 64 * 1024 {
            return Err("DAP header grew past 64 KiB without a terminator".into());
        }
        return Ok(None);
    };
    let header = String::from_utf8_lossy(&buf[..sep]);
    let mut length = None;
    for line in header.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(length) = length else {
        return Err(format!("DAP header without Content-Length: {header:?}"));
    };
    let start = sep + 4;
    if buf.len() < start + length {
        return Ok(None);
    }
    let body = &buf[start..start + length];
    let message: Value = serde_json::from_slice(body).map_err(|e| format!("DAP body: {e}"))?;
    buf.drain(..start + length);
    Ok(Some(DapMessage(message)))
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// Adapter resolution (from the language table + the run config's shape)
// ---------------------------------------------------------------------------

/// One adapter start: the command to spawn and how to reach it.
#[derive(Clone, Debug)]
pub struct AdapterSpec {
    pub program: String,
    pub args: Vec<String>,
    /// `tcp` — the session dials `127.0.0.1:<port>` after spawn; `stdio` —
    /// the child's own pipes carry the protocol.
    pub transport: Transport,
    /// The language table row that produced this spec (the report's
    /// per-adapter notes name it).
    pub adapter: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp(u16),
    Stdio,
}

/// Resolves the adapter spec from the declared `[debugger]` row. `port` is
/// the caller-chosen listener port (the `${port}` substitution slot).
pub fn adapter_spec(cfg: &ide_language::DebuggerConfig, port: u16) -> AdapterSpec {
    let port_s = port.to_string();
    let args = cfg.args.iter().map(|a| a.replace("${port}", &port_s)).collect();
    AdapterSpec {
        program: cfg.command.clone(),
        args,
        transport: match cfg.transport.as_str() {
            "stdio" => Transport::Stdio,
            _ => Transport::Tcp(port),
        },
        adapter: cfg.adapter.clone(),
    }
}

/// A free localhost port (the socket the adapter will listen on): bound and
/// released so the adapter can claim it. The race window is the adapter's
/// own; DAP hosts do the same.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").ok().and_then(|l| l.local_addr().ok()).map(|a| a.port()).unwrap_or(4711)
}

/// The launch request body for an adapter family. The shapes are the
/// adapters' own脾性 (documented per family in M11-REPORT); the run
/// configuration supplies program/args/cwd.
pub fn launch_arguments(adapter: &str, target: &DebugTarget) -> Value {
    let mut body = json!({
        "request": "launch",
        "cwd": target.cwd.to_string_lossy(),
        "name": "openidea",
    });
    // Per-family required keys.
    match adapter {
        // delve: `mode=debug` compiles the package at `program`.
        "delve" => {
            body["type"] = json!("go");
            body["mode"] = json!("debug");
            body["program"] = json!(target.program.to_string_lossy());
        }
        // codelldb: a prebuilt executable.
        "codelldb" => {
            body["type"] = json!("codelldb");
            body["program"] = json!(target.program.to_string_lossy());
        }
        // debugpy: `-m` configs launch the module; the rest a script file
        // (its own interpreter resolves imports from the cwd).
        "debugpy" => {
            body["type"] = json!("python");
            if let Some(module) = &target.module {
                body["module"] = json!(module);
            } else {
                body["program"] = json!(target.program.to_string_lossy());
            }
        }
        // js-debug: node on a JS entry (the ts leg compiles first).
        _ => {
            body["type"] = json!("node");
            body["program"] = json!(target.program.to_string_lossy());
        }
    }
    if !target.args.is_empty() {
        body["args"] = json!(target.args);
    }
    if !target.env.is_empty() {
        body["env"] = json!(target.env.iter().cloned().collect::<std::collections::BTreeMap<_, _>>());
    }
    body
}

/// What a debug start runs (mirrors the run configuration's shape).
#[derive(Clone, Debug)]
pub struct DebugTarget {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// debugpy's `-m` face: launch a module instead of a file.
    pub module: Option<String>,
}

// ---------------------------------------------------------------------------
// Session state the Debug window renders (plain data; the entity owns it)
// ---------------------------------------------------------------------------

/// One stack frame of the stopped thread.
#[derive(Clone, Debug)]
pub struct StackFrame {
    pub id: u64,
    pub name: String,
    pub file: Option<PathBuf>,
    pub line: u32,
}

/// One variable row (`value` is the adapter's formatted summary).
#[derive(Clone, Debug)]
pub struct VariableRow {
    pub name: String,
    pub value: String,
    /// The children handle (structure expansion is M12's face); kept in the
    /// row data so the parse tests pin it.
    #[allow(dead_code)]
    pub variables_reference: u64,
}

/// One scope row of the current frame (Locals / Arguments / Globals…).
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ScopeRow {
    pub name: String,
    pub variables_reference: u64,
}

/// The session face the Debug window reads: adapter name, state, the stopped
/// frame's location, the collected stack/scopes, and the console lines.
#[derive(Clone)]
pub struct DebugState {
    pub adapter: String,
    pub config_name: String,
    /// Running (between stops), stopped at a frame, or gone.
    pub phase: DebugPhase,
    /// The stopped thread (the stepping targets); `None` while running.
    pub thread_id: Option<u64>,
    pub frames: Vec<StackFrame>,
    pub scopes: Vec<ScopeRow>,
    /// The console: DAP `output` events and the session's own lines.
    pub console: Vec<String>,
    /// The current line for the editor's execution marker (per file; one
    /// session marks one location).
    pub stopped_at: Option<(PathBuf, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugPhase {
    Starting,
    Running,
    Stopped,
    Exited,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            adapter: String::new(),
            config_name: String::new(),
            phase: DebugPhase::Starting,
            thread_id: None,
            frames: Vec::new(),
            scopes: Vec::new(),
            console: Vec::new(),
            stopped_at: None,
        }
    }
}

/// The stepping trio's shared face (the keymap actions and the Debug
/// window's buttons both land here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Into,
    Over,
    Resume,
}

/// A handle the workbench holds: pushes UI-bound events onto the bus.
#[derive(Clone)]
pub struct DebugEvents {
    bus: Bus,
}

impl DebugEvents {
    pub fn new(bus: Bus) -> Self {
        Self { bus }
    }

    pub fn updated(&self) {
        self.bus.send(BusMessage::DebugUpdated);
    }
}

/// The session's own bookkeeping the background tasks share.
pub(crate) struct SessionShared {
    pub events: DebugEvents,
    pub state: std::sync::Mutex<DebugState>,
    /// The last `variables` answer (the first scope's rows; refreshed on
    /// every stop).
    pub variables: std::sync::Mutex<Vec<VariableRow>>,
    /// The Watches section's expression input (the workbench creates it in
    /// a window-having context and slots it in).
    pub watch_input: std::sync::Mutex<Option<gpui::Entity<gpui_component::input::InputState>>>,
}

impl SessionShared {
    /// The cached variable rows (the first scope's).
    pub fn variables(&self) -> Vec<VariableRow> {
        self.variables.lock().unwrap().clone()
    }

    /// The watch expression's text (the input's value).
    pub fn watch_text(&self, cx: &gpui::App) -> String {
        self.watch_input.lock().unwrap().as_ref().map(|input| input.read(cx).value().to_string()).unwrap_or_default()
    }
}

impl SessionShared {
    pub fn console_line(&self, line: String) {
        let mut state = self.state.lock().unwrap();
        state.console.push(line);
        // The console is bounded like every list the window renders.
        if state.console.len() > 1000 {
            state.console.remove(0);
        }
        drop(state);
        self.events.updated();
    }

    pub fn set_phase(&self, phase: DebugPhase) {
        self.state.lock().unwrap().phase = phase;
        self.events.updated();
    }
}

// ---------------------------------------------------------------------------
// Pure helpers the session flow uses (unit-tested)
// ---------------------------------------------------------------------------

/// The first frame whose location the editor can mark (file + line), from a
/// stackTrace answer.
pub fn stopped_location(stack: &[StackFrame]) -> Option<(PathBuf, u32)> {
    stack.iter().find_map(|f| f.file.clone().map(|file| (file, f.line)))
}

/// Extracts the stack frames from a `stackTrace` response body.
pub fn parse_stack_trace(body: &Value) -> Vec<StackFrame> {
    body.get("stackFrames")
        .and_then(|v| v.as_array())
        .map(|frames| {
            frames
                .iter()
                .map(|f| StackFrame {
                    id: f.get("id").and_then(Value::as_u64).unwrap_or(0),
                    name: f.get("name").and_then(Value::as_str).unwrap_or("?").to_owned(),
                    file: f.get("source").and_then(|s| s.get("path")).and_then(Value::as_str).map(PathBuf::from),
                    line: f.get("line").and_then(Value::as_u64).unwrap_or(0) as u32,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extracts the scopes from a `scopes` response body.
pub fn parse_scopes(body: &Value) -> Vec<ScopeRow> {
    body.get("scopes")
        .and_then(|v| v.as_array())
        .map(|scopes| {
            scopes
                .iter()
                .map(|s| ScopeRow {
                    name: s.get("name").and_then(Value::as_str).unwrap_or("?").to_owned(),
                    variables_reference: s.get("variablesReference").and_then(Value::as_u64).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extracts variable rows from a `variables` response body.
pub fn parse_variables(body: &Value) -> Vec<VariableRow> {
    body.get("variables")
        .and_then(|v| v.as_array())
        .map(|vars| {
            vars.iter()
                .map(|v| VariableRow {
                    name: v.get("name").and_then(Value::as_str).unwrap_or("?").to_owned(),
                    value: v.get("value").and_then(Value::as_str).unwrap_or("").to_owned(),
                    variables_reference: v.get("variablesReference").and_then(Value::as_u64).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `setBreakpoints` request body for one file (DAP line semantics are
/// the source lines themselves — the editor's 0-based gutter rows shift by
/// one here, the single deliberate conversion at the crate edge).
pub fn breakpoints_arguments(file: &Path, lines: &[u32], conditions: &[(u32, String)]) -> Value {
    let breakpoints: Vec<Value> = lines
        .iter()
        .map(|&line| {
            let mut bp = json!({ "line": line + 1 });
            if let Some((_, cond)) = conditions.iter().find(|(l, _)| *l == line) {
                bp["condition"] = json!(cond);
            }
            bp
        })
        .collect();
    json!({
        "source": { "path": file.to_string_lossy() },
        "breakpoints": breakpoints,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The framing round-trips: encode → the decoder yields the same JSON.
    #[test]
    fn frames_round_trip() {
        let message = json!({"seq": 1, "type": "request", "command": "initialize"});
        let mut buf = encode_frame(&message);
        let decoded = decode_frame(&mut buf).expect("decode").expect("one message");
        assert_eq!(decoded.0, message);
        assert!(buf.is_empty(), "the frame is consumed");
    }

    /// Two messages in one read are both yielded in order.
    #[test]
    fn two_frames_decode_in_order() {
        let mut buf = Vec::new();
        buf.extend(encode_frame(&json!({"seq": 1, "type": "request", "command": "a"})));
        buf.extend(encode_frame(&json!({"seq": 2, "type": "request", "command": "b"})));
        let first = decode_frame(&mut buf).expect("decode").expect("first");
        assert_eq!(first.0["command"], "a");
        let second = decode_frame(&mut buf).expect("decode").expect("second");
        assert_eq!(second.0["command"], "b");
        assert!(decode_frame(&mut buf).expect("decode").is_none());
    }

    /// A partial frame asks for more bytes; a headerless stream errors once
    /// it grows beyond the guard.
    #[test]
    fn partial_frames_wait_and_garbage_errors() {
        let message = json!({"seq": 1, "type": "request", "command": "x"});
        let full = encode_frame(&message);
        let (head, tail) = full.split_at(full.len() - 3);
        let mut buf = head.to_vec();
        assert!(decode_frame(&mut buf).expect("no error").is_none());
        buf.extend_from_slice(tail);
        assert!(decode_frame(&mut buf).expect("no error").is_some());

        let mut garbage = b"not-a-dap-stream-at-all".to_vec();
        // Below the guard: waits. Above it: the stream aborts.
        assert!(decode_frame(&mut garbage).expect("no error").is_none());
        let junk = vec![b'x'; 65 * 1024];
        assert!(decode_frame(&mut junk.clone()).is_err());
    }

    /// `${port}` substitutes in adapter args; stdio rows keep their pipes.
    #[test]
    fn adapter_spec_substitutes_the_port() {
        let cfg: ide_language::DebuggerConfig = toml::from_str(
            r#"
adapter = "delve"
command = "dlv"
args = ["dap", "--listen", "127.0.0.1:${port}"]
transport = "tcp"
"#,
        )
        .expect("delve row parses");
        let spec = adapter_spec(&cfg, 48620);
        assert_eq!(spec.program, "dlv");
        assert_eq!(spec.args, vec!["dap", "--listen", "127.0.0.1:48620"]);
        assert_eq!(spec.transport, Transport::Tcp(48620));

        let stdio: ide_language::DebuggerConfig = toml::from_str(
            r#"
adapter = "debugpy"
command = "python3"
args = ["-m", "debugpy.adapter"]
transport = "stdio"
"#,
        )
        .expect("debugpy row parses");
        let spec = adapter_spec(&stdio, 1);
        assert_eq!(spec.transport, Transport::Stdio);
    }

    /// Each adapter family gets its launch face (the per-family required
    /// keys); args and env ride through.
    #[test]
    fn launch_arguments_carry_the_family_face() {
        let target = DebugTarget {
            program: PathBuf::from("/tmp/demo"),
            args: vec!["--flag".into()],
            cwd: PathBuf::from("/tmp"),
            env: vec![("KEY".into(), "v".into())],
            module: None,
        };
        let go = launch_arguments("delve", &target);
        assert_eq!(go["type"], "go");
        assert_eq!(go["mode"], "debug");
        assert_eq!(go["program"], "/tmp/demo");
        assert_eq!(go["args"], json!(["--flag"]));
        let py = launch_arguments("debugpy", &target);
        assert_eq!(py["type"], "python");
        let node = launch_arguments("js-debug", &target);
        assert_eq!(node["type"], "node");
        assert_eq!(node["env"]["KEY"], "v");
    }

    /// The DAP answers parse into the window's rows.
    #[test]
    fn dap_answers_parse_into_rows() {
        let stack = parse_stack_trace(&json!({
            "stackFrames": [
                {"id": 1, "name": "main", "source": {"path": "/tmp/main.go"}, "line": 12},
                {"id": 2, "name": "runtime.main", "line": 3},
            ]
        }));
        assert_eq!(stack.len(), 2);
        assert_eq!(stopped_location(&stack), Some((PathBuf::from("/tmp/main.go"), 12)));

        let scopes = parse_scopes(&json!({
            "scopes": [{"name": "Locals", "variablesReference": 7}]
        }));
        assert_eq!(scopes[0].name, "Locals");
        assert_eq!(scopes[0].variables_reference, 7);

        let vars = parse_variables(&json!({
            "variables": [{"name": "tasks", "value": "TaskList (3)", "variablesReference": 9}]
        }));
        assert_eq!(vars[0].value, "TaskList (3)");
    }

    /// Breakpoint lines cross the crate edge here: gutter rows are 0-based,
    /// DAP lines are 1-based; conditions ride the row they guard.
    #[test]
    fn breakpoints_arguments_shift_lines_and_carry_conditions() {
        let args = breakpoints_arguments(Path::new("/tmp/main.go"), &[9, 14], &[(14, "tasks.len() > 2".into())]);
        assert_eq!(args["source"]["path"], "/tmp/main.go");
        assert_eq!(args["breakpoints"][0]["line"], 10);
        assert_eq!(args["breakpoints"][1]["line"], 15);
        assert_eq!(args["breakpoints"][1]["condition"], "tasks.len() > 2");
        assert!(args["breakpoints"][0].get("condition").is_none());
    }
}

// ---------------------------------------------------------------------------
// The running session: adapter process + protocol tasks
// ---------------------------------------------------------------------------

/// What the reader should do with a response (registered when the request
/// went out; DAP responses carry `request_seq`).
#[derive(Clone, Copy, Debug)]
enum FollowUp {
    StackTrace,
    Scopes,
    Variables,
    Evaluate,
}

/// One debug session: the adapter process, its protocol tasks, and the
/// state the Debug window renders. Clone-free handle; drop/stop ends it.
pub struct DebugSession {
    pub shared: Arc<SessionShared>,
    #[allow(dead_code)]
    seq: Arc<std::sync::atomic::AtomicU64>,
    req_tx: tokio::sync::mpsc::UnboundedSender<(String, Value, Option<FollowUp>)>,
    /// The adapter's process handle (the stop path kills it; delve/codelldb
    /// spawn their debuggee as a child, so killing the tree goes through
    /// ide-run's group kill face — the plain kill here is the MVP face).
    child_pid: Arc<std::sync::Mutex<Option<u32>>>,
    /// Kept alive for the session's lifetime (the tasks hold their own
    /// clones; this one keeps the JoinSet semantics obvious).
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl DebugSession {
    /// Starts an adapter session: spawns the process (or dials its socket),
    /// performs the DAP handshake, sends the breakpoints, launches the
    /// target, and starts the event pump. Runs the handshake on `runtime`
    /// (never the UI thread) and reports through the bus.
    pub fn start(
        spec: AdapterSpec,
        target: DebugTarget,
        breakpoints: FileBreakpointSet,
        config_name: String,
        events: DebugEvents,
        runtime: tokio::runtime::Handle,
    ) -> IdeResult<Self> {
        let shared = Arc::new(SessionShared {
            state: std::sync::Mutex::new(DebugState {
                adapter: spec.adapter.clone(),
                config_name: config_name.clone(),
                ..DebugState::default()
            }),
            events: events.clone(),
            variables: std::sync::Mutex::new(Vec::new()),
            watch_input: std::sync::Mutex::new(None),
        });
        let seq = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let child_pid = Arc::new(std::sync::Mutex::new(None));
        let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Value, Option<FollowUp>)>();
        let pending: Arc<std::sync::Mutex<std::collections::HashMap<u64, FollowUp>>> =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        // `done` fires once the handshake settled: the launch went out, or
        // the session failed (spawn/connect/protocol) — start() reports the
        // failure through IdeResult; everything later rides the bus.
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
        let task = {
            let shared = shared.clone();
            let seq = seq.clone();
            let child_pid = child_pid.clone();
            runtime.spawn(async move {
                let result = tokio::select! {
                    result = run_session(
                        spec,
                        target,
                        breakpoints,
                        shared,
                        seq,
                        req_rx,
                        pending,
                        child_pid,
                        done_tx,
                    ) => result,
                    _ = abort_rx => Ok(()),
                };
                // `done_tx` was consumed by run_session (the settle signal); the
                // abort arm closes it implicitly, which the waiter reads as a
                // timeout and the Exited phase explains.
                let _ = result;
            })
        };

        // Wait (off the UI thread) for the handshake to settle.
        let waiter = std::thread::Builder::new()
            .name("dap-handshake".into())
            .spawn(move || done_rx.recv_timeout(std::time::Duration::from_secs(20)))
            .map_err(|e| IdeError::internal(e.to_string()))?;
        // Three layers (join → the recv timeout → the session's own outcome):
        // fold the outer two into the session's settle result.
        let settle = waiter
            .join()
            .map_err(|_| "the handshake thread died".to_string())
            .and_then(|inner| inner.map_err(|e| e.to_string()));
        let outcome = settle;
        if outcome.is_err() {
            let _ = abort_tx.send(());
        }

        Ok(Self { shared, seq, req_tx, child_pid, _tasks: vec![task] })
    }

    /// The stopped thread the stepping targets (`None` while running).
    pub fn stopped_thread(&self) -> Option<u64> {
        self.shared.state.lock().unwrap().thread_id
    }

    // -- requests -----------------------------------------------------------

    fn send(&self, command: &str, arguments: Value, follow: Option<FollowUp>) {
        let _ = self.req_tx.send((command.to_owned(), arguments, follow));
    }

    /// Sets breakpoints for one file (the editor's toggle path; also run for
    /// every file with breakpoints during the start handshake).
    pub fn set_breakpoints(&self, file: &Path, lines: &[u32], conditions: &[(u32, String)]) {
        self.send("setBreakpoints", breakpoints_arguments(file, lines, conditions), None);
    }

    pub fn continue_run(&self, thread_id: u64) {
        self.send("continue", json!({ "threadId": thread_id }), None);
    }

    pub fn step_in(&self, thread_id: u64) {
        self.send("stepIn", json!({ "threadId": thread_id }), None);
    }

    pub fn step_over(&self, thread_id: u64) {
        self.send("next", json!({ "threadId": thread_id }), None);
    }

    pub fn evaluate(&self, expression: &str, frame_id: Option<u64>) {
        let mut args = json!({ "expression": expression, "context": "watch" });
        if let Some(frame) = frame_id {
            args["frameId"] = json!(frame);
        }
        self.send("evaluate", args, Some(FollowUp::Evaluate));
    }

    /// Stops the session: disconnect (graceful), then the process kill.
    pub fn stop(&self) {
        let _ = self.req_tx.send(("disconnect".into(), json!({ "terminateDebuggee": true }), None));
        if let Some(pid) = *self.child_pid.lock().unwrap() {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
        self.shared.set_phase(DebugPhase::Exited);
    }
}

/// The whole protocol flow on the runtime: dial/spawn → handshake →
/// breakpoints → launch → configurationDone → event loop.
/// The protocol flow: dial/spawn → handshake → breakpoints → launch →
/// configurationDone → event loop. `done` carries the settle signal (the
/// launch answered, or any failure) so `DebugSession::start` can report.
#[allow(clippy::too_many_arguments)]
async fn run_session(
    spec: AdapterSpec,
    target: DebugTarget,
    breakpoints: FileBreakpointSet,
    shared: Arc<SessionShared>,
    seq: Arc<std::sync::atomic::AtomicU64>,
    mut req_rx: tokio::sync::mpsc::UnboundedReceiver<(String, Value, Option<FollowUp>)>,
    pending: Arc<std::sync::Mutex<std::collections::HashMap<u64, FollowUp>>>,
    child_pid: Arc<std::sync::Mutex<Option<u32>>>,
    done: std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let outcome =
        run_session_body(&spec, &target, &breakpoints, &shared, &seq, &mut req_rx, &pending, &child_pid, &done).await;
    if outcome.is_err() {
        // The settle signal on every failure path (start() reports it).
        let _ = done.send(outcome.clone());
    }
    outcome
}

/// The adapter's two transport sides behind one read/write face.
enum Sides {
    Pipe(BufReader<tokio::process::ChildStdout>, tokio::process::ChildStdin),
    Tcp(BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf),
}

impl Sides {
    async fn read_frame(&mut self, buf: &mut Vec<u8>) -> Result<Option<DapMessage>, String> {
        match self {
            Sides::Pipe(reader, _) => read_frame_into(reader, buf).await,
            Sides::Tcp(reader, _) => read_frame_into(reader, buf).await,
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        match self {
            Sides::Pipe(_, writer) => writer.write_all(bytes).await,
            Sides::Tcp(_, writer) => writer.write_all(bytes).await,
        }
        .map_err(|e| format!("adapter write: {e}"))
    }
}

async fn read_frame_into<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    buf: &mut Vec<u8>,
) -> Result<Option<DapMessage>, String> {
    use tokio::io::AsyncReadExt;
    loop {
        if let Some(message) = decode_frame(buf)? {
            return Ok(Some(message));
        }
        let mut chunk = [0u8; 8192];
        let n = reader.read(&mut chunk).await.map_err(|e| format!("adapter read: {e}"))?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// The protocol body (the settle signal fires from `run_session` on any
/// error exit here, and from the launch answer inside).
#[allow(clippy::too_many_arguments)]
async fn run_session_body(
    spec: &AdapterSpec,
    target: &DebugTarget,
    breakpoints: &FileBreakpointSet,
    shared: &Arc<SessionShared>,
    seq: &Arc<std::sync::atomic::AtomicU64>,
    req_rx: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value, Option<FollowUp>)>,
    pending: &Arc<std::sync::Mutex<std::collections::HashMap<u64, FollowUp>>>,
    child_pid: &Arc<std::sync::Mutex<Option<u32>>>,
    done: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let mut sides = match spec.transport {
        Transport::Tcp(port) => {
            let child = tokio::process::Command::new(&spec.program)
                .args(&spec.args)
                .current_dir(&target.cwd)
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("spawn `{}`: {e}", spec.program))?;
            *child_pid.lock().unwrap() = child.id();
            // The adapter owns the socket once it is up; retry briefly.
            let mut stream = None;
            for _ in 0..40 {
                if let Ok(s) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                    stream = Some(s);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            let stream = stream.ok_or_else(|| format!("adapter `{}` never opened port {port}", spec.program))?;
            let (read, write) = stream.into_split();
            Sides::Tcp(BufReader::new(read), write)
        }
        Transport::Stdio => {
            let mut child = tokio::process::Command::new(&spec.program)
                .args(&spec.args)
                .current_dir(&target.cwd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("spawn `{}`: {e}", spec.program))?;
            *child_pid.lock().unwrap() = child.id();
            let stdin = child.stdin.take().ok_or("adapter stdin missing")?;
            let stdout = child.stdout.take().ok_or("adapter stdout missing")?;
            Sides::Pipe(BufReader::new(stdout), stdin)
        }
    };

    shared.console_line(format!("[session] adapter {} started", spec.adapter));

    // Handshake: initialize → (adapter capabilities) → breakpoints → launch
    // → configurationDone. Requests carry monotonically assigned seqs; the
    // loop below routes responses by `request_seq`.
    let next_seq = || seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let mut write_buf = Vec::new();
    let init_req = next_seq();
    write_buf.extend(encode_frame(&json!({
        "seq": init_req, "type": "request", "command": "initialize",
        "arguments": {
            "clientID": "openidea-gui",
            "adapterID": spec.adapter,
            "pathFormat": "path",
        }
    })));
    sides.write_all(&write_buf).await?;
    write_buf.clear();

    // The launch/config arguments are only sent after the adapter answers
    // initialize AND fires `initialized` (the DAP contract).
    let mut initialized = false;
    let mut launch_sent = false;
    let mut config_done = false;
    let mut read_buf: Vec<u8> = Vec::new();
    let mut current_thread: Option<u64> = None;
    #[allow(unused_assignments)]
    let mut current_frame = None;

    loop {
        tokio::select! {
            // Outgoing requests queue through the channel; each frame is
            // built with its seq and the follow-up registered.
            maybe_req = req_rx.recv() => {
                let Some((command, arguments, follow)) = maybe_req else { break };
                let s = next_seq();
                if let Some(follow) = follow {
                    pending.lock().unwrap().insert(s, follow);
                }
                let mut frame = encode_frame(&json!({
                    "seq": s, "type": "request", "command": command, "arguments": arguments,
                }));
                sides.write_all(&frame).await?;
                let _ = std::mem::take(&mut frame);
            }
            maybe_msg = sides.read_frame(&mut read_buf) => {
                let Some(DapMessage(message)) = maybe_msg? else {
                    shared.set_phase(DebugPhase::Exited);
                    break;
                };
                let kind = message["type"].as_str().unwrap_or("");
                match kind {
                    "response" => {
                        let seq_no = message["request_seq"].as_u64().unwrap_or(0);
                        let follow = pending.lock().unwrap().remove(&seq_no);
                        // The launch's own answer settles the handshake: the
                        // session task keeps running, start() returns here.
                        if message["command"] == "launch" {
                            let _ = done.send(Ok(()));
                        }
                        // The initialize answer unblocks the launch sequence.
                        if !launch_sent && message["command"] == "initialize" {
                            launch_sent = true;
                            let args = launch_arguments(&spec.adapter, target);
                            let s = next_seq();
                            sides.write_all(encode_frame(&json!({
                                "seq": s, "type": "request", "command": "launch", "arguments": args,
                            })).as_slice()).await?;
                        }
                        match follow {
                            Some(FollowUp::StackTrace) => {
                                let frames = parse_stack_trace(message.get("body").unwrap_or(&Value::Null));
                                current_frame = frames.first().map(|f| f.id);
                                shared.state.lock().unwrap().stopped_at = stopped_location(&frames);
                                shared.state.lock().unwrap().frames = frames;
                                // Scopes for the frame follow.
                                if let Some(frame) = current_frame {
                                    let s = next_seq();
                                    pending.lock().unwrap().insert(s, FollowUp::Scopes);
                                    sides.write_all(encode_frame(&json!({
                                        "seq": s, "type": "request", "command": "scopes",
                                        "arguments": { "frameId": frame },
                                    })).as_slice()).await?;
                                }
                                shared.events.updated();
                            }
                            Some(FollowUp::Scopes) => {
                                let scopes = parse_scopes(message.get("body").unwrap_or(&Value::Null));
                                // Variables per scope follow.
                                // The MVP face: the first scope's rows (locals).
                                if let Some(scope) = scopes.first() {
                                    let s = next_seq();
                                    pending.lock().unwrap().insert(s, FollowUp::Variables);
                                    let frame_body = json!({ "variablesReference": scope.variables_reference });
                                    sides.write_all(encode_frame(&json!({
                                        "seq": s, "type": "request", "command": "variables",
                                        "arguments": frame_body,
                                    }))
                                    .as_slice())
                                    .await?;
                                }
                                shared.state.lock().unwrap().scopes = scopes;
                                shared.events.updated();
                            }
                            Some(FollowUp::Variables) => {
                                let vars = parse_variables(message.get("body").unwrap_or(&Value::Null));
                                *shared.variables.lock().unwrap() = vars;
                                shared.events.updated();
                            }
                            Some(FollowUp::Evaluate) => {
                                let result = message.pointer("/body/result").and_then(Value::as_str).unwrap_or("?");
                                shared.console_line(format!("[watch] {result}"));
                            }
                            None => {}
                        }
                    }
                    "event" => {
                        let event = message["event"].as_str().unwrap_or("");
                        match event {
                            "initialized" => {
                                initialized = true;
                                // Every file's breakpoints, then configurationDone.
                                for (file, lines, conditions) in breakpoints {
                                    let s = next_seq();
                                    let args = breakpoints_arguments(file, lines, conditions);
                                    sides.write_all(encode_frame(&json!({
                                        "seq": s, "type": "request",
                                        "command": "setBreakpoints",
                                        "arguments": args,
                                    })).as_slice()).await?;
                                }
                                let s = next_seq();
                                sides.write_all(encode_frame(&json!({
                                    "seq": s, "type": "request", "command": "configurationDone",
                                    "arguments": {},
                                })).as_slice()).await?;
                                config_done = true;
                            }
                            "stopped" => {
                                let thread = message["body"]["threadId"].as_u64().unwrap_or(1);
                                current_thread = Some(thread);
                                {
                                    let mut state = shared.state.lock().unwrap();
                                    state.phase = DebugPhase::Stopped;
                                    state.thread_id = Some(thread);
                                    state.console.push(format!(
                                        "[stopped] {} (thread {thread})",
                                        message["body"]["reason"].as_str().unwrap_or("?"),
                                    ));
                                    state.stopped_at = None;
                                    state.frames.clear();
                                    state.scopes.clear();
                                }
                                let s = next_seq();
                                pending.lock().unwrap().insert(s, FollowUp::StackTrace);
                                sides.write_all(encode_frame(&json!({
                                    "seq": s, "type": "request", "command": "stackTrace",
                                    "arguments": { "threadId": thread, "levels": 20 },
                                })).as_slice()).await?;
                            }
                            "continued" => {
                                let mut state = shared.state.lock().unwrap();
                                state.phase = DebugPhase::Running;
                                state.thread_id = None;
                                state.stopped_at = None;
                                state.frames.clear();
                                state.scopes.clear();
                                drop(state);
                                shared.events.updated();
                            }
                            "output" => {
                                let text = message["body"]["output"].as_str().unwrap_or("");
                                for line in text.lines() {
                                    shared.console_line(line.to_owned());
                                }
                            }
                            "process" | "thread" => {}
                            "terminated" | "exited" => {
                                if !config_done {
                                    shared.console_line("[session] adapter exited before configurationDone".into());
                                }
                                shared.set_phase(DebugPhase::Exited);
                            }
                            "breakpoint" => {}
                            other => {
                                shared.console_line(format!("[event] {other}"));
                            }
                        }
                    }
                    _ => {}
                }
                if launch_sent && initialized && config_done && current_thread.is_none() {
                    // The program is free-running until it stops; reflect it.
                    let mut state = shared.state.lock().unwrap();
                    if state.phase == DebugPhase::Starting {
                        state.phase = DebugPhase::Running;
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Target derivation (run configuration → the adapter's launch face)
// ---------------------------------------------------------------------------

/// What a debug start needs beyond the plain `DebugTarget`: some adapters
/// need a prep step (cargo's build) or a module face (python -m).
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct DebugStart {
    pub spec_adapter: String,
    pub target: DebugTarget,
    /// cargo: build first, then launch the binary this yields (resolved on
    /// the background thread by `cargo_executable` over its JSON output).
    pub build_command: Option<Vec<String>>,
    /// debugpy: `-m` configs launch a module, not a file.
    pub module: Option<String>,
}

/// Derives the debug start from a run configuration (the brief's
/// "launch/attach 并入 run 配置体系"). `root` is the project root; the
/// config's own cwd wins when it names one. The adapter comes from the
/// config's own tag (`debug_adapter`, set by the detectors from the
/// language tables).
pub fn debug_start(config: &RunConfig, root: &Path) -> Option<DebugStart> {
    let cwd = config.cwd.as_ref().map(|p| p.to_path_buf()).unwrap_or_else(|| root.to_path_buf());
    let target_args = config.args.clone();
    let env = config.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>();
    match config.kind.as_str() {
        // delve compiles the package itself: `program` is the package dir.
        "go" => {
            let dir = target_args.iter().find(|a| a.starts_with('.')).cloned().unwrap_or_else(|| ".".to_owned());
            let program = cwd.join(&dir);
            Some(DebugStart {
                spec_adapter: "delve".into(),
                target: DebugTarget { program, args: Vec::new(), cwd, env, module: None },
                build_command: None,
                module: None,
            })
        }
        // codelldb wants an executable: cargo builds it first.
        "cargo" => Some(DebugStart {
            spec_adapter: "codelldb".into(),
            target: DebugTarget { program: PathBuf::new(), args: Vec::new(), cwd, env, module: None },
            build_command: Some(vec!["cargo".into(), "build".into(), "--message-format=json".into()]),
            module: None,
        }),
        // debugpy: `-m mod` launches the module; a script path launches it.
        "python" | "pytest" => {
            let mut module = None;
            let mut program = PathBuf::new();
            let mut rest = Vec::new();
            let mut args = target_args.into_iter().peekable();
            while let Some(arg) = args.next() {
                if arg == "-m" {
                    module = args.next();
                    rest = args.collect();
                    break;
                }
                if program.as_os_str().is_empty() && (arg.ends_with(".py")) {
                    program = cwd.join(&arg);
                } else if !arg.starts_with('-') {
                    rest.push(arg);
                }
            }
            if module.is_none() && program.as_os_str().is_empty() {
                return None;
            }
            Some(DebugStart {
                spec_adapter: "debugpy".into(),
                target: DebugTarget { program, args: rest, cwd, env, module: module.clone() },
                build_command: None,
                module,
            })
        }
        // node family: the compiled JS entry (the ts leg builds first, the
        // config then names the js file).
        "npm" | "npx" | "node" | "shell" => {
            let tokens: Vec<&str> = config.command.split_whitespace().collect();
            let program = tokens
                .iter()
                .find(|t| t.ends_with(".js") || t.ends_with(".mjs") || t.ends_with(".cjs"))
                .map(|t| cwd.join(t));
            let program = match program {
                Some(p) => p,
                None => {
                    let arg_file = target_args
                        .iter()
                        .find(|a| a.ends_with(".js") || a.ends_with(".mjs") || a.ends_with(".cjs"))?;
                    cwd.join(arg_file)
                }
            };
            Some(DebugStart {
                spec_adapter: "js-debug".into(),
                target: DebugTarget { program, args: Vec::new(), cwd, env, module: None },
                build_command: None,
                module: None,
            })
        }
        _ => None,
    }
}

/// The executable `cargo build --message-format=json` produced: the last
/// compiler-artifact message with an executable path (cargo emits one per
/// target; binaries carry the key, libraries don't).
pub fn cargo_executable(build_output: &str) -> Option<PathBuf> {
    build_output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["executable"].is_string())
        .filter_map(|m| m["executable"].as_str().map(PathBuf::from))
        .next_back()
}

#[cfg(test)]
mod target_tests {
    use super::*;

    fn cfg(kind: &str, command: &str, args: &[&str]) -> RunConfig {
        let args_list = args.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", ");
        let c: RunConfig = toml::from_str(&format!(
            r#"
id = "t"
name = "t"
kind = "{kind}"
command = "{command}"
args = [{args_list}]
env = {{}}
detected = true
isTest = false
"#,
        ))
        .expect("row parses");
        c
    }

    /// go run ./cmd/server → delve compiles the package dir.
    #[test]
    fn go_configs_launch_their_package_dir() {
        let start = debug_start(&cfg("go", "go", &["run", "./cmd/server"]), Path::new("/proj")).expect("go debuggable");
        assert_eq!(start.spec_adapter, "delve");
        assert_eq!(start.target.program, PathBuf::from("/proj/cmd/server"));
        assert!(start.build_command.is_none());
    }

    /// cargo run → a build step, then the executable the build emitted.
    #[test]
    fn cargo_configs_build_first() {
        let start = debug_start(&cfg("cargo", "cargo", &["run"]), Path::new("/proj")).expect("cargo debuggable");
        assert_eq!(start.spec_adapter, "codelldb");
        assert_eq!(
            start.build_command.expect("build step"),
            vec!["cargo".to_owned(), "build".to_owned(), "--message-format=json".to_owned()]
        );
    }

    /// python -m module → the module face; a script path → the file face.
    #[test]
    fn python_configs_split_module_and_script() {
        let module = debug_start(&cfg("python", "python3", &["-m", "tasks.cli"]), Path::new("/proj"))
            .expect("module debuggable");
        assert_eq!(module.module.as_deref(), Some("tasks.cli"));
        let script =
            debug_start(&cfg("python", "python3", &["tools/x.py"]), Path::new("/proj")).expect("script debuggable");
        assert_eq!(script.target.program, PathBuf::from("/proj/tools/x.py"));
        assert_eq!(script.module, None);
    }

    /// A node entry (from the command line or the args) → js-debug.
    #[test]
    fn node_configs_launch_the_js_entry() {
        let start = debug_start(&cfg("shell", "node out/index.js", &[]), Path::new("/proj")).expect("node debuggable");
        assert_eq!(start.spec_adapter, "js-debug");
        assert_eq!(start.target.program, PathBuf::from("/proj/out/index.js"));
    }

    /// The cargo JSON stream's last executable wins (test targets precede).
    #[test]
    fn cargo_output_parses_the_executable() {
        let output = r#"{"reason":"compiler-artifact","executable":null}
{"reason":"compiler-artifact","executable":"/proj/target/debug/rust-app"}
{"reason":"build-finished"}"#;
        assert_eq!(cargo_executable(output), Some(PathBuf::from("/proj/target/debug/rust-app")));
        assert_eq!(cargo_executable("not json"), None);
    }
}
