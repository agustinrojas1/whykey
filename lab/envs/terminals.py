"""A real terminal emulator under headless Sway, running lab/termagent.py.

"terminal": "kitty" | "alacritty" | "foot" | "wezterm" | "konsole" | "ghostty"
picks the emulator; its configuration comes from the scenario's `files`. The
emulator runs termagent.py: commands arrive through a FIFO and run on the
terminal's tty with the environment the emulator gives its children. whykey
runs with that environment (TERM, KITTY_WINDOW_ID, ...), as a shell in the
terminal would see it.

ACTIONS
    {"tty_log": "start"[, "flags": N]}  record the bytes the terminal writes
                                        to the program's tty (ttylog.py);
                                        "flags" pushes Kitty keyboard flags N
    {"tty_log": "stop"}                 end the recording
CHECKS
    {"tty_input": {"equals": "\\x1b[1;5D"}}  the program received exactly these bytes
    {"tty_input": {"any": true}}             the program received some byte
"""

from __future__ import annotations

import itertools
import os
import shlex
import shutil

from common import decode_escapes, wait_for

from . import sway

DIR = "/tmp/termagent"
FIFO = f"{DIR}/commands"
LOG = f"{DIR}/tty.log"
CONFIG = "/tmp/termagent-sway.conf"
AGENT = ["python3", "/lab/termagent.py", DIR]
# Set for the emulator only; whykey must not see them as the user's session.
LAB_ONLY = {"PWD", "SHLVL", "_", "OLDPWD", "LIBGL_ALWAYS_SOFTWARE", "GALLIUM_DRIVER",
            "QT_QPA_PLATFORM", "GDK_BACKEND"}
GL_ENV = {"LIBGL_ALWAYS_SOFTWARE": "1", "GALLIUM_DRIVER": "llvmpipe",
          "QT_QPA_PLATFORM": "wayland", "GDK_BACKEND": "wayland"}

# name -> (app id Sway reports, argv that runs the agent as the command)
EMULATORS = {
    "kitty": ("kitty", ["kitty"]),
    "alacritty": ("Alacritty", ["alacritty", "-e"]),
    "foot": ("foot", ["foot"]),
    "wezterm": ("org.wezfurlong.wezterm", ["wezterm", "start", "--always-new-process", "--"]),
    "konsole": ("org.kde.konsole", ["konsole", "--nofork", "-e"]),
    "ghostty": ("com.mitchellh.ghostty", ["ghostty", "-e"]),
}

_ids = itertools.count(1)
_recording: str | None = None  # .done path of the running ttylog job


def _queue(command: str) -> str:
    """Queue a shell command on the terminal's agent; returns its .done path."""
    job = f"job{next(_ids)}"
    with open(FIFO, "w", encoding="utf-8") as fifo:
        fifo.write(f"{job}\t{command}\n")
    return f"{DIR}/{job}.done"


def start(lab) -> None:
    name = lab.scenario["terminal"]
    app_id, launcher = EMULATORS[name]
    shutil.rmtree(DIR, ignore_errors=True)
    os.makedirs(DIR)
    with open(CONFIG, "w", encoding="utf-8") as handle:
        handle.write("default_border none\ndefault_floating_border none\n")
    lab.scenario["sway"] = {"config": CONFIG}
    sway.start(lab)
    env = dict(lab.env)
    env.update(GL_ENV)
    lab.spawn(name, [*launcher, *AGENT], env)
    wait_for(
        lambda: (lab.ensure_alive(), os.path.exists(f"{DIR}/ready"))[1],
        60.0,
        f"{name} running the agent",
    )
    wait_for(
        lambda: sway.focused_window(lab)[0] == app_id,
        60.0,
        f"{name} ({app_id}) focused",
    )
    done = _queue(f"env -0 > {DIR}/env")
    wait_for(lambda: os.path.exists(done), 20.0, "the terminal's environment")
    with open(f"{DIR}/env", "rb") as handle:
        for entry in handle.read().split(b"\0"):
            key, sep, value = entry.partition(b"=")
            if sep and key.decode("utf-8", "replace") not in LAB_ONLY:
                lab.env[key.decode("utf-8", "replace")] = value.decode("utf-8", "replace")


def tty_log(lab, action: dict) -> None:
    global _recording
    if action["tty_log"] == "start":
        for path in (LOG, f"{LOG}.ready", f"{LOG}.stop"):
            if os.path.exists(path):
                os.remove(path)
        flags = ["--flags", str(int(action["flags"]))] if "flags" in action else []
        _recording = _queue(shlex.join(["python3", "/lab/ttylog.py", LOG, *flags]))
        wait_for(lambda: os.path.exists(f"{LOG}.ready"), 10.0, "the tty recorder")
        return
    if _recording is None:
        raise RuntimeError("tty_log stop without a recording")
    open(f"{LOG}.stop", "w", encoding="utf-8").close()
    done = _recording
    _recording = None
    wait_for(lambda: os.path.exists(done), 10.0, "the tty recorder to exit")


def _received() -> bytes:
    try:
        with open(LOG, "rb") as handle:
            return handle.read()
    except FileNotFoundError:
        return b""


def tty_input(lab, spec: dict) -> tuple[bool, str]:
    # spec is the whole check: {"tty_input": {...}}
    args = spec["tty_input"]
    data = _received()
    if args.get("any"):
        return bool(data), f"the program received {data!r}"
    expected = decode_escapes(args["equals"])
    return data == expected, f"the program received {data!r}, expected {expected!r}"


def inject(lab, keys: list[str]) -> None:
    sway.inject(lab, keys)


def focused_window(lab) -> tuple[str, str]:
    return sway.focused_window(lab)


def logs(lab) -> dict[str, str]:
    return {"tty input": repr(_received())[-2000:]}


ACTIONS = {"tty_log": tty_log}
CHECKS = {"tty_input": tty_input}
