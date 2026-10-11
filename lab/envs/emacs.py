"""Emacs in a terminal: `emacs --daemon`, then one `emacsclient -t` frame on a
pty, which the scenario types keys into (`type` actions).

The scenario's "files" supply ~/.emacs.d/init.el (the bindings under test).
The environment installs an observer through the server before the frame
starts: pre-command-hook appends each command's key sequence to KEYLOG, which
the `keylog` check reads (the command loop read that key). The verdict oracle
is the program's reaction (a file a bound command writes), not Emacs's own
lookup. "emacs": {"key_binding": [notations]} records `key-binding` answers in
the logs as evidence only.

docker_args mounts extensions/ at /w/extensions so scenarios can run
extension programs such as /w/extensions/whykey-emacs.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from common import LOG_DIR, LabError, wait_for
from envs.pty import Pty

KEYLOG = "/tmp/emacs-keys"
EXTENSIONS = Path(__file__).resolve().parents[2] / "extensions"

# Runs in the daemon before the frame exists: records each command's keys.
HOOK = r"""(add-hook 'pre-command-hook
  (lambda ()
    (write-region (concat (key-description (this-command-keys-vector)) "\n")
                  nil "/tmp/emacs-keys" t 'silent)))"""


def docker_args() -> list[str]:
    return ["-v", f"{EXTENSIONS}:/w/extensions:ro"]


def _eval(lab, expr: str, check: bool = True, timeout: float = 15.0):
    return lab.run(["emacsclient", "--eval", expr], timeout=timeout, check=check)


def _terminal_frame(lab) -> bool:
    out = _eval(lab, "(frame-parameter (selected-frame) 'tty)", check=False).stdout.strip()
    return out not in (b"", b"nil")


def start(lab) -> None:
    with open(os.path.join(LOG_DIR, "emacs-daemon.log"), "wb") as log:
        completed = subprocess.run(
            ["emacs", "--daemon"],
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
            env=lab.env,
            cwd="/root",
            timeout=90,
        )
    if completed.returncode != 0:
        raise LabError(f"emacs --daemon exited {completed.returncode}; see emacs-daemon.log")
    lab.steps.append("emacs --daemon")
    wait_for(lambda: lab.ok(["emacsclient", "--eval", "t"]), 30.0, "the Emacs server")
    _eval(lab, HOOK)
    env = dict(lab.env)
    env["TERM"] = "xterm-256color"
    lab.pty = Pty(["emacsclient", "-t"], env, "/root")
    lab.steps.append("pty: emacsclient -t")
    wait_for(lambda: _terminal_frame(lab), 30.0, "an Emacs terminal frame")
    lab.pty.wait_quiet(quiet=0.6)


def stop(lab) -> None:
    if lab.pty is not None:
        lab.pty.close()
    lab.ok(["emacsclient", "--eval", "(kill-emacs)"])


def _keylog(lab, spec: dict) -> tuple[bool, str]:
    wanted = spec["keylog"]
    try:
        with open(KEYLOG, encoding="utf-8", errors="replace") as handle:
            lines = handle.read().splitlines()
    except FileNotFoundError:
        lines = []
    found = wanted in lines
    return found, f"{KEYLOG} {'has' if found else 'lacks'} {wanted!r} (read {lines or 'nothing'})"


CHECKS = {"keylog": _keylog}


def logs(lab) -> dict[str, str]:
    out: dict[str, str] = {}
    if lab.pty is not None:
        out["emacs pty transcript"] = lab.pty.text()[-3000:]
    for notation in lab.scenario.get("emacs", {}).get("key_binding", []):
        expr = f'(key-binding (kbd "{notation}"))'
        try:
            answer = _eval(lab, expr, check=False, timeout=10.0).stdout.decode("utf-8", "replace").strip()
        except subprocess.TimeoutExpired:
            answer = "timed out"
        out.setdefault("emacs key-binding", "")
        out["emacs key-binding"] += f"{notation} -> {answer}\n"
    return out
