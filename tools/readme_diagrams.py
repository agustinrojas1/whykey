#!/usr/bin/env python3
"""Generate the crates.io stand-ins for the README's two mermaid diagrams.

GitHub renders the README's mermaid blocks in the reader's theme. crates.io
renders them with mermaid's light theme even on its dark theme, where the
sequence diagram's labels disappear. generate_crates_readme.py replaces each
block with one of these SVGs, which follow the viewer's theme by themselves
(see adaptive_svg.py). Keep their text in step with the mermaid sources in
README.md.

Usage: python3 tools/readme_diagrams.py docs/assets
"""

from __future__ import annotations

import sys

from adaptive_svg import combine
from key_path_diagram import FONT, THEMES


def esc(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def svg_open(width: int, height: int, title: str, desc: str, c: dict) -> list[str]:
    return [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" role="img" aria-labelledby="t d" '
        f"font-family='{FONT}'>",
        f'<title id="t">{esc(title)}</title>',
        f'<desc id="d">{esc(desc)}</desc>',
        '<defs><marker id="arr" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" '
        'markerHeight="7" orient="auto-start-reverse">'
        f'<path d="M 0 0 L 10 5 L 0 10 z" fill="{c["arrow"]}"/></marker></defs>',
        f'<rect width="{width}" height="{height}" rx="12" fill="{c["bg"]}"/>',
    ]


EVIDENCE = [
    ("1 · Observe", [("capture the key itself", "title"), ("whykey listen", "muted")]),
    (
        "2 · Ask the running program",
        [
            ("compositor IPC · D-Bus · X server grabs", "muted"),
            ("tmux · live shell bindings", "muted"),
        ],
    ),
    ("3 · Read configuration", [("config files · built-in defaults", "muted")]),
]
EVIDENCE_EDGES = [["not", "observable"], ["cannot be", "asked"]]


def evidence_tiers(theme: str) -> str:
    c = THEMES[theme]
    width, height = 860, 140
    box_w, box_h, gap = 240, 96, 60
    left = (width - 3 * box_w - 2 * gap) // 2
    top = (height - box_h) // 2
    out = svg_open(
        width,
        height,
        "Evidence tiers",
        "whykey first tries to observe the key itself (whykey listen); when a layer "
        "is not observable it asks the running program (compositor IPC, D-Bus, X "
        "server grabs, tmux, live shell bindings); when the program cannot be asked "
        "it reads configuration files and built-in defaults.",
        c,
    )
    for index, (heading, lines) in enumerate(EVIDENCE):
        x = left + index * (box_w + gap)
        center = x + box_w / 2
        out.append(
            f'<rect x="{x}" y="{top}" width="{box_w}" height="{box_h}" rx="8" '
            f'fill="{c["box"]}" stroke="{c["stroke"]}"/>'
        )
        first = top + box_h / 2 - (len(lines) * 18) / 2 + 4
        out.append(
            f'<text x="{center}" y="{first}" text-anchor="middle" font-size="14" '
            f'font-weight="700" fill="{c["title"]}">{esc(heading)}</text>'
        )
        for line, (text, tone) in enumerate(lines, start=1):
            size = 13 if tone == "title" else 12
            out.append(
                f'<text x="{center}" y="{first + line * 20}" text-anchor="middle" '
                f'font-size="{size}" fill="{c[tone]}">{esc(text)}</text>'
            )
        if index < len(EVIDENCE_EDGES):
            x1, x2 = x + box_w + 4, x + box_w + gap - 4
            y = top + box_h / 2
            out.append(
                f'<line x1="{x1}" y1="{y}" x2="{x2}" y2="{y}" stroke="{c["arrow"]}" '
                'stroke-width="1.5" marker-end="url(#arr)"/>'
            )
            label = EVIDENCE_EDGES[index]
            for row, text in enumerate(label):
                out.append(
                    f'<text x="{(x1 + x2) / 2}" y="{y - 10 - (len(label) - 1 - row) * 14}" '
                    f'text-anchor="middle" font-size="11" fill="{c["muted"]}">{esc(text)}</text>'
                )
    out.append("</svg>")
    return "\n".join(out) + "\n"


PARTICIPANTS = [
    ("Scenario", None),
    ("Real program", "(Sway, KWin, tmux…)"),
    ("Independent oracle", None),
    ("whykey", None),
]
# (from, to, reply, label)
MESSAGES = [
    (0, 1, False, "start with a test config"),
    (0, 1, False, "synthetic key press (virtual or kernel keyboard)"),
    (1, 2, True, "did the bind run? did a test window receive the key?"),
    (0, 3, False, "whykey KEY --json"),
    (3, 0, True, "verdict"),
    (2, 0, True, "ground truth"),
]
NOTE = "passes only if the verdict matches reality"


def reality_lab(theme: str) -> str:
    c = THEMES[theme]
    width = 820
    box_w, box_h = 168, 48
    columns = [110, 330, 540, 720]
    top = 20
    first_row = top + box_h + 44
    row_h = 44
    note_y = first_row + len(MESSAGES) * row_h - 8
    height = note_y + 52
    out = svg_open(
        width,
        height,
        "How a reality-lab scenario runs",
        "A scenario starts the real program with a test config and sends it a "
        "synthetic key press; an independent oracle records whether the bind ran "
        "or a test window received the key; the scenario runs whykey KEY --json; "
        "the scenario passes only if whykey's verdict matches the oracle's ground truth.",
        c,
    )
    for x, (name, detail) in zip(columns, PARTICIPANTS):
        out.append(
            f'<line x1="{x}" y1="{top + box_h}" x2="{x}" y2="{note_y + 30}" '
            f'stroke="{c["stroke"]}" stroke-width="1.5" stroke-dasharray="4 4"/>'
        )
        out.append(
            f'<rect x="{x - box_w / 2}" y="{top}" width="{box_w}" height="{box_h}" rx="8" '
            f'fill="{c["box"]}" stroke="{c["stroke"]}"/>'
        )
        if detail:
            out.append(
                f'<text x="{x}" y="{top + 21}" text-anchor="middle" font-size="13" '
                f'font-weight="700" fill="{c["title"]}">{esc(name)}</text>'
            )
            out.append(
                f'<text x="{x}" y="{top + 38}" text-anchor="middle" font-size="11" '
                f'fill="{c["muted"]}">{esc(detail)}</text>'
            )
        else:
            out.append(
                f'<text x="{x}" y="{top + 29}" text-anchor="middle" font-size="13" '
                f'font-weight="700" fill="{c["title"]}">{esc(name)}</text>'
            )
    for number, (source, target, reply, label) in enumerate(MESSAGES, start=1):
        y = first_row + (number - 1) * row_h
        x1, x2 = columns[source], columns[target]
        step = 6 if x2 > x1 else -6
        dash = ' stroke-dasharray="5 4"' if reply else ""
        out.append(
            f'<line x1="{x1 + step}" y1="{y}" x2="{x2 - step}" y2="{y}" '
            f'stroke="{c["arrow"]}" stroke-width="1.5"{dash} marker-end="url(#arr)"/>'
        )
        # Labels sit above the number circle; a label wider than its span
        # is kept inside the drawing.
        half = len(label) * 3.3
        center = min(max((x1 + x2) / 2, half + 12), width - half - 12)
        out.append(
            f'<text x="{center}" y="{y - 14}" text-anchor="middle" font-size="12" '
            f'fill="{c["title"]}">{esc(label)}</text>'
        )
        out.append(f'<circle cx="{x1}" cy="{y}" r="9" fill="{c["key_bg"]}"/>')
        out.append(
            f'<text x="{x1}" y="{y + 4}" text-anchor="middle" font-size="11" '
            f'font-weight="700" fill="{c["key_fg"]}">{number}</text>'
        )
    note_w = 300
    out.append(
        f'<rect x="{columns[0] - 60}" y="{note_y}" width="{note_w}" height="30" rx="6" '
        f'fill="{c["callout"]}" stroke="{c["remap"]}"/>'
    )
    out.append(
        f'<text x="{columns[0] - 60 + note_w / 2}" y="{note_y + 20}" text-anchor="middle" '
        f'font-size="12" fill="{c["title"]}">{esc(NOTE)}</text>'
    )
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main() -> int:
    for name, build in (("evidence-tiers", evidence_tiers), ("reality-lab", reality_lab)):
        path = f"{sys.argv[1]}/{name}.svg"
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(combine(build("light"), build("dark")))
        print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
