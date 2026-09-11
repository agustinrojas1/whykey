#!/usr/bin/env python3
"""Measure the E1 inspection baseline without changing production behavior.

The fixture run puts deterministic command wrappers first on PATH. Their log
counts external launches, while wall time and child CPU/memory come from the
same subprocess boundary. The live run is opt-in and never required by CI.
"""

from __future__ import annotations

import argparse
import os
import resource
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ITERATIONS = 20
WRAPPED_COMMANDS = (
    "hyprctl",
    "swaymsg",
    "i3-msg",
    "gsettings",
    "xfconf-query",
    "pgrep",
    "ps",
)


def run(command: list[str], env: dict[str, str], iterations: int) -> dict[str, float | int | str]:
    samples: list[float] = []
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    for _ in range(iterations):
        start = time.perf_counter()
        subprocess.run(
            command,
            cwd=ROOT,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=15,
        )
        samples.append((time.perf_counter() - start) * 1000)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {
        "iterations": iterations,
        "median_ms": statistics.median(samples),
        "p95_ms": sorted(samples)[max(0, int(len(samples) * 0.95) - 1)],
        "child_user_ms": (after.ru_utime - before.ru_utime) * 1000,
        "child_sys_ms": (after.ru_stime - before.ru_stime) * 1000,
        "child_maxrss_kib": max(0, after.ru_maxrss - before.ru_maxrss),
    }


def fixture_environment(directory: Path) -> tuple[dict[str, str], Path]:
    bin_dir = directory / "bin"
    bin_dir.mkdir()
    log = directory / "external.log"
    script = bin_dir / "whykey-command-wrapper"
    script.write_text(
        "#!/bin/sh\n"
        "printf '%s %s\\n' \"$(basename \"$0\")\" \"$*\" >> \"$WHYKEY_PERF_LOG\"\n"
        "exit 1\n",
        encoding="utf-8",
    )
    script.chmod(0o755)
    for name in WRAPPED_COMMANDS:
        (bin_dir / name).symlink_to(script)
    env = {
        "PATH": f"{bin_dir}:/usr/bin:/bin",
        "HOME": str(directory / "home"),
        "WHYKEY_PERF_LOG": str(log),
        "LC_ALL": "C",
        "HYPRLAND_INSTANCE_SIGNATURE": "fixture",
        "XDG_CURRENT_DESKTOP": "Hyprland",
        "XDG_SESSION_TYPE": "wayland",
    }
    (directory / "home").mkdir()
    return env, log


def format_result(result: dict[str, float | int | str]) -> str:
    return (
        f"{result['median_ms']:.2f} ms median / {result['p95_ms']:.2f} ms p95; "
        f"child CPU {result['child_user_ms'] + result['child_sys_ms']:.2f} ms; "
        f"maxrss delta {result['child_maxrss_kib']} KiB"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--iterations", type=int, default=DEFAULT_ITERATIONS)
    parser.add_argument("--binary", default=str(ROOT / "target/debug/whykey"))
    parser.add_argument("--live", action="store_true", help="also measure the current session")
    args = parser.parse_args()
    if args.iterations < 3:
        parser.error("--iterations must be at least 3")

    binary = Path(args.binary)
    if not binary.is_file():
        raise SystemExit(f"missing {binary}; run cargo build --locked first")

    with tempfile.TemporaryDirectory(prefix="whykey-perf-e1-") as temporary:
        fixture_env, log = fixture_environment(Path(temporary))
        fixture = run([str(binary), "inspect", "ctrl+c"], fixture_env, args.iterations)
        launches = log.read_text(encoding="utf-8").splitlines() if log.exists() else []
        ipc = [line for line in launches if line.split(" ", 1)[0] in {"hyprctl", "swaymsg", "i3-msg"}]
        print("fixture inspection:", format_result(fixture))
        print(f"fixture external launches: {len(launches)} total, {len(ipc)} compositor IPC-like")
        print("fixture repeated parsing: one process snapshot per inspection; parser reads are not attributed to Rust computation")

    if args.live:
        live_env = os.environ.copy()
        live = run([str(binary), "inspect", "ctrl+c"], live_env, args.iterations)
        print("live-system inspection:", format_result(live))
    else:
        print("live-system inspection: skipped (pass --live explicitly)")
    print(f"binary size: {binary.stat().st_size} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
