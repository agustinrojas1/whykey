"""pty.command started on a pseudo-terminal (TERM from pty.term, default
xterm-256color; pty.env; waits pty.settle seconds, default 0.6, of quiet
output); `type` actions are keystrokes into it and whykey "run": "pty" types
its command line there.
"""

from __future__ import annotations

import fcntl
import os
import pty
import re
import select
import shlex
import struct
import termios
import threading
import time

from common import decode_escapes


class Pty:
    """A program running on a pseudo-terminal, typed into like a keyboard.

    The master side answers the two queries shells block on, like any real
    terminal: Primary Device Attributes (fish waits up to 10s for it) and the
    cursor position report. Every other query goes unanswered, which a
    terminal without that feature also does.
    """

    REPLIES = (
        (re.compile(rb"\x1b\[0?c"), b"\x1b[?62;22c"),
        (re.compile(rb"\x1b\[6n"), b"\x1b[1;1R"),
    )

    def __init__(self, argv: list[str], env: dict[str, str], cwd: str):
        pid, fd = pty.fork()
        if pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
                os.chdir(cwd)
                os.execvpe(argv[0], argv, env)
            finally:
                os._exit(127)
        self.pid = pid
        self.fd = fd
        self.transcript = bytearray()
        self.last_output = time.monotonic()
        self.lock = threading.Lock()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        pending = b""
        while True:
            try:
                ready, _, _ = select.select([self.fd], [], [], 0.5)
                if not ready:
                    continue
                data = os.read(self.fd, 4096)
            except OSError:
                return
            if not data:
                return
            # Keep a short tail so a query split across reads is still seen.
            window = pending + data
            for query, reply in self.REPLIES:
                for _ in query.finditer(window):
                    os.write(self.fd, reply)
            escape = window.rfind(b"\x1b")
            tail = window[escape:] if escape != -1 else b""
            pending = tail if len(tail) < 4 and not tail.endswith((b"c", b"n")) else b""
            with self.lock:
                self.transcript.extend(data)
                self.last_output = time.monotonic()

    def wait_quiet(self, quiet: float = 0.4, timeout: float = 8.0) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                idle = time.monotonic() - self.last_output
            if idle >= quiet:
                return
            time.sleep(0.05)

    def type(self, data: bytes) -> None:
        os.write(self.fd, data)

    def text(self) -> str:
        with self.lock:
            return self.transcript.decode("utf-8", "replace")

    def close(self) -> None:
        try:
            os.kill(self.pid, 9)
        except ProcessLookupError:
            pass


def start(lab) -> None:
    spec = lab.scenario["pty"]
    env = dict(lab.env)
    env.update(TERM=spec.get("term", "xterm-256color"))
    env.update(spec.get("env", {}))
    lab.pty = Pty(spec["command"], env, "/root")
    lab.steps.append(f"pty: {shlex.join(spec['command'])}")
    lab.pty.wait_quiet(quiet=float(spec.get("settle", 0.6)))


def stop(lab) -> None:
    if lab.pty is not None:
        lab.pty.close()


def inject(lab, keys: list[str]) -> None:
    """Keys are bytes here, with the `type` action's escapes."""
    lab.pty.wait_quiet()
    for key in keys:
        lab.pty.type(decode_escapes(key))


def logs(lab) -> dict[str, str]:
    return {"pty transcript": lab.pty.text()[-3000:]} if lab.pty is not None else {}
