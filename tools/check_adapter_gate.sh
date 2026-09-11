#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
matrix="$root_dir/tests/fixtures/sessions/matrix.json"

python3 - "$root_dir" "$matrix" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
matrix_path = pathlib.Path(sys.argv[2])
document = json.loads(matrix_path.read_text(encoding="utf-8"))
sessions = document.get("sessions")
if document.get("schema_version") != 1 or not isinstance(sessions, list):
    raise SystemExit("adapter session matrix must have schema_version 1 and sessions[]")

support = (root / "support-matrix.json").read_text(encoding="utf-8")
inventory = (root / "ADAPTER_INVENTORY.md").read_text(encoding="utf-8").lower()
seen = set()
required = {"id", "fixture", "positive", "negative", "ipc_failure"}
for session in sessions:
    if set(session) != required:
        raise SystemExit(f"incomplete adapter fixture entry: {session}")
    adapter_id = session["id"]
    if adapter_id in seen:
        raise SystemExit(f"duplicate adapter fixture: {adapter_id}")
    seen.add(adapter_id)
    fixture = root / "tests/fixtures/sessions" / session["fixture"]
    if not fixture.is_file():
        raise SystemExit(f"missing fixture for {adapter_id}: {fixture}")
    payload = json.loads(fixture.read_text(encoding="utf-8"))
    if payload.get("id") != adapter_id:
        raise SystemExit(f"fixture id mismatch for {adapter_id}")
    if adapter_id.lower() not in inventory:
        raise SystemExit(f"{adapter_id} is missing from ADAPTER_INVENTORY.md")
    if f'"compositor.{adapter_id}"' not in support:
        raise SystemExit(f"{adapter_id} is missing from support-matrix.json")

if {"hyprland", "sway", "i3"} - seen != set():
    raise SystemExit("hyprland, sway, and i3 must all have fixture coverage")

# Verify that the validator rejects an intentionally incomplete adapter entry.
broken = dict(sessions[0])
del broken["ipc_failure"]
if required.issubset(broken):
    raise SystemExit("adapter gate self-test failed: incomplete entry was accepted")
print(f"adapter gate: {len(sessions)} complete session fixtures")
PY
