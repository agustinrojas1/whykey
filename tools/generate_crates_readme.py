#!/usr/bin/env python3
"""Generate or check README.crates.md, the README that crates.io shows.

crates.io rewrites relative <img src> paths to the GitHub repository but
leaves <source srcset> paths relative, so the dark variant of a <picture>
is a broken image there whenever the viewer's system is in dark mode. This
copy of README.md replaces each <picture> with its <img> fallback, the light
variant, which has an opaque background and reads on either crates.io theme.
"""

from __future__ import annotations

import argparse
import pathlib
import re


ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "README.md"
OUTPUT = ROOT / "README.crates.md"

PICTURE = re.compile(r"[ \t]*<picture>.*?(<img\b[^>]*>).*?</picture>", re.DOTALL)
HEADER = (
    "<!-- Generated from README.md by tools/generate_crates_readme.py. "
    "Do not edit it by hand; CI checks that it is current. -->\n\n"
)


def render() -> str:
    source = SOURCE.read_text(encoding="utf-8")

    def light_only(match: re.Match[str]) -> str:
        indent = match.group(0)[: len(match.group(0)) - len(match.group(0).lstrip())]
        return indent + match.group(1)

    return HEADER + PICTURE.sub(light_only, source)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    expected = render()
    if args.check:
        current = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        if current != expected:
            print(f"{OUTPUT.name} is stale; run tools/generate_crates_readme.py")
            return 1
        return 0
    OUTPUT.write_text(expected, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
