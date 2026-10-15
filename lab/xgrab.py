#!/usr/bin/env python3
"""Report whether another X11 client holds a passive grab on a key.

Speaks the X11 wire protocol directly (stdlib only): GrabKey on a window
fails with BadAccess exactly when another client (window manager or hotkey
daemon) already grabbed that modifiers+keycode combination there.

    python3 xgrab.py MODMASK KEYCODE          # exit 0 = grabbed on the root window
    python3 xgrab.py --path KEYSYM MODMASK    # exit 0 = grabbed on the root window or
                                              # on the windows from the root down to the
                                              # focused window, by core GrabKey or by an
                                              # XInput2 passive grab on device 3

The default mode is the readiness gate the x11 environment uses: a daemon is
ready once its binding is grabbed on the root. Cinnamon's muffin keeps some
bindings on the frame around the focused window (XInput2 grabs, invisible to
core GrabKey), so --path asks both protocols about every window a key press
passes through, in the order the X server searches passive grabs. Exit 2 is
an error, 1 means free.
"""

from __future__ import annotations

import os
import socket
import struct
import sys

BAD_ACCESS = 10
GRAB_KEY, UNGRAB_KEY, GET_INPUT_FOCUS, QUERY_TREE = 33, 34, 43, 15
GET_KEYBOARD_MAPPING, QUERY_EXTENSION = 101, 98
XI_QUERY_VERSION, XI_PASSIVE_GRAB, XI_PASSIVE_UNGRAB = 47, 54, 55
XI_DEVICE = 3  # the virtual core keyboard, which muffin grabs on
POINTER_ROOT = 1


def recv_exact(sock: socket.socket, size: int) -> bytes:
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise SystemExit("X server closed the connection")
        data += chunk
    return data


class Display:
    def __init__(self) -> None:
        display = os.environ.get("DISPLAY", ":0")
        number = display.split(":", 1)[1].split(".", 1)[0]
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(f"/tmp/.X11-unix/X{number}")
        # Little-endian, protocol 11.0, no authorization.
        self.sock.sendall(struct.pack("<BxHHHHxx", ord("l"), 11, 0, 0, 0))
        header = recv_exact(self.sock, 8)
        if header[0] != 1:
            raise SystemExit(f"X11 connection refused (status {header[0]})")
        setup = recv_exact(self.sock, struct.unpack_from("<H", header, 6)[0] * 4)
        vendor_length = struct.unpack_from("<H", setup, 16)[0]
        screen = 32 + vendor_length + (-vendor_length % 4) + 8 * setup[21]
        self.root = struct.unpack_from("<I", setup, screen)[0]
        self.min_keycode, self.max_keycode = setup[26], setup[27]
        self.sequence = 0
        self._xi: int | None = None

    def call(self, request: bytes) -> tuple[str, bytes]:
        """Send one request and a GetInputFocus sync. Returns ("reply", bytes),
        ("error", bytes) for the request, or ("none", b"") when it succeeded
        without a reply. Errors and replies arrive before the sync's reply."""
        self.sequence = (self.sequence + 1) & 0xFFFF
        request_sequence = self.sequence
        self.sequence = (self.sequence + 1) & 0xFFFF
        sync = self.sequence
        self.sock.sendall(request + struct.pack("<BxH", GET_INPUT_FOCUS, 1))
        outcome: tuple[str, bytes] = ("none", b"")
        while True:
            message = recv_exact(self.sock, 32)
            kind = message[0]
            if kind == 0:
                if struct.unpack_from("<H", message, 2)[0] == request_sequence:
                    outcome = ("error", message)
            elif kind == 1:
                seq, length = struct.unpack_from("<HI", message, 2)
                body = message + recv_exact(self.sock, length * 4)
                if seq == request_sequence:
                    outcome = ("reply", body)
                if seq == sync:
                    return outcome
            elif kind & 0x7F == 35:  # GenericEvent with trailing data
                recv_exact(self.sock, struct.unpack_from("<I", message, 4)[0] * 4)

    def focus(self) -> int:
        kind, body = self.call(struct.pack("<BxH", GET_INPUT_FOCUS, 1))
        return struct.unpack_from("<I", body, 8)[0] if kind == "reply" else self.root

    def parent(self, window: int) -> int:
        kind, body = self.call(struct.pack("<BxHI", QUERY_TREE, 2, window))
        if kind != "reply":
            return 0
        return struct.unpack_from("<I", body, 12)[0]

    def path(self) -> list[int]:
        """Windows from the root down to the focused window (the root alone
        when nothing or only the pointer's root has the focus)."""
        focus = self.focus()
        if focus in (0, POINTER_ROOT, self.root):
            return [self.root]
        windows = [focus]
        while windows[-1] != self.root and len(windows) < 256:
            parent = self.parent(windows[-1])
            if parent in (0, windows[-1]):
                break
            windows.append(parent)
        if windows[-1] != self.root:
            windows.append(self.root)
        return list(reversed(windows))

    def keycode(self, keysym: int) -> int | None:
        count = self.max_keycode - self.min_keycode + 1
        kind, body = self.call(struct.pack("<BxHBBH", GET_KEYBOARD_MAPPING, 2, self.min_keycode, count, 0))
        if kind != "reply":
            raise SystemExit("GetKeyboardMapping failed")
        per_keycode = body[1]
        for index in range(count):
            row = struct.unpack_from(f"<{per_keycode}I", body, 32 + index * per_keycode * 4)
            if keysym in row:
                return self.min_keycode + index
        return None

    def xinput(self) -> int | None:
        if self._xi is None:
            name = b"XInputExtension"
            request = struct.pack("<BBHHxx", QUERY_EXTENSION, 0, 2 + (len(name) + 3) // 4, len(name))
            kind, body = self.call(request + name + b"\0" * (-len(name) % 4))
            present = kind == "reply" and body[8] != 0
            self._xi = body[9] if present else -1
            if present:
                kind, _ = self.call(struct.pack("<BBHHH", self._xi, XI_QUERY_VERSION, 2, 2, 0))
                if kind != "reply":
                    self._xi = -1
        return None if self._xi < 0 else self._xi

    def core_grabbed(self, window: int, modmask: int, keycode: int) -> bool:
        grab = struct.pack("<BBHIHBBBxxx", GRAB_KEY, 0, 4, window, modmask, keycode, 1, 1)
        kind, body = self.call(grab)
        if kind == "error":
            return body[1] == BAD_ACCESS
        self.call(struct.pack("<BBHIHxx", UNGRAB_KEY, keycode, 3, window, modmask))
        return False

    def xi_grabbed(self, window: int, modmask: int, keycode: int) -> bool:
        opcode = self.xinput()
        if opcode is None:
            return False
        # xXIPassiveGrabDeviceReq: time, grab_window, cursor, detail, deviceid,
        # num_modifiers, mask_len, grab_type (KEYCODE 1), grab_mode and
        # paired_device_mode (ASYNC 1), owner_events (False), pad. Then the
        # KeyPress|KeyRelease mask and the modifier list.
        mask = (1 << 2) | (1 << 3)
        body = struct.pack("<IIII", 0, window, 0, keycode)
        body += struct.pack("<HHHBBBBH", XI_DEVICE, 1, 1, 1, 1, 1, 0, 0)
        body += struct.pack("<II", mask, modmask)
        kind, reply = self.call(struct.pack("<BBH", opcode, XI_PASSIVE_GRAB, (len(body) + 4) // 4) + body)
        if kind == "error":
            return reply[1] == BAD_ACCESS
        # The reply lists only the modifier combinations that were refused.
        (count,) = struct.unpack_from("<H", reply, 8)
        refused = {
            struct.unpack_from("<I", reply, 32 + index * 8)[0]: reply[32 + index * 8 + 4]
            for index in range(count)
        }
        if modmask not in refused:
            # xXIPassiveUngrabDeviceReq: grab_window, detail, deviceid, num_modifiers, grab_type, pad
            ungrab = struct.pack("<BBHIIHHBxH", opcode, XI_PASSIVE_UNGRAB, 6, window, keycode, XI_DEVICE, 1, 1, 0)
            self.call(ungrab + struct.pack("<I", modmask))
        return refused.get(modmask) == BAD_ACCESS


def path_check(keysym: int, modmask: int) -> int:
    display = Display()
    keycode = display.keycode(keysym)
    if keycode is None:
        print(f"no keycode produces keysym {keysym:#x}", file=sys.stderr)
        return 2
    for window in display.path():
        core = display.core_grabbed(window, modmask, keycode)
        xi2 = display.xi_grabbed(window, modmask, keycode)
        if core or xi2:
            role = "root" if window == display.root else "focused path"
            print(f"keycode {keycode} modmask {modmask:#x} grabbed on {window:#x} ({role}): "
                  f"core={core} xi2={xi2}")
            return 0
    print(f"keycode {keycode} modmask {modmask:#x} not grabbed on {display.path()}")
    return 1


def root_check(modmask: int, keycode: int) -> int:
    display = Display()
    return 0 if display.core_grabbed(display.root, modmask, keycode) else 1


def main() -> int:
    if len(sys.argv) == 4 and sys.argv[1] == "--path":
        return path_check(int(sys.argv[2], 0), int(sys.argv[3], 0))
    if len(sys.argv) == 3:
        return root_check(int(sys.argv[1]), int(sys.argv[2]))
    raise SystemExit(__doc__)


if __name__ == "__main__":
    sys.exit(main())
