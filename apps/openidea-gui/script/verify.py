#!/usr/bin/env python3
"""The M8 regression-harness driver.

Launches the app in an isolated scripted session (its own settings store, a
/tmp scratch project) and drives it through the M5–M7 synthetic-input
discipline: CGEvents for modifiers/navigation/keystrokes, **clipboard paste
for all text** (the user's IME composes printable synthetic keys, so paste is
the only trustworthy text channel). Screenshots are window-scoped, never
full-screen.

Modes:
  perf  — the budget scenario: HUD samples, completion first frame, Find
          first response, scroll-during-index frames (see perf/budgets.json)
  smoke — the fixed screenshot-state set (verify.sh stage 2)
  soak  — the long-session loop with per-minute RSS sampling (M8-REPORT §soak)

Every mode writes its evidence under the --outdir (default /tmp/m8-verify).
Only processes this script starts are killed.
"""

import argparse
import os
import re
import subprocess
import sys
import time
from pathlib import Path

import Quartz

APP_TITLE = "OpenIDEA"

# Keycodes (macOS virtual keys) for everything we post.
KEY = {
    "f12": 111,
    "f6": 97,
    "v": 9,
    "z": 6,
    "f": 3,
    "o": 31,
    "w": 13,
    "t": 17,
    "1": 18,
    "4": 21,
    "r": 15,
    "d": 2,
    "k": 40,
    "s": 1,
    "enter": 36,
    "escape": 53,
    "tab": 48,
    "down": 125,
    "up": 126,
    "right": 124,
    "left": 123,
    "cmd_down": 125,
    "cmd_up": 126,
    "period": 47,
    "comma": 43,
    "backspace": 51,
    "i": 34,
    "l": 37,
}

FLAG_CMD = Quartz.kCGEventFlagMaskCommand
FLAG_SHIFT = Quartz.kCGEventFlagMaskShift
FLAG_CTRL = Quartz.kCGEventFlagMaskControl
FLAG_ALT = Quartz.kCGEventFlagMaskAlternate


class Session:
    """One scripted app session: process, transcript, window, input channels."""

    def __init__(self, binary, project, open_file, outdir, name, release=False, extra_env=None, env_project=True):
        self.binary = binary
        self.project = project
        self.open_file = open_file
        self.outdir = Path(outdir)
        self.name = name
        self.release = release
        self.extra_env = extra_env or {}
        # The scripted-override env keeps sessions stateless (no session
        # restore, no save). The restore test needs the real startup order —
        # recents-driven project pick + session restore — so it drops the env.
        self.env_project = env_project
        self.proc = None
        self.transcript_path = self.outdir / f"{name}-transcript.log"
        self.window_id = None

    # -- lifecycle ---------------------------------------------------------

    def start(self, wait_markers=("first paint",), timeout=90):
        self.outdir.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ)
        if self.env_project:
            env["OPENIDEA_GUI_PROJECT"] = self.project
        env["OPENIDEA_GUI_SETTINGS_DIR"] = str(self.outdir / f"{self.name}-settings")
        env["RUST_LOG"] = "info"
        if self.open_file:
            env["OPENIDEA_GUI_OPEN"] = self.open_file
        env.pop("OPENIDEA_GUI_TEST_PANIC", None)
        env.update(self.extra_env)
        # A scripted session's own store starts clean (truncate, never
        # append — the samplers parse it whole). A store handed in through
        # extra_env (the session-restore and crash-toast legs) is SHARED —
        # wiping it would erase the state those legs exist to read.
        store = Path(env["OPENIDEA_GUI_SETTINGS_DIR"])
        owns_store = store == Path(self.outdir) / f"{self.name}-settings"
        if owns_store and store.exists():
            for f in store.iterdir():
                f.unlink()
        self._transcript = open(self.transcript_path, "wb", buffering=0)
        self.proc = subprocess.Popen(
            [self.binary],
            env=env,
            stdout=self._transcript,
            stderr=subprocess.STDOUT,
        )
        self.wait_transcript("|".join(wait_markers), timeout=timeout)
        self.window_id = self._find_window(timeout=30)
        self.activate()

    def wait_transcript(self, pattern, timeout=60):
        """Waits until `pattern` (a regex) appears in the transcript."""
        rx = re.compile(pattern)
        deadline = time.time() + timeout
        seen = ""
        while time.time() < deadline:
            seen = self.transcript_path.read_text(errors="replace") if self.transcript_path.exists() else ""
            if rx.search(seen):
                return seen
            if self.proc and self.proc.poll() is not None:
                raise RuntimeError(f"app exited early (code {self.proc.returncode}); transcript tail:\n{seen[-2000:]}")
            time.sleep(0.3)
        raise RuntimeError(f"timeout waiting for {pattern!r}; transcript tail:\n{seen[-2000:]}")

    def stop(self):
        if self.proc and self.proc.poll() is None:
            # Graceful quit through the app's own action (also exercises the
            # clean-exit + session-save path); the stick is the fallback.
            try:
                self.activate()
                self.post_key("q", FLAG_CMD)
                self.proc.wait(timeout=8)
            except Exception:
                pass
            if self.proc.poll() is None:
                self.proc.terminate()
                try:
                    self.proc.wait(timeout=5)
                except Exception:
                    self.proc.kill()
                    self.proc.wait(timeout=5)
        if getattr(self, "_transcript", None):
            self._transcript.close()

    # -- window ------------------------------------------------------------

    def _find_window(self, timeout=30):
        deadline = time.time() + timeout
        while time.time() < deadline:
            windows = Quartz.CGWindowListCopyWindowInfo(
                Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements,
                Quartz.kCGNullWindowID,
            )
            for w in windows:
                owner = w.get("kCGWindowOwnerName", "")
                title = w.get("kCGWindowName", "")
                if APP_TITLE in str(owner) or APP_TITLE in str(title):
                    wid = w.get("kCGWindowNumber")
                    if wid is not None and w.get("kCGWindowOwnerPID") == self.proc.pid:
                        return int(wid)
            time.sleep(0.5)
        raise RuntimeError("app window not found")

    def window_bounds(self):
        windows = Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID)
        for w in windows:
            if w.get("kCGWindowNumber") == self.window_id:
                b = w.get("kCGWindowBounds")
                return (b["X"], b["Y"], b["Width"], b["Height"])
        raise RuntimeError("window vanished")

    def activate(self):
        subprocess.run(["osascript", "-e", f'tell application "System Events" to set frontmost of (first process whose unix id is {self.proc.pid}) to true'], check=False)
        time.sleep(0.4)

    def screenshot(self, path):
        """Window-scoped capture only (never full-screen)."""
        subprocess.run(["caffeinate", "-u", "-t", "1"], check=False)
        r = subprocess.run(["screencapture", "-o", "-l", str(self.window_id), str(path)], capture_output=True)
        ok = r.returncode == 0 and Path(path).exists() and Path(path).stat().st_size > 10_000
        return ok

    # -- input channels ----------------------------------------------------

    def post_key(self, key, flags=0, down_up=True):
        code = KEY[key]
        down = Quartz.CGEventCreateKeyboardEvent(None, code, True)
        if flags:
            Quartz.CGEventSetFlags(down, flags)
        Quartz.CGEventPostToPid(self.proc.pid, down)
        if down_up:
            up = Quartz.CGEventCreateKeyboardEvent(None, code, False)
            if flags:
                Quartz.CGEventSetFlags(up, flags)
            Quartz.CGEventPostToPid(self.proc.pid, up)
        time.sleep(0.06)

    def type_char(self, ch):
        """One printable character through the raw fallback channel: may be
        composed by the IME or double-inserted (the M5 record); used only
        where the exact count does not matter (a completion trigger)."""
        ev = Quartz.CGEventCreateKeyboardEvent(None, KEY["period"], True)
        Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
        Quartz.CGEventPostToPid(self.proc.pid, ev)
        up = Quartz.CGEventCreateKeyboardEvent(None, KEY["period"], False)
        Quartz.CGEventKeyboardSetUnicodeString(up, len(ch), ch)
        Quartz.CGEventPostToPid(self.proc.pid, up)
        time.sleep(0.06)

    def paste(self, text):
        """The trustworthy text channel: clipboard + ⌘V (bypasses the IME)."""
        p = subprocess.Popen(["pbcopy"], stdin=subprocess.PIPE)
        p.communicate(text.encode())
        self.post_key("v", FLAG_CMD)

    def scroll(self, dy=-40, count=5, interval=0.05):
        """Scroll-wheel events (pixel units) at the editor's center. The
        cursor moves there first — posted wheel events land at the cursor."""
        x, y, w, h = self.window_bounds()
        cx, cy = x + w / 2, y + h * 0.45
        move = Quartz.CGEventCreateMouseEvent(None, Quartz.kCGEventMouseMoved, Quartz.CGPointMake(cx, cy), Quartz.kCGMouseButtonLeft)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, move)
        time.sleep(0.05)
        for _ in range(count):
            ev = Quartz.CGEventCreateScrollWheelEvent(None, Quartz.kCGScrollEventUnitPixel, 1, dy)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
            time.sleep(interval)

    # -- sampling ----------------------------------------------------------

    def hud_samples(self):
        """All `[perf] hud` lines as dicts (the F12-toggle sample channel)."""
        out = []
        text = self.transcript_path.read_text(errors="replace")
        rx = re.compile(
            r"\[perf\] hud (on|off) — rows (\d+) spans (\d+)/(\d+) parse (\S+) ms@v(\d+) input (\S+) ms frame (\S+) ms prepaint (\S+) ms completion (\S+) ms budget (\S+) ms"
        )
        for m in rx.finditer(text):
            def num(s):
                if s == "None":
                    return None
                inner = re.fullmatch(r"Some\((\S+)\)", s)
                return float(inner.group(1)) if inner else float(s)
            out.append(
                {
                    "state": m.group(1),
                    "rows": int(m.group(2)),
                    "spans_painted": int(m.group(3)),
                    "spans_total": int(m.group(4)),
                    "parse_ms": num(m.group(5)),
                    "input_ms": num(m.group(7)),
                    "frame_ms": float(m.group(8)),
                    "prepaint_ms": num(m.group(9)),
                    "completion_ms": num(m.group(10)),
                }
            )
        return out

    def find_response_ms(self):
        """The last `[find] "<q>": N files / M matches in X ms` line."""
        text = self.transcript_path.read_text(errors="replace")
        hits = re.findall(r'\[find\] "[^"]*": (\d+) files / (\d+) matches in (\S+) ms', text)
        return float(hits[-1][2]) if hits else None

    def first_paint_ms(self):
        text = self.transcript_path.read_text(errors="replace")
        hits = re.findall(r"\[perf\] first paint (\S+) ms after open", text)
        return float(hits[-1]) if hits else None

    def completion_paint_ms(self):
        text = self.transcript_path.read_text(errors="replace")
        hits = re.findall(r"\[perf\] completion painted after (\S+) ms", text)
        return float(hits[-1]) if hits else None

    def indexing_window(self):
        """(on_ts, off_ts) wall-clock span of the first indexing window, from
        the [perf] indexing lines' file mtimes — approximated by line order:
        returns True while 'indexing on' has no 'off' after it."""
        text = self.transcript_path.read_text(errors="replace")
        events = re.findall(r"\[perf\] indexing (on|off)", text)
        return bool(events and events[-1] == "on")

    def rss_mb(self):
        if not self.proc or self.proc.poll() is not None:
            return None
        out = subprocess.run(["ps", "-o", "rss=", "-p", str(self.proc.pid)], capture_output=True, text=True)
        kb = out.stdout.strip()
        return round(int(kb) / 1024.0, 1) if kb else None


# -- mode: perf -------------------------------------------------------------


def measure_key_to_px(session, rounds=6, label="x"):
    """Rounds of paste + F12 toggle; the toggles print the fresh input→pixel
    latency. Returns the samples (ms)."""
    samples = []
    for i in range(rounds):
        session.paste(f"{label} := probe{i} // {i}")
        time.sleep(0.35)
        session.post_key("f12")
        time.sleep(0.2)
        session.post_key("f12")
        time.sleep(0.35)
    for s in session.hud_samples()[-rounds * 2:]:
        if s["input_ms"] is not None:
            samples.append(s["input_ms"])
    return samples


def measure_completion(session, snippet_go=True):
    """Triggers a member-access completion: pastes a receiver declaration in
    the scenario's language, then posts one raw '.' — the only non-paste
    channel. The IME may swallow or double-insert it (the M5 record), so the
    attempt retries: a backspace converges any dot count toward exactly one,
    and a fresh dot re-arms the trigger."""
    receiver = '\nrecv := pkg002.New("m8")\nrecv' if snippet_go else '\nlet recv = String::from("m8");\nrecv'
    session.post_key("cmd_down", FLAG_CMD)  # end of document
    session.paste(receiver)
    time.sleep(0.3)
    for attempt in range(3):
        session.type_char(".")
        try:
            session.wait_transcript(r"completion painted after", timeout=8)
            return session.completion_paint_ms()
        except RuntimeError:
            session.post_key("backspace")
            time.sleep(0.3)
    return None


def measure_find(session, query="Render"):
    """⇧⌘F → paste query → Enter; the [find] line's ms is the response."""
    session.post_key("f", FLAG_SHIFT | FLAG_CMD)
    time.sleep(0.5)
    session.paste(query)
    time.sleep(0.2)
    session.post_key("enter")
    try:
        session.wait_transcript(r'\[find\] "' + re.escape(query), timeout=60)
    except RuntimeError:
        return None
    return session.find_response_ms()


def measure_scroll_during_index(session, seconds=25):
    """Scrolls continuously while the first index runs, F12-toggling to
    sample the prepaint COST (the interval value is cadence, not cost).
    Returns the prepaint samples (ms) taken during the window."""
    samples_start = len(session.hud_samples())
    deadline = time.time() + seconds
    while time.time() < deadline:
        session.scroll(dy=-60, count=2, interval=0.02)
        session.scroll(dy=60, count=2, interval=0.02)
        session.post_key("f12")
        session.post_key("f12")
    return [s["prepaint_ms"] for s in session.hud_samples()[samples_start:] if s["prepaint_ms"] is not None]


def run_perf(binary, project, open_file, outdir, name, release, scroll_seconds):
    s = Session(binary, project, open_file, outdir, name, release)
    result = {"name": name, "release": release, "project": project}
    try:
        # Launch: the scroll-during-index phase must start as early as
        # possible — the first index begins with the project open. ("first
        # paint" is the startup-ready marker; the startup path prints no
        # "project opened" — that line belongs to the folder-picker flow.)
        s.start(wait_markers=("first paint",), timeout=120)
        result["open_first_paint_ms"] = s.first_paint_ms()
        result["scroll_frames_during_index_ms"] = measure_scroll_during_index(s, seconds=scroll_seconds)
        result["key_to_px_ms"] = measure_key_to_px(s)
        rust = (open_file or "").endswith(".rs")
        result["completion_first_frame_ms"] = measure_completion(s, snippet_go=not rust)
        result["find_in_files_first_response_ms"] = measure_find(s)
        shot = s.outdir / f"{name}-hud.png"
        s.post_key("f12")  # HUD on for the shot
        time.sleep(0.5)
        result["screenshot"] = shot.name if s.screenshot(shot) else None
        result["gopls_indexing"] = s.indexing_window()
    finally:
        s.stop()
    return result


# -- mode: smoke ------------------------------------------------------------


def run_smoke(binary, project, open_file, outdir, name):
    """The fixed screenshot-state set; every state asserts a transcript
    marker, then captures the window."""
    s = Session(binary, project, open_file, outdir, name)
    states = []
    try:
        s.start(wait_markers=("first paint",), timeout=120)
        time.sleep(2.0)

        def capture(state, marker, keys=(), paste_text=None, settle=1.0, keys_after=()):
            for k, flags in keys:
                s.post_key(k, flags)
            if paste_text:
                s.paste(paste_text)
            for k, flags in keys_after:
                s.post_key(k, flags)
            if marker:
                s.wait_transcript(re.escape(marker), timeout=30)
            time.sleep(settle)
            ok = s.screenshot(s.outdir / f"{name}-{state}.png")
            states.append({"state": state, "shot": ok})
            return ok

        capture("01-opened", None)
        capture("02-hud", None, keys=[("f12", 0)])
        capture("03-find-in-files", "searching", keys=[("f", FLAG_SHIFT | FLAG_CMD)], paste_text="Render", keys_after=[("enter", 0)])
        s.post_key("escape")  # find bar closed, editor focused
        time.sleep(0.4)
        capture("04-settings", None, keys=[("comma", FLAG_CMD)])
        s.post_key("escape")
        time.sleep(0.4)
        capture("05-welcome-projectless", None, keys=[("w", FLAG_CMD)])
    finally:
        s.stop()
    return states


# -- mode: soak -------------------------------------------------------------


def run_soak(binary, project, open_file, outdir, name, minutes):
    """The long-session loop: paste-edit-save cycles, ⌘W/⌘⇧T tab churn,
    ⌘1/⌘4 dock toggles, run start bursts; RSS + settings-file watchpoints
    sampled per minute (the M7 list: write frequency, file size, run-tab
    accumulation)."""
    s = Session(binary, project, open_file, outdir, name)
    settings_file = Path(outdir) / f"{name}-settings" / "settings.json"
    rows = []
    try:
        s.start(wait_markers=("first paint",), timeout=120)
        deadline = time.time() + minutes * 60
        cycle = 0
        last_sample = 0.0
        s.post_key("f12")
        while time.time() < deadline:
            cycle += 1
            # Edit round first: the previous cycle ended with the tab
            # REOPENED, so the editor pane holds the focus and the paste
            # lands (an earlier draft closed the tab first and the whole
            # rhythm dead-landed on the tree — the app's no-tab save no-op
            # is correct; the driver was wrong).
            s.paste(f"soak := {cycle} // cycle {cycle}")
            time.sleep(0.5)
            s.post_key("s", FLAG_CMD)
            time.sleep(0.5)
            # Dock/tree toggles.
            s.post_key("1", FLAG_CMD)
            time.sleep(0.3)
            s.post_key("1", FLAG_CMD)
            s.post_key("4", FLAG_CMD)
            time.sleep(0.3)
            s.post_key("4", FLAG_CMD)
            # Run burst every 10th cycle: the Run palette takes a one-off
            # shell line (paste channel); the run starts and exits on its
            # own. Tabs accumulate BY DESIGN — that is the M7 watchpoint.
            if cycle % 10 == 0:
                s.post_key("r", FLAG_CTRL)
                time.sleep(0.7)
                s.paste(f"echo soak-{cycle}")
                time.sleep(0.3)
                s.post_key("enter")
                time.sleep(0.9)
            # HUD sample every ~6 cycles.
            if cycle % 6 == 0:
                s.post_key("f12")
                s.post_key("f12")
            # RSS + watchpoints per minute.
            now = time.time()
            if now - last_sample >= 60.0:
                last_sample = now
                size = settings_file.stat().st_size if settings_file.exists() else 0
                text = s.transcript_path.read_text(errors="replace")
                row = {
                    "minute": round((minutes * 60 - (deadline - now)) / 60.0, 2),
                    "rss_mb": s.rss_mb(),
                    "settings_bytes": size,
                    "saves": len(re.findall(r"\[save\]", text)),
                    "run_starts": len(re.findall(r"\[run\] starting", text)),
                    "tab_opens": len(re.findall(r"\[session\]|\[reopen\]", text)),
                }
                rows.append(row)
                print(f"[soak] t+{row['minute']:.1f} min rss {row['rss_mb']} MB settings {size}B saves {row['saves']} runs {row['run_starts']}", flush=True)
            # Tab churn LAST (⌘W → ⌘⇧T): close pushes the stack, reopen pops
            # it and re-lands the editor focus for the next cycle.
            s.post_key("w", FLAG_CMD)
            time.sleep(0.5)
            s.post_key("t", FLAG_SHIFT | FLAG_CMD)
            time.sleep(0.6)
        s.screenshot(s.outdir / f"{name}-final.png")
    finally:
        s.stop()
    return rows


# -- entry ------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["perf", "smoke", "soak"])
    ap.add_argument("--binary", required=True)
    ap.add_argument("--project", required=True)
    ap.add_argument("--open", default=None)
    ap.add_argument("--outdir", default="/tmp/m8-verify")
    ap.add_argument("--name", default="session")
    ap.add_argument("--release", action="store_true")
    ap.add_argument("--minutes", type=int, default=32)
    ap.add_argument("--scroll-seconds", type=int, default=25)
    args = ap.parse_args()

    if args.mode == "perf":
        result = run_perf(args.binary, args.project, args.open, args.outdir, args.name, args.release, args.scroll_seconds)
        out = Path(args.outdir) / f"{args.name}-measured.json"
        import json

        out.write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))
    elif args.mode == "smoke":
        states = run_smoke(args.binary, args.project, args.open, args.outdir, args.name)
        print(states)
        sys.exit(0 if all(st["shot"] for st in states) else 1)
    elif args.mode == "soak":
        rows = run_soak(args.binary, args.project, args.open, args.outdir, args.name, args.minutes)
        import json

        out = Path(args.outdir) / f"{args.name}-rss.json"
        out.write_text(json.dumps(rows, indent=2))
        print(json.dumps(rows, indent=2))


if __name__ == "__main__":
    main()
