#!/usr/bin/env python3
"""Expose the sxhkd configuration used alongside bspwm."""

import os
import pathlib
import sys


def parse(text):
    return [
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]


def main():
    if "--test" in sys.argv:
        assert parse("super + Return\n# comment\n") == ["super + Return"]
        return 0
    path = os.environ.get("WHYKEY_SXHKD_CONFIG")
    if not path:
        print("WHYKEY_SXHKD_CONFIG is not set", file=sys.stderr)
        return 2
    try:
        sys.stdout.write(pathlib.Path(path).read_text())
    except OSError as error:
        print(error, file=sys.stderr)
        return 1
    return 0


# Fixture string used by --test: super + Return
if __name__ == "__main__":
    raise SystemExit(main())
