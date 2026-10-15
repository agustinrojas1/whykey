#!/usr/bin/env python3
"""Render real whykey output as the README's terminal screenshot (SVG).

The picture is never drawn by hand: this script runs the binary with
`--color always`, reads the ANSI styling it prints, and lays the text out in
a terminal-window SVG.

    tools/render_demo.py --bin target/release/whykey \
        --output docs/assets/whykey-demo.svg super+return

Everything after the options is the whykey command line. COLUMNS is pinned so
the layout does not depend on the terminal that runs the script.
"""

import argparse
import html
import os
import re
import subprocess
import sys

CELL_WIDTH = 8.43  # px per column at font-size 14px (monospace)
LINE_HEIGHT = 20
FONT_SIZE = 14
TOP = 64  # y of the first text line
LEFT = 24
COLUMNS = 80

# SGR code -> CSS class understood by the stylesheet below.
COLORS = {"31": "r", "32": "p", "33": "y", "36": "h"}
SGR = re.compile(r"\x1b\[([0-9;]*)m")


def spans(line):
    """Split one line into (text, classes) runs from its SGR sequences."""
    runs, bold, dim, color, position = [], False, False, None, 0

    def classes():
        names = [name for name, on in (("b", bold), ("d", dim)) if on]
        return " ".join(names + ([color] if color else []))

    for match in SGR.finditer(line):
        if match.start() > position:
            runs.append((line[position : match.start()], classes()))
        position = match.end()
        for code in match.group(1).split(";") or ["0"]:
            if code in ("", "0"):
                bold, dim, color = False, False, None
            elif code == "1":
                bold = True
            elif code == "2":
                dim = True
            elif code in COLORS:
                color = COLORS[code]
    if position < len(line):
        runs.append((line[position:], classes()))
    return runs


def render(command, output):
    lines = output.rstrip("\n").split("\n")
    width = max(len(SGR.sub("", line)) for line in lines + ["$ " + command])
    page_width = max(820, int(LEFT * 2 + width * CELL_WIDTH))
    height = TOP + LINE_HEIGHT * (len(lines) + 2) + 14
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{page_width}" height="{height}" '
        f'viewBox="0 0 {page_width} {height}" role="img" aria-labelledby="title desc">',
        f"  <title id=\"title\">whykey {html.escape(command)}</title>",
        f'  <desc id="desc">The output of whykey {html.escape(command)}: the verdict, '
        "the route through the seven layers, and the evidence behind it.</desc>",
        "  <style>",
        '    .t { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", '
        f'"DejaVu Sans Mono", monospace; font-size: {FONT_SIZE}px; fill: #e6edf3; white-space: pre; }}',
        "    .b { font-weight: 700; }",
        "    .h { fill: #56d4dd; }",
        "    .d { fill: #8b949e; }",
        "    .p { fill: #7ee787; }",
        "    .y { fill: #e3b341; }",
        "    .r { fill: #ff7b72; }",
        "    .k { fill: #ffa657; }",
        "  </style>",
        f'  <rect x="0.5" y="0.5" width="{page_width - 1}" height="{height - 1}" rx="10" '
        'fill="#0d1117" stroke="#30363d"/>',
        f'  <rect x="0.5" y="0.5" width="{page_width - 1}" height="34" rx="10" fill="#161b22"/>',
        f'  <rect x="0.5" y="24" width="{page_width - 1}" height="11" fill="#161b22"/>',
        f'  <line x1="0" y1="34.5" x2="{page_width}" y2="34.5" stroke="#30363d"/>',
        '  <circle cx="22" cy="17.5" r="6" fill="#ff5f57"/>',
        '  <circle cx="42" cy="17.5" r="6" fill="#febc2e"/>',
        '  <circle cx="62" cy="17.5" r="6" fill="#28c840"/>',
        f'  <text x="{page_width // 2}" y="22" text-anchor="middle" class="t d" '
        'style="font-size:12px">whykey</text>',
        '  <text class="t" xml:space="preserve">',
        f'    <tspan x="{LEFT}" y="{TOP}"><tspan class="p">$</tspan> whykey '
        f'<tspan class="k">{html.escape(command)}</tspan></tspan>',
    ]
    y = TOP + LINE_HEIGHT + 6
    for line in lines:
        body = "".join(
            f'<tspan class="{cls}">{html.escape(text)}</tspan>' if cls else html.escape(text)
            for text, cls in spans(line)
        )
        out.append(f'    <tspan x="{LEFT}" y="{y}">{body}</tspan>')
        y += LINE_HEIGHT
    out.append("  </text>")
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--bin", default="target/release/whykey", help="whykey binary")
    parser.add_argument("--output", default="docs/assets/whykey-demo.svg")
    parser.add_argument("--stdin", help="render this saved output instead of running whykey")
    parser.add_argument("command", nargs="+", help="whykey arguments, e.g. super+return")
    args = parser.parse_args()
    shown = " ".join(args.command)
    if args.stdin:
        with open(args.stdin, encoding="utf-8") as handle:
            output = handle.read()
    else:
        env = dict(os.environ, COLUMNS=str(COLUMNS), LC_ALL="C.UTF-8")
        env.pop("NO_COLOR", None)
        result = subprocess.run(
            [args.bin, "--color", "always", *args.command],
            capture_output=True, text=True, env=env, check=False,
        )
        output = result.stdout
        if not output.strip():
            sys.exit(f"whykey printed nothing (exit {result.returncode}): {result.stderr}")
    with open(args.output, "w", encoding="utf-8") as handle:
        handle.write(render(shown, output))
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
