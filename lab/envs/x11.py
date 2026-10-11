"""Xvfb :5 plus x11.daemons started in order (argv list or {"argv": [...],
"env": {...}, "ready": "wm", "ready_grab": [modmask, keycode],
"ready_timeout": seconds}); "wm" waits for _NET_SUPPORTING_WM_CHECK,
ready_grab until the daemon holds that X11 key grab (lab/xgrab.py); x11.env
adds scenario-wide variables to every daemon and whykey; i3 also exports
I3SOCK; then x11.settle seconds (default 0.5).
"""

from __future__ import annotations

import os
import time

from common import X_DISPLAY, wait_for


def start(lab) -> None:
    x11 = lab.scenario.get("x11", {})
    # -noreset: Xvfb otherwise resets when its last client disconnects,
    # and a daemon connecting during that reset fails to open the display.
    lab.spawn("Xvfb", ["Xvfb", X_DISPLAY, "-screen", "0", "1024x768x24", "-nolisten", "tcp", "-noreset"])
    wait_for(
        lambda: (lab.ensure_alive(), os.path.exists("/tmp/.X11-unix/X5"))[1],
        10.0,
        "Xvfb",
    )
    lab.env["DISPLAY"] = X_DISPLAY
    lab.env.update({key: str(value) for key, value in x11.get("env", {}).items()})
    wait_for(lambda: lab.ok(["xdotool", "getmouselocation"]), 10.0, "the X server")
    for daemon in x11.get("daemons", []):
        if isinstance(daemon, list):
            daemon = {"argv": daemon}
        argv = daemon["argv"]
        name = os.path.basename(argv[0])
        env = dict(lab.env)
        env.update(daemon.get("env", {}))
        lab.spawn(name, argv, env)
        if name == "i3":
            socket = wait_for(lambda: _i3_socket(lab), 10.0, "the i3 IPC socket")
            # i3 exports I3SOCK to the programs it starts.
            lab.env["I3SOCK"] = socket
            wait_for(lambda: lab.ok(["i3-msg", "-t", "get_version"]), 10.0, "i3 IPC")
        if daemon.get("ready") == "wm":
            wait_for(lambda: _wm_ready(lab), daemon.get("ready_timeout", 10.0), f"{name} to manage the screen")
        grab = daemon.get("ready_grab")
        if grab:
            wait_for(
                lambda: (lab.ensure_alive(), lab.run(
                    ["python3", "/lab/xgrab.py", str(grab[0]), str(grab[1])], check=False
                ).returncode == 0)[1],
                daemon.get("ready_timeout", 10.0),
                f"{name} to grab modmask {grab[0]} keycode {grab[1]}",
            )
    time.sleep(float(x11.get("settle", 0.5)))
    lab.ensure_alive()


def _i3_socket(lab) -> str | None:
    lab.ensure_alive()
    completed = lab.run(["i3", "--get-socketpath"], check=False)
    path = completed.stdout.decode().strip()
    return path if completed.returncode == 0 and path and os.path.exists(path) else None


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
