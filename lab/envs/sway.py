"""Headless Sway; config at sway.config (default /root/.config/sway/config).
whykey gets SWAYSOCK and WAYLAND_DISPLAY, as Sway exports them to its clients.
"""

from __future__ import annotations

import glob
import json
import os

from common import RUNTIME_DIR, wait_for


def start(lab) -> None:
    config = lab.scenario.get("sway", {}).get("config", "/root/.config/sway/config")
    env = dict(lab.env)
    env.update(
        WLR_BACKENDS="headless",
        WLR_LIBINPUT_NO_DEVICES="1",
        WLR_RENDERER="pixman",
    )
    lab.spawn("sway", ["sway", "-c", config], env)
    socket = wait_for(
        lambda: (lab.ensure_alive(), glob.glob(f"{RUNTIME_DIR}/sway-ipc.*.sock"))[1],
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
    lab.env["SWAYSOCK"] = socket
    lab.env["WAYLAND_DISPLAY"] = os.path.basename(display)
    wait_for(lambda: lab.ok(["swaymsg", "-t", "get_version"]), 10.0, "Sway IPC")


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/vkbd.py", *keys])


def focused_window(lab) -> tuple[str, str]:
    completed = lab.run(["swaymsg", "-t", "get_tree"], check=False)
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
