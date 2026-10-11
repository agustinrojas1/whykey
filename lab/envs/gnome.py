"""Headless mutter (GNOME's compositor and window manager) on a virtual
monitor. GNOME Shell does not start in a container (it aborts without
systemd-logind), so only mutter-owned keybindings
(org.gnome.desktop.wm.keybindings) behave for real; whykey gets
XDG_CURRENT_DESKTOP=GNOME. Input enters through mutter's RemoteDesktop D-Bus
API (lab/rdkbd.py). Mutter has no window-query IPC, so there is no
focused_window: the probe's Shift tap is its focus check.
"""

from __future__ import annotations

import glob
import os

from common import RUNTIME_DIR, VM_INPUT_SETTLE, wait_for


def start(lab) -> None:
    lab.spawn("mutter", ["mutter", "--headless", "--wayland", "--no-x11",
                         "--virtual-monitor", "1024x768"])
    display = wait_for(
        lambda: (lab.ensure_alive(), sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]")))[1],
        10.0,
        "the mutter Wayland socket",
    )[0]
    # What a GNOME session exports to its clients.
    lab.env.update(
        WAYLAND_DISPLAY=os.path.basename(display),
        XDG_CURRENT_DESKTOP="GNOME",
        XDG_SESSION_DESKTOP="gnome",
        XDG_SESSION_TYPE="wayland",
    )
    wait_for(
        lambda: lab.ok(["gdbus", "introspect", "--session", "--dest",
                        "org.gnome.Mutter.RemoteDesktop", "--object-path",
                        "/org/gnome/Mutter/RemoteDesktop"]),
        10.0,
        "mutter's RemoteDesktop D-Bus service",
    )
    # A close request travels mutter -> foot -> foot's exit; give it time.
    lab.settle = VM_INPUT_SETTLE


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/rdkbd.py", *keys])
