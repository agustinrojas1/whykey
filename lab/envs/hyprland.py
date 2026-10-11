"""Hyprland 0.56.2 in a KVM guest booted inside the container, on a
virtio-gpu KMS device; config from hyprland.config or Hyprland's own lookup
(~/.config/hypr/hyprland.lua, then hyprland.conf). whykey gets the
environment Hyprland exports to its clients.

Hyprland cannot run in a plain container: aquamarine needs a KMS device to
allocate buffers even for headless outputs. The driver therefore boots the
image's own kernel in a KVM guest (VM below; virtme-ng, root filesystem =
the container over 9p, virtio-gpu for KMS, Mesa kms_swrast/llvmpipe for GL)
and runs the scenario there.
The guest's keyboard is a QEMU virtio keyboard driven over QMP (the "keyboard"
VM spec and the qkbd action), so keys take the kernel path evdev -> libinput ->
Hyprland.
"""

from __future__ import annotations

import glob
import json
import os

from common import RUNTIME_DIR, VM_INPUT_SETTLE, LabError, wait_for

VM = {"qemu_opts": "-device virtio-gpu-pci", "memory": "2G", "cpus": 2, "keyboard": True}


def docker_args() -> list[str]:
    # The VM uses KVM when the host offers it, else (slow) TCG.
    return ["--device", "/dev/kvm"] if os.path.exists("/dev/kvm") else []


def start(lab) -> None:
    if not glob.glob("/dev/dri/card*"):
        raise LabError("no KMS device: Hyprland scenarios must run inside the lab VM")
    argv = ["Hyprland", "--i-am-really-stupid"]  # root, in a throwaway VM
    config = lab.scenario.get("hyprland", {}).get("config")
    if config:
        argv += ["--config", config]
    env = dict(lab.env)
    # No logind/seatd in the guest: libseat opens devices directly.
    env["LIBSEAT_BACKEND"] = "noop"
    lab.spawn("Hyprland", argv, env)
    socket = wait_for(
        lambda: (lab.ensure_alive(), glob.glob(f"{RUNTIME_DIR}/hypr/*/.socket.sock"))[1],
        30.0,
        "the Hyprland IPC socket",
    )[0]
    display = wait_for(
        lambda: sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]")),
        10.0,
        "the Wayland socket",
    )[0]
    # What Hyprland exports to every client it starts (checked with an
    # `exec env` bind); whykey run from a terminal in the session inherits it.
    lab.env.update(
        HYPRLAND_INSTANCE_SIGNATURE=os.path.basename(os.path.dirname(socket)),
        WAYLAND_DISPLAY=os.path.basename(display),
        XDG_CURRENT_DESKTOP="Hyprland",
        XDG_SESSION_TYPE="wayland",
    )
    wait_for(lambda: lab.ok(["hyprctl", "version"]), 30.0, "Hyprland IPC")
    version = lab.run(["hyprctl", "version"]).stdout.decode().splitlines()[0]
    lab.steps.append(f"hyprctl version: {version}")
    # D-090: the VM's keyboard (QEMU virtio-keyboard-pci) reaches Hyprland as a
    # kernel keyboard device, listed by name under "keyboards".
    keyboard = wait_for(lambda: kernel_keyboards(lab), 15.0, "the VM keyboard in hyprctl devices")[0]
    lab.steps.append(f"hyprctl devices: {keyboard.get('name')!r} keyboard")
    # Binds exec through Hyprland in a 2-vCPU guest that shares the host
    # with other scenarios; give their effects time to land.
    lab.settle = VM_INPUT_SETTLE


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/vkbd.py", *keys])


def kernel_keyboards(lab) -> list[dict]:
    """Keyboards Hyprland lists under the name of QEMU's virtio keyboard device."""
    devices = json.loads(lab.run(["hyprctl", "devices", "-j"]).stdout or b"{}")
    return [kb for kb in devices.get("keyboards", []) if kb.get("name") == "qemu-virtio-keyboard"]


def qkbd(lab, action: dict) -> None:
    """Press keys on the kernel keyboard (lab/qkbd.py), e.g. ["super+t"]."""
    lab.run(["python3", "/lab/qkbd.py", *action["qkbd"]])


ACTIONS = {"qkbd": qkbd}


def focused_window(lab) -> tuple[str, str]:
    completed = lab.run(["hyprctl", "activewindow", "-j"], check=False)
    try:
        window = json.loads(completed.stdout or b"{}")
    except json.JSONDecodeError:
        return "", ""
    return window.get("class") or "", window.get("title") or ""


def logs(lab) -> dict[str, str]:
    # Hyprland stops logging to stdout once its config is parsed.
    logs = {}
    logs["hyprctl devices"] = lab.run(["hyprctl", "devices", "-j"], check=False).stdout.decode(
        errors="replace"
    )
    for path in sorted(glob.glob(f"{RUNTIME_DIR}/hypr/*/hyprland.log")):
        with open(path, encoding="utf-8", errors="replace") as handle:
            logs["Hyprland instance log"] = "\n".join(handle.read().splitlines()[-25:])
    return logs
