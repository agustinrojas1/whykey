#!/usr/bin/env python3
"""Type a key chord through a Wayland virtual keyboard with a real US keymap.

wtype uploads a private keymap that assigns arbitrary keycodes to the keysyms
it needs, so keycode-based bindings (Sway `--to-code`, `bindcode`) never see
the keycodes a physical keyboard produces. This client speaks the Wayland wire
protocol directly (stdlib only): it binds zwp_virtual_keyboard_manager_v1,
uploads the keymap `xkbcli compile-keymap --layout us` produces, and sends evdev
keycodes exactly like a physical US keyboard: modifiers down, key down, key up,
modifiers up. Virtual keyboards do not derive modifier state from key events
(wlroots applies key events with update_state=false), so the client also sends
the modifiers request after each modifier change, as wayvnc and on-screen
keyboards do.

    python3 vkbd.py [--numlock] super+t [ctrl+shift+F7 ...]

--numlock types with NumLock locked (Mod2 in the locked modifiers), so the
keypad digit keys produce KP_0..KP_9.
"""

from __future__ import annotations

import array
import os
import socket
import struct
import subprocess
import sys
import time

EVDEV = {
    "escape": 1, "1": 2, "2": 3, "3": 4, "4": 5, "5": 6, "6": 7, "7": 8, "8": 9,
    "9": 10, "0": 11, "minus": 12, "equal": 13, "backspace": 14, "tab": 15,
    "q": 16, "w": 17, "e": 18, "r": 19, "t": 20, "y": 21, "u": 22, "i": 23,
    "o": 24, "p": 25, "bracketleft": 26, "bracketright": 27, "return": 28,
    "a": 30, "s": 31, "d": 32, "f": 33, "g": 34, "h": 35, "j": 36, "k": 37,
    "l": 38, "semicolon": 39, "apostrophe": 40, "grave": 41, "backslash": 43,
    "z": 44, "x": 45, "c": 46, "v": 47, "b": 48, "n": 49, "m": 50, "comma": 51,
    "period": 52, "slash": 53, "space": 57, "f1": 59, "f2": 60, "f3": 61,
    "f4": 62, "f5": 63, "f6": 64, "f7": 65, "f8": 66, "f9": 67, "f10": 68,
    "f11": 87, "f12": 88, "home": 102, "up": 103, "pageup": 104, "left": 105,
    "right": 106, "end": 107, "down": 108, "pagedown": 109, "insert": 110,
    "delete": 111,
    "kp_0": 82, "kp_1": 79, "kp_2": 80, "kp_3": 81, "kp_4": 75, "kp_5": 76,
    "kp_6": 77, "kp_7": 71, "kp_8": 72, "kp_9": 73, "kp_enter": 96,
    "kp_add": 78, "kp_subtract": 74, "kp_multiply": 55, "kp_divide": 98,
    "kp_decimal": 83,
}
# name -> (evdev keycode, real-modifier bit in the standard XKB keymap)
MODIFIERS = {
    "ctrl": (29, 4), "control": (29, 4), "shift": (42, 1), "alt": (56, 8),
    "mod1": (56, 8), "super": (125, 64), "logo": (125, 64), "mod4": (125, 64),
    "win": (125, 64),
}


class Wayland:
    def __init__(self) -> None:
        path = os.path.join(os.environ["XDG_RUNTIME_DIR"], os.environ.get("WAYLAND_DISPLAY", "wayland-0"))
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.next_id = 2
        self.buffer = b""

    def new_id(self) -> int:
        identifier = self.next_id
        self.next_id += 1
        return identifier

    def send(self, obj: int, opcode: int, payload: bytes = b"", fds: list[int] | None = None) -> None:
        header = struct.pack("<II", obj, ((8 + len(payload)) << 16) | opcode)
        ancillary = []
        if fds:
            ancillary = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", fds).tobytes())]
        self.sock.sendmsg([header + payload], ancillary)

    def events(self):
        """Yield (object, opcode, payload) until a sync callback fires."""
        while True:
            while len(self.buffer) >= 8:
                obj, word = struct.unpack_from("<II", self.buffer)
                size = word >> 16
                if len(self.buffer) < size:
                    break
                payload = self.buffer[8:size]
                self.buffer = self.buffer[size:]
                yield obj, word & 0xFFFF, payload
            data = self.sock.recv(65536)
            if not data:
                raise SystemExit("Wayland connection closed")
            self.buffer += data

    def roundtrip(self, handler) -> None:
        callback = self.new_id()
        self.send(1, 0, struct.pack("<I", callback))  # wl_display.sync
        for obj, opcode, payload in self.events():
            if obj == callback:
                return
            if obj == 1 and opcode == 0:
                _, code = struct.unpack_from("<II", payload)
                message = string_at(payload, 8)[0]
                raise SystemExit(f"Wayland protocol error {code}: {message}")
            handler(obj, opcode, payload)


def string(value: str) -> bytes:
    data = value.encode() + b"\0"
    return struct.pack("<I", len(data)) + data + b"\0" * (-len(data) % 4)


def string_at(payload: bytes, offset: int) -> tuple[str, int]:
    (length,) = struct.unpack_from("<I", payload, offset)
    start = offset + 4
    value = payload[start:start + length - 1].decode()
    return value, start + length + (-length % 4)


def parse_chord(chord: str) -> tuple[list[tuple[int, int]], int]:
    parts = chord.split("+")
    modifiers = []
    for part in parts[:-1]:
        modifier = MODIFIERS.get(part.lower())
        if modifier is None:
            raise SystemExit(f"unknown modifier {part!r}")
        modifiers.append(modifier)
    # A lone modifier ("shift") is a chord whose key is that modifier key.
    name = parts[-1].lower()
    key = EVDEV.get(name, MODIFIERS.get(name, (None,))[0])
    if key is None:
        raise SystemExit(f"unknown key {parts[-1]!r}")
    return modifiers, key


def main() -> None:
    arguments = sys.argv[1:]
    locked = 0
    if arguments[:1] == ["--numlock"]:
        arguments = arguments[1:]
        locked = 16  # Mod2, NumLock in the standard XKB keymap
    chords = [parse_chord(chord) for chord in arguments]
    keymap = subprocess.run(
        ["xkbcli", "compile-keymap", "--layout", "us"],
        check=True, capture_output=True,
    ).stdout + b"\0"
    wayland = Wayland()
    registry = wayland.new_id()
    wayland.send(1, 1, struct.pack("<I", registry))  # wl_display.get_registry
    globals_: dict[str, tuple[int, int]] = {}

    def on_global(obj: int, opcode: int, payload: bytes) -> None:
        if obj == registry and opcode == 0:
            (name,) = struct.unpack_from("<I", payload)
            interface, offset = string_at(payload, 4)
            (version,) = struct.unpack_from("<I", payload, offset)
            globals_.setdefault(interface, (name, version))

    wayland.roundtrip(on_global)
    for needed in ("wl_seat", "zwp_virtual_keyboard_manager_v1"):
        if needed not in globals_:
            raise SystemExit(f"compositor does not advertise {needed}")

    def bind(interface: str, version: int) -> int:
        identifier = wayland.new_id()
        name, _ = globals_[interface]
        wayland.send(registry, 0, struct.pack("<I", name) + string(interface) + struct.pack("<II", version, identifier))
        return identifier

    seat = bind("wl_seat", 1)
    manager = bind("zwp_virtual_keyboard_manager_v1", 1)
    keyboard = wayland.new_id()
    wayland.send(manager, 0, struct.pack("<II", seat, keyboard))  # create_virtual_keyboard
    memfd = os.memfd_create("whykey-lab-keymap")
    os.write(memfd, keymap)
    wayland.send(keyboard, 0, struct.pack("<II", 1, len(keymap)), [memfd])  # keymap, XKB_V1
    os.close(memfd)
    wayland.roundtrip(lambda *event: None)

    clock = int(time.monotonic() * 1000) & 0xFFFFFFFF

    def key(code: int, pressed: bool) -> None:
        nonlocal clock
        clock = (clock + 10) & 0xFFFFFFFF
        wayland.send(keyboard, 1, struct.pack("<III", clock, code, 1 if pressed else 0))
        wayland.roundtrip(lambda *event: None)
        time.sleep(0.02)

    def modifiers(mask: int) -> None:
        wayland.send(keyboard, 2, struct.pack("<IIII", mask, 0, locked, 0))
        wayland.roundtrip(lambda *event: None)

    # Prime the new device: its first event makes the compositor switch the
    # seat to this keyboard and send clients the new keymap. A key press that
    # is that first event can be swallowed by the switch (seen on Sway: only
    # the release reached foot), so make the switch with a no-op modifiers
    # event and give clients time to apply the keymap before typing.
    modifiers(0)
    time.sleep(0.15)

    for chord_modifiers, code in chords:
        mask = 0
        for modifier, bit in chord_modifiers:
            key(modifier, True)
            mask |= bit
            modifiers(mask)
        key(code, True)
        key(code, False)
        for modifier, bit in reversed(chord_modifiers):
            key(modifier, False)
            mask &= ~bit
            modifiers(mask)
    wayland.send(keyboard, 3)  # destroy
    wayland.roundtrip(lambda *event: None)


if __name__ == "__main__":
    main()
