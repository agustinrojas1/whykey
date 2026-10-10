#!/usr/bin/env python3
"""Generate the human-readable support matrix from support-matrix.json.

Verification comes from reality, not from fixtures:

    lab       scenario ids (lab/scenarios/**/<id>.json) whose real program
              agreed with whykey in a lab run. `--lab-results` (default:
              lab/results.json when present) folds a run into the manifest:
              passing scenarios are added to the entries they prove, failing
              or merely tolerated (PASS~) ones are removed, and scenarios
              that no longer exist are dropped. Scenarios a run skipped
              (`run.py --only`) keep their earlier standing.
    live      what the maintainer verified by hand on a real desktop.

`--check` never reads lab/results.json (it is untracked and differs per
run): it verifies that SUPPORT_MATRIX.md matches the manifest and that every
recorded scenario still exists. `--check --lab-results FILE` additionally
fails when FILE would change the manifest.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "support-matrix.json"
OUTPUT = ROOT / "SUPPORT_MATRIX.md"
SCENARIOS = ROOT / "lab" / "scenarios"
DEFAULT_RESULTS = ROOT / "lab" / "results.json"

# The whykey layer a scenario judges -> the matrix entries its verdict proves.
LAYER_ENTRIES: dict[str, tuple[str, ...]] = {
    "Hyprland": ("compositor.hyprland",),
    "Sway": ("compositor.sway",),
    "i3": ("compositor.i3",),
    "KDE Plasma": ("compositor.kde", "compositor.kglobalaccel-runtime"),
    "GNOME": ("compositor.gnome", "compositor.gnome-gsettings"),
    "Openbox": ("compositor.openbox",),
    "AwesomeWM": ("compositor.programmable-x11",),
    "sxhkd": ("compositor.bspwm-sxhkd",),
    "X11 xbindkeys": ("compositor.x11.xbindkeys",),
    "tmux": ("session.multiplexers",),
    "GNU Screen": ("session.multiplexers",),
}
# Shell answers prove the runtime snapshot only when whykey answered from it.
SHELL_LAYERS = frozenset({"Bash / Readline", "Zsh / ZLE", "Fish"})
SHELL_ENTRY = "shell.runtime-snapshot"
# Which backend `whykey listen` uses in a lab environment.
FORCED_CAPTURE = {"--window": "capture.probe-window", "--terminal": "capture.terminal"}
DEFAULT_CAPTURE = {
    "hyprland": "capture.native-compositor",
    "sway": "capture.native-sway",
    "x11": "capture.probe-window",
    "pty": "capture.terminal",
}
# lab/run.py's layer name for rows that judge a `whykey listen` observation.
OBSERVATION_LAYER = "observation"


def load_document() -> dict[str, object]:
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
        required = {"id", "area", "implemented", "summary", "lab"}
        if not required <= set(entry) <= required | {"live"}:
            raise SystemExit(f"invalid support-matrix entry keys: {entry}")
        identifier = entry["id"]
        if not isinstance(identifier, str) or not identifier or identifier in seen:
            raise SystemExit(f"duplicate or invalid support-matrix id: {identifier!r}")
        if not isinstance(entry["area"], str) or not isinstance(entry["summary"], str):
            raise SystemExit(f"invalid text in support-matrix entry: {identifier}")
        if not isinstance(entry["implemented"], bool):
            raise SystemExit(f"invalid implementation flag: {identifier}")
        lab = entry["lab"]
        if not isinstance(lab, list) or not all(isinstance(item, str) for item in lab):
            raise SystemExit(f"invalid lab scenario list: {identifier}")
        if lab != sorted(set(lab)):
            raise SystemExit(f"lab scenarios must be sorted and unique: {identifier}")
        if "live" in entry and (not isinstance(entry["live"], str) or not entry["live"]):
            raise SystemExit(f"invalid live verification note: {identifier}")
        seen.add(identifier)
    return document


def scenario_ids() -> set[str]:
    return {path.stem for path in SCENARIOS.glob("*/*.json")}


def proven_entries(record: dict[str, object]) -> set[str]:
    """Matrix entries a passing lab record proves."""
    entries: set[str] = set()
    layer = record.get("layer")
    if isinstance(layer, str):
        entries.update(LAYER_ENTRIES.get(layer, ()))
        tier = (record.get("whykey_layer") or {}).get("tier")
        if layer in SHELL_LAYERS and tier == "live_query":
            entries.add(SHELL_ENTRY)
    args = record.get("whykey_args") or []
    if args and args[0] == "listen":
        forced = [FORCED_CAPTURE[arg] for arg in args if arg in FORCED_CAPTURE]
        if forced:
            entries.update(forced)
        elif layer == OBSERVATION_LAYER:
            # Only the probe window reports a bare delivered/withheld key.
            entries.add("capture.probe-window")
        elif str(record.get("environment")) in DEFAULT_CAPTURE:
            entries.add(DEFAULT_CAPTURE[str(record.get("environment"))])
    return entries


def fold_results(entries: list[dict[str, object]], records: list[dict[str, object]]) -> None:
    """Fold one lab run into the entries' `lab` lists in place."""
    known = {str(entry["id"]) for entry in entries}
    lab = {str(entry["id"]): set(entry["lab"]) for entry in entries}
    for record in records:
        scenario = record.get("scenario")
        if not isinstance(scenario, str):
            raise SystemExit(f"lab result without a scenario id: {record}")
        proof = record.get("pass") is True and record.get("tolerated") is not True
        proven = proven_entries(record) if proof else set()
        unknown = proven - known
        if unknown:
            raise SystemExit(f"lab mapping names unknown matrix entries: {sorted(unknown)}")
        for identifier, scenarios in lab.items():
            if identifier in proven:
                scenarios.add(scenario)
            else:
                scenarios.discard(scenario)
    existing = scenario_ids()
    for entry in entries:
        entry["lab"] = sorted(lab[str(entry["id"])] & existing)


def load_results(path: Path) -> list[dict[str, object]]:
    records = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(records, list) or not all(isinstance(item, dict) for item in records):
        raise SystemExit(f"{path} must be the list written by lab/run.py")
    return records


def manifest_text(document: dict[str, object]) -> str:
    entries = ",\n".join(
        "    " + json.dumps(entry, ensure_ascii=False) for entry in document["entries"]
    )
    return (
        "{\n"
        f'  "schema_version": {document["schema_version"]},\n'
        '  "entries": [\n'
        f"{entries}\n"
        "  ]\n"
        "}\n"
    )


def verification(entry: dict[str, object]) -> str:
    parts = []
    lab = entry["lab"]
    if lab:
        parts.append(f"lab-verified ({len(lab)} scenario{'' if len(lab) == 1 else 's'})")
    if "live" in entry:
        parts.append(f"live-verified on maintainer machine: {entry['live']}")
    return "; ".join(parts) or "unverified"


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
        "Verification is what reality confirmed: *lab-verified* entries name the",
        "[lab](lab/run.py) scenarios where the real program agreed with whykey (see",
        "[Lab evidence](#lab-evidence)); *live-verified* entries were checked by hand",
        "on the maintainer's desktop; everything else is *unverified*, however well",
        "its fixtures pass.",
        "",
        "| ID | Area | Implementation | Verification | Scope |",
        "| --- | --- | --- | --- | --- |",
    ]
    for entry in entries:
        status = "Implemented" if entry["implemented"] else "Planned"
        summary = str(entry["summary"]).replace("|", "\\|")
        lines.append(
            f"| `{entry['id']}` | {entry['area']} | {status} | {verification(entry)} | {summary} |"
        )
    lines += ["", "## Lab evidence", ""]
    verified = [entry for entry in entries if entry["lab"]]
    if not verified:
        lines.append("No lab scenario has been recorded yet.")
    for entry in verified:
        scenarios = ", ".join(f"`{scenario}`" for scenario in entry["lab"])
        lines.append(f"- `{entry['id']}`: {scenarios}")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if the generated files are stale")
    parser.add_argument(
        "--lab-results",
        type=Path,
        help="lab/run.py results to fold into support-matrix.json "
        "(default without --check: lab/results.json when present)",
    )
    args = parser.parse_args()
    document = load_document()
    entries = document["entries"]
    results = args.lab_results
    if results is None and not args.check and DEFAULT_RESULTS.exists():
        results = DEFAULT_RESULTS
    if args.check:
        stale = []
        missing = sorted(
            {scenario for entry in entries for scenario in entry["lab"]} - scenario_ids()
        )
        if missing:
            stale.append(f"support-matrix.json names missing lab scenarios: {', '.join(missing)}")
        if results is not None:
            before = manifest_text(document)
            fold_results(entries, load_results(results))
            if manifest_text(document) != before:
                stale.append(f"support-matrix.json does not reflect {results}")
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != markdown(entries):
            stale.append(f"{OUTPUT} is stale")
        for problem in stale:
            print(f"{problem}; run tools/generate_support_matrix.py")
        return 1 if stale else 0
    if results is not None:
        fold_results(entries, load_results(results))
        MANIFEST.write_text(manifest_text(document), encoding="utf-8")
    OUTPUT.write_text(markdown(entries), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
