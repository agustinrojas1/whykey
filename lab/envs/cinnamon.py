"""Cinnamon (its shell, Cinnamon's JavaScript keybinding manager, and muffin,
its window manager) on Xvfb in the container.

`cinnamon --replace` starts the shell on the X server, and muffin runs in
its process. The shell also needs the system bus (org.freedesktop.locale1
and friends): without it it stops at startup with "Script <main> threw an
exception". The environment starts a system bus for it. Keys are sent with
XTEST through xdotool, as in the x11 environment, so muffin and the shell
grab them as on a real X11 session.

Shortcuts the scenarios check: org.cinnamon.desktop.keybindings.wm (muffin's
window bindings) and org.cinnamon.desktop.keybindings.custom-keybinding (the
shell's custom shortcuts, which run their command).

Where muffin grabs a key (the cause of the readiness gate below): a binding
muffin treats as per-window (toggle-maximized, close, ...) is grabbed on the
frame around the focused client, a child of the root window; a root-only
probe sees nothing there. Other bindings (show-desktop, the custom shortcuts)
are grabbed on the root window. Both are XInput2 passive grabs, which core
GrabKey does not see. Scenarios therefore wait for the grab before typing.

Scenario fields:
  session_setup  must write the custom shortcut's fields before "custom-list":
                 Cinnamon's keybinding manager re-reads a custom shortcut only
                 when "custom-list" changes, so a binding written after the
                 list is never registered.
  action  {"wait_grab": {"keysym": "q", "modmask": 64}}
         Waits (up to 20 s) until the root window or the focused window's path
         holds an XInput2 or core grab for that key (lab/xgrab.py --path). The
         grab is the lab's own check, not whykey's. Put it before the input.
  expect "fired" on bound scenarios (lab/run.py): a bind that did not run is a
         lab error, never a pass.
"""

from __future__ import annotations

import os

from common import LOG_DIR, VM_INPUT_SETTLE, X_DISPLAY, LabError, wait_for

# What a Cinnamon X11 session exports to its clients.
SESSION_ENV = {
    "XDG_CURRENT_DESKTOP": "X-Cinnamon",
    "XDG_SESSION_DESKTOP": "cinnamon",
    "XDG_SESSION_TYPE": "x11",
}
GRAB_TIMEOUT = 20.0
XGRAB = "/lab/xgrab.py"


def start(lab) -> None:
    os.makedirs("/run/dbus", exist_ok=True)
    lab.run(["dbus-daemon", "--system", "--fork", "--nopidfile"])
    lab.spawn("Xvfb", ["Xvfb", X_DISPLAY, "-screen", "0", "1024x768x24", "-nolisten", "tcp", "-noreset"])
    wait_for(
        lambda: (lab.ensure_alive(), os.path.exists(f"/tmp/.X11-unix/X{X_DISPLAY.lstrip(':')}"))[1],
        10.0,
        "Xvfb",
    )
    lab.env["DISPLAY"] = X_DISPLAY
    lab.env.update(SESSION_ENV)
    wait_for(lambda: lab.ok(["xdotool", "getmouselocation"]), 10.0, "the X server")
    lab.spawn("cinnamon", ["cinnamon", "--replace"])
    wait_for(lambda: _wm_ready(lab), 90.0, "Cinnamon's window manager to manage the screen")
    # muffin manages the root before the shell's JavaScript has loaded its
    # keybinding manager; wait for the shell's own "started" line.
    wait_for(lambda: _shell_started(), 90.0, "the Cinnamon shell to finish starting")
    lab.steps.append("cinnamon --replace running; muffin manages the root window")
    lab.settle = VM_INPUT_SETTLE


def _shell_started() -> bool:
    """Cinnamon's JavaScript logs "Cinnamon started at ..." after its startup."""
    try:
        with open(os.path.join(LOG_DIR, "cinnamon.log"), encoding="utf-8", errors="replace") as handle:
            return "Cinnamon started at" in handle.read()
    except OSError:
        return False


def _wm_ready(lab) -> bool:
    """EWMH window managers set _NET_SUPPORTING_WM_CHECK once they manage the root."""
    lab.ensure_alive()
    completed = lab.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"], check=False)
    return b"window id" in completed.stdout


def inject(lab, keys: list[str]) -> None:
    lab.run(["xdotool", "key", *keys])


def focused_window(lab) -> tuple[str, str]:
    completed = lab.run(["xdotool", "getwindowfocus", "getwindowname"], check=False)
    return "", completed.stdout.decode("utf-8", "replace").strip()


def _wait_grab(lab, action: dict) -> None:
    spec = action["wait_grab"]
    name = spec["keysym"]
    if len(name) != 1:
        raise LabError(f"wait_grab keysym must be one character, got {name!r}")
    # For the Latin-1 letters and digits used here the keysym is the code point.
    keysym, modmask = ord(name), int(spec["modmask"])
    wait_for(
        lambda: _path_grabbed(lab, keysym, modmask),
        GRAB_TIMEOUT,
        f"a grab on {name!r} (modmask {modmask:#x}) on the root window or the focused window's path",
    )
    lab.steps.append(f"grab on keysym {name!r} modmask {modmask:#x} is in place")


def _path_grabbed(lab, keysym: int, modmask: int) -> bool:
    lab.ensure_alive()
    completed = lab.run(["python3", XGRAB, "--path", hex(keysym), str(modmask)], check=False)
    return completed.returncode == 0


ACTIONS = {"wait_grab": _wait_grab}
