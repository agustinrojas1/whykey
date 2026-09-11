#!/usr/bin/env python3
"""Expose the sxhkd configuration used alongside bspwm."""

import os
import pathlib
import sys


def main():
    if "--test" in sys.argv:
        assert "super + Return" in pathlib.Path(__file__).read_text()
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
