"""Headless labwc; rc.xml at labwc.config (default /root/.config/labwc/rc.xml),
passed with -c so the scenario's keybinds are the whole config. labwc has no
window-query IPC, so the probe's focus is checked with the Shift tap alone.
"""

from __future__ import annotations

import glob
import os

from common import RUNTIME_DIR, wait_for


def start(lab) -> None:
    config = lab.scenario.get("labwc", {}).get("config", "/root/.config/labwc/rc.xml")
    env = dict(lab.env)
    env.update(
        WLR_BACKENDS="headless",
        WLR_LIBINPUT_NO_DEVICES="1",
        WLR_RENDERER="pixman",
    )
    lab.spawn("labwc", ["labwc", "-c", config], env)
    display = wait_for(
        lambda: (lab.ensure_alive(), sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]")))[1],
        10.0,
        "the labwc Wayland socket",
    )[0]
    lab.env["WAYLAND_DISPLAY"] = os.path.basename(display)


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/vkbd.py", *keys])
