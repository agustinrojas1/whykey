"""Run inside the lab's wayland image under `dbus-run-session`: start Fcitx5
with no configuration and print `<config>/<option>\\t<default keys>` for every
key-list option of the configurations whykey embeds.

Fcitx5 answers `GetConfig` on its D-Bus controller with the current value of
every option (the defaults, with no configuration file) and a description whose
option types say which ones are key lists. No display is needed.
"""

import json
import os
import subprocess
import time

os.environ.update(HOME="/tmp/h", XDG_RUNTIME_DIR="/tmp/r")
os.makedirs("/tmp/h", exist_ok=True)
os.makedirs("/tmp/r", mode=0o700, exist_ok=True)
subprocess.Popen(["fcitx5", "--disable=all"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(3)

CONFIGS = [("global", "fcitx://config/global")] + [
    (name, "fcitx://config/addon/" + name)
    for name in ("quickphrase", "clipboard", "unicode", "imselector")
]


def keys(value):
    data = value["data"]
    if isinstance(data, dict):
        return [item["data"] for item in data.values()]
    return [data] if data else []


for name, url in CONFIGS:
    reply = subprocess.run(
        ["busctl", "--user", "--json=short", "call", "org.fcitx.Fcitx5", "/controller",
         "org.fcitx.Fcitx.Controller1", "GetConfig", "s", url],
        capture_output=True, text=True, check=True,
    ).stdout
    _values, description = json.loads(reply)["data"]
    for _struct, options in description:
        for option, kind, _title, default, _extra in options:
            if kind in ("List|Key", "Key"):
                print(f"{name}/{option}\t{' '.join(keys(default))}")
