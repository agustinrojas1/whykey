#!/usr/bin/env python3
"""Type a key chord into GNOME (mutter / GNOME Shell) as a remote keyboard.

Mutter implements no Wayland virtual-keyboard protocol and no XTEST for
Wayland clients; its input-injection path is the org.gnome.Mutter.RemoteDesktop
D-Bus API that gnome-remote-desktop uses. This client creates a remote desktop
session on the session bus, starts it and sends evdev keycodes (same chord
syntax and US keycodes as lab/vkbd.py): modifiers down, key down, key up,
modifiers up. The events enter mutter's input stack like a physical
keyboard's, so mutter and GNOME Shell keybindings see them. The session ends
when this process disconnects.

    python3 rdkbd.py alt+F4 [super+y ...]
"""

from __future__ import annotations

import sys
import time

import dbus

from vkbd import parse_chord

SERVICE = "org.gnome.Mutter.RemoteDesktop"
STEP = 0.05


def main() -> None:
    chords = [parse_chord(chord) for chord in sys.argv[1:]]
    bus = dbus.SessionBus()
    manager = dbus.Interface(
        bus.get_object(SERVICE, "/org/gnome/Mutter/RemoteDesktop"), SERVICE
    )
    session = dbus.Interface(
        bus.get_object(SERVICE, manager.CreateSession()), f"{SERVICE}.Session"
    )
    session.Start()
    try:
        for modifiers, key in chords:
            for code, _ in modifiers:
                session.NotifyKeyboardKeycode(dbus.UInt32(code), True)
                time.sleep(STEP)
            session.NotifyKeyboardKeycode(dbus.UInt32(key), True)
            time.sleep(STEP)
            session.NotifyKeyboardKeycode(dbus.UInt32(key), False)
            time.sleep(STEP)
            for code, _ in reversed(modifiers):
                session.NotifyKeyboardKeycode(dbus.UInt32(code), False)
                time.sleep(STEP)
        # Let mutter process the queued events before the session closes.
        time.sleep(0.2)
    finally:
        session.Stop()


if __name__ == "__main__":
    main()
