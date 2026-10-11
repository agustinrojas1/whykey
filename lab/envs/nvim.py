"""Neovim in a terminal: the pty environment (scenario "pty": {"command": [...]}),
plus docker_args mounting extensions/ at /w/extensions for whykey-nvim.

Scenarios log the keys Neovim reads with vim.on_key in their own init.lua (a
"sh" check reads that log), and the program's reaction (a file a mapping
writes) is the fired oracle.
"""

from __future__ import annotations

from pathlib import Path

from envs.pty import inject, logs, start, stop  # noqa: F401  (pty hooks)

EXTENSIONS = Path(__file__).resolve().parents[2] / "extensions"


def docker_args() -> list[str]:
    return ["-v", f"{EXTENSIONS}:/w/extensions:ro"]
