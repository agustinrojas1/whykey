"""Helpers shared by lab/driver.py and the environment plugins in lab/envs.

Stdlib only: lab/run.py imports the plugins on the host.
"""

from __future__ import annotations

import codecs
import time

LOG_DIR = "/tmp/lab-logs"
RUNTIME_DIR = "/tmp/rt"
X_DISPLAY = ":5"
# Title of whykey's own probe window (`whykey listen --window`).
WHYKEY_WINDOW_TITLE = "whykey: press a shortcut here (Esc exits)"
PROBE_TITLE = "whykey-probe"
INPUT_SETTLE = 0.3
VM_INPUT_SETTLE = 1.0


class LabError(Exception):
    pass


def decode_escapes(text: str) -> bytes:
    """Turn JSON-friendly escapes ("\\x14", "\\e", "\\r") into raw bytes."""
    text = text.replace("\\e", "\\x1b")
    return codecs.escape_decode(text.encode("utf-8"))[0]


def wait_for(predicate, timeout: float, what: str, interval: float = 0.05):
    deadline = time.monotonic() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise LabError(f"timed out after {timeout:g}s waiting for {what}")
        time.sleep(interval)
