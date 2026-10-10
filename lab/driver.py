#!/usr/bin/env python3
"""In-container scenario driver for the whykey reality lab.

Runs exactly one scenario inside a fresh lab container: writes the scenario
files, starts the real programs (Sway, Xvfb + X11 window managers or hotkey
daemons, a session bus, or a shell in a pty), optionally opens an independent
input probe (foot running lab/keylog.py on Wayland, xev on X11), performs the
scenario actions with real synthetic input, records ground truth from the
programs' reaction, and finally runs whykey. The result is printed as one JSON
line prefixed with RESULT_MARKER; lab/run.py compares it with whykey's verdict.

Stdlib only; the scenario format is documented in lab/run.py.
"""

from __future__ import annotations

import codecs
import glob
import json
import os
import pty
import re
import select
import shlex
import struct
import subprocess
import sys
import termios
import fcntl
import threading
import time

RESULT_MARKER = "@@WHYKEY-LAB-RESULT@@"
LOG_DIR = "/tmp/lab-logs"
RUNTIME_DIR = "/tmp/rt"
WHYKEY = "/w/whykey"
X_DISPLAY = ":5"
PROBE_TITLE = "whykey-probe"
INPUT_SETTLE = 0.3

# Keysyms a probe ignores when deciding whether "the key" arrived: synthetic
# chords press and release modifiers around the key itself.
X11_MODIFIER_KEYSYMS = {
    "Shift_L", "Shift_R", "Control_L", "Control_R", "Alt_L", "Alt_R",
    "Meta_L", "Meta_R", "Super_L", "Super_R", "Hyper_L", "Hyper_R",
    "ISO_Level3_Shift", "ISO_Level5_Shift", "Caps_Lock", "Num_Lock",
    "Mode_switch",
}
# kitty keyboard protocol functional codes for modifier keys.
KITTY_MODIFIER_CODES = set(range(57441, 57455))
KITTY_SEQUENCE = re.compile(rb"\x1b\[(\d+)(?:;(\d+)(?::(\d+))?)?([u~A-Z])")


class LabError(Exception):
    pass


def decode_escapes(text: str) -> bytes:
    """Turn JSON-friendly escapes ("\\x14", "\\e", "\\r") into raw bytes."""
    text = text.replace("\\e", "\\x1b")
    return codecs.escape_decode(text.encode("utf-8"))[0]


def wait_for(predicate, timeout: float, what: str, interval: float = 0.05):
    deadline = time.monotonic() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise LabError(f"timed out after {timeout:g}s waiting for {what}")
        time.sleep(interval)


class Pty:
    """A program running on a pseudo-terminal, typed into like a keyboard.

    The master side answers the two queries shells block on, like any real
    terminal: Primary Device Attributes (fish waits up to 10s for it) and the
    cursor position report. Every other query goes unanswered, which a
    terminal without that feature also does.
    """

    REPLIES = (
        (re.compile(rb"\x1b\[0?c"), b"\x1b[?62;22c"),
        (re.compile(rb"\x1b\[6n"), b"\x1b[1;1R"),
    )

    def __init__(self, argv: list[str], env: dict[str, str], cwd: str):
        pid, fd = pty.fork()
        if pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
                os.chdir(cwd)
                os.execvpe(argv[0], argv, env)
            finally:
                os._exit(127)
        self.pid = pid
        self.fd = fd
        self.transcript = bytearray()
        self.last_output = time.monotonic()
        self.lock = threading.Lock()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        pending = b""
        while True:
            try:
                ready, _, _ = select.select([self.fd], [], [], 0.5)
                if not ready:
                    continue
                data = os.read(self.fd, 4096)
            except OSError:
                return
            if not data:
                return
            # Keep a short tail so a query split across reads is still seen.
            window = pending + data
            for query, reply in self.REPLIES:
                for _ in query.finditer(window):
                    os.write(self.fd, reply)
            escape = window.rfind(b"\x1b")
            tail = window[escape:] if escape != -1 else b""
            pending = tail if len(tail) < 4 and not tail.endswith((b"c", b"n")) else b""
            with self.lock:
                self.transcript.extend(data)
                self.last_output = time.monotonic()

    def wait_quiet(self, quiet: float = 0.4, timeout: float = 8.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                idle = time.monotonic() - self.last_output
            if idle >= quiet:
                return
            time.sleep(0.05)

    def type(self, data: bytes) -> None:
        os.write(self.fd, data)

    def text(self) -> str:
        with self.lock:
            return self.transcript.decode("utf-8", "replace")

    def close(self) -> None:
        try:
            os.kill(self.pid, 9)
        except ProcessLookupError:
            pass


class Lab:
    def __init__(self, scenario: dict):
        self.scenario = scenario
        self.env = dict(os.environ)
        self.env["PATH"] = "/w:" + self.env.get("PATH", "/usr/bin:/bin")
        self.env["HOME"] = "/root"
        self.env["SHELL"] = "/bin/bash"
        self.env["XDG_RUNTIME_DIR"] = RUNTIME_DIR
        self.env.setdefault("LANG", "C.UTF-8")
        self.processes: list[tuple[str, subprocess.Popen]] = []
        self.pty: Pty | None = None
        self.probe: str | None = None
        self.steps: list[str] = []

    # -- process helpers -------------------------------------------------
    def spawn(self, name: str, argv: list[str], env: dict | None = None) -> subprocess.Popen:
        log = open(os.path.join(LOG_DIR, f"{name}.log"), "wb")
        process = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            env=env or self.env,
            cwd="/root",
            start_new_session=True,
        )
        self.processes.append((name, process))
        self.steps.append(f"spawn {name}: {shlex.join(argv)}")
        return process

    def run(self, argv: list[str], timeout: float = 10.0, check: bool = True) -> subprocess.CompletedProcess:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            env=self.env,
            cwd="/root",
            timeout=timeout,
        )
        if check and completed.returncode != 0:
            raise LabError(
                f"{shlex.join(argv)} exited {completed.returncode}: "
                f"{completed.stderr.decode('utf-8', 'replace').strip()}"
            )
        return completed

    def ok(self, argv: list[str]) -> bool:
        try:
            return self.run(argv, timeout=5.0, check=False).returncode == 0
        except subprocess.TimeoutExpired:
            return False

    def ensure_alive(self) -> None:
        for name, process in self.processes:
            if process.poll() is not None:
                raise LabError(f"{name} exited with {process.returncode}; see logs")

    # -- setup -----------------------------------------------------------
    def write_files(self) -> None:
        for path, content in self.scenario.get("files", {}).items():
            if isinstance(content, list):
                content = "\n".join(content) + "\n"
            os.makedirs(os.path.dirname(path) or "/", exist_ok=True)
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(content)
            self.steps.append(f"write {path}")

    def start_session_bus(self) -> None:
        completed = self.run(
            ["dbus-daemon", "--session", "--fork", "--nopidfile", "--print-address=1"]
        )
        address = completed.stdout.decode().strip().splitlines()[0]
        self.env["DBUS_SESSION_BUS_ADDRESS"] = address
        self.steps.append("session bus started")

    def start_sway(self) -> None:
        config = self.scenario.get("sway", {}).get("config", "/root/.config/sway/config")
        env = dict(self.env)
        env.update(
            WLR_BACKENDS="headless",
            WLR_LIBINPUT_NO_DEVICES="1",
            WLR_RENDERER="pixman",
        )
        self.spawn("sway", ["sway", "-c", config], env)
        socket = wait_for(
            lambda: (self.ensure_alive(), glob.glob(f"{RUNTIME_DIR}/sway-ipc.*.sock"))[1],
            10.0,
            "the Sway IPC socket",
        )[0]
        wait_for(
            lambda: glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]"),
            10.0,
            "the Wayland socket",
        )
        display = sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]"))[0]
        # Sway exports these to every client it starts; whykey run from a
        # terminal in the session inherits them.
        self.env["SWAYSOCK"] = socket
        self.env["WAYLAND_DISPLAY"] = os.path.basename(display)
        wait_for(lambda: self.ok(["swaymsg", "-t", "get_version"]), 10.0, "Sway IPC")

    def start_x11(self) -> None:
        x11 = self.scenario.get("x11", {})
        # -noreset: Xvfb otherwise resets when its last client disconnects,
        # and a daemon connecting during that reset fails to open the display.
        self.spawn("Xvfb", ["Xvfb", X_DISPLAY, "-screen", "0", "1024x768x24", "-nolisten", "tcp", "-noreset"])
        wait_for(
            lambda: (self.ensure_alive(), os.path.exists("/tmp/.X11-unix/X5"))[1],
            10.0,
            "Xvfb",
        )
        self.env["DISPLAY"] = X_DISPLAY
        wait_for(lambda: self.ok(["xdotool", "getmouselocation"]), 10.0, "the X server")
        for daemon in x11.get("daemons", []):
            if isinstance(daemon, list):
                daemon = {"argv": daemon}
            argv = daemon["argv"]
            name = os.path.basename(argv[0])
            env = dict(self.env)
            env.update(daemon.get("env", {}))
            self.spawn(name, argv, env)
            if name == "i3":
                socket = wait_for(self._i3_socket, 10.0, "the i3 IPC socket")
                # i3 exports I3SOCK to the programs it starts.
                self.env["I3SOCK"] = socket
                wait_for(lambda: self.ok(["i3-msg", "-t", "get_version"]), 10.0, "i3 IPC")
            if daemon.get("ready") == "wm":
                wait_for(self._wm_ready, 10.0, f"{name} to manage the screen")
            grab = daemon.get("ready_grab")
            if grab:
                wait_for(
                    lambda: (self.ensure_alive(), self.run(
                        ["python3", "/lab/xgrab.py", str(grab[0]), str(grab[1])], check=False
                    ).returncode == 0)[1],
                    10.0,
                    f"{name} to grab modmask {grab[0]} keycode {grab[1]}",
                )
        time.sleep(float(x11.get("settle", 0.5)))
        self.ensure_alive()

    def _i3_socket(self) -> str | None:
        self.ensure_alive()
        completed = self.run(["i3", "--get-socketpath"], check=False)
        path = completed.stdout.decode().strip()
        return path if completed.returncode == 0 and path and os.path.exists(path) else None

    def _wm_ready(self) -> bool:
        """EWMH window managers set _NET_SUPPORTING_WM_CHECK once they manage the root."""
        self.ensure_alive()
        completed = self.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], check=False)
        return b"window id" in completed.stdout

    def start_pty(self) -> None:
        spec = self.scenario["pty"]
        env = dict(self.env)
        env.update(TERM=spec.get("term", "xterm-256color"))
        env.update(spec.get("env", {}))
        self.pty = Pty(spec["command"], env, "/root")
        self.steps.append(f"pty: {shlex.join(spec['command'])}")
        self.pty.wait_quiet(quiet=float(spec.get("settle", 0.6)))

    # -- probes ----------------------------------------------------------
    def start_probe(self) -> None:
        kind = self.scenario.get("probe")
        if not kind:
            return
        if kind == "wayland":
            self.spawn(
                "probe",
                ["foot", "--app-id", PROBE_TITLE, "--title", PROBE_TITLE,
                 "python3", "/lab/keylog.py", "/tmp/probe.log"],
            )
            wait_for(lambda: os.path.exists("/tmp/probe.log.ready"), 10.0, "the foot probe")
            wait_for(self._sway_probe_focused, 5.0, "probe focus")
        elif kind == "x11":
            # xev uses stdio; without line buffering its log lags the input.
            self.spawn("probe", ["stdbuf", "-oL", "xev", "-event", "keyboard", "-name", PROBE_TITLE])
            window = wait_for(
                lambda: self.run(["xdotool", "search", "--name", PROBE_TITLE], check=False)
                .stdout.decode().split(),
                10.0,
                "the xev probe window",
            )[0]
            self.run(["xdotool", "windowfocus", "--sync", window], check=False)
            wait_for(
                lambda: self.run(["xdotool", "getwindowfocus", "getwindowname"], check=False)
                .stdout.decode().strip() == PROBE_TITLE,
                5.0,
                "probe focus",
            )
            # xev prints its startup events; only count what follows.
            time.sleep(0.3)
            self.probe_offset = os.path.getsize(os.path.join(LOG_DIR, "probe.log"))
        else:
            raise LabError(f"unknown probe {kind!r}")
        self.probe = kind
        self.steps.append(f"probe {kind} focused")

    def _sway_probe_focused(self) -> bool:
        completed = self.run(["swaymsg", "-t", "get_tree"], check=False)
        if completed.returncode != 0:
            return False

        def focused(node: dict) -> bool:
            if node.get("focused") and node.get("app_id") == PROBE_TITLE:
                return True
            return any(focused(child) for child in node.get("nodes", []) + node.get("floating_nodes", []))

        return focused(json.loads(completed.stdout))

    def probe_keys(self) -> list[str]:
        """Non-modifier keys the probe client received, in order."""
        if self.probe == "wayland":
            with open("/tmp/probe.log", "rb") as handle:
                data = handle.read()
            keys = []
            for code, mods, event, final in KITTY_SEQUENCE.findall(data):
                if event == b"3":
                    continue
                if final == b"u" and int(code) in KITTY_MODIFIER_CODES:
                    continue
                keys.append((b"CSI " + code + b";" + (mods or b"1") + b" " + final).decode())
            return keys
        if self.probe == "x11":
            with open(os.path.join(LOG_DIR, "probe.log"), encoding="utf-8", errors="replace") as handle:
                handle.seek(self.probe_offset)
                text = handle.read()
            keys = []
            for block in text.split("\n\n"):
                if not block.lstrip().startswith("KeyPress"):
                    continue
                match = re.search(r"keysym 0x[0-9a-f]+, ([^)]+)\)", block)
                if match and match.group(1) not in X11_MODIFIER_KEYSYMS:
                    keys.append(match.group(1))
            return keys
        return []

    # -- actions ---------------------------------------------------------
    def perform(self, action: dict) -> None:
        if "sleep" in action:
            time.sleep(float(action["sleep"]))
            return
        if "wtype" in action:
            self.run(["wtype", *action["wtype"]])
        elif "vkbd" in action:
            self.run(["python3", "/lab/vkbd.py", *action["vkbd"]])
        elif "xdotool" in action:
            self.run(["xdotool", *action["xdotool"]])
        elif "sh" in action:
            self.run(["sh", "-c", action["sh"]], timeout=float(action.get("timeout", 10)))
        elif "type" in action:
            if self.pty is None:
                raise LabError("type action needs the pty environment")
            self.pty.wait_quiet()
            self.pty.type(decode_escapes(action["type"]))
        elif "wait_quiet" in action:
            if self.pty is not None:
                self.pty.wait_quiet(quiet=float(action["wait_quiet"]))
            return
        else:
            raise LabError(f"unknown action {action}")
        self.steps.append(f"action {json.dumps(action)}")
        time.sleep(float(action.get("settle", INPUT_SETTLE)))

    # -- ground truth ----------------------------------------------------
    def check_fired(self, spec: dict) -> tuple[bool, str]:
        if "file" in spec:
            path = spec["file"]
            if "equals" not in spec:
                exists = os.path.exists(path)
                return exists, f"{path} {'exists' if exists else 'missing'}"
            try:
                with open(path, encoding="utf-8", errors="replace") as handle:
                    content = handle.read().rstrip("\n")
            except FileNotFoundError:
                return False, f"{path} missing"
            return content == spec["equals"], f"{path} contains {content!r}"
        if "gsettings" in spec:
            schema, key = spec["gsettings"]
            value = self.run(["gsettings", "get", schema, key]).stdout.decode().strip()
            accelerators = re.findall(r"'([^']*)'", value) or [value.strip("'")]
            bound = spec["contains"] in accelerators
            return bound, f"gsettings get {schema} {key} = {value}"
        if "sh" in spec:
            completed = self.run(["sh", "-c", spec["sh"]], check=False)
            output = completed.stdout.decode("utf-8", "replace").strip()
            return completed.returncode == 0, f"`{spec['sh']}` exited {completed.returncode}: {output}"
        raise LabError(f"unknown fired check {spec}")

    def ground_truth(self) -> dict:
        truth_spec = self.scenario["ground_truth"]
        fired, fired_evidence = self.check_fired(truth_spec["fired"])
        delivered = None
        delivered_evidence = None
        delivered_spec = truth_spec.get("delivered")
        if delivered_spec == "probe":
            keys = self.probe_keys()
            delivered = bool(keys)
            delivered_evidence = f"{self.probe} probe received {keys or 'no key'}"
        elif isinstance(delivered_spec, dict):
            delivered, delivered_evidence = self.check_fired(delivered_spec)
        return {
            "fired": fired,
            "fired_evidence": fired_evidence,
            "delivered": delivered,
            "delivered_evidence": delivered_evidence,
        }

    # -- whykey ----------------------------------------------------------
    def run_whykey(self) -> dict:
        spec = self.scenario["whykey"]
        args = [*spec["args"], "--json", "--schema-version", "2"]
        if spec.get("run", "direct") == "pty":
            if self.pty is None:
                raise LabError("whykey run=pty needs the pty environment")
            for path in ("/tmp/whykey.out", "/tmp/whykey.err", "/tmp/whykey.done"):
                if os.path.exists(path):
                    os.remove(path)
            command = (
                f"{spec.get('command', 'whykey')} {shlex.join(args)}"
                " >/tmp/whykey.out 2>/tmp/whykey.err; touch /tmp/whykey.done\r"
            )
            self.pty.wait_quiet()
            self.pty.type(command.encode())
            wait_for(lambda: os.path.exists("/tmp/whykey.done"), 20.0, "whykey in the pty")
            with open("/tmp/whykey.out", encoding="utf-8", errors="replace") as handle:
                stdout = handle.read()
            with open("/tmp/whykey.err", encoding="utf-8", errors="replace") as handle:
                stderr = handle.read()
            exit_code = None
        else:
            env = dict(self.env)
            env.update(spec.get("env", {}))
            completed = subprocess.run(
                [WHYKEY, *args],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                env=env,
                cwd="/root",
                timeout=30,
            )
            stdout = completed.stdout.decode("utf-8", "replace")
            stderr = completed.stderr.decode("utf-8", "replace")
            exit_code = completed.returncode
        try:
            report = json.loads(stdout)
        except json.JSONDecodeError:
            report = None
        return {"exit_code": exit_code, "report": report, "stdout": stdout if report is None else None, "stderr": stderr}

    # -- lifecycle -------------------------------------------------------
    def execute(self) -> dict:
        environment = self.scenario["environment"]
        self.write_files()
        for command in self.scenario.get("setup", []):
            self.run(["sh", "-c", command])
            self.steps.append(f"setup {command}")
        self.start_session_bus()
        if environment == "sway":
            self.start_sway()
        elif environment == "x11":
            self.start_x11()
        elif environment == "pty":
            self.start_pty()
        elif environment != "session":
            raise LabError(f"unknown environment {environment!r}")
        for command in self.scenario.get("session_setup", []):
            self.run(["sh", "-c", command])
            self.steps.append(f"session setup {command}")
        self.start_probe()
        for action in self.scenario.get("actions", []):
            self.perform(action)
        truth = self.ground_truth()
        whykey = self.run_whykey()
        return {"ground_truth": truth, "whykey": whykey}

    def logs(self) -> dict[str, str]:
        logs = {}
        for path in sorted(glob.glob(os.path.join(LOG_DIR, "*.log"))):
            with open(path, encoding="utf-8", errors="replace") as handle:
                lines = handle.read().splitlines()
            logs[os.path.basename(path)] = "\n".join(lines[-25:])
        if self.pty is not None:
            logs["pty transcript"] = self.pty.text()[-3000:]
        return logs

    def shutdown(self) -> None:
        if self.pty is not None:
            self.pty.close()
        for _, process in reversed(self.processes):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, 9)
                except ProcessLookupError:
                    pass


def main() -> int:
    with open(sys.argv[1], encoding="utf-8") as handle:
        scenario = json.load(handle)
    os.makedirs(LOG_DIR, exist_ok=True)
    os.makedirs(RUNTIME_DIR, mode=0o700, exist_ok=True)
    os.chmod(RUNTIME_DIR, 0o700)
    lab = Lab(scenario)
    result: dict = {"scenario": scenario["id"]}
    try:
        result.update(lab.execute())
    except (LabError, subprocess.TimeoutExpired, OSError) as error:
        result["error"] = str(error)
    result["steps"] = lab.steps
    result["logs"] = lab.logs()
    print(RESULT_MARKER + json.dumps(result), flush=True)
    if os.environ.get("WHYKEY_LAB_KEEP") == "1":
        print("keeping container alive for debugging (docker exec -it <name> bash)", flush=True)
        while True:
            time.sleep(3600)
    lab.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
