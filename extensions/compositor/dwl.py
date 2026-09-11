#!/usr/bin/env python3
"""Read a user-maintained static dump for a dwl config.h."""

import os
import pathlib
import sys


def main():
    if "--test" in sys.argv:
        assert "super+return spawn foot".split()[0] == "super+return"
        return 0
    path = os.environ.get("WHYKEY_DWL_BINDINGS")
    if not path:
        print("WHYKEY_DWL_BINDINGS is not set; dwl has no generic IPC", file=sys.stderr)
        return 2
    try:
        sys.stdout.write(pathlib.Path(path).read_text())
    except OSError as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
