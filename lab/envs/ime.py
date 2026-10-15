"""An input-method daemon (Fcitx5 or IBus) running on a session the lab starts.

ime.daemon   required: "fcitx5" or "ibus"
ime.session  "sway" (default: headless Sway, the probe is foot on Wayland) or
             "x11" (Xvfb, the probe is xev, or xterm with ime.client)
ime.engine   optional selection made before the client starts, in the shape of
             the actions below, e.g. {"ibus": {"engine": "libpinyin"}}
ime.client   "xterm" (X11 only): an XIM client, xterm, is started on a pty
             that logs every byte it receives to /tmp/probe.log, and focused
             after the daemon is up. Its committed text and raw keys are the
             ground truth; use "probe": null with it
ime.env      variables exported to the daemon, to the scenario's actions and
             to whykey, the way a desktop session exports them (for example
             GTK_IM_MODULE, QT_IM_MODULE, XMODIFIERS)
ime.profile  Fcitx5 only: optional profile text for ~/.config/fcitx5/profile.
             The default has the keyboard layout and pinyin, both in the
             default group; the scenario selects one with `fcitx5-remote -s`

The session comes up through lab/envs/sway.py or lab/envs/x11.py, then the
daemon is started on the scenario's session D-Bus and waited on until it owns
its well-known name. Input is sent with the session's own injector (vkbd on
Sway, xdotool on X11).

Actions (scenario setup only; whykey's own runtime reads are never replaced):
{"fcitx5": {"engine": NAME, "state": "active" | "inactive"}} selects a Fcitx5
input method and opens or closes it on the focused client, and waits until
the daemon reports both; {"ibus": {"engine": NAME}} sets the IBus global
engine and waits until `ibus engine` reports it; {"dbus_send": [args]} runs
dbus-send on the session bus.
Checks: {"committed": "text"} is true when /tmp/probe.log holds the UTF-8 bytes
of "text" (the text an input method committed reached the client).
{"typed": "text"} is true when /tmp/probe.log holds those bytes as the client
received them raw (ime.client xterm only; keys the input method consumed do
not appear).
"""

from __future__ import annotations

import os
import time

from . import sway
from . import x11
from common import wait_for

# Well-known bus names the daemons own once they are ready.
BUS_NAMES = {"fcitx5": "org.fcitx.Fcitx5", "ibus": "org.freedesktop.IBus"}
PROBE_LOG = "/tmp/probe.log"
IBUS_ENGINE_SETTLE = 3.0
XTERM_TITLE = "whykey-probe"

# Keep both pinyin and the keyboard layout in the default group, so a
# scenario selects its input method with `fcitx5-remote -s`.
DEFAULT_FCITX5_PROFILE = """[Groups/0]
Name=Default
Default Layout=us
DefaultIM=keyboard-us

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=pinyin
Layout=

[GroupOrder]
0=Default
"""


def start(lab) -> None:
    spec = lab.scenario.get("ime", {})
    daemon = spec.get("daemon")
    if daemon not in BUS_NAMES:
        raise RuntimeError(f"ime.daemon must be one of {sorted(BUS_NAMES)}, not {daemon!r}")
    session = spec.get("session", "sway")
    lab.env.update({key: str(value) for key, value in spec.get("env", {}).items()})
    if session == "sway":
        sway.start(lab)
    elif session == "x11":
        lab.scenario.setdefault("x11", {})["daemons"] = []
        x11.start(lab)
    else:
        raise RuntimeError(f"ime.session must be 'sway' or 'x11', not {session!r}")
    if daemon == "fcitx5":
        _start_fcitx5(lab, spec)
    else:
        _start_ibus(lab)
    # An engine chosen before the client starts is the one its input context
    # is created with (IBus XIM clients keep an engine picked after focus only
    # after a second switch).
    for key, value in spec.get("engine", {}).items():
        ACTIONS[key](lab, {key: value})
    if spec.get("client") == "xterm":
        if session != "x11":
            raise RuntimeError("ime.client xterm needs ime.session x11")
        _start_xterm(lab)


def _start_fcitx5(lab, spec) -> None:
    path = "/root/.config/fcitx5/profile"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    profile = spec.get("profile", DEFAULT_FCITX5_PROFILE)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(profile if profile.endswith("\n") else profile + "\n")
    lab.spawn("fcitx5", ["fcitx5"], lab.env)
    _wait_for_bus_name(lab, "fcitx5")


def _start_ibus(lab) -> None:
    lab.spawn("ibus-daemon", ["ibus-daemon", "--panel=disable", "--replace", "--xim"], lab.env)
    _wait_for_bus_name(lab, "ibus")


def _start_xterm(lab) -> None:
    # `stty raw -echo` makes xterm's bytes arrive unmodified; cat logs them.
    lab.spawn(
        "xterm",
        ["xterm", "-title", XTERM_TITLE, "-e", "sh", "-c", f"stty raw -echo; cat > {PROBE_LOG}"],
        lab.env,
    )

    def focused() -> bool:
        lab.ensure_alive()
        windows = lab.run(["xdotool", "search", "--name", XTERM_TITLE], check=False).stdout.split()
        if not windows:
            return False
        lab.run(["xdotool", "windowfocus", "--sync", windows[0].decode()], check=False)
        focus = lab.run(["xdotool", "getwindowfocus", "getwindowname"], check=False)
        return focus.stdout.decode().strip() == XTERM_TITLE

    wait_for(focused, 15.0, "the xterm input client to take focus")
    lab.steps.append("xterm client focused")


def _wait_for_bus_name(lab, daemon: str) -> None:
    name = BUS_NAMES[daemon]

    def registered() -> bool:
        lab.ensure_alive()
        completed = lab.run(
            ["dbus-send", "--session", "--print-reply=literal", "--dest=org.freedesktop.DBus",
             "/org/freedesktop/DBus", "org.freedesktop.DBus.ListNames"],
            check=False,
        )
        return name.encode() in completed.stdout

    wait_for(registered, 30.0, f"{daemon} to own {name} on the session bus")


def inject(lab, keys: list[str]) -> None:
    if lab.scenario.get("ime", {}).get("session", "sway") == "x11":
        x11.inject(lab, keys)
    else:
        sway.inject(lab, keys)


def focused_window(lab) -> tuple[str, str]:
    if lab.scenario.get("ime", {}).get("session", "sway") == "x11":
        return x11.focused_window(lab)
    return sway.focused_window(lab)


def _dbus_send(lab, action: dict) -> None:
    lab.run(["dbus-send", "--session", *action["dbus_send"]])


def _fcitx5(lab, action: dict) -> None:
    """{"fcitx5": {"engine": NAME, "state": "active" | "inactive"}}: select the
    input method and open or close it on the focused client, then wait until
    the daemon reports the choice (fcitx5-remote -n and the state query)."""
    spec = action["fcitx5"]
    engine = spec["engine"]
    lab.run(["fcitx5-remote", "-s", engine])
    wait_for(lambda: _remote(lab, "-n") == engine, 10.0, f"fcitx5 to select {engine}")
    wanted = {"active": "2", "inactive": "1"}[spec.get("state", "active")]
    lab.run(["fcitx5-remote", "-o" if wanted == "2" else "-c"])
    wait_for(lambda: _remote(lab, "") == wanted, 10.0, f"fcitx5 state {wanted}")


def _remote(lab, option: str) -> str:
    completed = lab.run(["fcitx5-remote", *([option] if option else [])], check=False)
    return completed.stdout.decode().strip()


def _ibus(lab, action: dict) -> None:
    """{"ibus": {"engine": NAME}}: set the global IBus engine and wait until
    `ibus engine` reports it. The CLI can exit 1 after a switch that took
    effect (the engine process is still starting), so the exit status is not
    the success test; the reported engine is."""
    engine = action["ibus"]["engine"]

    def selected() -> bool:
        lab.ensure_alive()
        lab.run(["ibus", "engine", engine], check=False)
        return lab.run(["ibus", "engine"], check=False).stdout.decode().strip() == engine

    wait_for(selected, 30.0, f"IBus to select {engine}", interval=1.0)
    # The reported engine can be selected before its process accepts focus
    # and key events; keys sent at once pass through uncomposed.
    time.sleep(IBUS_ENGINE_SETTLE)


ACTIONS = {"dbus_send": _dbus_send, "fcitx5": _fcitx5, "ibus": _ibus}


def _read_log() -> bytes:
    try:
        with open(PROBE_LOG, "rb") as handle:
            return handle.read()
    except FileNotFoundError:
        return b""


def _committed(lab, spec: dict) -> tuple[bool, str]:
    text = spec["committed"]
    found = text.encode("utf-8") in _read_log()
    return found, f"probe log {'contains' if found else 'lacks'} committed text {text!r}"


def _typed(lab, spec: dict) -> tuple[bool, str]:
    text = spec["typed"]
    found = text.encode("utf-8") in _read_log()
    return found, f"probe log {'contains' if found else 'lacks'} typed key {text!r}"


CHECKS = {"committed": _committed, "typed": _typed}
