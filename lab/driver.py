#!/usr/bin/env python3
"""In-container scenario driver for the whykey reality lab.

Runs exactly one scenario inside a fresh lab container: writes the scenario
files, starts the real programs (Sway, Hyprland, headless mutter, Xvfb + X11
window managers or hotkey daemons, a session bus, or a shell in a pty),
optionally opens an independent input probe (foot running lab/keylog.py on
Wayland, xev on X11),
performs the scenario actions with real synthetic input, records ground truth
from the programs' reaction, and finally runs whykey. The result is printed as
one JSON line prefixed with RESULT_MARKER; lab/run.py compares it with
whykey's verdict.

Hyprland cannot run in a plain container: aquamarine needs a KMS device to
allocate buffers even for headless outputs. A "hyprland" scenario therefore
boots the image's own kernel in a KVM guest (virtme-ng, root filesystem = this
container over 9p, virtio-gpu for KMS, Mesa kms_swrast/llvmpipe for GL) and
runs this driver again inside it with WHYKEY_LAB_VM=1; the guest writes its
result to VM_RESULT_DIR, shared read-write with the container.

Stdlib only (lab/rdkbd.py uses the image's python3-dbus); the scenario format
is documented in lab/run.py.
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
# Title of whykey's own probe window (`whykey listen --window`).
WHYKEY_WINDOW_TITLE = "whykey: press a shortcut here (Esc exits)"
PROBE_TITLE = "whykey-probe"
INPUT_SETTLE = 0.3
VM_INPUT_SETTLE = 1.0
VM_RESULT_DIR = "/tmp/vm-result"
VM_TIMEOUT = 150

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
        self.settle = INPUT_SETTLE

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

    def start_hyprland(self) -> None:
        """Hyprland on the guest's virtio-gpu KMS device (see the module docstring)."""
        if not glob.glob("/dev/dri/card*"):
            raise LabError("no KMS device: Hyprland scenarios must run inside the lab VM")
        argv = ["Hyprland", "--i-am-really-stupid"]  # root, in a throwaway VM
        config = self.scenario.get("hyprland", {}).get("config")
        if config:
            argv += ["--config", config]
        env = dict(self.env)
        # No logind/seatd in the guest: libseat opens devices directly.
        env["LIBSEAT_BACKEND"] = "noop"
        self.spawn("Hyprland", argv, env)
        socket = wait_for(
            lambda: (self.ensure_alive(), glob.glob(f"{RUNTIME_DIR}/hypr/*/.socket.sock"))[1],
            30.0,
            "the Hyprland IPC socket",
        )[0]
        display = wait_for(
            lambda: sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]")),
            10.0,
            "the Wayland socket",
        )[0]
        # What Hyprland exports to every client it starts (checked with an
        # `exec env` bind); whykey run from a terminal in the session inherits it.
        self.env.update(
            HYPRLAND_INSTANCE_SIGNATURE=os.path.basename(os.path.dirname(socket)),
            WAYLAND_DISPLAY=os.path.basename(display),
            XDG_CURRENT_DESKTOP="Hyprland",
            XDG_SESSION_TYPE="wayland",
        )
        wait_for(lambda: self.ok(["hyprctl", "version"]), 30.0, "Hyprland IPC")
        version = self.run(["hyprctl", "version"]).stdout.decode().splitlines()[0]
        self.steps.append(f"hyprctl version: {version}")
        # Binds exec through Hyprland in a 2-vCPU guest that shares the host
        # with other scenarios; give their effects time to land.
        self.settle = VM_INPUT_SETTLE

    def start_gnome(self) -> None:
        """Mutter, GNOME's compositor and window manager, headless on a
        virtual monitor. GNOME Shell itself does not start in the container
        (it aborts without systemd-logind), so this exercises mutter-owned
        keybindings (org.gnome.desktop.wm.keybindings) only. Input enters
        through mutter's RemoteDesktop D-Bus API (lab/rdkbd.py)."""
        self.spawn("mutter", ["mutter", "--headless", "--wayland", "--no-x11",
                              "--virtual-monitor", "1024x768"])
        display = wait_for(
            lambda: (self.ensure_alive(), sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]")))[1],
            10.0,
            "the mutter Wayland socket",
        )[0]
        # What a GNOME session exports to its clients.
        self.env.update(
            WAYLAND_DISPLAY=os.path.basename(display),
            XDG_CURRENT_DESKTOP="GNOME",
            XDG_SESSION_DESKTOP="gnome",
            XDG_SESSION_TYPE="wayland",
        )
        wait_for(
            lambda: self.ok(["gdbus", "introspect", "--session", "--dest",
                             "org.gnome.Mutter.RemoteDesktop", "--object-path",
                             "/org/gnome/Mutter/RemoteDesktop"]),
            10.0,
            "mutter's RemoteDesktop D-Bus service",
        )
        # A close request travels mutter -> foot -> foot's exit; give it time.
        self.settle = VM_INPUT_SETTLE

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
            if self.scenario["environment"] != "gnome":
                wait_for(self._probe_focused, 5.0, "probe focus")
            # Focus as the compositor reports it is not yet keyboard input
            # reaching the client (under load foot can still be processing
            # wl_keyboard.enter): wait until a Shift tap arrives. Shift is a
            # modifier, so "delivered" ignores it.
            wait_for(self._probe_receives_input, 10.0, "the probe to receive a Shift tap")
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

    def _focused_window(self) -> tuple[str, str]:
        """(Wayland app id, title) of the window with keyboard focus; on X11
        the app id is empty. ("", "") when nothing has focus."""
        environment = self.scenario["environment"]
        if environment == "hyprland":
            completed = self.run(["hyprctl", "activewindow", "-j"], check=False)
            try:
                window = json.loads(completed.stdout or b"{}")
            except json.JSONDecodeError:
                return "", ""
            return window.get("class") or "", window.get("title") or ""
        if environment == "x11":
            completed = self.run(["xdotool", "getwindowfocus", "getwindowname"], check=False)
            return "", completed.stdout.decode("utf-8", "replace").strip()
        completed = self.run(["swaymsg", "-t", "get_tree"], check=False)
        if completed.returncode != 0:
            return "", ""

        def focused(node: dict) -> tuple[str, str] | None:
            if node.get("focused") and node.get("type") in ("con", "floating_con"):
                return node.get("app_id") or "", node.get("name") or ""
            for child in node.get("nodes", []) + node.get("floating_nodes", []):
                found = focused(child)
                if found:
                    return found
            return None

        return focused(json.loads(completed.stdout)) or ("", "")

    def _probe_focused(self) -> bool:
        """The foot probe (app id PROBE_TITLE) has focus."""
        app_id, title = self._focused_window()
        return app_id == PROBE_TITLE and title != WHYKEY_WINDOW_TITLE

    def _probe_receives_input(self) -> bool:
        """A Shift tap typed like the scenario's input reaches the foot probe:
        the log holds a Shift *press* (a lone release can arrive when the press
        was swallowed by a keyboard switch, which proves nothing).
        Mutter has no window-query IPC, so on "gnome" this is the focus check."""
        injector = "rdkbd" if self.scenario["environment"] == "gnome" else "vkbd"
        self.run(["python3", f"/lab/{injector}.py", "shift"])
        with open("/tmp/probe.log", "rb") as handle:
            data = handle.read()
        return any(
            code == b"57441" and event in (b"", b"1") and final == b"u"
            for code, _mods, event, final in KITTY_SEQUENCE.findall(data)
        )

    def _whykey_window_focused(self) -> bool:
        """whykey's own probe window (`listen --window`) has focus."""
        return self._focused_window()[1] == WHYKEY_WINDOW_TITLE

    def probe_keys(self, modifiers: bool = False) -> list[str]:
        """Keys the probe client received, in order: non-modifier key presses,
        or with `modifiers` every key press including modifier keys."""
        if self.probe == "wayland":
            with open("/tmp/probe.log", "rb") as handle:
                data = handle.read()
            keys = []
            for code, mods, event, final in KITTY_SEQUENCE.findall(data):
                if event == b"3":
                    continue
                if final == b"u" and int(code) in KITTY_MODIFIER_CODES and not modifiers:
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
                if match and (modifiers or match.group(1) not in X11_MODIFIER_KEYSYMS):
                    keys.append(match.group(1))
            return keys
        return []

    def probe_events(self) -> list[str]:
        return self.probe_keys(modifiers=True)

    # -- actions ---------------------------------------------------------
    def perform(self, action: dict) -> None:
        if "sleep" in action:
            time.sleep(float(action["sleep"]))
            return
        if "start_whykey" in action:
            self.start_whykey_background()
            return
        if "wtype" in action:
            self.run(["wtype", *action["wtype"]])
        elif "vkbd" in action:
            self.run(["python3", "/lab/vkbd.py", *action["vkbd"]])
        elif "rdkbd" in action:
            self.run(["python3", "/lab/rdkbd.py", *action["rdkbd"]])
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
        time.sleep(float(action.get("settle", self.settle)))

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
        if "sh" in spec:
            completed = self.run(["sh", "-c", spec["sh"]], check=False)
            output = completed.stdout.decode("utf-8", "replace").strip()
            return completed.returncode == 0, f"`{spec['sh']}` exited {completed.returncode}: {output}"
        if spec.get("probe") == "any":
            events = self.probe_events()
            return bool(events), f"{self.probe} probe received key events {events or 'none'}"
        raise LabError(f"unknown check {spec}")

    def ground_truth(self) -> dict:
        truth_spec = self.scenario["ground_truth"]
        fired, fired_evidence = self.check_fired(truth_spec["fired"])
        truth = {"fired": fired, "fired_evidence": fired_evidence,
                 "delivered": None, "delivered_evidence": None}
        delivered_spec = truth_spec.get("delivered")
        if delivered_spec == "probe":
            keys = self.probe_keys()
            truth["delivered"] = bool(keys)
            truth["delivered_evidence"] = f"{self.probe} probe received {keys or 'no key'}"
        elif isinstance(delivered_spec, dict):
            truth["delivered"], truth["delivered_evidence"] = self.check_fired(delivered_spec)
        reached_spec = truth_spec.get("reached")
        if reached_spec == "fired":
            truth["reached"] = fired
            truth["reached_evidence"] = "reached: the fired observation required the key"
        elif isinstance(reached_spec, dict):
            reached, evidence = self.check_fired(reached_spec)
            truth["reached"], truth["reached_evidence"] = reached, f"reached: {evidence}"
        return truth

    # -- whykey ----------------------------------------------------------
    def start_whykey_background(self) -> None:
        """`run: background`: start whykey (e.g. `listen --window`) before the
        actions, or at a {"start_whykey": true} action, and wait until it is
        ready: "ready": "window" waits for its probe window to have focus,
        {"stderr": regex} for a line on its stderr. "tty": true runs it with a
        controlling terminal (`script`), as `whykey listen` requires for
        native capture; stdout/stderr still go to files, not the terminal."""
        spec = self.scenario["whykey"]
        env = dict(self.env)
        env.update(spec.get("env", {}))
        argv = [WHYKEY, *spec["args"], "--json", "--schema-version", "2"]
        if spec.get("tty"):
            command = f"{shlex.join(argv)} >/tmp/whykey.out 2>/tmp/whykey.err"
            open("/tmp/whykey.err", "wb").close()
            self.whykey_output = ()
            self.whykey_process = subprocess.Popen(
                ["script", "-qec", command, "/dev/null"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env=env,
                cwd="/root",
                start_new_session=True,
            )
        else:
            self.whykey_output = (open("/tmp/whykey.out", "wb"), open("/tmp/whykey.err", "wb"))
            self.whykey_process = subprocess.Popen(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=self.whykey_output[0],
                stderr=self.whykey_output[1],
                env=env,
                cwd="/root",
                start_new_session=True,
            )
        self.steps.append(f"spawn whykey {shlex.join(spec['args'])}")
        condition = spec.get("ready")

        def ready() -> bool:
            with open("/tmp/whykey.err", encoding="utf-8", errors="replace") as handle:
                stderr = handle.read()
            if self.whykey_process.poll() is not None:
                raise LabError(f"whykey exited with {self.whykey_process.returncode} "
                               f"before it was ready: {stderr.strip()}")
            if condition == "window":
                return self._whykey_window_focused()
            if isinstance(condition, dict) and "stderr" in condition:
                return re.search(condition["stderr"], stderr, re.MULTILINE) is not None
            return True

        wait_for(ready, 10.0, f"whykey to be ready ({condition})")
        time.sleep(float(spec.get("settle", 0.3)))
        self.steps.append("whykey ready")

    def run_whykey(self) -> dict:
        spec = self.scenario["whykey"]
        args = [*spec["args"], "--json", "--schema-version", "2"]
        mode = spec.get("run", "direct")
        if mode == "background":
            try:
                exit_code = self.whykey_process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                os.killpg(self.whykey_process.pid, 9)
                exit_code = self.whykey_process.wait()
            for handle in self.whykey_output:
                handle.close()
            with open("/tmp/whykey.out", encoding="utf-8", errors="replace") as handle:
                stdout = handle.read()
            with open("/tmp/whykey.err", encoding="utf-8", errors="replace") as handle:
                stderr = handle.read()
        elif mode == "pty":
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
        elif environment == "hyprland":
            self.start_hyprland()
        elif environment == "gnome":
            self.start_gnome()
        elif environment == "x11":
            self.start_x11()
        elif environment == "pty":
            self.start_pty()
        else:
            raise LabError(f"unknown environment {environment!r}")
        for command in self.scenario.get("session_setup", []):
            self.run(["sh", "-c", command])
            self.steps.append(f"session setup {command}")
        self.start_probe()
        actions = self.scenario.get("actions", [])
        if self.scenario["whykey"].get("run") == "background" and not any(
            "start_whykey" in action for action in actions
        ):
            self.start_whykey_background()
        for action in actions:
            self.perform(action)
        truth = self.ground_truth()
        whykey = self.run_whykey()
        return {"ground_truth": truth, "whykey": whykey}

    def logs(self) -> dict[str, str]:
        logs = {}
        paths = glob.glob(os.path.join(LOG_DIR, "*.log"))
        # Hyprland stops logging to stdout once its config is parsed.
        paths += glob.glob(f"{RUNTIME_DIR}/hypr/*/hyprland.log")
        for path in sorted(paths):
            with open(path, encoding="utf-8", errors="replace") as handle:
                lines = handle.read().splitlines()
            name = os.path.basename(path)
            logs["Hyprland instance log" if name == "hyprland.log" else name] = "\n".join(lines[-25:])
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


def run_in_vm(scenario_path: str) -> dict:
    """Boot the image's kernel with this container as root filesystem and run
    the scenario there (see the module docstring)."""
    kernels = sorted(glob.glob("/lib/modules/*/vmlinuz"))
    if not kernels:
        return {"error": "the lab image has no kernel for the Hyprland VM (kernel-core)"}
    os.makedirs(VM_RESULT_DIR, exist_ok=True)
    result_path = os.path.join(VM_RESULT_DIR, "result.json")
    if os.path.exists(result_path):
        os.remove(result_path)
    inner = f"env WHYKEY_LAB_VM=1 python3 /lab/driver.py {shlex.quote(scenario_path)}"
    argv = [
        "vng", "--run", kernels[-1], "--rw", "--force-9p", "--disable-microvm",
        "--memory", "2G", "--cpus", "2", "--rwdir", VM_RESULT_DIR,
        "--qemu-opts=-device virtio-gpu-pci", "--exec", inner,
    ]
    kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
    if not kvm:
        argv.append("--disable-kvm")
    steps = [f"vm ({'kvm' if kvm else 'tcg, no /dev/kvm'}): {shlex.join(argv)}"]
    try:
        completed = subprocess.run(
            argv, stdin=subprocess.DEVNULL, capture_output=True, cwd="/root", timeout=VM_TIMEOUT
        )
        console = (completed.stdout + completed.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired as error:
        console = ((error.stdout or b"") + (error.stderr or b"")).decode("utf-8", "replace")
        return {"error": f"the Hyprland VM timed out after {VM_TIMEOUT}s", "steps": steps,
                "logs": {"vm console": console[-4000:]}}
    try:
        with open(result_path, encoding="utf-8") as handle:
            result = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        return {"error": f"the Hyprland VM produced no result ({error})", "steps": steps,
                "logs": {"vm console": console[-4000:]}}
    result["steps"] = steps + result.get("steps", [])
    return result


def main() -> int:
    with open(sys.argv[1], encoding="utf-8") as handle:
        scenario = json.load(handle)
    in_vm = os.environ.get("WHYKEY_LAB_VM") == "1"
    if scenario["environment"] == "hyprland" and not in_vm:
        result = {"scenario": scenario["id"], **run_in_vm(sys.argv[1])}
        print(RESULT_MARKER + json.dumps(result), flush=True)
        if os.environ.get("WHYKEY_LAB_KEEP") == "1":
            print("keeping container alive for debugging (docker exec -it <name> bash)", flush=True)
            while True:
                time.sleep(3600)
        return 0
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
    if in_vm:
        with open(os.path.join(VM_RESULT_DIR, "result.json"), "w", encoding="utf-8") as handle:
            json.dump(result, handle)
        lab.shutdown()
        return 0
    print(RESULT_MARKER + json.dumps(result), flush=True)
    if os.environ.get("WHYKEY_LAB_KEEP") == "1":
        print("keeping container alive for debugging (docker exec -it <name> bash)", flush=True)
        while True:
            time.sleep(3600)
    lab.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
