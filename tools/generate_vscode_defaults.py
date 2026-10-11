#!/usr/bin/env python3
"""Regenerate extensions/whykey-vscode-defaults.json from a real VS Code.

VS Code registers its default keybindings in code (the workbench, editor
contributions, and built-in extensions), not as a data file. This script asks
a running VS Code for the list it actually resolved: it starts the given
install in a throwaway container (the `whykey-lab` image: Xvfb with the X
server's default US keymap, a session D-Bus, no network) with a one-file
development extension. That extension reads
`vscode://defaultsettings/keybindings.json`, the document behind
"Preferences: Open Default Keyboard Shortcuts (JSON)", which VS Code renders
from its keybinding resolver's default list in resolver order (later entries
win). The script then keeps `key`, `command`, and `when`, substitutes the
platform constants VS Code substitutes on desktop Linux, drops entries that
can never register (`when` false), and records the version.

Usage:
  python3 tools/generate_vscode_defaults.py                     # /usr/share/code
  python3 tools/generate_vscode_defaults.py --code-dir DIR
  python3 tools/generate_vscode_defaults.py --download 1.140.0  # official tarball

Nothing runs on the host desktop: VS Code runs only inside the container.
"""

import argparse
import difflib
import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
import tarfile
import urllib.request

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
WORK = os.path.join(REPO, "target", "vscode-defaults")
OUTPUT = os.path.join(REPO, "extensions", "whykey-vscode-defaults.json")
DUMP_URI = "vscode://defaultsettings/keybindings.json"

PACKAGE_JSON = {
    "name": "whykey-dump-default-keybindings",
    "publisher": "whykey",
    "version": "0.0.0",
    "engines": {"vscode": "*"},
}

# Runs in the extension host as the `--extensionTestsPath` module; VS Code
# exits when `run()` settles. Some defaults are registered late (workbench
# contributions of the "eventually" lifecycle phase, views created after
# restore), and the rendered document is cached once opened, so the dump
# waits for startup to settle before opening it.
RUN_JS = """const vscode = require('vscode');
const fs = require('fs');
exports.run = async function () {
  await new Promise(r => setTimeout(r, Number(process.env.WHYKEY_SETTLE_MS)));
  const uri = vscode.Uri.parse('%s');
  let text = null, error = null;
  for (let i = 0; i < 120 && text === null; i++) {
    try {
      const t = (await vscode.workspace.openTextDocument(uri)).getText();
      if (t.includes('"command"')) text = t;
    } catch (e) { error = String(e); }
    if (text === null) await new Promise(r => setTimeout(r, 500));
  }
  if (text === null) throw new Error('default keybindings unavailable: ' + error);
  fs.writeFileSync(process.env.WHYKEY_DUMP, text);
};
""" % DUMP_URI

CONTAINER_SCRIPT = """set -e
trap 'chown -R "$HOST_OWNER" /work' EXIT
Xvfb :9 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
for i in $(seq 50); do [ -e /tmp/.X11-unix/X9 ] && break; sleep 0.1; done
mkdir -p "$HOME"
timeout 180 dbus-run-session -- /opt/code/code --no-sandbox --disable-gpu \
  --password-store=basic --skip-welcome --disable-workspace-trust \
  --user-data-dir /tmp/ud --extensions-dir /tmp/exts \
  --extensionDevelopmentPath=/work/ext --extensionTestsPath=/work/ext/run.js \
  >/work/code.log 2>&1
"""


def load_extension():
    path = os.path.join(REPO, "extensions", "whykey-vscode")
    loader = importlib.machinery.SourceFileLoader("whykey_vscode", path)
    spec = importlib.util.spec_from_loader("whykey_vscode", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def download(version):
    root = os.path.join(WORK, f"VSCode-{version}")
    code_dir = os.path.join(root, "VSCode-linux-x64")
    if os.path.isfile(os.path.join(code_dir, "code")):
        return code_dir
    os.makedirs(root, exist_ok=True)
    archive = os.path.join(root, "code.tar.gz")
    url = f"https://update.code.visualstudio.com/{version}/linux-x64/stable"
    print(f"downloading {url}", file=sys.stderr)
    urllib.request.urlretrieve(url, archive)
    with tarfile.open(archive) as tar:
        tar.extractall(root, filter="data")
    os.remove(archive)
    return code_dir


def product_info(code_dir):
    with open(os.path.join(code_dir, "resources", "app", "product.json"), encoding="utf-8") as f:
        product = json.load(f)
    return {key: product.get(key) for key in ("nameShort", "version", "commit", "quality")}


def dump_defaults(code_dir, image, settle_ms):
    ext_dir = os.path.join(WORK, "ext")
    os.makedirs(ext_dir, exist_ok=True)
    with open(os.path.join(ext_dir, "package.json"), "w", encoding="utf-8") as f:
        json.dump(PACKAGE_JSON, f)
    with open(os.path.join(ext_dir, "run.js"), "w", encoding="utf-8") as f:
        f.write(RUN_JS)
    dump = os.path.join(WORK, "default-keybindings.jsonc")
    if os.path.exists(dump):
        os.remove(dump)
    subprocess.run(
        [
            "docker", "run", "--rm", "--network", "none", "--init", "--shm-size=1g",
            "-e", f"HOST_OWNER={os.getuid()}:{os.getgid()}",
            "-e", "DISPLAY=:9", "-e", "HOME=/tmp/home", "-e", "LANG=C.UTF-8",
            "-e", "WHYKEY_DUMP=/work/default-keybindings.jsonc", "-e", f"WHYKEY_SETTLE_MS={settle_ms}",
            "-v", f"{os.path.realpath(code_dir)}:/opt/code:ro",
            "-v", f"{WORK}:/work",
            image, "sh", "-c", CONTAINER_SCRIPT,
        ],
        check=True,
    )
    with open(dump, encoding="utf-8") as f:
        return f.read()


def parse_dump(text, ext):
    # The rendered file is a JSON array followed by a comment listing unbound
    # commands; comments are stripped with the extension's own JSONC reader.
    entries = json.loads(ext.strip_jsonc(text))
    bindings = []
    for entry in entries:
        when = entry.get("when", "")
        dnf = ext.parse_dnf(when)
        if dnf is None:
            raise SystemExit(f"cannot parse when clause of {entry}: {when!r}")
        dnf = ext.substitute_constants(dnf)
        if not dnf:
            continue  # constant false: VS Code never registers it
        row = [entry["key"], entry["command"]]
        serialized = ext.serialize_dnf(dnf)
        if serialized:
            row.append(serialized)
        bindings.append(row)
    return bindings


def write_table(path, product, method, bindings, ext):
    header = {
        "vscode": {
            "product": product["nameShort"],
            "version": product["version"],
            "commit": product["commit"],
            "quality": product["quality"],
        },
        "platform": "linux",
        "keyboard_layout": "us",
        "generated_by": "tools/generate_vscode_defaults.py",
        "method": method,
        "when_constants": ext.WHEN_CONSTANTS,
        "format": "bindings: [key, command, when?] in VS Code resolver order; later entries win",
    }
    lines = ["{"]
    for name, value in header.items():
        lines.append(f"  {json.dumps(name)}: {json.dumps(value, ensure_ascii=False)},")
    lines.append('  "bindings": [')
    rows = [json.dumps(row, ensure_ascii=False) for row in bindings]
    lines.append(",\n".join(f"    {row}" for row in rows))
    lines.append("  ]")
    lines.append("}")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


def merge_runs(base, other):
    """Union of two dumps in resolver order.

    A few defaults are registered only once their view or editor is created
    (the Output view's filter, the chat Models editor's search), which a
    fresh window does not always do before the dump. Their `when` clauses
    confine them to that view, so they belong in the table; runs are merged
    instead of required to agree.
    """
    a, b = [tuple(row) for row in base], [tuple(row) for row in other]
    merged = []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(a=a, b=b, autojunk=False).get_opcodes():
        merged.extend(base[i1:i2])
        if tag in ("insert", "replace"):
            merged.extend(row for row in other[j1:j2] if row not in base[i1:i2])
    return merged


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--code-dir", default="/usr/share/code", help="VS Code install root (has code, resources/)")
    source.add_argument("--download", metavar="VERSION", help="fetch the official linux-x64 tarball of VERSION")
    parser.add_argument("--image", default="whykey-lab", help="container image with Xvfb and dbus-run-session")
    parser.add_argument("--settle", type=float, default=20, help="seconds to let VS Code finish starting")
    parser.add_argument("--runs", type=int, default=3, help="independent runs to merge")
    parser.add_argument("--output", default=OUTPUT)
    args = parser.parse_args()

    os.makedirs(WORK, exist_ok=True)
    code_dir = download(args.download) if args.download else args.code_dir
    product = product_info(code_dir)
    ext = load_extension()
    runs = [
        parse_dump(dump_defaults(code_dir, args.image, int(args.settle * 1000)), ext)
        for _ in range(args.runs)
    ]
    bindings = runs[0]
    for other in runs[1:]:
        bindings = merge_runs(bindings, other)
    partial = [row for row in bindings if not all(row in run for run in runs)]
    for row in partial:
        print(f"present in only some runs: {row}", file=sys.stderr)
    origin = "official linux-x64 tarball" if args.download else "linux-x64 install"
    method = (
        f"{product['nameShort']} {product['version']} ({origin}) run in the {args.image} container "
        f"under Xvfb (US keymap) with a development extension that read {DUMP_URI} "
        f"(Open Default Keyboard Shortcuts (JSON)) {args.settle:g} s after startup; union of "
        f"{args.runs} runs ({len(partial)} view-registered bindings missing from some); "
        "when-clause platform constants substituted"
    )
    write_table(args.output, product, method, bindings, ext)
    print(f"wrote {len(bindings)} bindings for {product['nameShort']} {product['version']} to {args.output}")


if __name__ == "__main__":
    main()
