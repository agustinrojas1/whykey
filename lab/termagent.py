#!/usr/bin/env python3
"""The program a lab terminal emulator runs: a shell fed through a FIFO.

    python3 termagent.py DIR

lab/envs/terminals.py starts a real terminal emulator with this script as its
command. It creates DIR/commands (a FIFO), writes DIR/ready, and runs each
line written to the FIFO with `sh -c`, one at a time, attached to the
terminal's tty exactly like a command typed at a shell prompt (stdin, stdout
and stderr are the terminal; the process inherits the terminal's
environment: TERM, TERM_PROGRAM, KITTY_WINDOW_ID, ...). Each line is
`ID<TAB>COMMAND`; when the command exits the agent writes its status to
DIR/ID.done. Commands, not keystrokes: injected keys stay what the scenario
types, and nothing but the command reads the tty while it runs.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys


def main() -> None:
    directory = sys.argv[1]
    fifo = os.path.join(directory, "commands")
    if not os.path.exists(fifo):
        os.mkfifo(fifo)
    # Ctrl+C typed at the terminal must not end the agent.
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    # Read-write: the FIFO never reaches EOF between writers.
    with open(os.open(fifo, os.O_RDWR), "r", encoding="utf-8") as commands:
        with open(os.path.join(directory, "ready"), "w") as ready:
            ready.write(f"{os.getpid()}\n")
        for line in commands:
            identifier, _, command = line.rstrip("\n").partition("\t")
            completed = subprocess.run(
                ["sh", "-c", command],
                preexec_fn=lambda: signal.signal(signal.SIGINT, signal.SIG_DFL),
            )
            with open(os.path.join(directory, f"{identifier}.done"), "w") as done:
                done.write(f"{completed.returncode}\n")


if __name__ == "__main__":
    main()
