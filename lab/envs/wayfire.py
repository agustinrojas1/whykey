"""Headless Wayfire, run as the `wayfire` user (wayfire refuses to start as root).

The config is wayfire.config (default /root/.config/wayfire.ini). The plugin
copies it where the wayfire user can read it, gives that user its own runtime
directory, and points lab.env's XDG_RUNTIME_DIR and WAYLAND_DISPLAY at the
Wayfire socket so the probe and the injector connect to it. Wayfire has no
window-query IPC in the lab image, so focus is checked with the Shift tap alone.
"""

from __future__ import annotations

import glob
import os
import shutil

from common import wait_for

USER = "wayfire"
HOME = "/home/wayfire"
RUNTIME = "/tmp/wayfire-rt"


def start(lab) -> None:
    source = lab.scenario.get("wayfire", {}).get("config", "/root/.config/wayfire.ini")
    config_dir = os.path.join(HOME, ".config")
    config = os.path.join(config_dir, "wayfire.ini")
    os.makedirs(config_dir, exist_ok=True)
    shutil.copyfile(source, config)
    shutil.chown(config, USER, USER)
    os.makedirs(RUNTIME, exist_ok=True)
    os.chmod(RUNTIME, 0o700)
    shutil.chown(RUNTIME, USER, USER)
    env = dict(lab.env)
    env.update(
        HOME=HOME,
        XDG_RUNTIME_DIR=RUNTIME,
        WLR_BACKENDS="headless",
        WLR_LIBINPUT_NO_DEVICES="1",
        WLR_RENDERER="pixman",
    )
    lab.spawn(
        "wayfire",
        [
            "setpriv", f"--reuid={USER}", f"--regid={USER}", "--init-groups", "--",
            "wayfire", "-c", config,
        ],
        env,
    )
    display = wait_for(
        lambda: (lab.ensure_alive(), sorted(glob.glob(f"{RUNTIME}/wayland-*[0-9]")))[1],
        15.0,
        "the Wayfire Wayland socket",
    )[0]
    lab.env["XDG_RUNTIME_DIR"] = RUNTIME
    lab.env["WAYLAND_DISPLAY"] = os.path.basename(display)


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/vkbd.py", *keys])
