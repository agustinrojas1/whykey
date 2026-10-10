#!/usr/bin/env python3
"""Terminal input probe: log every byte the terminal delivers to its pty.

Runs inside a terminal emulator (foot) in the lab. It switches the tty to raw
mode and asks the terminal for the kitty keyboard protocol with "report all
keys as escape codes", so every key press, modifiers included, arrives as a
CSI sequence. The log is the ground truth for "did the compositor deliver this
key to the focused client".
"""

import os
import sys
import termios
import tty


def main() -> None:
    log_path = sys.argv[1]
    fd = sys.stdin.fileno()
    tty.setraw(fd)
    # Push kitty keyboard flags: disambiguate (1) | report events (2) |
    # report all keys as escape codes (8).
    os.write(sys.stdout.fileno(), b"\x1b[>11u")
    with open(log_path, "ab", buffering=0) as log:
        log.write(b"")
        with open(log_path + ".ready", "w") as ready:
            ready.write("ready\n")
        while True:
            data = os.read(fd, 4096)
            if not data:
                break
            log.write(data)


if __name__ == "__main__":
    try:
        main()
    finally:
        try:
            termios.tcflush(sys.stdin.fileno(), termios.TCIFLUSH)
        except termios.error:
            pass
