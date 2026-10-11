#!/usr/bin/env python3
"""Type key chords on the lab VM's kernel keyboard (QEMU virtio-keyboard).

Inside the lab VM (lab/driver.py, VM spec "keyboard") QEMU emulates a
virtio-keyboard-pci device, the guest's only keyboard. This client sends one
QMP `input-send-event` per key transition through the virtio-serial port
whykey.keyboard; lab/driver.py relays each command line to QEMU's QMP socket and
returns QEMU's reply line. QEMU puts the event into the device's queue, the
guest's virtio_input driver reports it as evdev EV_KEY + SYN_REPORT, and the
compositor reads it through libinput like any physical keyboard: kernel
keycodes, the compositor's own keymap, no virtual-keyboard protocol.

    python3 qkbd.py super+t [ctrl+shift+F7 ...]

A chord presses its modifiers in order, then the key, and releases them in
reverse; a lone modifier ("super") taps that modifier key.
"""

from __future__ import annotations

import glob
import json
import os
import select
import sys
import time

PORT_NAME = "whykey.keyboard"
# Gap between key transitions: separate evdev frames, like a human typist.
STEP = 0.03
REPLY_TIMEOUT = 5.0


def find_port() -> str:
    """The guest device node of the whykey.keyboard virtio-serial port."""
    for name in sorted(glob.glob("/sys/class/virtio-ports/*/name")):
        with open(name, encoding="utf-8") as handle:
            if handle.read().strip() == PORT_NAME:
                return "/dev/" + os.path.basename(os.path.dirname(name))
    raise SystemExit(f"no {PORT_NAME} virtio port: the VM has no keyboard (VM spec \"keyboard\")")

# Chord names (as lab/vkbd.py spells them) -> QEMU QKeyCode.
KEYS = {
    "escape": "esc", "minus": "minus", "equal": "equal", "backspace": "backspace",
    "tab": "tab", "bracketleft": "bracket_left", "bracketright": "bracket_right",
    "return": "ret", "semicolon": "semicolon", "apostrophe": "apostrophe",
    "grave": "grave_accent", "backslash": "backslash", "comma": "comma",
    "period": "dot", "slash": "slash", "space": "spc", "home": "home", "up": "up",
    "pageup": "pgup", "left": "left", "right": "right", "end": "end", "down": "down",
    "pagedown": "pgdn", "insert": "insert", "delete": "delete",
    "kp_enter": "kp_enter", "kp_add": "kp_add", "kp_subtract": "kp_subtract",
    "kp_multiply": "kp_multiply", "kp_divide": "kp_divide", "kp_decimal": "kp_decimal",
    **{chr(c): chr(c) for c in range(ord("a"), ord("z") + 1)},
    **{str(d): str(d) for d in range(10)},
    **{f"kp_{d}": f"kp_{d}" for d in range(10)},
    **{f"f{n}": f"f{n}" for n in range(1, 13)},
}
MODIFIERS = {
    "ctrl": "ctrl", "control": "ctrl", "shift": "shift", "alt": "alt", "mod1": "alt",
    "super": "meta_l", "logo": "meta_l", "mod4": "meta_l", "win": "meta_l",
}


def parse_chord(chord: str) -> list[str]:
    """QKeyCodes of a chord: modifiers in order, then the key."""
    parts = chord.split("+")
    codes = []
    for part in parts[:-1]:
        code = MODIFIERS.get(part.lower())
        if code is None:
            raise SystemExit(f"unknown modifier {part!r}")
        codes.append(code)
    name = parts[-1].lower()
    code = KEYS.get(name) or MODIFIERS.get(name)
    if code is None:
        raise SystemExit(f"unknown key {parts[-1]!r}")
    return codes + [code]


class Port:
    def __init__(self) -> None:
        self.fd = os.open(find_port(), os.O_RDWR | os.O_NOCTTY)
        self.buffer = b""
        self.serial = 0

    def command(self, execute: str, arguments: dict) -> dict:
        self.serial += 1
        ident = f"qkbd-{os.getpid()}-{self.serial}"
        line = json.dumps({"execute": execute, "arguments": arguments, "id": ident})
        os.write(self.fd, line.encode() + b"\n")
        deadline = time.monotonic() + REPLY_TIMEOUT
        while True:
            while b"\n" not in self.buffer:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise SystemExit(f"no QMP reply for {execute} within {REPLY_TIMEOUT:g}s")
                if not select.select([self.fd], [], [], remaining)[0]:
                    continue
                chunk = os.read(self.fd, 4096)
                if not chunk:
                    raise SystemExit("the keyboard port closed: QEMU or the relay is gone")
                self.buffer += chunk
            raw, self.buffer = self.buffer.split(b"\n", 1)
            reply = json.loads(raw)
            # A reply left over from an earlier, interrupted client.
            if reply.get("id") != ident:
                continue
            if "error" in reply:
                raise SystemExit(f"QMP {execute} failed: {reply['error']}")
            return reply

    def key(self, qcode: str, down: bool) -> None:
        event = {"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": qcode}}}
        self.command("input-send-event", {"events": [event]})
        time.sleep(STEP)


def main() -> None:
    chords = [parse_chord(chord) for chord in sys.argv[1:]]
    if not chords:
        raise SystemExit(__doc__)
    port = Port()
    for codes in chords:
        for code in codes:
            port.key(code, True)
        for code in reversed(codes):
            port.key(code, False)


if __name__ == "__main__":
    main()
