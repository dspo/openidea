//! PTY transport: spawn the shell, expose writer/reader/resize/wait/terminate.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, PtySize, native_pty_system};

use crate::process::{ProcessOutput, TerminalProcess};
use crate::shell_integration;

/// Grace between the SIGTERM pass and the SIGKILL pass of [`Pty::terminate`]:
/// long enough for editors and TUIs to restore the terminal, short enough
/// that a close feels immediate.
const SIGTERM_GRACE: Duration = Duration::from_millis(100);

/// Poll cadence of [`Pty::wait`]. The exit watcher sleeps here between
/// non-blocking reaps so [`Pty::terminate`] can take the child lock promptly;
/// a blocking `wait` held that lock for the child's whole life, which made
/// termination from the UI thread impossible.
const REAP_POLL: Duration = Duration::from_millis(50);

pub struct Pty {
    /// Keystroke/paste bytes land on this queue and a dedicated thread
    /// drains it — a blocking `write_all` on the UI thread would hold the
    /// terminal lock and stall against a child that never reads stdin.
    writer_tx: async_channel::Sender<Vec<u8>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    /// Cloned from the child at spawn: the lock-free kill handle
    /// [`Pty::terminate`] falls back to.
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The shell's pid, when the platform exposes it: the root of the
    /// descendant sweep in [`Pty::terminate`].
    pid: Option<u32>,
    /// Taken by [`TerminalProcess::attach`] to start the reader thread.
    reader: Mutex<Option<Box<dyn Read + Send>>>,
    /// `None` once reaped or terminated: this is the handle every kill path
    /// takes, and dropping it is what lets the process go.
    child: Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
    closed: AtomicBool,
}

impl Pty {
    /// Spawn `shell` on a new pty. The reader end is held internally and
    /// starts with [`TerminalProcess::attach`].
    pub fn spawn_program(
        program: &str,
        args: &[&str],
        cwd: &std::path::Path,
        env: &[(String, String)],
        cols: u16,
        rows: u16,
    ) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("openpty: {e}"))?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        // Caller pairs ride OVER the process-inherited world
        // (last-entry-wins against same-named inherited variables).
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // The honest name: libghostty-vt implements the kitty keyboard
        // protocol natively, so no iTerm spoofing is wanted here.
        cmd.env("TERM_PROGRAM", "openidea-gui");
        // OSC 7 pwd reporting (the new-tab cwd): shell integration injects a
        // precmd hook without touching the user's rc files.
        let mut extra_args: Vec<String> = Vec::new();
        if let Some(injected) = shell_integration::inject(&shell_integration::detect(program), args) {
            for (k, v) in injected.env {
                cmd.env(k, v);
            }
            extra_args = injected.args;
        }
        for a in &extra_args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| format!("spawn `{program}`: {e}"))?;
        // Drop the parent-side slave handle so the pty EOFs when the child's
        // session dies.
        drop(pair.slave);

        let writer = pair.master.take_writer().map_err(|e| format!("pty writer: {e}"))?;
        let reader = pair.master.try_clone_reader().map_err(|e| format!("pty reader: {e}"))?;
        let killer = Mutex::new(child.clone_killer());
        let pid = child.process_id();
        // Writer thread: drain the queue off the caller's thread. The
        // channel closes when the Pty drops its send-end, ending the thread.
        let (writer_tx, writer_rx) = async_channel::unbounded::<Vec<u8>>();
        std::thread::Builder::new()
            .name("terminal-ghostty-writer".into())
            .spawn(move || {
                let mut w = writer;
                while let Ok(bytes) = writer_rx.recv_blocking() {
                    let _ = w.write_all(&bytes);
                    let _ = w.flush();
                }
            })
            .map_err(|e| format!("writer thread: {e}"))?;
        Ok(Self {
            writer_tx,
            master: Mutex::new(pair.master),
            reader: Mutex::new(Some(reader)),
            child: Mutex::new(Some(child)),
            killer,
            pid,
            closed: AtomicBool::new(false),
        })
    }

    /// Spawn `$SHELL` (or any program) at `cwd` — the built-in
    /// [`TerminalProcess`] for shell tabs.
    pub fn spawn_shell(
        program: &str,
        args: &[&str],
        cwd: &std::path::Path,
        env: &[(String, String)],
        cols: u16,
        rows: u16,
    ) -> Result<Arc<dyn TerminalProcess>, String> {
        Ok(Arc::new(Self::spawn_program(program, args, cwd, env, cols, rows)?))
    }

    pub fn write(&self, bytes: &[u8]) {
        // The queue is unbounded, so this never blocks the UI thread; the
        // writer thread owns the blocking `write_all`.
        let _ = self.writer_tx.send_blocking(bytes.to_vec());
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let master = self.master.lock().unwrap();
        let _ = master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }

    /// Terminate the shell. Idempotent and safe from any thread, but
    /// **blocking**: the sweep costs a sysinfo snapshot plus the SIGTERM
    /// grace window (~100ms+), so callers must run it off the UI thread
    /// ([`GhosttyView::kill`](crate::GhosttyView::kill) spawns it detached;
    /// the `Pty::Drop` fallback blocks whatever thread drops the view).
    /// Without a terminate, a closed tab leaves the shell — and both of its
    /// watcher threads — running for the life of the process.
    pub fn terminate(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Interactive shells give every job its own process group, so one
        // group signal leaks background jobs (`npm run dev &` would survive
        // the tab closing). Sweep the shell's whole descendant tree across
        // their groups, plus the pty's current foreground group:
        // SIGTERM first — editors and TUIs restore the terminal — then
        // SIGKILL whatever ignored the grace window.
        let fg_group = self.master.lock().unwrap().process_group_leader();
        if let Some(pid) = self.pid {
            let descendants = descendant_pids(pid);
            let mut targets: Vec<i32> = descendants.iter().map(|p| *p as i32).collect();
            targets.push(pid as i32);
            for t in &targets {
                unsafe { libc::kill(*t, libc::SIGTERM) };
            }
            // The foreground group needs the negative-pgid form — the plain
            // call would only reach the group leader, not its members.
            if let Some(group) = fg_group {
                unsafe { libc::kill(-group, libc::SIGTERM) };
            }
            std::thread::sleep(SIGTERM_GRACE);
            for t in &targets {
                if alive(*t) {
                    unsafe { libc::kill(*t, libc::SIGKILL) };
                }
            }
            if let Some(group) = fg_group {
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
            // The shell's own group: background jobs the sweep's parent
            // chain cannot see are still members of it.
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        // Belt and braces regardless of pid visibility: the direct kill
        // handle lands even when the sweep could not.
        let _ = self.killer.lock().unwrap().kill();
    }

    /// Reap the child, polling so the lock is never held across a blocking
    /// wait. Returns the exit code when the child is gone, or `None` when it
    /// was terminated by us.
    pub fn wait(&self) -> Option<u32> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            // `Some(_)` = the child is gone (the inner code is `None` for a
            // reaped-or-failed wait); `None` = still running.
            let reaped: Option<Option<u32>> = {
                let mut guard = self.child.lock().unwrap();
                let child = guard.as_mut()?;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        Some(Some(status.exit_code()))
                    }
                    Ok(None) => None,
                    Err(_) => {
                        *guard = None;
                        Some(None)
                    }
                }
            };
            match reaped {
                Some(code) => return code,
                None => std::thread::sleep(REAP_POLL),
            }
        }
    }
}

/// Every live pid whose parent chain reaches `root` (the shell), from one
/// sysinfo snapshot.
fn descendant_pids(root: u32) -> Vec<u32> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let processes = system.processes();
    let root_pid = Pid::from_u32(root);
    let parent_of: std::collections::HashMap<Pid, Option<Pid>> =
        processes.values().map(|p| (p.pid(), p.parent())).collect();
    let mut out = Vec::new();
    for &pid in processes.keys() {
        let mut cur = Some(pid);
        loop {
            match cur {
                Some(p) if p == root_pid => {
                    out.push(pid.as_u32());
                    break;
                }
                // `parent == self` is a cycle guard, not a hit.
                Some(p) => cur = parent_of.get(&p).copied().flatten().filter(|n| *n != p),
                None => break,
            }
        }
    }
    out
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

impl TerminalProcess for Pty {
    fn attach(self: Arc<Self>, out: ProcessOutput) {
        // Reader: blocking pty read → folded into the terminal.
        let this = Arc::clone(&self);
        let out_for_exit = out.clone();
        if let Some(mut reader) = self.reader.lock().unwrap().take() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => out.output(&buf[..n]),
                    }
                }
            });
        }
        // Waiter: `wait` polls (never holds the child lock across a block)
        // and returns `None` only when the host terminated the process — a
        // host-side kill is close semantics, not a reportable exit.
        std::thread::spawn(move || {
            if let Some(code) = this.wait() {
                out_for_exit.exited(Some(code));
            }
        });
    }

    fn write(&self, bytes: &[u8]) {
        Pty::write(self, bytes)
    }

    fn resize(&self, cols: u16, rows: u16) {
        Pty::resize(self, cols, rows)
    }

    fn kill(&self) {
        Pty::terminate(self)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Belt and braces: a view dropped without `terminate` must not orphan
        // the shell.
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::descendant_pids;

    /// A spawned child is in the sweep's parent chain — the exact regression
    /// behind a single-group kill (a lone group signal misses `sleep &`,
    /// which sits in its own group).
    #[test]
    fn a_spawned_child_is_a_descendant() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().expect("spawn the probe child");
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let found = descendant_pids(std::process::id());
        assert!(found.contains(&pid), "a spawned child is in the sweep: {found:?}");
        let _ = child.kill();
        let _ = child.wait();
    }
}
