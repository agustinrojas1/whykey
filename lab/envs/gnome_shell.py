"""GNOME Shell (the full shell, with its own keybindings) on headless mutter,
in the container. GNOME Shell needs systemd-logind on the system bus; systemd
cannot run here, so lab/envs/_logind.py serves the login1 subset the shell
asks for (one seat, one wayland session with XDG_SESSION_ID=1). Input enters
through mutter's RemoteDesktop API (lab/rdkbd.py), as in the gnome environment.

The shell starts with its overview hidden. Its state is read back over its
own D-Bus interface, `org.gnome.Shell.Eval`, which needs --unsafe-mode. The
lab uses it only for this read-back, so a shell binding is judged by what the
shell did, not by whykey's reading of the settings.

Scenario fields:
  check  "shell": {"eval": "<JS expression>", "is": "<expected result>"}
         Evaluates the expression in the running shell and compares its text
         result ("true", "false", ...).
  action {"gsd": "media-keys"}
         Starts /usr/libexec/gsd-media-keys (a gnome-settings-daemon plugin)
         and waits until it owns its D-Bus name. It grabs its keys a moment
         later, so follow it with {"sleep": 2}.
"""

from __future__ import annotations

import os
import re

from common import RUNTIME_DIR, VM_INPUT_SETTLE, LabError, wait_for

SHELL_ARGS = [
    "gnome-shell", "--headless", "--wayland", "--no-x11",
    "--virtual-monitor", "1024x768", "--unsafe-mode",
]
SHELL_BUS = "org.gnome.Shell"
SHELL_OBJECT = "/org/gnome/Shell"
GSD_NAMES = {"media-keys": "org.gnome.SettingsDaemon.MediaKeys"}
LOGIND = "/lab/envs/_logind.py"
SYSTEM_BUS_RUN = "/run/dbus"
SEATS_DIR = "/run/systemd/seats"
_RESULT = re.compile(r"^\((true|false), (.*)\)\s*$", re.DOTALL)


def start(lab) -> None:
    os.makedirs(SYSTEM_BUS_RUN, exist_ok=True)
    # GNOME Shell takes the logind path when this directory exists.
    os.makedirs(SEATS_DIR, exist_ok=True)
    lab.run(["dbus-daemon", "--system", "--fork", "--nopidfile"])
    lab.spawn("logind", ["python3", LOGIND])
    wait_for(
        lambda: (lab.ensure_alive(), lab.ok([
            "gdbus", "introspect", "--system", "--dest", "org.freedesktop.login1",
            "--object-path", "/org/freedesktop/login1",
        ]))[1],
        10.0,
        "the fake systemd-logind on the system bus",
    )
    # What a GNOME session exports to its clients.
    lab.env.update(
        XDG_SESSION_ID="1",
        XDG_CURRENT_DESKTOP="GNOME",
        XDG_SESSION_DESKTOP="gnome",
        XDG_SESSION_TYPE="wayland",
    )
    lab.spawn("gnome-shell", SHELL_ARGS)
    display = wait_for(
        lambda: (lab.ensure_alive(), sorted(
            name for name in os.listdir(RUNTIME_DIR) if re.fullmatch(r"wayland-\d+", name)
        ))[1],
        60.0,
        "the GNOME Shell Wayland socket",
    )[0]
    lab.env["WAYLAND_DISPLAY"] = display
    wait_for(lambda: _eval(lab, "true")[0], 60.0, "org.gnome.Shell on the session bus")
    # The shell opens its overview when startup finishes, so hiding it earlier
    # does not last: wait for the end of startup, then hide it.
    wait_for(lambda: _eval(lab, "!Main.layoutManager._startingUp") == (True, "true"),
             60.0, "the GNOME Shell startup to finish")
    # Start every scenario with the overview hidden, so a toggle is visible.
    hidden, value = _eval(lab, "Main.overview.hide(); Main.overview.visible")
    if not hidden or value != "false":
        raise LabError(f"could not hide the GNOME Shell overview: {value!r}")
    lab.steps.append("gnome-shell started; overview hidden")
    lab.settle = VM_INPUT_SETTLE


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/rdkbd.py", *keys])


def _eval(lab, expression: str) -> tuple[bool, str]:
    """Evaluate JavaScript in the shell: (succeeded, result text)."""
    completed = lab.run(
        ["gdbus", "call", "--session", "--dest", SHELL_BUS, "--object-path", SHELL_OBJECT,
         "--method", "org.gnome.Shell.Eval", expression],
        timeout=10.0, check=False,
    )
    text = completed.stdout.decode("utf-8", "replace").strip()
    match = _RESULT.match(text)
    if completed.returncode != 0 or not match:
        return False, text or completed.stderr.decode("utf-8", "replace").strip()
    value = match.group(2)
    if len(value) >= 2 and value[0] == value[-1] == "'":
        value = value[1:-1]
    if len(value) >= 2 and value[0] == value[-1] == '"':
        value = value[1:-1]
    return match.group(1) == "true", value


def _shell_check(lab, spec: dict) -> tuple[bool, str]:
    config = spec["shell"]
    succeeded, value = _eval(lab, config["eval"])
    if not succeeded:
        return False, f"Eval({config['eval']!r}) failed: {value}"
    return value == config["is"], f"Eval({config['eval']!r}) = {value!r}"


def _gsd(lab, action: dict) -> None:
    name = action["gsd"]
    if name not in GSD_NAMES:
        raise LabError(f"unknown gsd plugin {name!r}; known: {sorted(GSD_NAMES)}")
    lab.spawn(f"gsd-{name}", [f"/usr/libexec/gsd-{name}"])
    wait_for(
        lambda: (lab.ensure_alive(), _has_owner(lab, GSD_NAMES[name]))[1],
        15.0,
        f"gsd-{name} to own {GSD_NAMES[name]}",
    )


def _has_owner(lab, name: str) -> bool:
    completed = lab.run(
        ["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
         "--object-path", "/org/freedesktop/DBus", "--method",
         "org.freedesktop.DBus.NameHasOwner", name],
        timeout=5.0, check=False,
    )
    return completed.returncode == 0 and b"true" in completed.stdout


CHECKS = {"shell": _shell_check}
ACTIONS = {"gsd": _gsd}
