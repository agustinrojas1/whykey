#!/usr/bin/env python3
"""Read herbstluftwm's literal key binding list for whykey."""

import subprocess
import sys


def parse(lines):
    records = []
    for line in lines.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            records.append(line)
    return records


def main():
    if "--test" in sys.argv:
        assert parse("Mod4-Return spawn foot\n# comment\n") == ["Mod4-Return spawn foot"]
        return 0
    result = subprocess.run(
        ["herbstclient", "list_keybinds"],
        check=False,
        capture_output=True,
        text=True,
    )
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
