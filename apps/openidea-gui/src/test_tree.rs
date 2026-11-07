//! The test-tree data plane (M12): the state the Run window's tree renders,
//! and the app-side parsers that fill it from the five runner families the
//! detectors tag (`is_test` on the run configurations).
//!
//! Execution rides the run-configuration system unchanged (ide-run's
//! ProcessManager; the session shows in Services like any run): the workbench
//! derives the effective command from the config ([`test_run_config`] — the
//! JSON/verbose faces the parsers need, zero toml changes) and feeds the
//! output chunks here. One [`TestRunShared`] per run, shaped like the debug
//! session's `SessionShared` (Mutex state, notified through the bus).
//!
//! Parsers stream per line where the runner streams (`go test -json` events,
//! libtest lines, `pytest -v` rows) and parse whole documents at finish where
//! the runner prints a report at the end (vitest/jest JSON; pytest's FAILURES
//! section). Real runner output is embedded as the test fixtures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

// ---------------------------------------------------------------------------
// The data model the tree renders
// ---------------------------------------------------------------------------

/// One node's live state (the tree's icon face).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestState {
    Pending,
    Running,
    Passed,
    Failed,
    Skipped,
}

/// One leaf of the tree. `id` is the runner's own test path (stable within a
/// run: `TestAdd/two-plus-two`, `tests::broken_subtraction`,
/// `tests/test_calc.py::TestSuite::test_inside_class_fail`); `name` is the
/// display leaf.
#[derive(Clone, Debug, PartialEq)]
pub struct TestCase {
    pub id: String,
    pub name: String,
    pub state: TestState,
    pub duration_ms: Option<f64>,
    /// The runner's failure/skip text (first meaningful lines; capped).
    pub message: Option<String>,
    /// Where the test (or its failure) lives; 0-based line. Relative paths
    /// resolve against the project root (every runner reports those).
    pub file: Option<String>,
    pub line: Option<u32>,
    /// The failure's `expected` / `actual` faces when the message carries
    /// them (the diff overlay's two sides; `None` = no diff view).
    pub expected: Option<String>,
    pub actual: Option<String>,
}

/// One suite (package / file / describe block) with its cases.
#[derive(Clone, Debug, PartialEq)]
pub struct TestSuite {
    pub id: String,
    pub name: String,
    pub state: TestState,
    pub duration_ms: Option<f64>,
    pub cases: Vec<TestCase>,
}

impl TestSuite {
    /// The aggregate state: a failure anywhere fails the suite; all passed
    /// (with cases) passes; any running marks it; a skip; else pending.
    pub fn aggregate(&mut self) {
        self.state = if self.cases.iter().any(|c| c.state == TestState::Failed) {
            TestState::Failed
        } else if !self.cases.is_empty() && self.cases.iter().all(|c| c.state == TestState::Passed) {
            TestState::Passed
        } else if self.cases.iter().any(|c| c.state == TestState::Running) {
            TestState::Running
        } else if self.cases.iter().any(|c| c.state == TestState::Skipped) {
            TestState::Skipped
        } else {
            TestState::Pending
        };
    }
}

/// The totals line: `x passed / y failed / z skipped`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestPhase {
    #[default]
    Starting,
    Running,
    Finished,
}

/// The run's whole face (the tree data + totals), plain data so the parse
/// tests pin it.
#[derive(Clone, Debug, Default)]
pub struct TestRunState {
    pub suites: Vec<TestSuite>,
    pub summary: TestSummary,
    pub phase: TestPhase,
}

impl TestRunState {
    /// Recomputes suite aggregates and the totals (every parse step ends here).
    pub fn recount(&mut self) {
        for suite in &mut self.suites {
            suite.aggregate();
        }
        let mut s = TestSummary::default();
        for suite in &self.suites {
            for case in &suite.cases {
                match case.state {
                    TestState::Passed => s.passed += 1,
                    TestState::Failed => s.failed += 1,
                    TestState::Skipped => s.skipped += 1,
                    _ => {}
                }
            }
        }
        self.summary = s;
    }
}

/// The session the workbench holds (one per live test run): the tree state
/// behind a Mutex and the streaming parser behind another. `feed` takes one
/// output chunk (stdout lines parse; every stream joins the tail); `finish`
/// runs the whole-document parsers (JSON reports, pytest's failure section) —
/// called once on the run's exit.
pub struct TestRunShared {
    pub state: std::sync::Mutex<TestRunState>,
    parser: std::sync::Mutex<RunnerParser>,
    /// The output tail (capped) the finish parsers read (pytest's FAILURES
    /// section and the JSON reports ride the accumulated stream).
    tail: std::sync::Mutex<String>,
    /// A chunk's trailing fragment without its newline yet (ide-run batches
    /// at ~8 ms — chunk boundaries land mid-line, so the streaming parse
    /// joins fragments across chunks the way the Run dock's panel does).
    pending: std::sync::Mutex<String>,
    /// Where the runner's report file lands (`--outputFile` of the JSON
    /// reporters), if the derived command asked for one.
    report_file: Option<PathBuf>,
}

/// 512 KiB of headroom for the finish parsers; older bytes drop.
const TAIL_CAP: usize = 512 * 1024;

impl TestRunShared {
    pub fn new(runner: Runner, report_file: Option<PathBuf>) -> Self {
        Self {
            state: std::sync::Mutex::new(TestRunState { phase: TestPhase::Starting, ..Default::default() }),
            parser: std::sync::Mutex::new(RunnerParser::new(runner)),
            tail: std::sync::Mutex::new(String::new()),
            pending: std::sync::Mutex::new(String::new()),
            report_file,
        }
    }

    /// One output chunk: stdout lines stream-parse (joined across chunk
    /// boundaries); both streams join the tail.
    pub fn feed(&self, stream: crate::run_panel::Stream, text: &str) {
        {
            let mut tail = self.tail.lock().unwrap();
            tail.push_str(text);
            trim_cap(&mut tail);
        }
        if stream == crate::run_panel::Stream::Stdout {
            let complete = {
                let mut pending = self.pending.lock().unwrap();
                pending.push_str(text);
                match pending.rfind('\n') {
                    Some(pos) => {
                        let whole = pending[..=pos].to_owned();
                        *pending = pending[pos + 1..].to_owned();
                        whole
                    }
                    None => return, // one fragment short of a line so far
                }
            };
            let mut parser = self.parser.lock().unwrap();
            let mut state = self.state.lock().unwrap();
            state.phase = TestPhase::Running;
            for line in complete.lines() {
                parser.line(line, &mut state);
            }
            state.recount();
        }
    }

    /// The run exited: the trailing fragment joins, whole-document parsers
    /// run over the tail (and the report file the derived command asked
    /// for), then the totals settle.
    pub fn finish(&self) {
        let tail = {
            let mut tail = self.tail.lock().unwrap();
            let fragment = self.pending.lock().unwrap().clone();
            tail.push_str(&fragment);
            tail.clone()
        };
        let report = self.report_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut parser = self.parser.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        parser.finish(&tail, &report, &mut state);
        state.phase = TestPhase::Finished;
        state.recount();
    }
}

fn trim_cap(buf: &mut String) {
    if buf.len() > TAIL_CAP {
        let drop = buf.len() - TAIL_CAP;
        let cut = buf.ceil_char_boundary(drop).min(buf.len());
        buf.drain(..cut);
    }
}

// ---------------------------------------------------------------------------
// Runners: derivation from a run configuration + the derived test command
// ---------------------------------------------------------------------------

/// The four parser families (one per acceptance leg; ts and js share the
/// JSON-report face, with per-runner CLI flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner {
    /// `go test -json` NDJSON events.
    Go,
    /// `cargo test` libtest lines, or `cargo nextest run` JSON events —
    /// whichever the stream carries (JSON lines win per line).
    Cargo,
    /// `pytest -v` rows + the FAILURES section at finish.
    Pytest,
    /// vitest: `--reporter=json --outputFile` (the jest report shape).
    Vitest,
    /// jest: `--json --outputFile`.
    Jest,
}

/// The runner family for a test-tagged configuration from its shape alone.
/// The npm-script family needs the package body — [`resolve_runner`].
pub fn runner_for(config: &ide_run::RunConfig) -> Option<Runner> {
    match config.kind.as_str() {
        "go" if config.args.first().map(String::as_str) == Some("test") => Some(Runner::Go),
        "cargo" if config.args.first().map(String::as_str) == Some("test") => Some(Runner::Cargo),
        "pytest" => Some(Runner::Pytest),
        _ => None,
    }
}

/// The runner for a test-tagged configuration: the shape's family, or —
/// for the npm-script family — vitest vs jest from package.json's script
/// body (the detector's test scripts are `npm run <script>` rows).
pub fn resolve_runner(config: &ide_run::RunConfig, root: &Path) -> Option<Runner> {
    match runner_for(config) {
        Some(runner) => Some(runner),
        None if config.is_test && npm_script(config).is_some() => Some(js_runner(root, npm_script(config).as_deref())),
        _ => None,
    }
}

/// vitest vs jest from the project's package.json (the `test` script when
/// the configuration's own script name is unknown; vitest is the default
/// face — both report the same JSON shape).
pub fn js_runner(root: &Path, script: Option<&str>) -> Runner {
    let vitest = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|pkg| pkg.get("scripts").and_then(Value::as_object).cloned())
        .and_then(|scripts| {
            let key = script.unwrap_or("test");
            scripts.get(key).and_then(Value::as_str).map(str::to_owned)
        })
        .is_some_and(|body| body.contains("vitest"));
    if vitest { Runner::Vitest } else { Runner::Jest }
}

/// The script a `npm run <script>`-shaped config names (the rerun builder
/// needs it to re-derive the command).
pub fn npm_script(config: &ide_run::RunConfig) -> Option<String> {
    config.args.iter().position(|a| a == "run").and_then(|i| config.args.get(i + 1).cloned())
}

/// One rerun target: everything, one suite (a file/package), or one case.
/// `id`/`name`/`file` come from the tree node; the builder turns them into
/// the runner's own filter face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestFilter {
    All,
    Suite { name: String },
    Case { id: String, name: String, file: Option<String> },
}

/// The single-test filter for a gutter item (the run-gutter ▶ click).
/// go/rust filter by name (the `-run` regex / libtest substring); python
/// builds the full `file::class::name` id; the JSON reporters take the
/// file plus a `-t` title.
pub fn test_filter_for_item(language: &str, rel_path: &str, item: &TestItem) -> TestFilter {
    match language {
        "go" | "rust" => TestFilter::Case { id: item.name.clone(), name: item.name.clone(), file: None },
        "python" => {
            let mut id = String::from(rel_path);
            if let Some(container) = &item.container {
                id.push_str("::");
                id.push_str(container);
            }
            id.push_str("::");
            id.push_str(&item.name);
            TestFilter::Case { id, name: item.name.clone(), file: Some(rel_path.to_owned()) }
        }
        _ => TestFilter::Case { id: item.name.clone(), name: item.name.clone(), file: Some(rel_path.to_owned()) },
    }
}

/// The file-level filter (the ⌥-click face): a file path where the runner
/// takes one; go/rust have no file filter (the whole config runs).
pub fn file_filter(language: &str, rel_path: &str) -> TestFilter {
    match language {
        "go" | "rust" => TestFilter::All,
        _ => TestFilter::Suite { name: rel_path.to_owned() },
    }
}

/// The effective command a test run (or rerun) executes: the config's own
/// command with the runner's structured-output face and the filter applied.
/// Never mutates the stored configuration — a derived clone every time.
pub fn test_run_config(
    config: &ide_run::RunConfig,
    runner: Runner,
    filter: &TestFilter,
    report_file: Option<&Path>,
) -> ide_run::RunConfig {
    let mut out = config.clone();
    out.name = match filter {
        TestFilter::All => config.name.clone(),
        TestFilter::Suite { name } => format!("{} [{name}]", config.name),
        TestFilter::Case { name, .. } => format!("{} [{name}]", config.name),
    };
    match runner {
        Runner::Go => {
            // `go test -json` (inserted once) + `-run` anchored on the path.
            out.args.retain(|a| a != "-json");
            if let Some(i) = out.args.iter().position(|a| a == "test") {
                out.args.insert(i + 1, "-json".into());
            }
            if let TestFilter::Case { id, .. } = filter {
                // Subtests: `^TestAdd$/^two-plus-two$`; tops: `^TestAdd$`.
                let mut parts = id.split('/');
                let head = parts.next().unwrap_or(id.as_str());
                let mut regex = format!("^{head}$");
                for part in parts {
                    regex.push_str(&format!("/^{part}$"));
                }
                push_option(&mut out.args, "-run", &regex);
            }
        }
        Runner::Cargo => {
            // libtest: a positional substring filter on the full path
            // (nextest takes the same positional). A suite rerun names its
            // module prefix.
            match filter {
                TestFilter::Case { id, .. } => out.args.push(id.clone()),
                TestFilter::Suite { name } => out.args.push(name.clone()),
                TestFilter::All => {}
            }
        }
        Runner::Pytest => {
            out.args.retain(|a| a != "-v" && !a.starts_with("--junitxml") && !a.starts_with("--junit-xml"));
            out.args.push("-v".into());
            match filter {
                TestFilter::Case { id, .. } => out.args.push(id.clone()),
                TestFilter::Suite { name } => out.args.push(name.clone()),
                TestFilter::All => {}
            }
        }
        // The structured face: npx drives the runner directly (the script
        // wrapper's flag-forwarding dance avoided) with a JSON report
        // written next to the run.
        Runner::Vitest | Runner::Jest => {
            let binary = if runner == Runner::Vitest { "vitest" } else { "jest" };
            out.command = "npx".into();
            out.args = vec![binary.into()];
            if runner == Runner::Vitest {
                // vitest defaults to watch mode; every face is one-shot.
                out.args.push("run".into());
            } else {
                out.args.push("--json".into());
            }
            match filter {
                TestFilter::Case { name, file, .. } => {
                    if let Some(rel) = file {
                        out.args.push(rel.clone());
                    }
                    out.args.push("-t".into());
                    out.args.push(name.clone());
                }
                // A suite rerun re-runs its file (the runner's selection
                // unit; the `-t` face narrows per case only). The caller
                // passes the suite's file part as `name`.
                TestFilter::Suite { name } => out.args.push(name.clone()),
                TestFilter::All => {}
            }
            let reporter = if runner == Runner::Vitest { "--reporter=json" } else { "--json" };
            if let Some(file) = report_file {
                out.args.push(reporter.into());
                out.args.push("--outputFile".into());
                out.args.push(file.to_string_lossy().into_owned());
            }
        }
    }
    out
}

fn push_option(args: &mut Vec<String>, flag: &str, value: &str) {
    // Replace an existing flag's value; append otherwise (reruns keep one).
    if let Some(i) = args.iter().position(|a| a == flag)
        && let Some(slot) = args.get_mut(i + 1)
    {
        *slot = value.to_owned();
        return;
    }
    args.push(flag.to_owned());
    args.push(value.to_owned());
}

// ---------------------------------------------------------------------------
// The streaming parser (one per runner family, behind the session)
// ---------------------------------------------------------------------------

struct RunnerParser {
    runner: Runner,
    /// go: per-test output buffers (failure detail extraction at the fail
    /// event; the empty key buffers package-level output).
    go_outputs: HashMap<String, String>,
    /// cargo: the failure section being collected (name, text).
    cargo_section: Option<(String, String)>,
}

impl RunnerParser {
    fn new(runner: Runner) -> Self {
        Self { runner, go_outputs: HashMap::new(), cargo_section: None }
    }

    fn line(&mut self, line: &str, state: &mut TestRunState) {
        match self.runner {
            Runner::Go => self.go_line(line, state),
            Runner::Cargo => self.cargo_line(line, state),
            Runner::Pytest => self.pytest_line(line, state),
            Runner::Vitest | Runner::Jest => {} // the report parses at finish
        }
    }

    fn finish(&mut self, stdout: &str, report: &str, state: &mut TestRunState) {
        match self.runner {
            Runner::Go => {}
            Runner::Cargo => {
                // A trailing failure section without a successor settles now.
                if let Some((name, text)) = self.cargo_section.take() {
                    self.cargo_settle_failure(&name, &text, state);
                }
            }
            Runner::Pytest => pytest_failures(stdout, state),
            Runner::Vitest | Runner::Jest => json_report(if report.trim().is_empty() { stdout } else { report }, state),
        }
    }
}

// -- go test -json ----------------------------------------------------------

impl RunnerParser {
    fn go_line(&mut self, line: &str, state: &mut TestRunState) {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else { return };
        let Some(package) = event.get("Package").and_then(Value::as_str) else { return };
        let action = event.get("Action").and_then(Value::as_str).unwrap_or("");
        let test = event.get("Test").and_then(Value::as_str).unwrap_or("");
        let elapsed = event.get("Elapsed").and_then(Value::as_f64).map(|s| s * 1000.0);
        let suite = suite_or_new(state, package);
        if test.is_empty() {
            // Package-level events. A bare `fail` after failed cases is just
            // the test binary's non-zero exit; alone (no case failed) it is
            // the build/run failure face and becomes a synthesized case.
            if action == "fail" {
                let any_failed = suite.cases.iter().any(|c| c.state == TestState::Failed);
                if !any_failed {
                    let text = self.go_outputs.entry(String::new()).or_default().clone();
                    suite.state = TestState::Failed;
                    suite.cases.push(TestCase {
                        id: format!("{package} (build)"),
                        name: "package failed".into(),
                        state: TestState::Failed,
                        duration_ms: elapsed,
                        message: Some(go_first_output(&text).unwrap_or_else(|| "build failed".into())),
                        file: None,
                        line: None,
                        expected: None,
                        actual: None,
                    });
                } else {
                    suite.state = TestState::Failed;
                }
            } else if action == "output"
                && let Some(out) = event.get("Output").and_then(Value::as_str)
            {
                self.go_outputs.entry(String::new()).or_default().push_str(out);
            }
            return;
        }
        match action {
            "run" => {
                upsert_case(suite, test, |c| {
                    c.state = TestState::Running;
                    c.duration_ms = None;
                });
            }
            "pass" => {
                upsert_case(suite, test, |c| {
                    c.state = TestState::Passed;
                    c.duration_ms = elapsed;
                });
            }
            "fail" => {
                let text = self.go_outputs.get(test).cloned().unwrap_or_default();
                let (file, line_no, message) = go_failure_details(&text);
                let (expected, actual) = go_diff(message.as_deref());
                upsert_case(suite, test, |c| {
                    c.state = TestState::Failed;
                    c.duration_ms = elapsed;
                    c.message = message;
                    c.file = file;
                    c.line = line_no;
                    c.expected = expected;
                    c.actual = actual;
                });
            }
            "skip" => {
                let text = self.go_outputs.get(test).cloned().unwrap_or_default();
                upsert_case(suite, test, |c| {
                    c.state = TestState::Skipped;
                    c.duration_ms = elapsed;
                    c.message = go_first_output(&text);
                });
            }
            "output" => {
                if let Some(out) = event.get("Output").and_then(Value::as_str) {
                    let buf = self.go_outputs.entry(test.to_owned()).or_default();
                    buf.push_str(out);
                    if buf.len() > 64 * 1024 {
                        buf.drain(..buf.len() - 64 * 1024);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The suite row for `package`, created (insertion-ordered) once.
fn suite_or_new<'a>(state: &'a mut TestRunState, id: &str) -> &'a mut TestSuite {
    if !state.suites.iter().any(|s| s.id == id) {
        state.suites.push(TestSuite {
            id: id.to_owned(),
            name: id.to_owned(),
            state: TestState::Pending,
            duration_ms: None,
            cases: Vec::new(),
        });
    }
    state.suites.iter_mut().find(|s| s.id == id).expect("just inserted")
}

fn upsert_case(suite: &mut TestSuite, id: &str, f: impl FnOnce(&mut TestCase)) {
    if !suite.cases.iter().any(|c| c.id == id) {
        suite.cases.push(TestCase {
            id: id.to_owned(),
            name: id.rsplit('/').next().unwrap_or(id).to_owned(),
            state: TestState::Pending,
            duration_ms: None,
            message: None,
            file: None,
            line: None,
            expected: None,
            actual: None,
        });
    }
    if let Some(case) = suite.cases.iter_mut().find(|c| c.id == id) {
        f(case);
    }
}

/// The first `file:line: message` of a go failure block (1-based in,
/// 0-based out for the crate-wide coordinate convention).
fn go_failure_details(text: &str) -> (Option<String>, Option<u32>, Option<String>) {
    for raw in text.lines() {
        let line = raw.trim_start();
        if let Some((loc, message)) = line.split_once(": ")
            && let Some((file, num)) = loc.rsplit_once(':')
            && file.ends_with(".go")
            && let Ok(n) = num.parse::<u32>()
        {
            return (Some(file.to_owned()), Some(n.saturating_sub(1)), Some(message.to_owned()));
        }
    }
    (None, None, None)
}

/// `got X, want Y` / `... = X, want Y` — the stdlib testing idiom the
/// samples (and most table tests) speak.
fn go_diff(message: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(message) = message else { return (None, None) };
    let Some((head, want)) = message.split_once(", want ") else { return (None, None) };
    let actual = match head.rsplit_once("got ").or_else(|| head.rsplit_once("= ")) {
        Some((_, a)) => a,
        None => return (None, None),
    };
    (Some(want.to_owned()), Some(actual.to_owned()))
}

/// The first meaningful output line, its `file:line: ` location prefix off
/// (the skip reason / build message face).
fn go_first_output(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("=== RUN") || line.starts_with("--- ") {
            continue;
        }
        return match line.split_once(": ") {
            Some((head, rest))
                if head.rsplit(':').next().is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) =>
            {
                Some(rest.to_owned())
            }
            _ => Some(line.to_owned()),
        };
    }
    None
}

// -- cargo test (libtest lines; nextest JSON events win per line) -----------

impl RunnerParser {
    fn cargo_line(&mut self, line: &str, state: &mut TestRunState) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('{')
            && let Some(event) = serde_json::from_str::<Value>(trimmed).ok()
            && event.get("type").and_then(Value::as_str) == Some("test")
        {
            self.cargo_nextest_event(&event, state);
            return;
        }
        // A failure section's stdout collects until the next section, the
        // summary listing, or the result line (then plain parsing resumes).
        if let Some((current, buf)) = self.cargo_section.as_mut() {
            let ends = trimmed.starts_with("failures:")
                || trimmed.starts_with("test result:")
                || (trimmed.starts_with("---- ") && !line.contains(current.as_str()));
            if ends {
                let text = std::mem::take(buf);
                let name = current.clone();
                self.cargo_section = None;
                self.cargo_settle_failure(&name, &text, state);
            } else {
                buf.push_str(line);
                buf.push('\n');
                return;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("---- ")
            && let Some(name) = rest.strip_suffix(" stdout ----")
        {
            self.cargo_section = Some((name.to_owned(), String::new()));
            return;
        }
        if let Some(rest) = trimmed.strip_prefix("test ") {
            // `test <path> ... ok|FAILED|ignored[, reason]`
            if let Some((name, verdict)) = rest.split_once(" ... ") {
                let id = name.trim();
                let verdict = verdict.trim();
                let (new_state, message) = if verdict == "ok" {
                    (TestState::Passed, None)
                } else if verdict == "FAILED" {
                    (TestState::Failed, None)
                } else if let Some(reason) = verdict.strip_prefix("ignored") {
                    let reason = reason.trim_start_matches(',').trim();
                    (TestState::Skipped, (!reason.is_empty()).then(|| reason.to_owned()))
                } else {
                    return;
                };
                let duration_ms = verdict
                    .rsplit(' ')
                    .next()
                    .and_then(|t| t.strip_suffix('s'))
                    .and_then(|t| t.parse::<f64>().ok())
                    .filter(|_| verdict.starts_with("ok "));
                let suite = suite_or_new(state, "cargo test");
                upsert_case(suite, id, |c| {
                    c.state = new_state;
                    c.duration_ms = duration_ms;
                    c.message = message;
                });
            }
            return;
        }
        if let Some(rest) = trimmed.strip_prefix("test result: ") {
            // `FAILED. 2 passed; 1 failed; 1 ignored; 0 filtered out`
            for (needle, slot) in [("passed", 0usize), ("failed", 1usize), ("ignored", 2usize), ("skipped", 2usize)] {
                if let Some(count) = count_before(rest, needle) {
                    match slot {
                        0 => state.summary.passed += count,
                        1 => state.summary.failed += count,
                        _ => state.summary.skipped += count,
                    }
                }
            }
        }
    }

    /// One `cargo nextest run` JSON test event (`{"type":"test",...}`).
    fn cargo_nextest_event(&mut self, event: &Value, state: &mut TestRunState) {
        let Some(name) = event.get("name").and_then(Value::as_str) else { return };
        let event_kind = event.get("event").and_then(Value::as_str).unwrap_or("");
        let elapsed = event.get("duration_ms").and_then(Value::as_f64);
        let suite = suite_or_new(state, "cargo test");
        match event_kind {
            "ok" => {
                upsert_case(suite, name, |c| {
                    c.state = TestState::Passed;
                    c.duration_ms = elapsed;
                });
            }
            "skipped" => {
                upsert_case(suite, name, |c| {
                    c.state = TestState::Skipped;
                });
            }
            "failed" => {
                upsert_case(suite, name, |c| {
                    c.state = TestState::Failed;
                    c.duration_ms = elapsed;
                });
                let stdout = event.get("stdout").and_then(Value::as_str).unwrap_or("");
                if !stdout.is_empty() {
                    self.cargo_settle_failure(name, stdout, state);
                }
            }
            // retry/attempt and slow/flaky markers: no face.
            _ => {}
        }
    }

    /// Parses one failure block: the panic location and the assertion pair.
    fn cargo_settle_failure(&mut self, name: &str, text: &str, state: &mut TestRunState) {
        let (file, line_no, message, expected, actual) = cargo_failure_details(text);
        let suite = suite_or_new(state, "cargo test");
        upsert_case(suite, name, |c| {
            if c.message.is_none() {
                c.message = message;
            }
            if c.file.is_none() {
                c.file = file;
                c.line = line_no;
            }
            c.expected = expected;
            c.actual = actual;
        });
    }
}

/// The parser-extraction tuple: (location file, 0-based line, message,
/// expected, actual) — one shared face across the runner families.
pub type FailureDetails = (Option<String>, Option<u32>, Option<String>, Option<String>, Option<String>);

/// `panicked at src/lib.rs:16:9:` + the assertion block under it (libtest
/// and nextest print the same shape inside their own framing).
fn cargo_failure_details(text: &str) -> FailureDetails {
    let mut file = None;
    let mut line_no = None;
    let mut message = None;
    let mut expected = None;
    let mut actual = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("left: ") {
            actual = Some(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("right: ") {
            expected = Some(rest.to_owned());
        }
        if message.is_none()
            && let Some(rest) = line.strip_prefix("assertion ")
        {
            message = Some(rest.to_owned());
        }
        if file.is_none()
            && let Some(idx) = line.find("panicked at ")
        {
            let loc = line[idx + "panicked at ".len()..].trim_end().trim_end_matches(':');
            let mut parts = loc.rsplit(':');
            let _col = parts.next();
            if let (Some(row), Some(path)) = (parts.next(), parts.next())
                && path.ends_with(".rs")
                && let Ok(n) = row.parse::<u32>()
            {
                file = Some(path.to_owned());
                line_no = Some(n.saturating_sub(1));
            }
        }
    }
    (file, line_no, message, expected, actual)
}

fn count_before(rest: &str, needle: &str) -> Option<usize> {
    let idx = rest.find(needle)?;
    let num: String = rest[..idx].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>();
    num.chars().rev().collect::<String>().parse().ok()
}

// -- pytest -v ----------------------------------------------------------------

impl RunnerParser {
    fn pytest_line(&mut self, line: &str, state: &mut TestRunState) {
        // `tests/test_calc.py::test_add_pass PASSED  [ 12%]`
        // `tests/test_calc.py::test_skipped SKIPPED (reason) [ 37%]`
        let trimmed = line.trim_start();
        let verb = [" PASSED ", " FAILED ", " SKIPPED ", " XFAILED ", " XPASS ", " ERROR "]
            .iter()
            .filter_map(|v| trimmed.find(v).map(|i| (i, v.trim_matches(' '))))
            .min_by_key(|(i, _)| *i);
        let Some((pos, verb)) = verb else { return };
        let id = trimmed[..pos].trim();
        if !id.contains("::") {
            return;
        }
        let Some((file, rest)) = id.split_once("::") else { return };
        let tail = trimmed[pos + verb.len() + 2..].trim();
        let reason = if tail.starts_with('(') {
            tail.strip_prefix('(').and_then(|r| r.split_once(')')).map(|(r, _)| r.to_owned())
        } else {
            None
        };
        let new_state = match verb {
            "PASSED" | "XPASS" => TestState::Passed,
            "FAILED" | "ERROR" => TestState::Failed,
            _ => TestState::Skipped,
        };
        let suite = suite_or_new(state, file);
        upsert_case(suite, id, |c| {
            c.state = new_state;
            c.name = rest.rsplit("::").next().unwrap_or(rest).to_owned();
            c.message = reason;
        });
    }
}

/// The FAILURES section at finish: per-failed-case location, message and the
/// `assert A == B` pair (pytest's own rendering of both).
fn pytest_failures(stdout: &str, state: &mut TestRunState) {
    for (name, block) in pytest_failure_sections(stdout) {
        // Headers carry pytest's dotted face (`TestSuite.test_x`) while the
        // -v ids are `::`-joined; the leaf name is the shared key.
        let leaf = name.rsplit('.').next().unwrap_or(name.as_str());
        let mut matched = false;
        for suite in &mut state.suites {
            for case in &mut suite.cases {
                if case.id.rsplit("::").next() == Some(leaf) {
                    let (file, line_no, message, expected, actual) = pytest_block_details(&block);
                    if file.is_some() {
                        case.file = file;
                        case.line = line_no;
                    }
                    if message.is_some() {
                        case.message = message;
                    }
                    case.expected = expected;
                    case.actual = actual;
                    matched = true;
                    break;
                }
            }
            if matched {
                break;
            }
        }
    }
}

/// Splits the `_______ name _______` section headers of the FAILURES block
/// (headers wrap one name; a `====` divider ends the last section).
fn pytest_failure_sections(stdout: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        let header = trimmed.starts_with('_') && trimmed.ends_with('_') && {
            let inner = trimmed.trim_matches('_').trim();
            !inner.is_empty() && !inner.contains(' ')
        };
        let divider = trimmed.starts_with('=') && trimmed.ends_with('=');
        if header || divider {
            if let Some(pair) = current.take() {
                out.push(pair);
            }
            if header {
                current = Some((trimmed.trim_matches('_').trim().to_owned(), String::new()));
            }
        } else if let Some((_, block)) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    if let Some(pair) = current.take() {
        out.push(pair);
    }
    out
}

fn pytest_block_details(block: &str) -> FailureDetails {
    let mut file = None;
    let mut line_no = None;
    let mut message = None;
    let mut expected = None;
    let mut actual = None;
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("E ") {
            let rest = rest.trim();
            if let Some(body) = rest.strip_prefix("assert ")
                && let Some((a, b)) = body.split_once(" == ")
            {
                // `assert 4 == 5` — the b side must look like a literal, not
                // the start of an expression chain.
                let b = b.trim();
                if !b.is_empty() && !b.contains(' ') {
                    actual = Some(a.trim().to_owned());
                    expected = Some(b.to_owned());
                }
            }
            if message.is_none() {
                message = Some(rest.to_owned());
            }
        }
        if file.is_none()
            && let Some((loc, kind)) = trimmed.rsplit_once(": ")
            && kind.ends_with("Error")
            && let Some((f, n)) = loc.rsplit_once(':')
            && f.ends_with(".py")
            && let Ok(num) = n.parse::<u32>()
        {
            file = Some(f.to_owned());
            line_no = Some(num.saturating_sub(1));
        }
    }
    (file, line_no, message, expected, actual)
}

// -- vitest / jest JSON report -------------------------------------------------

/// The jest JSON shape (vitest 5's reporter emits the same keys; `skipped`
/// and `pending` both mean a skip). Suites key on the describe ancestors.
fn json_report(text: &str, state: &mut TestRunState) {
    let Some(start) = text.find('{') else { return };
    let Ok(report) = serde_json::from_str::<Value>(&text[start..]) else { return };
    let Some(results) = report.get("testResults").and_then(Value::as_array) else { return };
    for file_result in results {
        let Some(file) = file_result.get("name").and_then(Value::as_str) else { continue };
        let Some(assertions) = file_result.get("assertionResults").and_then(Value::as_array) else { continue };
        let mut per_suite: HashMap<String, Vec<TestCase>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for a in assertions {
            let status = a.get("status").and_then(Value::as_str).unwrap_or("");
            let case_state = match status {
                "passed" => TestState::Passed,
                "failed" => TestState::Failed,
                "skipped" | "pending" | "todo" => TestState::Skipped,
                _ => TestState::Pending,
            };
            let title = a.get("title").and_then(Value::as_str).unwrap_or("?").to_owned();
            let ancestors: Vec<String> = a
                .get("ancestorTitles")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            let suite_name = if ancestors.is_empty() { "tests".to_owned() } else { ancestors.join(" › ") };
            let failure = a
                .get("failureMessages")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(Value::as_str)
                .unwrap_or("");
            let (frame, line_no, message, expected, actual) = js_failure_details(failure);
            let case = TestCase {
                id: format!("{file}::{suite_name}::{title}"),
                name: title,
                state: case_state,
                duration_ms: a.get("duration").and_then(Value::as_f64),
                message,
                file: frame.or_else(|| Some(strip_file_scheme(file))),
                line: line_no,
                expected,
                actual,
            };
            if !per_suite.contains_key(&suite_name) {
                order.push(suite_name.clone());
            }
            per_suite.entry(suite_name).or_default().push(case);
        }
        let file_id = strip_file_scheme(file);
        for name in order {
            let cases = per_suite.remove(&name).unwrap_or_default();
            let suite_id = format!("{file_id}::{name}");
            if let Some(existing) = state.suites.iter_mut().find(|s| s.id == suite_id) {
                existing.cases.extend(cases);
            } else {
                state.suites.push(TestSuite {
                    id: suite_id,
                    name: if name == "tests" { file_id.clone() } else { name },
                    state: TestState::Pending,
                    duration_ms: None,
                    cases,
                });
            }
        }
    }
}

/// A `file://` prefix collapses off (the workbench resolves the rest).
fn strip_file_scheme(path: &str) -> String {
    path.strip_prefix("file://").unwrap_or(path).to_owned()
}

/// The failure message face: first meaningful line, the first non-internals
/// code frame, and the expected/actual pair — jest's `Expected:/Received:`
/// or vitest's `expected A to be B`.
fn js_failure_details(failure: &str) -> FailureDetails {
    let mut message = None;
    let mut file = None;
    let mut line_no = None;
    let mut expected = None;
    let mut actual = None;
    for raw in failure.lines() {
        let line = raw.trim();
        if message.is_none() && !line.is_empty() && !line.starts_with("at ") {
            message = Some(line.to_owned());
        }
        if file.is_none()
            && let Some((path, at_line)) = frame_location(line)
            && is_js_path(&path)
            && !path.contains("node_modules")
        {
            file = Some(path);
            line_no = Some(at_line);
        }
        if let Some(rest) = line.strip_prefix("Expected: ") {
            expected = Some(rest.trim().to_owned());
        }
        if let Some(rest) = line.strip_prefix("Received: ") {
            actual = Some(rest.trim().to_owned());
        }
        if expected.is_none()
            && let Some(rest) = line.strip_prefix("AssertionError: ")
            && let Some(body) = rest.strip_prefix("expected ")
            && let Some((a, b)) = body.split_once(" to be ")
        {
            let b = b.split(" //").next().unwrap_or(b);
            actual = Some(a.trim().to_owned());
            expected = Some(b.trim().to_owned());
        }
    }
    // Only a complete pair renders a diff (one-sided faces mislead).
    if expected.is_some() != actual.is_some() {
        expected = None;
        actual = None;
    }
    (file, line_no, message, expected, actual)
}

/// The `path:line(:col)` token of a stack frame line (jest wraps it in
/// parens, vitest marks it with a `❯` — both reduce to the same token).
fn frame_location(line: &str) -> Option<(String, u32)> {
    let segment = match line.rfind('(') {
        Some(i) => &line[i + 1..],
        None => line,
    };
    let end = segment.find(')').unwrap_or(segment.len());
    let token = segment[..end].trim();
    let token = token.rsplit(' ').next().unwrap_or(token);
    // `path:line:col` (a frame) or `path:line`.
    let (before, last) = token.rsplit_once(':')?;
    if let Ok(_col) = last.parse::<u32>()
        && let Some((path, row)) = before.rsplit_once(':')
        && let Ok(n) = row.parse::<u32>()
    {
        return Some((path.to_owned(), n.saturating_sub(1)));
    }
    let n: u32 = last.parse().ok()?;
    Some((before.to_owned(), n.saturating_sub(1)))
}

fn is_js_path(path: &str) -> bool {
    [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"].iter().any(|ext| path.ends_with(ext))
}

// ---------------------------------------------------------------------------
// The editor-side test-item scan (the gutter's run markers)
// ---------------------------------------------------------------------------

/// One declaration the gutter can mark (0-based line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestItem {
    pub line: u32,
    pub name: String,
    /// The enclosing describe/class name when the grammar exposes it.
    pub container: Option<String>,
}

/// The test declarations of `text` for a language id (the registry's
/// `go`/`rust`/`python`/`typescript`/`javascript` faces). Unknown languages
/// scan nothing.
pub fn test_items(language: &str, text: &str) -> Vec<TestItem> {
    match language {
        "go" => go_items(text),
        "rust" => rust_items(text),
        "python" => python_items(text),
        "typescript" | "tsx" | "javascript" | "jsx" => js_items(text),
        _ => Vec::new(),
    }
}

fn go_items(text: &str) -> Vec<TestItem> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("func ") else { continue };
        for prefix in ["Test", "Benchmark", "Fuzz"] {
            if let Some(name) = take_ident(rest.strip_prefix(prefix).unwrap_or("")) {
                out.push(TestItem { line: index as u32, name: format!("{prefix}{name}"), container: None });
                break;
            }
        }
    }
    out
}

fn rust_items(text: &str) -> Vec<TestItem> {
    let mut out = Vec::new();
    let mut test_attr_pending = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test]") {
            test_attr_pending = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            if test_attr_pending && let Some(name) = take_ident(rest) {
                out.push(TestItem { line: index as u32, name, container: None });
            }
            test_attr_pending = false;
            continue;
        }
        // An attribute only guards its own item (blank/comment gaps tolerated).
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            test_attr_pending = false;
        }
    }
    out
}

fn python_items(text: &str) -> Vec<TestItem> {
    let mut out = Vec::new();
    let mut container: Option<String> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("class ")
            && let Some(name) = take_ident(rest)
        {
            container = Some(name);
        }
        if let Some(rest) = trimmed.strip_prefix("def ")
            && let Some(name) = take_ident(rest)
            && name.starts_with("test")
        {
            let inside = line.starts_with(' ') || line.starts_with('\t');
            out.push(TestItem { line: index as u32, name, container: inside.then(|| container.clone()).flatten() });
        }
    }
    out
}

fn js_items(text: &str) -> Vec<TestItem> {
    let mut out = Vec::new();
    // The describe stack approximates by the line shape (a bare `});`
    // closer pops one level); nested one-line closes are the known limit.
    let mut stack: Vec<String> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("describe(").or_else(|| trimmed.strip_prefix("describe "))
            && let Some(name) = quoted(rest)
        {
            stack.push(name);
        }
        for opener in ["it(", "it.skip(", "test(", "test.skip(", "it.each(", "test.each("] {
            if let Some(rest) = trimmed.strip_prefix(opener)
                && let Some(name) = quoted(rest)
            {
                out.push(TestItem { line: index as u32, name, container: stack.last().cloned() });
                break;
            }
        }
        if trimmed == "});" || trimmed == "})" {
            stack.pop();
        }
    }
    out
}

/// The first identifier of `rest` (`name(`, `name <`, `name`...).
fn take_ident(rest: &str) -> Option<String> {
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// The first quoted string literal of `rest` (`'x'`, `"x"`, `` `x` ``).
fn quoted(rest: &str) -> Option<String> {
    let quote = rest.chars().find(|c| matches!(c, '\'' | '"' | '`'))?;
    let body = &rest[rest.find(quote)? + 1..];
    let end = body.find(quote)?;
    Some(body[..end].to_owned())
}

// ---------------------------------------------------------------------------
// Unit tests: real runner output is the fixture set
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_panel::Stream;

    fn go_config() -> ide_run::RunConfig {
        toml::from_str(
            r#"
id = "go:test"
name = "go test ./..."
kind = "go"
command = "go"
args = ["test", "./..."]
env = {}
detected = true
isTest = true
"#,
        )
        .expect("row parses")
    }

    /// The `go test -json` stream of the /tmp sample (pass/fail/skip/subtests).
    const GO_JSON: &str = r#"{"Time":"2026-11-07T22:52:34.854847+08:00","Action":"start","Package":"m12/goapp/calc"}
{"Time":"2026-11-07T22:52:35.391282+08:00","Action":"run","Package":"m12/goapp/calc","Test":"TestAdd"}
{"Time":"2026-11-07T22:52:35.391344+08:00","Action":"output","Package":"m12/goapp/calc","Test":"TestAdd","Output":"=== RUN   TestAdd\n"}
{"Time":"2026-11-07T22:52:35.391371+08:00","Action":"run","Package":"m12/goapp/calc","Test":"TestAdd/two-plus-two"}
{"Time":"2026-11-07T22:52:35.391399+08:00","Action":"output","Package":"m12/goapp/calc","Test":"TestAdd/two-plus-two","Output":"--- PASS: TestAdd/two-plus-two (0.00s)\n"}
{"Time":"2026-11-07T22:52:35.391405+08:00","Action":"pass","Package":"m12/goapp/calc","Test":"TestAdd/two-plus-two","Elapsed":0}
{"Time":"2026-11-07T22:52:35.391413+08:00","Action":"run","Package":"m12/goapp/calc","Test":"TestAdd/negatives"}
{"Time":"2026-11-07T22:52:35.391421+08:00","Action":"pass","Package":"m12/goapp/calc","Test":"TestAdd/negatives","Elapsed":0}
{"Time":"2026-11-07T22:52:35.391430+08:00","Action":"pass","Package":"m12/goapp/calc","Test":"TestAdd","Elapsed":0}
{"Time":"2026-11-07T22:52:35.391440+08:00","Action":"run","Package":"m12/goapp/calc","Test":"TestBroken"}
{"Time":"2026-11-07T22:52:35.391449+08:00","Action":"output","Package":"m12/goapp/calc","Test":"TestBroken","Output":"    calc_test.go:27: got 9, want 10\n"}
{"Time":"2026-11-07T22:52:35.391457+08:00","Action":"fail","Package":"m12/goapp/calc","Test":"TestBroken","Elapsed":0}
{"Time":"2026-11-07T22:52:35.391470+08:00","Action":"run","Package":"m12/goapp/calc","Test":"TestSkipped"}
{"Time":"2026-11-07T22:52:35.391478+08:00","Action":"output","Package":"m12/goapp/calc","Test":"TestSkipped","Output":"    calc_test.go:31: m12 sample: skipped on purpose\n"}
{"Time":"2026-11-07T22:52:35.391490+08:00","Action":"skip","Package":"m12/goapp/calc","Test":"TestSkipped","Elapsed":0}
{"Time":"2026-11-07T22:52:35.391500+08:00","Action":"run","Package":"m12/goapp/calc","Test":"TestPass"}
{"Time":"2026-11-07T22:52:35.391508+08:00","Action":"pass","Package":"m12/goapp/calc","Test":"TestPass","Elapsed":0}
{"Time":"2026-11-07T22:52:35.392740+08:00","Action":"fail","Package":"m12/goapp/calc","Elapsed":0.538}
"#;

    #[test]
    fn go_stream_builds_the_package_tree() {
        let mut state = TestRunState::default();
        let mut parser = RunnerParser::new(Runner::Go);
        for line in GO_JSON.lines() {
            parser.line(line, &mut state);
        }
        state.recount();
        assert_eq!(state.suites.len(), 1);
        let suite = &state.suites[0];
        assert_eq!(suite.id, "m12/goapp/calc");
        assert_eq!(suite.cases.len(), 6, "TestAdd + 2 subtests + TestBroken + TestSkipped + TestPass");
        let broken = suite.cases.iter().find(|c| c.id == "TestBroken").expect("broken");
        assert_eq!(broken.state, TestState::Failed);
        assert_eq!(broken.file.as_deref(), Some("calc_test.go"));
        assert_eq!(broken.line, Some(26), "1-based 27 -> 0-based");
        assert_eq!(broken.message.as_deref(), Some("got 9, want 10"));
        assert_eq!(broken.expected.as_deref(), Some("10"));
        assert_eq!(broken.actual.as_deref(), Some("9"));
        assert_eq!(suite.cases.iter().filter(|c| c.state == TestState::Passed).count(), 4);
        let skipped = suite.cases.iter().find(|c| c.id == "TestSkipped").expect("skipped");
        assert_eq!(skipped.state, TestState::Skipped);
        assert_eq!(skipped.message.as_deref(), Some("m12 sample: skipped on purpose"));
        assert_eq!(state.summary, TestSummary { passed: 4, failed: 1, skipped: 1 });
    }

    #[test]
    fn go_package_build_failure_becomes_a_case() {
        let mut state = TestRunState::default();
        let mut parser = RunnerParser::new(Runner::Go);
        parser.line(r#"{"Action":"output","Package":"x/y","Output":"cannot find package pkg\n"}"#, &mut state);
        parser.line(r#"{"Action":"fail","Package":"x/y","Elapsed":0.1}"#, &mut state);
        state.recount();
        let suite = &state.suites[0];
        assert_eq!(suite.state, TestState::Failed);
        assert_eq!(suite.cases[0].name, "package failed");
    }

    /// The `cargo test` text of the /tmp sample (failure section included).
    const CARGO_TEST: &str = r#"running 4 tests
test tests::skipped_flaky ... ignored, m12 sample: ignored on purpose
test tests::outer_pass ... ok
test tests::adds_two_numbers ... ok
test tests::broken_subtraction ... FAILED

failures:

---- tests::broken_subtraction stdout ----

thread 'tests::broken_subtraction' (5302923) panicked at src/lib.rs:16:9:
assertion `left == right` failed: 2 + -2 should be 1
  left: 0
 right: 1
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    tests::broken_subtraction

test result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
"#;

    #[test]
    fn cargo_libtest_stream_parses_all_states() {
        let mut state = TestRunState::default();
        let mut parser = RunnerParser::new(Runner::Cargo);
        for line in CARGO_TEST.lines() {
            parser.line(line, &mut state);
        }
        parser.finish("", "", &mut state);
        state.recount();
        let suite = &state.suites[0];
        assert_eq!(suite.id, "cargo test");
        assert_eq!(suite.cases.len(), 4);
        let broken = suite.cases.iter().find(|c| c.id == "tests::broken_subtraction").expect("broken");
        assert_eq!(broken.state, TestState::Failed);
        assert_eq!(broken.file.as_deref(), Some("src/lib.rs"));
        assert_eq!(broken.line, Some(15));
        assert_eq!(broken.message.as_deref(), Some("`left == right` failed: 2 + -2 should be 1"));
        assert_eq!(broken.expected.as_deref(), Some("1"));
        assert_eq!(broken.actual.as_deref(), Some("0"));
        let skipped = suite.cases.iter().find(|c| c.id == "tests::skipped_flaky").expect("skipped");
        assert_eq!(skipped.state, TestState::Skipped);
        assert_eq!(skipped.message.as_deref(), Some("m12 sample: ignored on purpose"));
        assert_eq!(state.summary, TestSummary { passed: 2, failed: 1, skipped: 1 });
    }

    /// A `cargo nextest run` event stream (the JSON face wins per line).
    const NEXTEST_JSON: &str = r#"{"type":"test","event":"ok","name":"tests::adds_two_numbers","duration_ms":0.4}
{"type":"test","event":"failed","name":"tests::broken_subtraction","duration_ms":1.2,"stdout":"thread 'tests::broken_subtraction' panicked at src/lib.rs:16:9:\nassertion `left == right` failed\n left: 0\nright: 1\n"}
{"type":"test","event":"skipped","name":"tests::skipped_flaky"}
"#;

    #[test]
    fn cargo_nextest_events_parse_beside_libtest() {
        let mut state = TestRunState::default();
        let mut parser = RunnerParser::new(Runner::Cargo);
        for line in NEXTEST_JSON.lines() {
            parser.line(line, &mut state);
        }
        state.recount();
        let suite = &state.suites[0];
        assert_eq!(suite.cases.len(), 3);
        let broken = suite.cases.iter().find(|c| c.id == "tests::broken_subtraction").expect("failed");
        assert_eq!(broken.state, TestState::Failed);
        assert_eq!(broken.duration_ms, Some(1.2));
        assert_eq!(broken.file.as_deref(), Some("src/lib.rs"));
        assert_eq!(broken.expected.as_deref(), Some("1"));
        assert_eq!(broken.actual.as_deref(), Some("0"));
        assert_eq!(state.summary.passed, 1);
        assert_eq!(state.summary.skipped, 1);
    }

    /// The `pytest -v` stream of the /tmp sample.
    const PYTEST_V: &str = r#"tests/test_calc.py::test_add_pass PASSED                                 [ 12%]
tests/test_calc.py::test_broken FAILED                                   [ 25%]
tests/test_calc.py::test_skipped SKIPPED (m12 sample: skipped on pur...) [ 37%]
tests/test_calc.py::TestSuite::test_inside_class_pass PASSED             [ 50%]
tests/test_calc.py::TestSuite::test_inside_class_fail FAILED             [ 62%]
tests/test_calc.py::test_parametrized[1-1-2] PASSED                      [ 75%]
tests/test_calc.py::test_parametrized[2-3-5] PASSED                      [ 87%]
tests/test_calc.py::test_parametrized[10--1-9] PASSED                    [100%]
"#;

    /// The FAILURES + summary sections pytest appends.
    const PYTEST_FAILURES: &str = r#"_________________________________ test_broken __________________________________

    def test_broken():
>       assert add(2, 2) == 5, "2+2 should be 5"
E       AssertionError: 2+2 should be 5
E       assert 4 == 5
E        +  where 4 = add(2, 2)

tests/test_calc.py:11: AssertionError
_______________________ TestSuite.test_inside_class_fail _______________________

    def test_inside_class_fail(self):
>       assert mul(2, 3) == 7
E       assert 6 == 7
E        +  where 6 = mul(2, 3)

tests/test_calc.py:24: AssertionError
=========================== short test summary info ============================
FAILED tests/test_calc.py::test_broken - AssertionError: 2+2 should be 5
FAILED tests/test_calc.py::TestSuite::test_inside_class_fail - assert 6 == 7
"#;

    #[test]
    fn pytest_stream_and_failure_sections_agree() {
        let mut state = TestRunState::default();
        let mut parser = RunnerParser::new(Runner::Pytest);
        for line in PYTEST_V.lines() {
            parser.line(line, &mut state);
        }
        state.recount();
        assert_eq!(state.suites.len(), 1, "one file, one suite");
        assert_eq!(state.summary, TestSummary { passed: 5, failed: 2, skipped: 1 });
        let skipped = state.suites[0].cases.iter().find(|c| c.id.ends_with("::test_skipped")).expect("skipped");
        assert_eq!(skipped.message.as_deref(), Some("m12 sample: skipped on pur..."), "the reason rides the -v row");
        let param = state.suites[0].cases.iter().find(|c| c.id.ends_with("::test_parametrized[2-3-5]"));
        assert!(param.is_some(), "parameterized ids stay distinct cases");

        pytest_failures(PYTEST_FAILURES, &mut state);
        state.recount();
        let broken = state.suites[0].cases.iter().find(|c| c.id.ends_with("::test_broken")).expect("broken");
        assert_eq!(broken.file.as_deref(), Some("tests/test_calc.py"));
        assert_eq!(broken.line, Some(10));
        assert_eq!(broken.expected.as_deref(), Some("5"));
        assert_eq!(broken.actual.as_deref(), Some("4"));
        assert_eq!(broken.message.as_deref(), Some("AssertionError: 2+2 should be 5"));
        let inside =
            state.suites[0].cases.iter().find(|c| c.id.ends_with("::test_inside_class_fail")).expect("class case");
        assert_eq!(inside.expected.as_deref(), Some("7"));
        assert_eq!(inside.actual.as_deref(), Some("6"));
    }

    /// The vitest 5 JSON report the /tmp sample wrote (`--outputFile`).
    const VITEST_JSON: &str = r#"{
  "numTotalTestSuites": 2, "numPassedTestSuites": 1, "numFailedTestSuites": 1,
  "numPendingTestSuites": 0, "numTotalTests": 6, "numPassedTests": 4,
  "numFailedTests": 1, "numPendingTests": 1, "numTodoTests": 0,
  "snapshot": {}, "startTime": 1762508000000, "success": false,
  "testResults": [
    {
      "assertionResults": [
        { "ancestorTitles": ["math.add"], "fullName": "math.add adds two numbers", "status": "passed",
          "title": "adds two numbers", "duration": 1.2, "failureMessages": [], "meta": {} },
        { "ancestorTitles": ["math.add"], "fullName": "math.add broken case", "status": "failed",
          "title": "broken case", "duration": 2.5,
          "failureMessages": ["AssertionError: expected 4 to be 5 // Object.is equality\n ❯ src/math.test.ts:12:24\n      at src/math.test.ts:12:24"],
          "meta": {} },
        { "ancestorTitles": ["math.add"], "fullName": "math.add skipped case", "status": "skipped",
          "title": "skipped case", "duration": 0, "failureMessages": [], "meta": {} },
        { "ancestorTitles": ["math.add"], "fullName": "math.add parametrized 1+1", "status": "passed",
          "title": "parametrized 1+1", "duration": 0.8, "failureMessages": [], "meta": {} },
        { "ancestorTitles": ["math.add"], "fullName": "math.add parametrized 2+3", "status": "passed",
          "title": "parametrized 2+3", "duration": 0.7, "failureMessages": [], "meta": {} },
        { "ancestorTitles": ["math.edge"], "fullName": "math.edge zero case", "status": "passed",
          "title": "zero case", "duration": 0.6, "failureMessages": [], "meta": {} }
      ],
      "startTime": 1762508000000, "endTime": 1762508001500, "status": "failed",
      "message": "", "name": "/private/tmp/m12-tests/ts-app/src/math.test.ts"
    }
  ]
}"#;

    #[test]
    fn vitest_json_builds_describe_suites() {
        let mut state = TestRunState::default();
        json_report(VITEST_JSON, &mut state);
        state.recount();
        assert_eq!(state.suites.len(), 2, "math.add + math.edge");
        let add = state.suites.iter().find(|s| s.name == "math.add").expect("describe suite");
        assert_eq!(add.cases.len(), 5);
        let broken = add.cases.iter().find(|c| c.name == "broken case").expect("broken");
        assert_eq!(broken.state, TestState::Failed);
        assert_eq!(broken.expected.as_deref(), Some("5"));
        assert_eq!(broken.actual.as_deref(), Some("4"));
        assert_eq!(broken.file.as_deref(), Some("src/math.test.ts"), "the frame's project face wins");
        assert_eq!(broken.line, Some(11));
        assert_eq!(state.summary, TestSummary { passed: 4, failed: 1, skipped: 1 });
    }

    /// jest's shape: `pending` skips, `Expected/Received` pairs, node frames.
    const JEST_JSON: &str = r#"{"numFailedTestSuites":1,"numFailedTests":1,"numPassedTestSuites":0,"numPassedTests":3,
"numPendingTestSuites":0,"numPendingTests":1,"numRuntimeErrorTestSuites":0,"numTodoTests":0,"numTotalTestSuites":1,
"numTotalTests":5,"openHandles":[],"snapshot":{},"startTime":1762508000000,"success":false,"wasInterrupted":false,
"testResults":[{"assertionResults":[
{"ancestorTitles":["math.add"],"duration":2,"failureMessages":["Error: expect(received).toBe(expected) // Object.is equality\n\nExpected: 5\nReceived: 4\n    at Object.<anonymous> (/tmp/m12-tests/js-app/__tests__/math.test.js:7:24)"],"fullName":"math.add broken case","location":null,"status":"failed","title":"broken case"},
{"ancestorTitles":["math.add"],"duration":1,"failureMessages":[],"fullName":"math.add adds two numbers","location":null,"status":"passed","title":"adds two numbers"},
{"ancestorTitles":["math.add"],"duration":0,"failureMessages":[],"fullName":"math.add skipped case","location":null,"status":"pending","title":"skipped case"},
{"ancestorTitles":["math.add"],"duration":1,"failureMessages":[],"fullName":"math.add parametrized 1+1","location":null,"status":"passed","title":"parametrized 1+1"},
{"ancestorTitles":["math.add"],"duration":1,"failureMessages":[],"fullName":"math.add parametrized 2+3","location":null,"status":"passed","title":"parametrized 2+3"}],
"endTime":1762508001500,"message":"","name":"/tmp/m12-tests/js-app/__tests__/math.test.js","startTime":1762508000000,"status":"failed","summary":""}]}"#;

    #[test]
    fn jest_json_parses_pending_and_expected_received() {
        let mut state = TestRunState::default();
        json_report(JEST_JSON, &mut state);
        state.recount();
        let add = state.suites.iter().find(|s| s.name == "math.add").expect("describe suite");
        let broken = add.cases.iter().find(|c| c.name == "broken case").expect("broken");
        assert_eq!(broken.state, TestState::Failed);
        assert_eq!(broken.expected.as_deref(), Some("5"));
        assert_eq!(broken.actual.as_deref(), Some("4"));
        assert_eq!(broken.file.as_deref(), Some("/tmp/m12-tests/js-app/__tests__/math.test.js"));
        assert_eq!(broken.line, Some(6));
        let skipped = add.cases.iter().find(|c| c.name == "skipped case").expect("skipped");
        assert_eq!(skipped.state, TestState::Skipped);
        assert_eq!(state.summary, TestSummary { passed: 3, failed: 1, skipped: 1 });
    }

    #[test]
    fn test_run_config_derives_the_json_faces() {
        let go = go_config();
        let derived = test_run_config(&go, Runner::Go, &TestFilter::All, None);
        assert_eq!(derived.args, vec!["test", "-json", "./..."]);
        let rerun = test_run_config(
            &go,
            Runner::Go,
            &TestFilter::Case { id: "TestAdd/two-plus-two".into(), name: "two-plus-two".into(), file: None },
            None,
        );
        // Flags after the package list are valid go syntax (push_option
        // appends; reruns keep a single -run).
        assert_eq!(rerun.args, vec!["test", "-json", "./...", "-run", "^TestAdd$/^two-plus-two$"]);
        assert_eq!(rerun.name, "go test ./... [two-plus-two]");

        let pytest: ide_run::RunConfig = toml::from_str(
            r#"
id = "pytest"
name = "pytest"
kind = "pytest"
command = "pytest"
args = []
env = {}
detected = true
isTest = true
"#,
        )
        .expect("row parses");
        let verbose = test_run_config(&pytest, Runner::Pytest, &TestFilter::All, None);
        assert_eq!(verbose.args, vec!["-v"]);
        let one = test_run_config(
            &pytest,
            Runner::Pytest,
            &TestFilter::Case {
                id: "tests/test_calc.py::TestSuite::test_inside_class_fail".into(),
                name: "x".into(),
                file: None,
            },
            None,
        );
        assert_eq!(one.args, vec!["-v", "tests/test_calc.py::TestSuite::test_inside_class_fail"]);

        let npm: ide_run::RunConfig = toml::from_str(
            r#"
id = "npm:test"
name = "npm run test"
kind = "npm"
command = "npm"
args = ["run", "test"]
env = {}
detected = true
isTest = true
"#,
        )
        .expect("row parses");
        let vitest = test_run_config(&npm, Runner::Vitest, &TestFilter::All, Some(Path::new("/tmp/report.json")));
        assert_eq!(vitest.command, "npx");
        assert_eq!(vitest.args, vec!["vitest", "run", "--reporter=json", "--outputFile", "/tmp/report.json"]);
        let case = test_run_config(
            &npm,
            Runner::Vitest,
            &TestFilter::Case {
                id: "broken case".into(),
                name: "broken case".into(),
                file: Some("src/math.test.ts".into()),
            },
            None,
        );
        // No report file, no reporter flags (the human stream stays).
        assert_eq!(case.args, ["vitest", "run", "src/math.test.ts", "-t", "broken case"]);
    }

    #[test]
    fn runner_derivation_maps_the_tagged_kinds() {
        let go = go_config();
        assert_eq!(runner_for(&go), Some(Runner::Go));
        let cargo: ide_run::RunConfig = toml::from_str(
            r#"
id = "cargo:test"
name = "cargo test"
kind = "cargo"
command = "cargo"
args = ["test"]
env = {}
detected = true
isTest = true
"#,
        )
        .expect("row parses");
        assert_eq!(runner_for(&cargo), Some(Runner::Cargo));
        let npm: ide_run::RunConfig = toml::from_str(
            r#"
id = "npm:test"
name = "npm run test"
kind = "npm"
command = "npm"
args = ["run", "test"]
env = {}
detected = true
isTest = true
"#,
        )
        .expect("row parses");
        // The npm family resolves through the package body (see resolve_runner).
        assert_eq!(runner_for(&npm), None);
        assert_eq!(npm_script(&npm).as_deref(), Some("test"));
        let plain: ide_run::RunConfig = toml::from_str(
            r#"
id = "cargo:run"
name = "cargo run"
kind = "cargo"
command = "cargo"
args = ["run"]
env = {}
detected = true
isTest = false
"#,
        )
        .expect("row parses");
        assert_eq!(runner_for(&plain), None);
        // resolve_runner reads the script body: vitest and jest faces.
        let dir = std::env::temp_dir().join(format!("openidea-gui-js-runner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts": {"test": "vitest run"}}"#).unwrap();
        assert_eq!(resolve_runner(&npm, &dir), Some(Runner::Vitest));
        std::fs::write(dir.join("package.json"), r#"{"scripts": {"test": "jest"}}"#).unwrap();
        assert_eq!(resolve_runner(&npm, &dir), Some(Runner::Jest));
        std::fs::remove_dir_all(&dir).ok();
        // A go config resolves from its shape without touching the disk.
        assert_eq!(resolve_runner(&go, &dir), Some(Runner::Go));
    }

    #[test]
    fn gutter_items_map_to_the_runner_filters() {
        let item = TestItem { line: 4, name: "test_inside".into(), container: Some("TestSuite".into()) };
        let filter = test_filter_for_item("python", "tests/test_calc.py", &item);
        assert_eq!(
            filter,
            TestFilter::Case {
                id: "tests/test_calc.py::TestSuite::test_inside".into(),
                name: "test_inside".into(),
                file: Some("tests/test_calc.py".into())
            }
        );
        let go_item = TestItem { line: 2, name: "TestAdd".into(), container: None };
        assert_eq!(
            test_filter_for_item("go", "calc/calc_test.go", &go_item),
            TestFilter::Case { id: "TestAdd".into(), name: "TestAdd".into(), file: None }
        );
        let js_item = TestItem { line: 1, name: "works".into(), container: Some("math".into()) };
        assert_eq!(
            test_filter_for_item("typescript", "src/math.test.ts", &js_item),
            TestFilter::Case { id: "works".into(), name: "works".into(), file: Some("src/math.test.ts".into()) }
        );
        assert_eq!(file_filter("go", "calc/calc_test.go"), TestFilter::All);
        assert_eq!(
            file_filter("python", "tests/test_calc.py"),
            TestFilter::Suite { name: "tests/test_calc.py".into() }
        );
    }

    // -- the gutter scanner ---------------------------------------------------

    #[test]
    fn test_items_scan_the_four_grammar_faces() {
        let go = "package calc\n\nfunc TestAdd(t *testing.T) {\n}\n\nfunc helper() {}\n\nfunc TestBroken(t *testing.T) {\n}\n";
        let items = test_items("go", go);
        assert_eq!(
            items,
            vec![
                TestItem { line: 2, name: "TestAdd".into(), container: None },
                TestItem { line: 7, name: "TestBroken".into(), container: None }
            ]
        );

        let rust = "#[test]\nfn adds() {\n}\n\n#[test]\n#[ignore]\nfn skipped() {\n}\n\nfn not_a_test() {}\n";
        let items = test_items("rust", rust);
        assert_eq!(
            items,
            vec![
                TestItem { line: 1, name: "adds".into(), container: None },
                TestItem { line: 6, name: "skipped".into(), container: None }
            ]
        );

        let py = "def test_top():\n    pass\n\nclass TestSuite:\n    def test_inside(self):\n        pass\n\ndef helper():\n    pass\n";
        let items = test_items("python", py);
        assert_eq!(
            items,
            vec![
                TestItem { line: 0, name: "test_top".into(), container: None },
                TestItem { line: 4, name: "test_inside".into(), container: Some("TestSuite".into()) }
            ]
        );

        let ts = "describe('math', () => {\n  it('works', () => {});\n  it.skip('later', () => {});\n});\nit('outside', () => {});\n";
        let items = test_items("typescript", ts);
        assert_eq!(
            items,
            vec![
                TestItem { line: 1, name: "works".into(), container: Some("math".into()) },
                TestItem { line: 2, name: "later".into(), container: Some("math".into()) },
                TestItem { line: 4, name: "outside".into(), container: None }
            ]
        );

        assert!(test_items("plaintext", "anything").is_empty());
    }

    #[test]
    fn feed_and_finish_settle_the_whole_session() {
        let shared = TestRunShared::new(Runner::Go, None);
        shared.feed(Stream::Stdout, GO_JSON);
        {
            let state = shared.state.lock().unwrap();
            assert_eq!(state.phase, TestPhase::Running);
            assert_eq!(state.summary, TestSummary { passed: 4, failed: 1, skipped: 1 });
        }
        shared.finish();
        let state = shared.state.lock().unwrap();
        assert_eq!(state.phase, TestPhase::Finished);
        assert_eq!(state.summary, TestSummary { passed: 4, failed: 1, skipped: 1 });
    }

    #[test]
    fn chunks_split_mid_line_still_parse() {
        // ide-run batches at ~8 ms: a chunk boundary can land inside a
        // `pytest -v` row; the streaming parse joins fragments.
        let shared = TestRunShared::new(Runner::Pytest, None);
        let (a, b) = PYTEST_V.split_at(PYTEST_V.len() / 2);
        shared.feed(Stream::Stdout, a);
        shared.feed(Stream::Stdout, b);
        shared.finish();
        let state = shared.state.lock().unwrap();
        assert_eq!(state.summary, TestSummary { passed: 5, failed: 2, skipped: 1 }, "every row parsed exactly once");
    }

    #[test]
    fn the_output_tail_stays_capped() {
        let mut buf = "x".repeat(TAIL_CAP + 4096);
        trim_cap(&mut buf);
        assert_eq!(buf.len(), TAIL_CAP);
    }

    #[test]
    fn js_locations_skip_internals_frames() {
        let (file, line, message, expected, actual) = js_failure_details(
            "Error: expect(received).toBe(expected)\n\nExpected: 5\nReceived: 4\n    at Object.toBe (/prefix/node_modules/expect/build/index.js:101:1)\n    at Object.<anonymous> (/tmp/js-app/__tests__/math.test.js:7:24)",
        );
        assert_eq!(message.as_deref(), Some("Error: expect(received).toBe(expected)"));
        assert_eq!(file.as_deref(), Some("/tmp/js-app/__tests__/math.test.js"), "node_modules frames never win");
        assert_eq!(line, Some(6));
        assert_eq!(expected.as_deref(), Some("5"));
        assert_eq!(actual.as_deref(), Some("4"));
    }
}
