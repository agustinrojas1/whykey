#!/usr/bin/env python3
"""In-container scenario driver for the whykey reality lab.

Runs exactly one scenario inside a fresh lab container: writes the scenario
files, starts a session bus and the scenario's environment (a plugin in
lab/envs: Sway, Hyprland, headless mutter, Xvfb + X11 window managers or
hotkey daemons, a shell in a pty, ...), optionally opens an independent input
probe (foot running lab/keylog.py on Wayland, xev on X11), performs the
scenario actions with real synthetic input, records ground truth from the
programs' reaction, and finally runs whykey. The result is printed as one
JSON line prefixed with RESULT_MARKER; lab/run.py compares it with whykey's
verdict.

An environment with a VM spec (Hyprland: aquamarine needs a KMS device even
for headless outputs) boots the image's own kernel in a KVM guest (virtme-ng,
root filesystem = this container over 9p) and runs this driver again inside
it with WHYKEY_LAB_VM=1; the guest writes its result to VM_RESULT_DIR, shared
read-write with the container.

Stdlib only (lab/rdkbd.py uses the image's python3-dbus); the scenario format
is documented in lab/run.py, the environment plugin contract in lab/envs.
"""

from __future__ import annotations

import glob
import json
import os
import re
import shlex
import socket
import subprocess
import sys
import threading
import time

from common import (
    INPUT_SETTLE,
    LOG_DIR,
    PROBE_TITLE,
    RUNTIME_DIR,
    WHYKEY_WINDOW_TITLE,
    LabError,
    decode_escapes,
    wait_for,
)
from envs import ENVIRONMENTS

RESULT_MARKER = "@@WHYKEY-LAB-RESULT@@"
WHYKEY = "/w/whykey"
VM_RESULT_DIR = "/tmp/vm-result"
VM_TIMEOUT = 150
# VM spec "keyboard": the guest's only keyboard is a QEMU virtio-keyboard-pci;
# the PC's i8042 PS/2 controller is off, so no second keyboard exists. lab/qkbd.py
# in the guest writes QMP command lines to the virtio-serial port whykey.keyboard;
# keyboard_relay forwards them to QEMU's QMP socket and writes each reply back.
VM_QMP_SOCKET = "/tmp/vm-qmp.sock"
VM_KEYBOARD_SOCKET = "/tmp/vm-keyboard.sock"
VM_KEYBOARD_OPTS = (
    "-machine i8042=off -device virtio-keyboard-pci "
    f"-qmp unix:{VM_QMP_SOCKET},server=on,wait=off "
    "-device virtio-serial-pci,id=whykeyserial "
    f"-chardev socket,id=whykeykbd,path={VM_KEYBOARD_SOCKET},server=on,wait=off "
    "-device virtserialport,bus=whykeyserial.0,name=whykey.keyboard,chardev=whykeykbd"
)

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


class Lab:
    def __init__(self, scenario: dict):
        self.scenario = scenario
        self.plugin = ENVIRONMENTS.get(scenario["environment"])
        self.env = dict(os.environ)
        self.env["PATH"] = "/w:" + self.env.get("PATH", "/usr/bin:/bin")
        self.env["HOME"] = "/root"
        self.env["SHELL"] = "/bin/bash"
        self.env["XDG_RUNTIME_DIR"] = RUNTIME_DIR
        self.env.setdefault("LANG", "C.UTF-8")
        self.processes: list[tuple[str, subprocess.Popen]] = []
        self.pty = None  # set by the pty environment
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
            if hasattr(self.plugin, "focused_window"):
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
        the app id is empty. ("", "") when nothing has focus or the
        environment cannot tell."""
        hook = getattr(self.plugin, "focused_window", None)
        return hook(self) if hook else ("", "")

    def _probe_focused(self) -> bool:
        """The foot probe (app id PROBE_TITLE) has focus."""
        app_id, title = self._focused_window()
        return app_id == PROBE_TITLE and title != WHYKEY_WINDOW_TITLE

    def _probe_receives_input(self) -> bool:
        """A Shift tap typed like the scenario's input reaches the foot probe:
        the log holds a Shift *press* (a lone release can arrive when the press
        was swallowed by a keyboard switch, which proves nothing). Without a
        focused_window hook (mutter has no window-query IPC) this is the
        focus check."""
        inject = getattr(self.plugin, "inject", None)
        if inject is None:
            raise LabError(f"environment {self.scenario['environment']!r} has no inject hook "
                           "for the Wayland probe")
        inject(self, ["shift"])
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
        if "wait_whykey" in action:
            # The background whykey ends by its own --count/--timeout (or a
            # cancel key); keys sent after this one go to the restored session.
            self.wait_whykey_exit(float(action.get("timeout", 20)))
            self.steps.append("whykey exited")
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
            actions = getattr(self.plugin, "ACTIONS", {})
            handler = next((actions[key] for key in action if key in actions), None)
            if handler is None:
                raise LabError(f"unknown action {action}")
            handler(self, action)
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
        checks = getattr(self.plugin, "CHECKS", {})
        for key in spec:
            if key in checks:
                return checks[key](self, spec)
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
        # Evaluated once whykey has exited (a `wait_whykey` action): the state
        # the capture must leave behind, such as a restored compositor submap.
        restored_spec = truth_spec.get("restored")
        if isinstance(restored_spec, dict):
            restored, evidence = self.check_fired(restored_spec)
            truth["restored"], truth["restored_evidence"] = restored, f"restored: {evidence}"
        return truth

    # -- whykey ----------------------------------------------------------
    def wait_whykey_exit(self, timeout: float) -> None:
        """Wait for the background whykey to exit on its own; its output is
        collected later by run_whykey, so this does not judge its exit code."""
        try:
            self.whykey_process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise LabError(f"whykey still running {timeout:g}s after its capture should have ended") from error

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
        if self.plugin is None:
            raise LabError(f"unknown environment {self.scenario['environment']!r}")
        self.write_files()
        for command in self.scenario.get("setup", []):
            self.run(["sh", "-c", command])
            self.steps.append(f"setup {command}")
        self.start_session_bus()
        self.plugin.start(self)
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
        for path in sorted(glob.glob(os.path.join(LOG_DIR, "*.log"))):
            with open(path, encoding="utf-8", errors="replace") as handle:
                lines = handle.read().splitlines()
            logs[os.path.basename(path)] = "\n".join(lines[-25:])
        hook = getattr(self.plugin, "logs", None)
        if hook:
            logs.update(hook(self))
        return logs

    def shutdown(self) -> None:
        hook = getattr(self.plugin, "stop", None)
        if hook:
            hook(self)
        for _, process in reversed(self.processes):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, 9)
                except ProcessLookupError:
                    pass


def _connect(path: str, done: threading.Event) -> socket.socket | None:
    """Connect to a QEMU socket, retrying until QEMU has created it; None once `done`."""
    while not done.is_set():
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.connect(path)
            return sock
        except OSError:
            sock.close()
            time.sleep(0.05)
    return None


def _qmp_reply(lines, ident: str) -> bytes:
    """The raw QMP reply line for command `ident`; events and other replies are skipped."""
    while True:
        raw = lines.readline()
        if not raw:
            raise OSError("QMP socket closed")
        try:
            reply = json.loads(raw)
        except ValueError:
            continue
        if isinstance(reply, dict) and reply.get("id") == ident:
            return raw


def keyboard_relay(done: threading.Event) -> None:
    """Host side of the VM keyboard (VM_KEYBOARD_OPTS): forwards each QMP command
    line that lab/qkbd.py writes to the whykey.keyboard port to QEMU's QMP socket
    and writes QEMU's reply line back. Ends when the VM ends."""
    try:
        qmp = _connect(VM_QMP_SOCKET, done)
        if qmp is None:
            return
        qmp_lines = qmp.makefile("rb")
        qmp_lines.readline()  # greeting
        qmp.sendall(b'{"execute": "qmp_capabilities", "id": "relay"}\n')
        _qmp_reply(qmp_lines, "relay")
        while not done.is_set():
            port = _connect(VM_KEYBOARD_SOCKET, done)
            if port is None:
                return
            with port:
                for line in port.makefile("rb"):
                    ident = json.loads(line).get("id")
                    qmp.sendall(line)
                    port.sendall(_qmp_reply(qmp_lines, ident))
    except (OSError, ValueError):
        return  # QEMU is gone


def run_in_vm(scenario_path: str, environment: str, vm: dict) -> dict:
    """Boot the image's kernel with this container as root filesystem and run
    the scenario there (see the module docstring). `vm` is the environment's
    VM spec: "qemu_opts", "memory" (default 2G), "cpus" (default 2), "keyboard" (a kernel keyboard for qkbd actions)."""
    kernels = sorted(glob.glob("/lib/modules/*/vmlinuz"))
    if not kernels:
        return {"error": f"the lab image has no kernel for the {environment} VM (kernel-core)"}
    os.makedirs(VM_RESULT_DIR, exist_ok=True)
    result_path = os.path.join(VM_RESULT_DIR, "result.json")
    if os.path.exists(result_path):
        os.remove(result_path)
    inner = f"env WHYKEY_LAB_VM=1 python3 /lab/driver.py {shlex.quote(scenario_path)}"
    argv = [
        "vng", "--run", kernels[-1], "--rw", "--force-9p", "--disable-microvm",
        "--memory", str(vm.get("memory", "2G")), "--cpus", str(vm.get("cpus", 2)),
        "--rwdir", VM_RESULT_DIR,
    ]
    keyboard = bool(vm.get("keyboard"))
    qemu_opts = " ".join(part for part in (vm.get("qemu_opts", ""), VM_KEYBOARD_OPTS if keyboard else "") if part)
    if qemu_opts:
        argv.append(f"--qemu-opts={qemu_opts}")
    argv += ["--exec", inner]
    kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
    if not kvm:
        argv.append("--disable-kvm")
    steps = [f"vm ({'kvm' if kvm else 'tcg, no /dev/kvm'}): {shlex.join(argv)}"]
    done = threading.Event()
    if keyboard:
        threading.Thread(target=keyboard_relay, args=(done,), daemon=True).start()
    try:
        completed = subprocess.run(
            argv, stdin=subprocess.DEVNULL, capture_output=True, cwd="/root", timeout=VM_TIMEOUT
        )
        console = (completed.stdout + completed.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired as error:
        console = ((error.stdout or b"") + (error.stderr or b"")).decode("utf-8", "replace")
        return {"error": f"the {environment} VM timed out after {VM_TIMEOUT}s", "steps": steps,
                "logs": {"vm console": console[-4000:]}}
    finally:
        done.set()
    try:
        with open(result_path, encoding="utf-8") as handle:
            result = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        return {"error": f"the {environment} VM produced no result ({error})", "steps": steps,
                "logs": {"vm console": console[-4000:]}}
    result["steps"] = steps + result.get("steps", [])
    return result


def main() -> int:
    with open(sys.argv[1], encoding="utf-8") as handle:
        scenario = json.load(handle)
    in_vm = os.environ.get("WHYKEY_LAB_VM") == "1"
    environment = scenario["environment"]
    vm = getattr(ENVIRONMENTS.get(environment), "VM", None)
    if vm and not in_vm:
        result = {"scenario": scenario["id"], **run_in_vm(sys.argv[1], environment, vm)}
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
