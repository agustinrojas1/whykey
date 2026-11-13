#!/usr/bin/env python3
"""Dump the built-in default key bindings of the programs whykey embeds.

whykey carries tables of upstream defaults (kitty, foot, Ghostty, Fcitx5,
bash Readline). Nothing in upstream tells whykey when they change, so each
table is checked against a fixture dumped from the real binary:

    python3 tools/dump_defaults.py list
    python3 tools/dump_defaults.py dump [--source docker|host] [--image-prefix P]
                                        [--check] [PROGRAM...]

`dump` runs the program's own listing command inside its lab image (or on the
host with `--source host`, for a program the lab image carries at another
version), normalises the output to `key<TAB>action` lines and writes
`lab/defaults/<program>-<version>.txt` with a header recording the version,
the source and the command. `--check` writes nothing: it exits 1 when a
fixture is missing or differs, which is how a refresh shows drift as a diff.

The Rust unit tests (`cargo test defaults_fixtures`) compare the embedded
tables with these files; they do not need Docker. Only refreshing a fixture
does.

Programs that cannot dump their defaults (Alacritty, Konsole, tmux, Hyprland)
are listed under `NOT_DUMPABLE` with the reason, which is repeated in the doc
comment of the table concerned.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "lab" / "defaults"
DEFAULT_PREFIX = "whykey-lab-"

NOT_DUMPABLE = {
    "alacritty": "no command lists the built-in bindings (`alacritty migrate` and "
    "`--print-events` do not); they are compiled into bindings.rs",
    "konsole": "the default shortcuts are set in C++ (`setDefaultShortcut`), the "
    "binary needs a display to start and has no listing command",
    "tmux": "the live `list-keys` is what whykey reads; no embedded table exists",
    "hyprland": "ships no default binds; whykey embeds no Hyprland table",
}

KITTY_SCRIPT = """\
from kitty.options.definition import definition
for m in definition.iter_all_maps():
    if getattr(m, 'add_to_default', True):
        print(m.key + '\\t' + m.action_def)
"""


@dataclass
class Program:
    name: str
    image: str  # lab image group
    command: str  # shell command printing the raw listing
    version: str  # shell command printing the version line
    version_re: str  # regex with one group over the version output
    parse: Callable[[str], list[str]]
    stdin: str | None = None
    # A host copy is acceptable for programs whose lab image carries another
    # version than the one the table was transcribed for.
    host_ok: bool = True


def parse_readline(text: str) -> list[str]:
    """`"\\C-g": abort` lines of `bind -p`; a printable key inserts itself, so
    `self-insert` rows are left out."""
    rows = []
    for line in text.splitlines():
        m = re.fullmatch(r'"(.*)": (\S+)', line)
        if not m or m.group(2) == "self-insert":
            continue
        rows.append(f"{m.group(1)}\t{m.group(2)}")
    return rows


def parse_tsv(text: str) -> list[str]:
    return [line for line in text.splitlines() if "\t" in line]


def parse_ghostty(text: str) -> list[str]:
    """`keybind = <trigger>=<action>`; the trigger may itself end in `=`
    (`ctrl+==increase_font_size:1`), so the action is the part that starts
    with a letter after the separating `=`."""
    rows = []
    for line in text.splitlines():
        m = re.fullmatch(r"keybind = (.+?)=([a-z_]\S*)", line)
        if m:
            rows.append(f"{m.group(1)}\t{m.group(2)}")
    return rows


def parse_foot(text: str) -> list[str]:
    """The `[key-bindings]` block of the packaged foot.ini lists every default
    as a commented `# action=Key Key` line (`none` for an unbound action)."""
    rows = []
    section = None
    for line in text.splitlines():
        header = re.fullmatch(r"\[([a-z-]+)\]", line.strip())
        if header:
            section = header.group(1)
            continue
        m = re.fullmatch(r"# ([a-z0-9-]+)=(.*)", line)
        if section != "key-bindings" or not m:
            continue
        keys = m.group(2).split(" #")[0].split()
        if keys and keys[-1] != "none" and not m.group(2).startswith("["):
            rows.append(f"{m.group(1)}\t{' '.join(keys)}")
    return rows


PROGRAMS: dict[str, Program] = {
    "readline-emacs": Program(
        "readline-emacs",
        "base",
        "env -i HOME=/tmp/e TERM=xterm-256color INPUTRC=/dev/null bash --norc --noprofile -i -c 'bind -p' 2>/dev/null",
        "bash --version | head -1",
        r"version (\S+?)\(",
        parse_readline,
    ),
    "readline-vi-insert": Program(
        "readline-vi-insert",
        "base",
        "env -i HOME=/tmp/e TERM=xterm-256color INPUTRC=/dev/null bash --norc --noprofile -i -c 'set -o vi; bind -p' 2>/dev/null",
        "bash --version | head -1",
        r"version (\S+?)\(",
        parse_readline,
    ),
    "kitty": Program(
        "kitty",
        "tty",
        "kitty +runpy 'import sys; exec(sys.stdin.read())'",
        "kitty --version",
        r"kitty (\S+)",
        parse_tsv,
        stdin=KITTY_SCRIPT,
        host_ok=True,
    ),
    "foot": Program(
        "foot",
        "base",
        "cat /etc/xdg/foot/foot.ini",
        "foot --version",
        r"foot version: (\S+)",
        parse_foot,
    ),
    "fcitx5": Program(
        "fcitx5",
        "wayland",
        "dbus-run-session -- python3 -c 'import sys; exec(sys.stdin.read())'",
        "fcitx5 --version",
        r"(\d+\.\d+\.\d+)",
        parse_tsv,
        stdin=(ROOT / "tools" / "dump_defaults" / "fcitx5.py").read_text(),
        host_ok=False,
    ),
    "ghostty": Program(
        "ghostty",
        "tty",
        "ghostty +list-keybinds --default --plain",
        "ghostty --version | head -1",
        r"Ghostty (\d+\.\d+\.\d+)",
        parse_ghostty,
    ),
}


def run(cmd: list[str], stdin: str | None = None) -> str:
    out = subprocess.run(cmd, input=stdin, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"{' '.join(cmd)[:120]}: exit {out.returncode}\n{out.stderr[-2000:]}")
    return out.stdout


def execute(prog: Program, source: str, prefix: str, shell: str) -> str:
    if source == "host":
        return run(["bash", "-c", shell], prog.stdin if shell == prog.command else None)
    image = f"{prefix}{prog.image}:latest"
    flags = ["-i"] if shell == prog.command and prog.stdin else []
    return run(
        ["docker", "run", "--rm", "--network", "none", *flags, image, "bash", "-c", shell],
        prog.stdin if shell == prog.command else None,
    )


def dump_one(prog: Program, source: str, prefix: str) -> tuple[str, str]:
    version_out = execute(prog, source, prefix, prog.version)
    m = re.search(prog.version_re, version_out)
    if not m:
        sys.exit(f"{prog.name}: cannot read a version from {version_out!r}")
    version = m.group(1)
    rows = parse(prog, execute(prog, source, prefix, prog.command))
    if not rows:
        sys.exit(f"{prog.name}: the listing produced no rows")
    where = "the host" if source == "host" else f"docker {prefix}{prog.image}"
    header = [
        "# whykey default-bindings fixture; regenerate with tools/dump_defaults.py",
        f"# program: {prog.name}",
        f"# version: {version}",
        f"# source: {where}",
        f"# command: {prog.command}",
    ]
    return version, "\n".join(header + rows) + "\n"


def parse(prog: Program, raw: str) -> list[str]:
    return prog.parse(raw)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    d = sub.add_parser("dump")
    d.add_argument("--source", choices=["docker", "host"], default="docker")
    d.add_argument("--image-prefix", default=DEFAULT_PREFIX)
    d.add_argument("--check", action="store_true")
    d.add_argument("programs", nargs="*")
    args = ap.parse_args()
    if args.cmd == "list":
        for p in PROGRAMS.values():
            print(f"{p.name:20} image={p.image}")
        for name, why in NOT_DUMPABLE.items():
            print(f"{name:20} not dumpable: {why}")
        return 0
    names = args.programs or list(PROGRAMS)
    status = 0
    OUT.mkdir(parents=True, exist_ok=True)
    for name in names:
        prog = PROGRAMS.get(name) or sys.exit(f"unknown program {name}")
        version, text = dump_one(prog, args.source, args.image_prefix)
        path = OUT / f"{name}-{version}.txt"
        if args.check:
            if not path.exists() or path.read_text() != text:
                print(f"DRIFT {path.relative_to(ROOT)}")
                status = 1
            else:
                print(f"ok    {path.relative_to(ROOT)}")
            continue
        for stale in OUT.glob(f"{name}-*.txt"):
            if stale != path:
                stale.unlink()
        path.write_text(text)
        print(f"wrote {path.relative_to(ROOT)} ({text.count(chr(10)) - 5} rows)")
    return status


if __name__ == "__main__":
    sys.exit(main())
