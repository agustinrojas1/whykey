"""A pre-compositor key remapper (keyd, kanata, xremap, kmonad or
input-remapper) in front of Hyprland, inside the Hyprland KVM guest.

The guest is hyprland.py's (Hyprland 0.56.2, the VM's kernel keyboard
"qemu-virtio-keyboard" from qkbd). Before the scenario's actions the
remapper daemon is started as root; it grabs that keyboard and writes
its output to a uinput device, so Hyprland receives the remapped keys
from the uinput device, not from qkbd. Keys the remapper does not handle
reach Hyprland unchanged through the same uinput device.

Scenario fields, all under "remap":
  "daemon": the remapper process, one {"argv": [...], "env": {...},
      "ready": NAME, "autoload": DIR, "autoload_user": USER,
      "autoload_home": HOME} object or a list of them, or [] for none.
      It runs as root with the lab environment. "ready": substring of the
      keyboard name the daemon's uinput device gets in `hyprctl devices`
      (normalized: case and punctuation ignored); start waits for it.
      "autoload": input-remapper only. Its root service autoloads nothing
      until a user's control process gives it DIR, so `input-remapper-control
      --command autoload` runs as USER (with HOME and XDG_CONFIG_HOME set).
  "system_bus": true starts a system D-Bus first (input-remapper's service
      talks over it).
  "settle": extra seconds after the daemons are ready (default 1).
Configuration files are plain "files" entries (e.g. /etc/keyd/default.conf).
Actions: "qkbd" (the VM kernel keyboard; CapsLock is "capslock").

The uinput kernel module is loaded here (`modprobe uinput`); the guest
kernel is the lab image's own kernel with its modules, so the module
is available from /lib/modules.
"""

from __future__ import annotations

import json
import os
import stat
import time

from common import LabError, wait_for
from envs import hyprland

VM = hyprland.VM
docker_args = hyprland.docker_args
inject = hyprland.inject
focused_window = hyprland.focused_window
logs = hyprland.logs


# lab/qkbd.py has no name for CapsLock; the wrapper adds the QEMU key name
# without changing qkbd.py for every environment.
QKBD_EXTRA_KEYS = {"capslock": "caps_lock"}
QKBD_WRAPPER = (
    "import sys; sys.path.insert(0, '/lab'); import qkbd; "
    f"qkbd.KEYS.update({QKBD_EXTRA_KEYS!r}); qkbd.main()"
)


def qkbd(lab, action: dict) -> None:
    """Press keys on the VM kernel keyboard, as hyprland.qkbd does."""
    lab.run(["python3", "-c", QKBD_WRAPPER, *action["qkbd"]])


ACTIONS = {"qkbd": qkbd}


def start(lab) -> None:
    hyprland.start(lab)
    load_uinput(lab)
    remap = lab.scenario.get("remap", {})
    if "daemon" not in remap:
        raise LabError('the remap environment needs "remap": {"daemon": ...} (or [] for none)')
    if remap.get("system_bus"):
        # input-remapper's service talks over the system bus (dasbus).
        os.makedirs("/run/dbus", exist_ok=True)
        lab.run(["dbus-daemon", "--system", "--fork", "--nopidfile"])
        lab.steps.append("system D-Bus started for the remapper service")
    daemons = remap["daemon"]
    if isinstance(daemons, dict):
        daemons = [daemons]
    for daemon in daemons:
        argv = daemon["argv"]
        name = os.path.basename(argv[0])
        env = dict(lab.env)
        env.update(daemon.get("env", {}))
        lab.spawn(name, argv, env)
        autoload = daemon.get("autoload")
        if autoload:
            # input-remapper's root service autoloads nothing until a logged-in
            # user's control process gives it the config directory (only a
            # non-root control process does that). The desktop autostart runs
            # `input-remapper-control --command autoload` as the user, so do the
            # same as "autoload_user" with that user's home.
            argv_control = ["input-remapper-control", "--command", "autoload", "--config-dir", autoload]
            user = daemon.get("autoload_user")
            if user:
                home = daemon["autoload_home"]
                argv_control = [
                    "runuser", "-u", user, "--", "env", f"HOME={home}", f"USER={user}",
                    f"XDG_CONFIG_HOME={home}/.config", *argv_control,
                ]
            wait_for(
                lambda: (lab.ensure_alive(), lab.run(argv_control, timeout=20.0, check=False).returncode == 0)[1],
                float(daemon.get("ready_timeout", 30.0)),
                f"{name} to accept the autoload command",
            )
            lab.steps.append(f"{name}: autoload requested for {autoload}")
        ready = daemon.get("ready")
        if ready:
            wait_for(
                lambda: (lab.ensure_alive(), uinput_keyboard(lab, ready))[1],
                float(daemon.get("ready_timeout", 30.0)),
                f"{name} to create its keyboard named like {ready!r}",
            )
            lab.steps.append(f"{name}: keyboard {ready!r} listed by Hyprland")
    time.sleep(float(remap.get("settle", 1.0)))
    lab.ensure_alive()


def load_uinput(lab) -> None:
    """Make /dev/uinput available; the lab kernel builds it as a module."""
    if not os.path.exists("/dev/uinput"):
        completed = lab.run(["modprobe", "uinput"], check=False)
        if completed.returncode != 0:
            raise LabError(f"modprobe uinput failed: {completed.stderr.decode(errors='replace').strip()}")
        # Without devtmpfs udev nodes, create the misc device by hand (10:223).
        if not os.path.exists("/dev/uinput"):
            os.mknod("/dev/uinput", 0o600 | stat.S_IFCHR, os.makedev(10, 223))
    lab.steps.append("uinput: /dev/uinput available in the guest")


def uinput_keyboard(lab, name: str) -> bool:
    """Whether Hyprland lists a keyboard whose name contains `name`."""
    completed = lab.run(["hyprctl", "devices", "-j"], check=False)
    try:
        devices = json.loads(completed.stdout or b"{}")
    except json.JSONDecodeError:
        return False
    # Hyprland spells the uinput names with dashes ("keyd-virtual-keyboard").
    wanted = normalized(name)
    return any(wanted in normalized(keyboard.get("name", "")) for keyboard in devices.get("keyboards", []))


def normalized(text: str) -> str:
    return "".join(character for character in text.lower() if character.isalnum())
