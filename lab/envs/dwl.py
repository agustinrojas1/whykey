"""Headless dwl 0.9 built from source in the image (/usr/local/src/dwl).

dwl reads its keys at compile time, so the scenario gives the key table:
    "dwl": {"definitions": [C lines placed before the table],
            "bindings": [C rows replacing the keys[] table]}
This plugin renders config.def.h with those rows, rebuilds dwl, and writes the
same config.h to /root/.config/dwl/config.h, where whykey reads it. Rows use
the upstream macros (MODKEY, XKB_KEY_x, spawn, ...). dwl has no window-query
IPC, so the probe's focus is checked with the Shift tap alone.
"""

from __future__ import annotations

import glob
import os
import re

from common import LabError, RUNTIME_DIR, wait_for

SOURCE = "/usr/local/src/dwl"
CONFIG = "/root/.config/dwl/config.h"
TABLE = re.compile(r"^static const Key keys\[\] = \{\n.*?^\};\n", re.S | re.M)


def render(template: str, definitions: list[str], bindings: list[str]) -> str:
    if not bindings:
        raise LabError("dwl scenario needs at least one binding row")
    table = "static const Key keys[] = {\n" + "".join(f"\t{row},\n" for row in bindings) + "};\n"
    head = "\n".join(definitions) + "\n" if definitions else ""
    rendered, count = TABLE.subn(lambda _match: head + table, template, count=1)
    if count != 1:
        raise LabError("dwl config.def.h has no keys[] table to replace")
    return rendered


def start(lab) -> None:
    spec = lab.scenario.get("dwl", {})
    with open(os.path.join(SOURCE, "config.def.h"), encoding="utf-8") as handle:
        template = handle.read()
    config = render(template, spec.get("definitions", []), spec.get("bindings", []))
    for path in (CONFIG, os.path.join(SOURCE, "config.h")):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(config)
    lab.run(["make", "-C", SOURCE], timeout=300.0)
    env = dict(lab.env)
    env.update(
        WLR_BACKENDS="headless",
        WLR_LIBINPUT_NO_DEVICES="1",
        WLR_RENDERER="pixman",
    )
    lab.spawn("dwl", [os.path.join(SOURCE, "dwl")], env)
    display = wait_for(
        lambda: (lab.ensure_alive(), sorted(glob.glob(f"{RUNTIME_DIR}/wayland-*[0-9]")))[1],
        10.0,
        "the dwl Wayland socket",
    )[0]
    lab.env["WAYLAND_DISPLAY"] = os.path.basename(display)


def inject(lab, keys: list[str]) -> None:
    lab.run(["python3", "/lab/vkbd.py", *keys])
