"""KWin (Plasma's window manager and compositor) on DRM in a KVM guest.

kwin_wayland --drm runs on a virtio-gpu KMS device in the guest (VM spec
below, booted like the hyprland environment), and keys come from the guest's
QEMU virtio keyboard (lab/qkbd.py), so they take the kernel path
evdev -> libinput -> KWin. KWin's virtual-keyboard protocol is not offered to
ordinary clients, so the lab does not use it.

KWin owns its global shortcuts in-process. kwin_wayland's
GlobalShortcutsManager starts a KGlobalAccelD with the org.kde.kwin platform
(src/globalshortcuts.cpp) and claims the org.kde.kglobalaccel bus name itself;
GlobalShortcutFilter then hands every key press to that registry before
clients see it. So no separate kglobalacceld runs here: its main() returns at
once when XDG_SESSION_TYPE is wayland, and a daemon started first (as the x11
and kde scenarios do) takes the bus name, so KWin's own registration fails
("Init of kglobalaccel failed") and KWin consumes nothing.

KWin's shortcuts are listed under the "kwin" component on org.kde.kglobalaccel.
Scenario files may write ~/.config/kglobalshortcutsrc, whose [kwin] group holds
KWin's shortcuts, to rebind them.

The clients see a KDE Wayland session through XDG_CURRENT_DESKTOP=KDE,
XDG_SESSION_DESKTOP=KDE, XDG_SESSION_TYPE=wayland and KDE_SESSION_VERSION=6.
KDE_FULL_SESSION is not exported to KWin: with it set, kwin_wayland aborts in
this image (no Plasma session components are installed).
"""

from __future__ import annotations

import glob
import os
import subprocess

from common import RUNTIME_DIR, VM_INPUT_SETTLE, X_DISPLAY, LabError, wait_for

VM = {"qemu_opts": "-device virtio-gpu-pci", "memory": "2G", "cpus": 2, "keyboard": True}
SOCKET = "wayland-kwin"
KWIN_ARGS = ["kwin_wayland", "--drm", "--xwayland", "--no-lockscreen", "--socket", SOCKET]
# Advertised to clients and whykey: a KDE Wayland session (see docstring).
SESSION_ENV = {
    "XDG_CURRENT_DESKTOP": "KDE",
    "XDG_SESSION_DESKTOP": "KDE",
    "XDG_SESSION_TYPE": "wayland",
    "KDE_SESSION_VERSION": "6",
}
KWIN_COMPONENT = "/component/kwin"


def docker_args() -> list[str]:
    # The guest uses KVM when the host offers it, else (slow) TCG.
    return ["--device", "/dev/kvm"] if os.path.exists("/dev/kvm") else []


def _busctl(lab, method_args: list[str], timeout: float = 3.0) -> str:
    """busctl --user output for a call on org.kde.kglobalaccel, or "" while
    the bus name is not answering."""
    try:
        completed = lab.run(
            ["busctl", "--user", "--auto-start=no", "--json=short", "call", "org.kde.kglobalaccel", *method_args],
            timeout=timeout, check=False,
        )
    except subprocess.TimeoutExpired:
        return ""
    return completed.stdout.decode("utf-8", "replace") if completed.returncode == 0 else ""


def _components(lab) -> str:
    """kglobalaccel's component object paths as JSON, or "" while it is not
    answering (KWin may register its name before it can serve calls)."""
    return _busctl(lab, ["/kglobalaccel", "org.kde.KGlobalAccel", "allComponents"])


def _start_kglobalacceld_first(lab) -> None:
    """The wrong setup, on purpose: a kglobalacceld on Xvfb takes
    org.kde.kglobalaccel before KWin starts (KWin's own registration then
    fails). It is started without XDG_SESSION_TYPE, so it does not exit."""
    lab.spawn("Xvfb", ["Xvfb", X_DISPLAY, "-screen", "0", "1024x768x24", "-nolisten", "tcp", "-noreset"])
    wait_for(
        lambda: (lab.ensure_alive(), os.path.exists(f"/tmp/.X11-unix/X{X_DISPLAY.lstrip(':')}"))[1],
        10.0,
        "Xvfb for kglobalacceld",
    )
    wait_for(lambda: lab.ok(["env", f"DISPLAY={X_DISPLAY}", "xdotool", "getmouselocation"]),
             10.0, "the X server")
    daemon_env = dict(lab.env, DISPLAY=X_DISPLAY, QT_QPA_PLATFORM="xcb")
    daemon_env.pop("XDG_SESSION_TYPE", None)
    lab.spawn("kglobalacceld", ["/usr/libexec/kglobalacceld"], daemon_env)
    wait_for(lambda: (lab.ensure_alive(), bool(_components(lab)))[1], 15.0, "kglobalacceld on the session bus")


def start(lab) -> None:
    if not glob.glob("/dev/dri/card*"):
        raise LabError("no KMS device: KWin scenarios must run inside the lab VM")
    if lab.scenario.get("kwin", {}).get("kglobalacceld_first"):
        _start_kglobalacceld_first(lab)
    # No seat daemon in the guest: libseat opens the devices directly.
    kwin_env = dict(lab.env, LIBSEAT_BACKEND="noop")
    kwin_env.pop("DISPLAY", None)
    kwin_env.update(SESSION_ENV)
    kwin_env["QT_LOGGING_RULES"] = "kwin_core.debug=true"
    lab.spawn("kwin_wayland", KWIN_ARGS, kwin_env)
    wait_for(
        lambda: (lab.ensure_alive(), os.path.exists(os.path.join(RUNTIME_DIR, SOCKET)))[1],
        60.0,
        "the KWin Wayland socket",
    )
    # KWin registers its shortcuts with org.kde.kglobalaccel in-process, once it runs.
    wait_for(
        lambda: (lab.ensure_alive(), KWIN_COMPONENT in _components(lab))[1],
        30.0,
        "KWin's shortcut component on org.kde.kglobalaccel",
    )
    lab.env.pop("DISPLAY", None)
    lab.env.update(SESSION_ENV, WAYLAND_DISPLAY=SOCKET)
    lab.steps.append("kwin_wayland --drm running; it owns org.kde.kglobalaccel with the kwin component")
    lab.settle = VM_INPUT_SETTLE


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/qkbd.py", *keys])


def qkbd(lab, action: dict) -> None:
    """Press keys on the guest's kernel keyboard (lab/qkbd.py), e.g. ["super+y"]."""
    lab.run(["python3", "/lab/qkbd.py", *action["qkbd"]])


ACTIONS = {"qkbd": qkbd}


def logs(lab) -> dict[str, str]:
    """kglobalaccel's view of KWin's shortcuts, for the result."""
    return {
        "kwin shortcuts (kglobalaccel allShortcutInfos)": _busctl(
            lab, [KWIN_COMPONENT, "org.kde.kglobalaccel.Component", "allShortcutInfos"], timeout=5.0),
        "kwin component active": _busctl(
            lab, [KWIN_COMPONENT, "org.kde.kglobalaccel.Component", "isActive"], timeout=5.0),
    }
