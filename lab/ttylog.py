#!/usr/bin/env python3
"""Record the exact bytes a terminal emulator writes to its pty.

    python3 ttylog.py LOG [--flags N]

Runs in the foreground of a terminal (lab/envs/terminals.py starts it through
lab/termagent.py). It switches the tty to raw mode, so line discipline and
signals do not touch the input, optionally pushes Kitty keyboard protocol
flags N (`CSI > N u`, as an application enabling the protocol does), appends
every byte it reads to LOG, and writes LOG.ready once it reads. It exits when
LOG.stop appears, popping the flags it pushed and restoring the tty.
"""

from __future__ import annotations

import argparse
import os
import select
import sys
import termios
import tty


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("log")
    parser.add_argument("--flags", type=int, default=0)
    options = parser.parse_args()
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    tty.setraw(fd)
    try:
        if options.flags:
            os.write(sys.stdout.fileno(), f"\x1b[>{options.flags}u".encode())
        with open(options.log, "ab", buffering=0) as log:
            with open(options.log + ".ready", "w") as ready:
                ready.write("ready\n")
            while not os.path.exists(options.log + ".stop"):
                readable, _, _ = select.select([fd], [], [], 0.05)
                if readable:
                    data = os.read(fd, 4096)
                    if not data:
                        break
                    log.write(data)
    finally:
        if options.flags:
            os.write(sys.stdout.fileno(), b"\x1b[<u")
        termios.tcsetattr(fd, termios.TCSAFLUSH, saved)


if __name__ == "__main__":
    main()
