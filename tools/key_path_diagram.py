"""Generate docs/assets/key-path-{light,dark}.svg, the README diagram of the
chain of programs a key press crosses on Linux.

Usage: python3 tools/key_path_diagram.py docs/assets"""
import sys

THEMES = {
    "light": dict(bg="#ffffff", box="#f6f8fa", stroke="#d0d7de", title="#1f2328",
                  muted="#656d76", arrow="#8c959f", key_bg="#1f2328", key_fg="#ffffff",
                  stop="#cf222e", pass_="#1a7f37", remap="#9a6700", callout="#f6f8fa",
                  callout_stroke="#d0d7de"),
    "dark": dict(bg="#0d1117", box="#161b22", stroke="#30363d", title="#e6edf3",
                 muted="#8b949e", arrow="#6e7681", key_bg="#e6edf3", key_fg="#0d1117",
                 stop="#ff7b72", pass_="#3fb950", remap="#d29922", callout="#161b22",
                 callout_stroke="#30363d"),
}

LAYERS = [
    ("Remapper", "keyd · kanata · xremap · kmonad"),
    ("Compositor / window manager", "Hyprland · Sway · GNOME · KDE · i3 · niri"),
    ("Terminal", "kitty · foot · Ghostty · WezTerm · Alacritty"),
    ("TTY driver", "Ctrl+C · Ctrl+Z · Ctrl+S (stty settings)"),
    ("Input method", "Fcitx5 · IBus · dead keys · Compose"),
    ("Multiplexer", "tmux · Screen · Zellij"),
    ("Application / shell", "bash · zsh · fish · Vim · Emacs"),
]

W = 820
FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif'
BOX_X, BOX_W, BOX_H = 56, 300, 46
GAP = 24
TOP = 36
EX_X = BOX_X + BOX_W + 18


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;")


def build(theme):
    c = THEMES[theme]
    out = []
    n = len(LAYERS)
    keycap_h = 40
    # y positions: keycap, then boxes
    key_y = TOP
    first_box_y = key_y + keycap_h + GAP
    last_box_y = first_box_y + (n - 1) * (BOX_H + GAP)
    H = last_box_y + BOX_H + 140

    out.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
               f'role="img" aria-labelledby="t d" font-family=\'{FONT}\'>')
    out.append('<title id="t">The path of a key press on Linux</title>')
    out.append('<desc id="d">A key press passes, in order, through a remapper, the compositor or window manager, '
               'the terminal, the TTY driver, the input method, a multiplexer, and finally the application or shell. '
               'Each layer is a checkpoint: it can stop the key, pass it on, or rewrite it into another key.</desc>')
    out.append(f'<defs><marker id="arr" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse">'
               f'<path d="M 0 0 L 10 5 L 0 10 z" fill="{c["arrow"]}"/></marker></defs>')
    out.append(f'<rect width="{W}" height="{H}" rx="12" fill="{c["bg"]}"/>')

    # keycap
    cx = BOX_X + BOX_W / 2
    kw = 150
    out.append(f'<rect x="{cx - kw/2}" y="{key_y}" width="{kw}" height="{keycap_h}" rx="10" fill="{c["key_bg"]}"/>')
    out.append(f'<text x="{cx}" y="{key_y + 25}" text-anchor="middle" font-size="15" font-weight="700" fill="{c["key_fg"]}">Key press</text>')
    out.append(f'<text x="{EX_X}" y="{key_y + 25}" font-size="13" fill="{c["muted"]}">e.g. Ctrl+Left, Super+Return</text>')

    # arrows + boxes
    prev_bottom = key_y + keycap_h
    for i, (name, examples) in enumerate(LAYERS):
        y = first_box_y + i * (BOX_H + GAP)
        out.append(f'<line x1="{cx}" y1="{prev_bottom + 2}" x2="{cx}" y2="{y - 2}" stroke="{c["arrow"]}" stroke-width="1.6" marker-end="url(#arr)"/>')
        out.append(f'<rect x="{BOX_X}" y="{y}" width="{BOX_W}" height="{BOX_H}" rx="8" fill="{c["box"]}" stroke="{c["stroke"]}"/>')
        out.append(f'<text x="{BOX_X + 16}" y="{y + 29}" font-size="15" font-weight="600" fill="{c["title"]}">{esc(name)}</text>')
        out.append(f'<text x="{EX_X}" y="{y + 29}" font-size="13" fill="{c["muted"]}">{esc(examples)}</text>')
        # step number, left of the box
        out.append(f'<text x="{BOX_X - 16}" y="{y + 29}" text-anchor="end" font-size="12" fill="{c["muted"]}">{i + 1}</text>')
        prev_bottom = y + BOX_H

    # legend row below the chain
    ly = last_box_y + BOX_H + 34
    out.append(f'<line x1="{BOX_X}" y1="{ly - 18}" x2="{W - 24}" y2="{ly - 18}" stroke="{c["stroke"]}"/>')
    out.append(f'<text x="{BOX_X}" y="{ly + 4}" font-size="13" font-weight="700" fill="{c["title"]}">Every layer is a checkpoint.</text>')
    out.append(f'<text x="{BOX_X + 196}" y="{ly + 4}" font-size="13" fill="{c["muted"]}">With its own binding, each one can:</text>')
    rows = [
        (c["stop"], "✕", "stop the key", "the next layer never sees it"),
        (c["pass_"], "↓", "pass it on", "handled or not, it continues"),
        (c["remap"], "↪", "rewrite it", "a different key continues"),
    ]
    for j, (color, glyph, head, detail) in enumerate(rows):
        xx = BOX_X
        yy = ly + 32 + j * 24
        out.append(f'<text x="{xx}" y="{yy}" font-size="15" font-weight="700" fill="{color}">{glyph}</text>')
        out.append(f'<text x="{xx + 22}" y="{yy}" font-size="13" fill="{c["title"]}" xml:space="preserve"><tspan font-weight="600">{head}</tspan>'
                   f'<tspan fill="{c["muted"]}">  {detail}</tspan></text>')
    out.append('</svg>')
    return "\n".join(out) + "\n"


for theme in THEMES:
    path = f"{sys.argv[1]}/key-path-{theme}.svg"
    with open(path, "w") as fh:
        fh.write(build(theme))
    print(path)
