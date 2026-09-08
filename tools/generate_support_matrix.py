#!/usr/bin/env python3
"""Generate the human-readable support matrix from support-matrix.json."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "support-matrix.json"
OUTPUT = ROOT / "SUPPORT_MATRIX.md"


def load_entries() -> list[dict[str, object]]:
    document = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if document.get("schema_version") != 1:
        raise SystemExit("support-matrix.json has an unsupported schema_version")
    entries = document.get("entries")
    if not isinstance(entries, list) or not entries:
        raise SystemExit("support-matrix.json must contain a non-empty entries list")
    seen: set[str] = set()
    for entry in entries:
        if not isinstance(entry, dict):
            raise SystemExit("support-matrix entries must be objects")
        required = {"id", "area", "implemented", "summary"}
        if set(entry) != required:
            raise SystemExit(f"invalid support-matrix entry keys: {entry}")
        identifier = entry["id"]
        if not isinstance(identifier, str) or not identifier or identifier in seen:
            raise SystemExit(f"duplicate or invalid support-matrix id: {identifier!r}")
        if not isinstance(entry["area"], str) or not isinstance(entry["summary"], str):
            raise SystemExit(f"invalid text in support-matrix entry: {identifier}")
        if not isinstance(entry["implemented"], bool):
            raise SystemExit(f"invalid implementation flag: {identifier}")
        seen.add(identifier)
    return entries


def markdown(entries: list[dict[str, object]]) -> str:
    lines = [
        "# Whykey support matrix",
        "",
        "This file is generated from [`support-matrix.json`](support-matrix.json) by",
        "[`tools/generate_support_matrix.py`](tools/generate_support_matrix.py).",
        "Do not edit it by hand. Runtime availability is environment-dependent and",
        "is reported by `whykey capabilities`; this matrix records the implementation",
        "contract shared by the capability registry and tests.",
        "",
        "| ID | Area | Implementation | Scope |",
        "| --- | --- | --- | --- |",
    ]
    for entry in entries:
        status = "Implemented" if entry["implemented"] else "Planned"
        summary = str(entry["summary"]).replace("|", "\\|")
        lines.append(f"| `{entry['id']}` | {entry['area']} | {status} | {summary} |")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if the generated file is stale")
    args = parser.parse_args()
    generated = markdown(load_entries())
    if args.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != generated:
            print(f"{OUTPUT} is stale; run tools/generate_support_matrix.py")
            return 1
        return 0
    OUTPUT.write_text(generated, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
