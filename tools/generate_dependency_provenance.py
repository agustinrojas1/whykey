#!/usr/bin/env python3
"""Generate or check the dependency provenance document from Cargo metadata."""

from __future__ import annotations

import argparse
import pathlib
import tomllib


ROOT = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "Cargo.toml"
LOCKFILE = ROOT / "Cargo.lock"
OUTPUT = ROOT / "DEPENDENCY_PROVENANCE.md"


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
        "This file is generated from `Cargo.toml` and `Cargo.lock` by",
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
