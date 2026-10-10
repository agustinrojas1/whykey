#!/usr/bin/env python3
"""Report whether another X11 client holds a passive grab on a key.

Speaks the X11 wire protocol directly (stdlib only): GrabKey on the root
window fails with BadAccess exactly when another client (window manager or
hotkey daemon) already grabbed that modifiers+keycode combination. The lab
uses it as a readiness gate: a daemon is ready once its binding is grabbed.

    python3 xgrab.py MODMASK KEYCODE   # exit 0 = grabbed by another client, 1 = free
"""

from __future__ import annotations

import os
import socket
import struct
import sys

BAD_ACCESS = 10


def recv_exact(sock: socket.socket, size: int) -> bytes:
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise SystemExit("X server closed the connection")
        data += chunk
    return data


def main() -> int:
    modifiers, keycode = int(sys.argv[1]), int(sys.argv[2])
    display = os.environ.get("DISPLAY", ":0").split(":", 1)[1].split(".", 1)[0]
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(f"/tmp/.X11-unix/X{display}")
    # Little-endian, protocol 11.0, no authorization.
    sock.sendall(struct.pack("<BxHHHHxx", ord("l"), 11, 0, 0, 0))
    header = recv_exact(sock, 8)
    if header[0] != 1:
        raise SystemExit(f"X11 connection refused (status {header[0]})")
    (extra_words,) = struct.unpack_from("<H", header, 6)
    setup = recv_exact(sock, extra_words * 4)
    vendor_length, = struct.unpack_from("<H", setup, 16)
    formats = setup[21]
    screens_offset = 32 + vendor_length + (-vendor_length % 4) + 8 * formats
    (root,) = struct.unpack_from("<I", setup, screens_offset)
    # GrabKey(owner_events=False, root, modifiers, keycode, async, async)
    grab = struct.pack("<BBHIHBBBxxx", 33, 0, 4, root, modifiers, keycode, 1, 1)
    # GetInputFocus: its reply orders after any GrabKey error.
    sync = struct.pack("<BxH", 43, 1)
    sock.sendall(grab + sync)
    while True:
        message = recv_exact(sock, 32)
        if message[0] == 0:
            return 0 if message[1] == BAD_ACCESS else 2
        if message[0] == 1:
            (length,) = struct.unpack_from("<I", message, 4)
            recv_exact(sock, length * 4)
            return 1


if __name__ == "__main__":
    sys.exit(main())
