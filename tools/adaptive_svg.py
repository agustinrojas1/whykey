#!/usr/bin/env python3
"""Combine a light and a dark SVG into one that follows the viewer's theme.

crates.io drops <picture>/<source> from READMEs, so a README image there has
one file for both themes. An SVG can switch by itself: this keeps the light
drawing and adds a `prefers-color-scheme: dark` style that recolors every
fill/stroke to the dark variant's color. CSS rules beat SVG presentation
attributes, so the light colors need no rewriting.

The two files must have the same structure: their colors, in document order,
are paired one to one, and a light color must always map to the same dark
color.

Usage: python3 tools/adaptive_svg.py LIGHT.svg DARK.svg OUT.svg
"""

from __future__ import annotations

import pathlib
import re
import sys

COLOR = re.compile(r'\b(fill|stroke)="(#[0-9a-fA-F]{3,8})"')


def color_map(light: str, dark: str) -> dict[str, str]:
    light_colors = COLOR.findall(light)
    dark_colors = COLOR.findall(dark)
    if [kind for kind, _ in light_colors] != [kind for kind, _ in dark_colors]:
        raise ValueError("light and dark SVGs do not have the same structure")
    mapping: dict[str, str] = {}
    for (_, light_color), (_, dark_color) in zip(light_colors, dark_colors):
        light_color, dark_color = light_color.lower(), dark_color.lower()
        if mapping.setdefault(light_color, dark_color) != dark_color:
            raise ValueError(
                f"{light_color} maps to both {mapping[light_color]} and {dark_color}"
            )
    return mapping


def adaptive(light: str, mapping: dict[str, str]) -> str:
    rules = "".join(
        f'[fill="{source}"]{{fill:{target}}}[stroke="{source}"]{{stroke:{target}}}'
        for source, target in sorted(mapping.items())
        if source != target
    )
    style = f"<style>@media (prefers-color-scheme: dark){{{rules}}}</style>"
    # Attribute selectors match the light file's exact spelling.
    light = COLOR.sub(lambda m: f'{m.group(1)}="{m.group(2).lower()}"', light)
    opening = re.search(r"<svg\b[^>]*>", light)
    if opening is None:
        raise ValueError("no <svg> element")
    return light[: opening.end()] + "\n" + style + light[opening.end() :]


def combine(light: str, dark: str) -> str:
    return adaptive(light, color_map(light, dark))


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    light, dark, out = (pathlib.Path(arg) for arg in sys.argv[1:])
    out.write_text(
        combine(light.read_text(encoding="utf-8"), dark.read_text(encoding="utf-8")),
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
