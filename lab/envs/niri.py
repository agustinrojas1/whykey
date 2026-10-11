"""niri, nested in a headless Sway (niri has no headless backend).

Sway is the outer compositor and the niri window is its client. The plain
`vkbd` action types into lab.env, which here is niri's own socket; observed:
niri did not run its binds for keys typed there, so that path is unusable for
bind scenarios. The `outer_vkbd` action types into Sway's socket instead; Sway
forwards that to niri as keyboard input, and scenarios use it for key presses.
niri's config is config.kdl (default /root/.config/niri/config.kdl). whykey
and the probe get niri's own WAYLAND_DISPLAY and NIRI_SOCKET.
"""

from __future__ import annotations

import glob
import json
import os
import subprocess

from common import LabError, RUNTIME_DIR, wait_for

_INPUT_DISPLAY = ""


def _displays() -> list[str]:
    return sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]"))


def start(lab) -> None:
    global _INPUT_DISPLAY
    config = lab.scenario.get("niri", {}).get("config", "/root/.config/niri/config.kdl")
    host = dict(lab.env)
    host.update(
        WLR_BACKENDS="headless",
        WLR_LIBINPUT_NO_DEVICES="1",
        WLR_RENDERER="pixman",
    )
    lab.spawn("sway", ["sway", "-c", os.devnull], host)
    outer = wait_for(
        lambda: (lab.ensure_alive(), _displays())[1], 10.0, "the outer Sway Wayland socket"
    )[0]
    _INPUT_DISPLAY = os.path.basename(outer)
    nested = dict(lab.env)
    nested["WAYLAND_DISPLAY"] = _INPUT_DISPLAY
    lab.spawn("niri", ["niri", "-c", config], nested)
    niri_display = wait_for(
        lambda: (lab.ensure_alive(), [d for d in _displays() if d != outer])[1],
        15.0,
        "the niri Wayland socket",
    )[0]
    niri_socket = wait_for(
        lambda: (lab.ensure_alive(), glob.glob(f"{RUNTIME_DIR}/niri.*.sock"))[1],
        15.0,
        "the niri IPC socket",
    )[0]
    lab.env["WAYLAND_DISPLAY"] = os.path.basename(niri_display)
    lab.env["NIRI_SOCKET"] = niri_socket


def inject(lab, keys: list[str]) -> None:
    # Typed into the outer Sway, which forwards to the focused niri window.
    env = dict(lab.env)
    env["WAYLAND_DISPLAY"] = _INPUT_DISPLAY
    completed = subprocess.run(
        ["python3", "/lab/vkbd.py", *keys],
        stdin=subprocess.DEVNULL,
        capture_output=True,
        env=env,
        cwd="/root",
        timeout=10.0,
    )
    if completed.returncode != 0:
        raise LabError(f"vkbd into Sway exited {completed.returncode}: "
                       f"{completed.stderr.decode('utf-8', 'replace').strip()}")


def outer_vkbd(lab, action: dict) -> None:
    """Type keys into the outer Sway, e.g. ["super+t"] (see the module docstring)."""
    inject(lab, action["outer_vkbd"])


ACTIONS = {"outer_vkbd": outer_vkbd}


def focused_window(lab) -> tuple[str, str]:
    completed = lab.run(["niri", "msg", "--json", "focused-window"], check=False)
    if completed.returncode != 0:
        return "", ""
    window = json.loads(completed.stdout or b"null")
    if not window:
        return "", ""
    return window.get("app_id") or "", window.get("title") or ""
