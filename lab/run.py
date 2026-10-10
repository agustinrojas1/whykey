#!/usr/bin/env python3
"""whykey reality lab runner.

Every scenario runs in a fresh `whykey-lab` container with the whykey binary
mounted at /w/whykey. lab/driver.py (inside the container) starts the real
programs, performs real synthetic input, records ground truth from the
programs' reaction, and runs `whykey <args> --json --schema-version 2`. This
runner compares whykey's verdict for the scenario's layer with that ground
truth, prints a table, writes lab/results.json and exits 1 on any mismatch.

    docker build -t whykey-lab lab/
    cargo build --release --locked
    python3 lab/run.py [--only GLOB]... [-j N] [--keep]

Scenario format: one JSON object per file under lab/scenarios/<group>/<id>.json
(the file stem must equal "id").

    id            unique name, equal to the file stem
    description   one line: what reality is being checked
    tier          "observed" (a program reacted to input) or "runtime-setting"
                  (a running settings store was queried; no input involved)
    environment   "sway"    headless Sway; config at sway.config
                            (default /root/.config/sway/config)
                  "x11"     Xvfb :5 plus x11.daemons started in order (argv
                            list or {"argv": [...], "env": {...}, "ready":
                            "wm", "ready_grab": [modmask, keycode]}); "wm"
                            waits for _NET_SUPPORTING_WM_CHECK, ready_grab
                            until the daemon holds that X11 key grab
                            (lab/xgrab.py); i3 also exports I3SOCK; then
                            x11.settle seconds (default 0.5)
                  "pty"     pty.command started on a pseudo-terminal (TERM,
                            pty.env); `type` actions are keystrokes into it
                  "session" only the session D-Bus (always started)
    files         {path: text or list of lines} written before anything starts
    setup         shell commands run before the session starts
    session_setup shell commands run after the environment is up
    probe         optional independent client that records delivered keys:
                  "wayland" (foot + lab/keylog.py) or "x11" (xev)
    actions       ordered list, each one of
                  {"vkbd": ["super+t", ...]}  Wayland virtual keyboard with the
                  real US keymap and evdev keycodes (lab/vkbd.py)
                  {"wtype": [args]}  {"xdotool": [args]}  {"type": "bytes"}
                  {"sh": "cmd"}  {"sleep": seconds}  {"wait_quiet": seconds}
                  ("type" accepts \\xNN, \\e, \\r escapes; optional "settle")
    ground_truth  {"fired": CHECK, "delivered": "probe" | CHECK}
                  CHECK = {"file": path[, "equals": text]}
                        | {"gsettings": [schema, key], "contains": accel}
                        | {"sh": "cmd"} (exit 0 = true)
                  "fired": did the layer under test act on the key.
                  "delivered": did the key continue past it (optional).
    whykey        {"args": [...], "layer": name or [names], "env": {...},
                   "run": "direct" | "pty", "command": "whykey",
                   "accept": [...], "summary": "text"}
                  "pty" types the command into the scenario's shell.
                  "summary": substring the layer's summary must contain, so
                  a right verdict for the wrong binding still fails.
                  "accept": honest-uncertainty verdicts tolerated when the
                  scenario documents that whykey cannot know more (e.g. no
                  shell-init): "handled-uncertain" only when the truth fired,
                  "uncertain-continues" only when it did not. Shown as PASS~.

Ground truth -> accepted whykey verdicts for the layer:
    fired, not delivered   consumed
    fired, delivered       handled-passed
    fired, no probe        consumed | handled-passed
    not fired              pass
Verdicts come from the layer's status/propagation pair: Handled+Stops =
consumed, Handled+Continues = handled-passed, NotHandled+Continues = pass,
Handled+Indeterminate = handled-uncertain, Indeterminate+Continues =
uncertain-continues; anything else is reported as-is (unavailable, absent, ...).
"""

from __future__ import annotations

import argparse
import concurrent.futures
import fnmatch
import json
import os
import subprocess
import sys
import time
from pathlib import Path

LAB = Path(__file__).resolve().parent
ROOT = LAB.parent
SCENARIOS = LAB / "scenarios"
RESULT_MARKER = "@@WHYKEY-LAB-RESULT@@"
ENVIRONMENTS = {"sway", "x11", "pty", "session"}
TIERS = {"observed", "runtime-setting"}
REQUIRED = {"id", "description", "tier", "environment", "ground_truth", "whykey"}
SCENARIO_TIMEOUT = 120
# Verdicts a scenario may tolerate via whykey.accept, keyed by "fired".
HONEST_UNCERTAINTY = {True: {"handled-uncertain"}, False: {"uncertain-continues"}}


def load_scenarios(patterns: list[str]) -> list[tuple[Path, dict]]:
    scenarios = []
    seen: set[str] = set()
    for path in sorted(SCENARIOS.rglob("*.json")):
        scenario = json.loads(path.read_text(encoding="utf-8"))
        missing = REQUIRED - set(scenario)
        if missing:
            raise SystemExit(f"{path}: missing {sorted(missing)}")
        identifier = scenario["id"]
        if identifier != path.stem:
            raise SystemExit(f"{path}: id {identifier!r} must equal the file stem")
        if identifier in seen:
            raise SystemExit(f"{path}: duplicate id {identifier!r}")
        if scenario["environment"] not in ENVIRONMENTS:
            raise SystemExit(f"{path}: unknown environment {scenario['environment']!r}")
        if scenario["tier"] not in TIERS:
            raise SystemExit(f"{path}: unknown tier {scenario['tier']!r}")
        allowed = HONEST_UNCERTAINTY[True] | HONEST_UNCERTAINTY[False]
        if not set(scenario["whykey"].get("accept", [])) <= allowed:
            raise SystemExit(f"{path}: whykey.accept may only list {sorted(allowed)}")
        seen.add(identifier)
        relative = path.relative_to(SCENARIOS).as_posix()
        if patterns and not any(
            fnmatch.fnmatch(identifier, pattern) or fnmatch.fnmatch(relative, pattern)
            for pattern in patterns
        ):
            continue
        scenarios.append((path, scenario))
    return scenarios


def expected_verdicts(truth: dict) -> tuple[str, set[str]]:
    fired, delivered = truth["fired"], truth["delivered"]
    if fired and delivered is False:
        return "consumed", {"consumed"}
    if fired and delivered:
        return "handled-passed", {"handled-passed"}
    if fired:
        return "handled", {"consumed", "handled-passed"}
    if delivered:
        return "passed", {"pass"}
    if delivered is False:
        return "not-fired, not-delivered", {"pass"}
    return "not-fired", {"pass"}


def layer_verdict(report: dict | None, layers: list[str]) -> tuple[str, dict | None]:
    if report is None:
        return "error", None
    for entry in report.get("path", []):
        if entry.get("layer") in layers:
            status, propagation = entry.get("status"), entry.get("propagation")
            verdict = {
                ("Handled", "Stops"): "consumed",
                ("Handled", "Continues"): "handled-passed",
                ("NotHandled", "Continues"): "pass",
                ("Handled", "Indeterminate"): "handled-uncertain",
                ("Indeterminate", "Continues"): "uncertain-continues",
            }.get((status, propagation))
            if verdict is None:
                if propagation == "Redirected":
                    verdict = "redirected"
                elif status == "Unavailable":
                    verdict = "unavailable"
                else:
                    verdict = "indeterminate"
            return verdict, entry
    return "absent", None


def docker_command(args, path: Path, name: str, keep: bool) -> list[str]:
    command = ["docker", "run", "--name", name, "--network", "none", "--init"]
    command += ["-d"] if keep else ["--rm"]
    command += [
        "-v", f"{LAB}:/lab:ro",
        "-v", f"{args.binary}:/w/whykey:ro",
        "-e", f"WHYKEY_LAB_KEEP={'1' if keep else '0'}",
        args.image,
        "python3", "/lab/driver.py", f"/lab/{path.relative_to(LAB).as_posix()}",
    ]
    return command


def run_container(args, path: Path, scenario: dict) -> dict:
    name = f"whykey-lab-{os.getpid()}-{scenario['id']}"
    command = docker_command(args, path, name, args.keep)
    started = time.monotonic()
    output = ""
    try:
        if args.keep:
            subprocess.run(command, check=True, capture_output=True, timeout=60)
            deadline = time.monotonic() + SCENARIO_TIMEOUT
            while time.monotonic() < deadline:
                logs = subprocess.run(["docker", "logs", name], capture_output=True, text=True)
                output = logs.stdout
                if RESULT_MARKER in output:
                    break
                time.sleep(0.5)
        else:
            completed = subprocess.run(
                command, capture_output=True, text=True, timeout=SCENARIO_TIMEOUT
            )
            output = completed.stdout + completed.stderr
    except subprocess.TimeoutExpired:
        subprocess.run(["docker", "kill", name], capture_output=True)
        return {"error": f"container timed out after {SCENARIO_TIMEOUT}s", "container": name}
    except subprocess.CalledProcessError as error:
        return {"error": f"docker run failed: {error.stderr.decode().strip()}"}
    for line in output.splitlines():
        if line.startswith(RESULT_MARKER):
            result = json.loads(line[len(RESULT_MARKER):])
            result["seconds"] = round(time.monotonic() - started, 1)
            if args.keep:
                result["container"] = name
            return result
    return {"error": "driver produced no result", "output": output[-4000:]}


def evaluate(path: Path, scenario: dict, result: dict) -> dict:
    layers = scenario["whykey"]["layer"]
    layers = [layers] if isinstance(layers, str) else layers
    record = {
        "scenario": scenario["id"],
        "file": path.relative_to(ROOT).as_posix(),
        "environment": scenario["environment"],
        "tier": scenario["tier"],
        "layer": layers[0],
        "whykey_args": scenario["whykey"]["args"],
    }
    if "error" in result:
        record.update(ground_truth=None, expected=None, whykey_verdict="lab-error")
        record["pass"] = False
        record["error"] = result["error"]
        record["logs"] = result.get("logs") or result.get("output")
        return record
    truth = result["ground_truth"]
    label, accepted = expected_verdicts(truth)
    whykey = result["whykey"]
    verdict, entry = layer_verdict(whykey["report"], layers)
    tolerated = set(scenario["whykey"].get("accept", [])) & HONEST_UNCERTAINTY[truth["fired"]]
    summary = scenario["whykey"].get("summary")
    summary_ok = summary is None or (entry is not None and summary in (entry.get("summary") or ""))
    passed = (verdict in accepted or verdict in tolerated) and summary_ok
    record.update(
        ground_truth={"label": label, **truth},
        expected=sorted(accepted | tolerated),
        expected_summary=summary,
        whykey_verdict=verdict,
        whykey_layer=entry,
    )
    record["pass"] = passed
    record["tolerated"] = passed and verdict in tolerated
    if not passed:
        report = whykey["report"]
        record["whykey_layers"] = [
            f"{item.get('layer')}: {item.get('status')}/{item.get('propagation')} - {item.get('summary')}"
            for item in (report or {}).get("path", [])
        ]
        record["whykey_stderr"] = whykey["stderr"][-2000:] or None
        if whykey.get("stdout"):
            record["whykey_stdout"] = whykey["stdout"][-2000:]
        record["logs"] = result.get("logs")
    if result.get("container"):
        record["container"] = result["container"]
    return record


def print_table(records: list[dict]) -> None:
    rows = [("scenario", "layer", "ground truth", "whykey", "result")]
    for record in records:
        truth = record["ground_truth"]["label"] if record["ground_truth"] else "-"
        rows.append(
            (
                record["scenario"],
                record["layer"],
                truth,
                record["whykey_verdict"],
                ("PASS~" if record.get("tolerated") else "PASS") if record["pass"] else "FAIL",
            )
        )
    widths = [max(len(row[column]) for row in rows) for column in range(len(rows[0]))]
    for index, row in enumerate(rows):
        print("  ".join(cell.ljust(width) for cell, width in zip(row, widths)).rstrip())
        if index == 0:
            print("  ".join("-" * width for width in widths))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--only", action="append", default=[], metavar="GLOB",
                        help="scenario id or scenarios/-relative path glob (repeatable)")
    parser.add_argument("-j", "--jobs", type=int, default=min(4, os.cpu_count() or 1))
    parser.add_argument("--keep", action="store_true",
                        help="leave each container running after the scenario for docker exec")
    parser.add_argument("--image", default="whykey-lab")
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "release" / "whykey")
    parser.add_argument("--results", type=Path, default=LAB / "results.json")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    if not args.binary.is_file():
        raise SystemExit(f"{args.binary} not found; run `cargo build --release --locked`")
    scenarios = load_scenarios(args.only)
    if not scenarios:
        raise SystemExit("no scenario matched")

    records: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futures = {
            pool.submit(run_container, args, path, scenario): (path, scenario)
            for path, scenario in scenarios
        }
        for future in concurrent.futures.as_completed(futures):
            path, scenario = futures[future]
            record = evaluate(path, scenario, future.result())
            records.append(record)
            status = "PASS" if record["pass"] else "FAIL"
            print(f"[{status}] {record['scenario']}", file=sys.stderr, flush=True)

    records.sort(key=lambda record: record["file"])
    args.results.write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    print()
    print_table(records)
    failed = [record for record in records if not record["pass"]]
    print(f"\n{len(records) - len(failed)}/{len(records)} scenarios match reality; results: {args.results}")
    for record in failed:
        print(f"\nFAIL {record['scenario']} ({record['file']})")
        if record.get("error"):
            print(f"  lab error: {record['error']}")
            continue
        truth = record["ground_truth"]
        print(f"  ground truth: {truth['label']} ({truth['fired_evidence']}"
              + (f"; {truth['delivered_evidence']}" if truth["delivered_evidence"] else "") + ")")
        print(f"  whykey {' '.join(record['whykey_args'])}: {record['whykey_verdict']}, expected {'|'.join(record['expected'])}")
        if record.get("expected_summary"):
            print(f"  layer summary must contain: {record['expected_summary']!r}")
        for line in record.get("whykey_layers", []):
            print(f"    {line}")
        if record.get("whykey_stderr"):
            print(f"  stderr: {record['whykey_stderr'].strip()}")
    if args.keep:
        print("\nkept containers:", " ".join(r["container"] for r in records if r.get("container")))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
