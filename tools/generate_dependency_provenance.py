#!/usr/bin/env python3
"""Generate or check the dependency provenance document from Cargo metadata
and the vendored keysym table."""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import tomllib


ROOT = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "Cargo.toml"
LOCKFILE = ROOT / "Cargo.lock"
OUTPUT = ROOT / "DEPENDENCY_PROVENANCE.md"
KEYSYM_TABLE = ROOT / "src" / "xkb_keysyms.txt"

# keysymdef.h ships these two notices; they must accompany the derived table.
KEYSYMDEF_NOTICES = """\
Copyright 1987, 1994, 1998  The Open Group

Permission to use, copy, modify, distribute, and sell this software and its
documentation for any purpose is hereby granted without fee, provided that
the above copyright notice appear in all copies and that both that
copyright notice and this permission notice appear in supporting
documentation.

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE OPEN GROUP BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

Except as contained in this notice, the name of The Open Group shall
not be used in advertising or otherwise to promote the sale, use or
other dealings in this Software without prior written authorization
from The Open Group.


Copyright 1987 by Digital Equipment Corporation, Maynard, Massachusetts

                        All Rights Reserved

Permission to use, copy, modify, and distribute this software and its
documentation for any purpose and without fee is hereby granted,
provided that the above copyright notice appear in all copies and that
both that copyright notice and this permission notice appear in
supporting documentation, and that the name of Digital not be
used in advertising or publicity pertaining to distribution of the
software without specific, written prior permission.

DIGITAL DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE, INCLUDING
ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS, IN NO EVENT SHALL
DIGITAL BE LIABLE FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR
ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS
SOFTWARE.
"""


def keysym_table_section() -> list[str]:
    data = KEYSYM_TABLE.read_bytes()
    entries = sum(
        1
        for line in data.decode("utf-8").splitlines()
        if line.strip() and not line.startswith("#")
    )
    return [
        "## Vendored data",
        "",
        "`src/xkb_keysyms.txt` is compiled into the binary with `include_str!` and",
        "maps X11 keysym names to Unicode code points for XKB text prediction.",
        "",
        "- Source: `keysymdef.h` from X.Org xorgproto 2025.1",
        "  (`include/X11/keysymdef.h`)",
        "- Derivation: every `#define XK_<name> 0x… /* U+XXXX` or `// U+XXXX`",
        "  line, in file order; the `<U+…>` and `(U+…)` annotations, which",
        "  keysymdef.h uses for duplicate or inexact correspondences, are excluded",
        f"- Entries: {entries}",
        f"- SHA-256: `{hashlib.sha256(data).hexdigest()}`",
        "- License: the X11-style permission notices of `keysymdef.h`, reproduced",
        "  below",
        "",
        "```text",
        *KEYSYMDEF_NOTICES.rstrip("\n").split("\n"),
        "```",
        "",
    ]


def load_metadata() -> tuple[dict, dict]:
    with MANIFEST.open("rb") as handle:
        manifest = tomllib.load(handle)
    with LOCKFILE.open("rb") as handle:
        lockfile = tomllib.load(handle)
    return manifest, lockfile


def render() -> str:
    manifest, lockfile = load_metadata()
    package = manifest["package"]
    locked_packages = [
        entry
        for entry in lockfile["package"]
        if entry["name"] != package["name"]
        and entry.get("source", "").startswith("registry+")
    ]
    locked_packages.sort(key=lambda entry: entry["name"])
    direct = set(manifest.get("dependencies", {}))

    lines = [
        "# Dependency provenance and tested versions",
        "",
        "This file is generated from `Cargo.toml`, `Cargo.lock`, and",
        "`src/xkb_keysyms.txt` by",
        "[`tools/generate_dependency_provenance.py`](tools/generate_dependency_provenance.py).",
        "Do not edit it by hand; CI checks that it is current.",
        "",
        f"- Package: `{package['name']} {package['version']}`",
        f"- Rust edition: `{package['edition']}`",
        f"- Declared MSRV: `{package['rust-version']}`",
        "- Dependency source: crates.io registry entries pinned by `Cargo.lock`",
        "- Checksums: Cargo registry checksums recorded in `Cargo.lock`",
        "- Verified toolchains: stable and Rust `1.85.0`",
        "- Verified host in this repository: Linux `x86_64-unknown-linux-gnu`",
        "- Other release targets, package builders, and distro toolchain versions",
        "  remain outside this verification record.",
        "",
        "| Package | Version | Source | SHA-256 registry checksum | Direct dependency |",
        "| --- | --- | --- | --- | --- |",
    ]
    for entry in locked_packages:
        lines.append(
            "| `{name}` | `{version}` | `{source}` | `{checksum}` | {direct} |".format(
                name=entry["name"],
                version=entry["version"],
                source=entry["source"],
                checksum=entry["checksum"],
                direct="yes" if entry["name"] in direct else "no",
            )
        )
    lines.extend(
        [
            "",
            "The lockfile remains the authoritative input for reproducible Cargo",
            "builds. A checksum proves the selected registry package contents; it",
            "does not claim that every downstream distro rebuild or native package",
            "has been executed. Release archives carry a separate SHA-256 checksum",
            "for the assembled archive.",
            "",
            *keysym_table_section(),
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    generated = render()
    if args.check:
        current = OUTPUT.read_text(encoding="utf-8") if OUTPUT.exists() else ""
        if current != generated:
            print(f"{OUTPUT} is stale; run tools/generate_dependency_provenance.py")
            return 1
        return 0
    OUTPUT.write_text(generated, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
