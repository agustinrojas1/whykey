#!/usr/bin/env python3
"""whykey reality lab runner.

Every scenario runs in a fresh container of the lab image for its environment,
with the whykey binary mounted at /w/whykey. The images are one base image and
a few group images built from it (lab/images.json; tools/lab_images.py builds
them). lab/driver.py (inside the container) starts the real programs, performs
real synthetic input, records ground truth from the programs' reaction, and
runs `whykey <args> --json --schema-version 2`. This runner compares whykey's
verdict for the scenario's layer with that ground truth, prints a table,
writes lab/results.json and exits 1 on any mismatch.

    python3 tools/lab_images.py build
    cargo build --release --locked
    python3 lab/run.py [--only GLOB]... [-j N] [--vms N] [--keep]
                       [--image-tag TAG|hash] [--image-prefix P]

Scenario format: one JSON object per file under lab/scenarios/<group>/<id>.json
(the file stem must equal "id").

    id            unique name, equal to the file stem
    description   one line: what reality is being checked
    tier          "observed": a real program reacted to real input (the only
                  tier; settings-store reads alone never verify anything)
    environment   name of a plugin lab/envs/<name>.py (sway, hyprland, gnome,
                  x11, pty, ...); its docstring documents its own fields
                  (e.g. sway.config, x11.daemons, pty.command), actions and
                  checks. A session D-Bus is always started.
    image         optional: the group image to run in (a name in
                  lab/images.json), when it is not the one the environment
                  belongs to. The x11 "kde" scenarios need kglobalacceld, which
                  lives in the vm image with KWin.
    files         {path: text or list of lines} written before anything starts
    setup         shell commands run before the session starts
    session_setup shell commands run after the environment is up
    probe         optional independent client that records delivered keys:
                  "wayland" (foot + lab/keylog.py) or "x11" (xev)
    actions       ordered list, each one of
                  {"vkbd": ["super+t", ...]}  Wayland virtual keyboard with the
                  real US keymap and evdev keycodes (lab/vkbd.py)
                  {"rdkbd": ["alt+F4", ...]}  the same chords through mutter's
                  RemoteDesktop D-Bus API (lab/rdkbd.py; "gnome" only)
                  {"wtype": [args]}  {"xdotool": [args]}  {"type": "bytes"}
                  {"sh": "cmd"}  {"sleep": seconds}  {"wait_quiet": seconds}
                  {"start_whykey": true}  start a "run": "background" whykey
                  here instead of before the first action
                  {"wait_whykey": true}  wait for the background whykey to
                  exit; later keys go to the restored session
                  {"mark_probe": true}  "delivered": "probe" counts only the
                  keys the probe received before this point
                  {key: ...} for a key in the environment's ACTIONS
                  ("type" accepts \\xNN, \\e, \\r escapes; optional "settle")
    ground_truth  {"fired": CHECK, "delivered": "probe" | CHECK,
                   "reached": "fired" | CHECK}
                  CHECK = {"file": path[, "equals": text]}
                        | {"sh": "cmd"} (exit 0 = true)
                        | {"probe": "any"} (the probe received any key
                          press, modifiers included)
                        | {"probe": "after_mark"} (the probe received a
                          non-modifier key after the mark_probe action)
                        | {key: ...} for a key in the environment's CHECKS
                  "fired": did the layer under test act on the key.
                  "delivered": did the key continue past it ("probe": the
                  probe received a non-modifier key).
                  "reached": positive control that the key arrived at the
                  layer at all. A layer that is the key's last consumer (a
                  shell's line editor: whether it runs a widget or inserts
                  the key, nothing receives it next) has no "delivered";
                  "reached" is then required ("fired" = the fired check
                  already proves it). Every scenario needs "delivered" or
                  "reached"; observation scenarios need "delivered".
    expect        optional "fired": the bind must have fired. When the "fired"
                  check is false the run is a LAB ERROR (retried, never a
                  pass), not "not fired, delivered -> pass": a bound scenario
                  whose bind silently did not register proves nothing.
    whykey        {"args": [...], "layer": name or [names], "env": {...},
                   "via": {"layer": NAME, "summary": text, "binding": text}
                   (a pre-compositor remapper must have redirected the key:
                   that layer's entry is Redirected, with the texts in its
                   summary and binding action), "not_via": NAME (that layer
                   must be absent from the path: it passed the key silently),
                   "run": "direct" | "pty" | "background", "command": "whykey",
                   "accept": [...], "summary": "text", "detail": "text",
                   "binding": "text", "observation": bool, "combo": {...},
                   "stderr": "text", "confidence": "label", "silent": bool,
                   "suppress": bool}
                  "pty" types the command into the scenario's shell.
                  "silent": the capture backend cannot see this key (Sway IPC
                  reports only keys that ran a binding), so the pass condition
                  is no report at all: whykey exits non-zero with the "stderr"
                  text (its capture timed out). Needs an unfired ground truth
                  that still reached the session, so the key is not lost.
                  "suppress": "consumed" | "pass": the key was held by
                  `listen --suppress` (fired false, not delivered, reached),
                  and whykey must report this normal-route verdict, which
                  the hold does not change. "pass" also needs a "restored"
                  check proving the key reaches the client after whykey
                  exits; "consumed" is backed by the same bind firing after
                  exit (hyprland-suppress-restored).
                  ground_truth "restored" (optional CHECK, evaluated after a
                  `wait_whykey` action) must hold: e.g. the compositor submap
                  is back to its original state.
                  "background" starts whykey (`listen`) before the actions or
                  at a start_whykey action, waits for "ready" ("window": its
                  probe window has focus; {"stderr": regex}), and collects its
                  output after the ground truth; "tty": true gives it a
                  controlling terminal (`script`).
                  "summary": substring the layer's summary must contain, and
                  "detail" one of its evidence lines must contain, so a right
                  verdict for the wrong binding still fails. "binding":
                  substring of the layer's primary binding action (the
                  bind that decides the key, not every listed match).
                  "observation": judge `whykey listen`'s observation instead
                  of a layer: "delivered" (the key reached its capture point)
                  or "not-delivered" (only modifiers arrived) against the
                  ground truth "delivered"; no "layer" needed.
                  "combo": the observation's combo must equal this
                  ({"key": "X", "modifiers": 64}); "stderr": substring of
                  whykey's stderr (e.g. why a capture backend was skipped);
                  "confidence": the report's assessment.confidence must equal
                  this (e.g. "conditional").
                  "accept": honest-uncertainty verdicts tolerated when the
                  scenario documents that whykey cannot know more (e.g. no
                  shell-init): "handled-uncertain" only when the truth fired,
                  "uncertain-continues" only when it did not. Shown as PASS~,
                  counted apart from matches and never lab-verified in the
                  support matrix.

Ground truth -> accepted whykey verdicts for the layer:
    fired, not delivered        consumed
    fired, delivered            handled-passed
    not fired, delivered        pass
    not fired, not delivered,   none: something whykey does not model
      reached                     withheld the key (FAIL)
    last consumer, fired        consumed
    last consumer, not fired,   pass (the layer read the key and left it
      reached                     unhandled)
A scenario where nothing observed the key (not fired, not delivered, not
reached) is a lab error: a broken injection or an unfocused probe must never
pass. Observation scenarios follow the same positive-control rule.
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

from envs import ENVIRONMENTS

LAB = Path(__file__).resolve().parent
ROOT = LAB.parent
sys.path.insert(0, str(ROOT / "tools"))
import lab_images  # noqa: E402  (tools/lab_images.py: the image layout, lab/images.json)

SCENARIOS = LAB / "scenarios"
# Environment -> group image (lab/images.json is the single source of truth).
MANIFEST = lab_images.load_manifest()
ENV_IMAGE = lab_images.environment_images(MANIFEST)
IMAGE_NAMES = set(lab_images.image_names(MANIFEST))
DEFAULT_IMAGE_PREFIX = lab_images.DEFAULT_PREFIX
RESULT_MARKER = "@@WHYKEY-LAB-RESULT@@"
TIERS = {"observed"}
# Scenario "expect": what must have happened for the run to count (see the docstring).
EXPECTATIONS = {"fired"}
REQUIRED = {"id", "description", "tier", "environment", "ground_truth", "whykey"}
# VM environments (Hyprland) boot a guest inside the container (lab/driver.py).
SCENARIO_TIMEOUT = 180
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
        if "image" in scenario and scenario["image"] not in IMAGE_NAMES:
            raise SystemExit(f"{path}: unknown image {scenario['image']!r}; known: {sorted(IMAGE_NAMES)}")
        if scenario["tier"] not in TIERS:
            raise SystemExit(f"{path}: unknown tier {scenario['tier']!r}")
        if scenario.get("expect", "fired") not in EXPECTATIONS:
            raise SystemExit(f"{path}: unknown expect {scenario['expect']!r}; known: {sorted(EXPECTATIONS)}")
        allowed = HONEST_UNCERTAINTY[True] | HONEST_UNCERTAINTY[False]
        if not set(scenario["whykey"].get("accept", [])) <= allowed:
            raise SystemExit(f"{path}: whykey.accept may only list {sorted(allowed)}")
        truth = scenario["ground_truth"]
        if "delivered" not in truth and "reached" not in truth:
            raise SystemExit(
                f"{path}: ground_truth needs \"delivered\" (what lies past the layer saw the "
                "key) or, for a last consumer, \"reached\" (the layer received it)"
            )
        if scenario["whykey"].get("observation") and "delivered" not in truth:
            raise SystemExit(f"{path}: an observation scenario needs ground_truth.delivered")
        seen.add(identifier)
        relative = path.relative_to(SCENARIOS).as_posix()
        if patterns and not any(
            fnmatch.fnmatch(identifier, pattern) or fnmatch.fnmatch(relative, pattern)
            for pattern in patterns
        ):
            continue
        scenarios.append((path, scenario))
    return scenarios


class PositiveControlFailed(Exception):
    """The key was observed nowhere: the scenario proves nothing about whykey."""


def expected_verdicts(truth: dict) -> tuple[str, set[str]]:
    """Ground truth -> (label, whykey verdicts it accepts). Raises
    PositiveControlFailed when nothing observed the key at all."""
    fired, delivered, reached = truth["fired"], truth["delivered"], truth.get("reached")
    if delivered is None:
        # The layer is the key's last consumer (a shell's line editor):
        # whether it ran a binding or inserted the key, nothing receives it next.
        if fired:
            return "consumed (last consumer)", {"consumed"}
        if reached:
            return "reached, unhandled", {"pass"}
        raise PositiveControlFailed("the key did not fire and the reached check failed")
    if fired and delivered is False:
        return "consumed", {"consumed"}
    if fired:
        return "handled-passed", {"handled-passed"}
    if delivered:
        return "passed", {"pass"}
    if reached:
        # The key arrived but neither the layer nor anything past it saw it
        # act: something whykey does not model withheld it.
        return "not fired, withheld", set()
    raise PositiveControlFailed("the key neither fired nor was delivered past the layer")


def suppressed_verdicts(truth: dict, expected: str) -> tuple[str, set[str]]:
    """`whykey.suppress`: the capture held the key (`listen --suppress`). The
    bind must not have run and the key must not have reached the client;
    whykey must then report the normal route the scenario declares
    (`expected`), because the hold is whykey's own doing. "pass" is only
    earned when a "restored" check shows the key reaching the client after
    whykey exits. A bind that ran anyway is a mismatch, never a tolerated
    uncertainty."""
    if expected not in ("consumed", "pass"):
        raise ValueError(f'whykey.suppress must be "consumed" or "pass", not {expected!r}')
    if truth["fired"]:
        return "bind ran despite suppression", set()
    if expected == "pass" and truth.get("restored") is None:
        raise PositiveControlFailed('suppress "pass" needs a "restored" check after whykey exits')
    if truth["delivered"] is False and truth.get("reached"):
        return f"held by suppression; normally {expected}", {expected}
    return expected_verdicts(truth)


def report_path(report: dict | None) -> list[dict]:
    """The layer path of a report; for a key sequence (`steps`), the last
    key's path, which carries the sequence's verdict."""
    if not report:
        return []
    # `whykey extension` reports carry a single layer.
    if isinstance(report.get("extension"), dict) and "layer" in report["extension"]:
        return [report["extension"]["layer"]]
    if "path" in report:
        return report["path"]
    steps = report.get("steps") or []
    return steps[-1].get("path", []) if steps else []


def via_check(spec: dict | None, report: dict | None) -> tuple[bool, str]:
    """`whykey.via`: an upstream layer that must have redirected the key
    (a pre-compositor remapper). {"layer": NAME, "summary": text,
    "binding": text}: the layer's entry in the path must have propagation
    Redirected, and its summary and binding action must contain the texts.
    The scenario's verdict is still the layer named by "layer"."""
    if not spec:
        return True, ""
    for entry in report_path(report or {}):
        if entry.get("layer") != spec["layer"]:
            continue
        texts = [entry.get("summary") or ""] + [item.get("text", "") for item in entry.get("evidence", [])]
        action = ((entry.get("binding") or {}).get("action")) or ""
        ok = (
            entry.get("propagation") == "Redirected"
            and (spec.get("summary") is None or any(spec["summary"] in text for text in texts))
            and (spec.get("binding") is None or spec["binding"] in action)
        )
        return ok, f"{spec['layer']} redirects the key (binding {action!r})"
    return False, f"no {spec['layer']} layer redirected the key"


def layer_verdict(report: dict | None, layers: list[str]) -> tuple[str, dict | None]:
    if report is None:
        return "error", None
    for entry in report_path(report):
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


def image_for(args, scenario: dict) -> str:
    """The docker image reference a scenario runs in. --image forces one
    reference for every scenario; otherwise the scenario's group image
    (its "image" field, else its environment's) as <prefix><name>:<tag>, where
    --image-tag "hash" resolves each image's content hash."""
    if args.image:
        return args.image
    name = scenario.get("image") or ENV_IMAGE[scenario["environment"]]
    tag = args.image_tag
    if tag == "hash":
        tag = lab_images.content_hash(name, MANIFEST)
    return f"{args.image_prefix}{name}:{tag}"


def docker_command(args, path: Path, scenario: dict, name: str, keep: bool) -> list[str]:
    command = ["docker", "run", "--name", name, "--network", "none", "--init"]
    command += ["-d"] if keep else ["--rm"]
    docker_args = getattr(ENVIRONMENTS[scenario["environment"]], "docker_args", None)
    if docker_args:
        command += docker_args()
    command += [
        "-v", f"{LAB}:/lab:ro",
        "-v", f"{args.binary}:/w/whykey:ro",
        "-e", f"WHYKEY_LAB_KEEP={'1' if keep else '0'}",
        image_for(args, scenario),
        "python3", "/lab/driver.py", f"/lab/{path.relative_to(LAB).as_posix()}",
    ]
    return command


def run_container(args, path: Path, scenario: dict, copy: int = 0) -> dict:
    name = f"whykey-lab-{os.getpid()}-{scenario['id']}" + (f"-{copy}" if copy else "")
    command = docker_command(args, path, scenario, name, args.keep)
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


def observation_verdict(report: dict | None) -> tuple[str, dict | None]:
    """What `whykey listen` observed: the key reached its capture point
    ("delivered"), or only modifiers did ("not-delivered")."""
    if report is None:
        return "error", None
    observation = report.get("observation")
    if not isinstance(observation, dict):
        return "absent", None
    if observation.get("disposition") == "not_delivered":
        return "not-delivered", observation
    return "delivered", observation


def truth_evidence(truth: dict) -> str:
    return "; ".join(
        truth[key] for key in ("fired_evidence", "delivered_evidence", "reached_evidence", "restored_evidence")
        if truth.get(key)
    )


def evaluate_silent(record: dict, scenario: dict, truth: dict, whykey: dict, result: dict) -> dict:
    """`whykey.silent`: the backend cannot see this key, so whykey must give no
    report and stop with the scenario's `stderr` text (its capture timed out).
    The key must have reached the session (positive control) and nothing may
    have fired; anything else is a lab error, never a pass."""
    reached = bool(truth["delivered"] or truth.get("reached"))
    if truth["fired"] or not reached:
        record.update(ground_truth={"label": "no positive control", **truth}, expected=None,
                      whykey_verdict="lab-error")
        record["pass"] = False
        record["error"] = (f"silent scenario needs an unfired key that reached the session "
                           f"({truth_evidence(truth)})")
        record["logs"] = result.get("logs")
        return record
    verdict = "silent" if whykey["report"] is None else "reported"
    stderr = scenario["whykey"].get("stderr")
    passed = (verdict == "silent" and (stderr is None or stderr in whykey["stderr"])
              and truth.get("restored", True))
    record.update(
        ground_truth={"label": "not fired, reached", **truth},
        expected=["silent"],
        expected_summary=f"stderr containing {stderr!r}" if stderr else "",
        whykey_verdict=verdict,
        whykey_layer=None,
    )
    record["pass"] = passed
    record["tolerated"] = False
    if not passed:
        record["whykey_stderr"] = whykey["stderr"][-2000:] or None
        if whykey.get("stdout"):
            record["whykey_stdout"] = whykey["stdout"][-2000:]
        record["logs"] = result.get("logs")
    if result.get("container"):
        record["container"] = result["container"]
    return record


def evaluate(path: Path, scenario: dict, result: dict) -> dict:
    observing = scenario["whykey"].get("observation", False)
    layers = scenario["whykey"].get("layer", [])
    layers = [layers] if isinstance(layers, str) else layers
    record = {
        "scenario": scenario["id"],
        "file": path.relative_to(ROOT).as_posix(),
        "environment": scenario["environment"],
        "tier": scenario["tier"],
        "layer": layers[0] if layers else "observation",
        "whykey_args": scenario["whykey"]["args"],
    }
    if "error" in result:
        record.update(ground_truth=None, expected=None, whykey_verdict="lab-error")
        record["pass"] = False
        record["error"] = result["error"]
        record["logs"] = result.get("logs") or result.get("output")
        return record
    truth = result["ground_truth"]
    whykey = result["whykey"]
    if scenario.get("expect") == "fired" and not truth["fired"]:
        # The bind did not register or did not run: nothing about whykey is known.
        record.update(ground_truth={"label": "expected to fire, did not", **truth},
                      expected=None, whykey_verdict="lab-error")
        record["pass"] = False
        record["error"] = (
            f"the scenario expects the binding to fire, but it did not ({truth['fired_evidence']})"
        )
        record["logs"] = result.get("logs")
        return record
    if scenario["whykey"].get("silent"):
        return evaluate_silent(record, scenario, truth, whykey, result)
    summary = scenario["whykey"].get("summary")
    detail = scenario["whykey"].get("detail")
    combo = scenario["whykey"].get("combo")
    confidence = scenario["whykey"].get("confidence")
    observed = (whykey["report"] or {}).get("observation")
    combo_ok = combo is None or (isinstance(observed, dict) and observed.get("combo") == combo)
    reported_confidence = ((whykey["report"] or {}).get("assessment") or {}).get("confidence")
    confidence_ok = confidence is None or reported_confidence == confidence
    try:
        if observing:
            if not (truth["fired"] or truth["delivered"] or truth.get("reached")):
                raise PositiveControlFailed("the key neither fired, nor reached the probe, nor passed the reached check")
            label = "delivered" if truth["delivered"] else "not-delivered"
            accepted = {label}
        else:
            label, accepted = expected_verdicts(truth)
            if scenario["whykey"].get("suppress"):
                label, accepted = suppressed_verdicts(truth, scenario["whykey"]["suppress"])
    except PositiveControlFailed as failure:
        record.update(ground_truth={"label": "no positive control", **truth}, expected=None, whykey_verdict="lab-error")
        record["pass"] = False
        record["error"] = f"positive control failed: {failure} ({truth_evidence(truth)})"
        record["logs"] = result.get("logs")
        return record
    if observing:
        tolerated: set[str] = set()
        verdict, entry = observation_verdict(whykey["report"])
        summary_ok = True
    else:
        verdict, entry = layer_verdict(whykey["report"], layers)
        tolerated = set(scenario["whykey"].get("accept", [])) & HONEST_UNCERTAINTY[truth["fired"]]
        texts = [] if entry is None else [entry.get("summary") or ""] + [
            item.get("text", "") for item in entry.get("evidence", [])
        ]
        binding = scenario["whykey"].get("binding")
        reported_binding = ((entry or {}).get("binding") or {}).get("action") or ""
        summary_ok = (summary is None or (entry is not None and summary in texts[0])) and (
            detail is None or any(detail in text for text in texts)
        ) and (binding is None or binding in reported_binding)
        via_ok, via_text = via_check(scenario["whykey"].get("via"), whykey["report"])
        summary_ok = summary_ok and via_ok
        not_via = scenario["whykey"].get("not_via")
        if not_via is not None:
            # A passing layer is omitted from the report path, so the layer must
            # be absent: any entry (a redirect, a consume, or an uncertain
            # "may change") means whykey claims the layer acts on the key.
            claimed = [
                entry.get("summary") or ""
                for entry in report_path(whykey["report"])
                if entry.get("layer") == not_via
            ]
            summary_ok = summary_ok and not claimed
            via_text = f"{via_text}; " if via_text else ""
            via_text += f"{not_via} passes the key" + (f" (reported: {claimed[0]})" if claimed else "")
        if binding is not None:
            detail = f"{detail}; " if detail else ""
            detail += f"binding action containing {binding!r} (reported {reported_binding!r})"
        if via_text:
            detail = f"{detail}; " if detail else ""
            detail += via_text
    stderr = scenario["whykey"].get("stderr")
    stderr_ok = stderr is None or stderr in whykey["stderr"]
    if combo is not None:
        summary = f"{summary}; " if summary else ""
        summary += f"observed combo {json.dumps(combo)}"
    if stderr is not None:
        summary = f"{summary}; " if summary else ""
        summary += f"stderr containing {stderr!r}"
    if confidence is not None:
        summary = f"{summary}; " if summary else ""
        summary += f"confidence {confidence!r} (reported {reported_confidence!r})"
    restored_ok = truth.get("restored", True)
    passed = (
        (verdict in accepted or verdict in tolerated)
        and summary_ok and combo_ok and stderr_ok and confidence_ok and restored_ok
    )
    record.update(
        ground_truth={"label": label, **truth},
        expected=sorted(accepted | tolerated),
        expected_summary=summary,
        expected_detail=detail,
        whykey_verdict=verdict,
        whykey_layer=entry,
    )
    record["pass"] = passed
    record["tolerated"] = passed and verdict in tolerated
    if not passed:
        report = whykey["report"]
        record["whykey_layers"] = [
            f"{item.get('layer')}: {item.get('status')}/{item.get('propagation')} - {item.get('summary')}"
            for item in report_path(report)
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
    parser.add_argument("--vms", type=int,
                        default=max(1, (os.cpu_count() or 1) // (3 if os.path.exists("/dev/kvm") else 4)),
                        help="VM scenarios (Hyprland; 2 vCPUs each) run at once, within --jobs "
                             "(default: a third of the CPUs with KVM, a quarter without; "
                             "starved guests time out)")
    parser.add_argument("--keep", action="store_true",
                        help="leave each container running after the scenario for docker exec")
    parser.add_argument("--image", metavar="REF", default=None,
                        help="run every scenario in this docker image reference, as is (overrides "
                             "each scenario's group image, --image-prefix and --image-tag; e.g. the "
                             "old single image 'whykey-lab')")
    parser.add_argument("--image-prefix", default=DEFAULT_IMAGE_PREFIX, metavar="P",
                        help=f"prefix of the group images, <P><name>:<tag> (default {DEFAULT_IMAGE_PREFIX!r})")
    parser.add_argument("--image-tag", default="latest", metavar="TAG",
                        help="tag of the group images (default: latest); 'hash' uses each image's "
                             "content hash (tools/lab_images.py hash NAME)")
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "release" / "whykey")
    parser.add_argument("--results", type=Path, default=LAB / "results.json")
    parser.add_argument("--repeat", type=int, default=1, metavar="N",
                        help="run every selected scenario N times at once (flake hunting)")
    parser.add_argument("--retries", type=int, default=2, metavar="N",
                        help="rerun a scenario up to N times, one at a time, when the lab "
                             "could not establish ground truth (lab error); a mismatch "
                             "with reality is never retried")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    if not args.binary.is_file():
        raise SystemExit(f"{args.binary} not found; run `cargo build --release --locked`")
    scenarios = load_scenarios(args.only)
    if not scenarios:
        raise SystemExit("no scenario matched")

    records: list[dict] = []
    jobs = max(1, args.jobs)
    vms = max(1, min(args.vms, jobs))
    with concurrent.futures.ThreadPoolExecutor(max_workers=vms) as vm_pool, \
            concurrent.futures.ThreadPoolExecutor(max_workers=max(1, jobs - vms)) as pool:
        futures = {
            (vm_pool if getattr(ENVIRONMENTS[scenario["environment"]], "VM", None) else pool).submit(
                run_container, args, path, scenario, copy
            ): (path, scenario)
            for path, scenario in scenarios
            for copy in range(max(1, args.repeat))
        }
        for future in concurrent.futures.as_completed(futures):
            path, scenario = futures[future]
            record = evaluate(path, scenario, future.result())
            record["image"] = image_for(args, scenario)
            records.append(record)
            status = ("PASS~" if record.get("tolerated") else "PASS") if record["pass"] else "FAIL"
            print(f"[{status}] {record['scenario']}", file=sys.stderr, flush=True)

    # A lab error means the oracle failed (slow start, lost first key), not
    # that whykey disagreed with reality. Rerun those alone, without load.
    loaded = {path.relative_to(ROOT).as_posix(): (path, scenario) for path, scenario in scenarios}
    for attempt in range(1, max(0, args.retries) + 1):
        errored = [record for record in records if record.get("error")]
        if not errored:
            break
        for record in errored:
            path, scenario = loaded[record["file"]]
            retry = evaluate(path, scenario, run_container(args, path, scenario, 0))
            retry["retried"] = attempt
            retry["image"] = image_for(args, scenario)
            records[records.index(record)] = retry
            status = ("PASS~" if retry.get("tolerated") else "PASS") if retry["pass"] else "FAIL"
            print(f"[{status}] {retry['scenario']} (retry {attempt})", file=sys.stderr, flush=True)

    records.sort(key=lambda record: record["file"])
    args.results.write_text(json.dumps(records, indent=2) + "\n", encoding="utf-8")
    print()
    print_table(records)
    failed = [record for record in records if not record["pass"]]
    tolerated = [record for record in records if record["pass"] and record.get("tolerated")]
    print(
        f"\n{len(records) - len(failed) - len(tolerated)}/{len(records)} scenarios match reality; "
        f"{len(tolerated)} only tolerated (PASS~: whykey honestly answered uncertain; "
        f"not lab-verified); {len(failed)} failed; results: {args.results}"
    )
    for record in tolerated:
        print(f"  PASS~ {record['scenario']}: {record['whykey_verdict']} where reality was "
              f"{record['ground_truth']['label']}")
    for record in failed:
        print(f"\nFAIL {record['scenario']} ({record['file']})")
        if record.get("error"):
            print(f"  lab error: {record['error']}")
            continue
        truth = record["ground_truth"]
        print(f"  ground truth: {truth['label']} ({truth_evidence(truth)})")
        expected = "|".join(record["expected"]) or "no verdict (the key was withheld by something whykey does not model)"
        print(f"  whykey {' '.join(record['whykey_args'])}: {record['whykey_verdict']}, expected {expected}")
        if record.get("expected_summary"):
            print(f"  whykey must also report: {record['expected_summary']!r}")
        if record.get("expected_detail"):
            print(f"  layer evidence must contain: {record['expected_detail']!r}")
        for line in record.get("whykey_layers", []):
            print(f"    {line}")
        if record.get("whykey_stderr"):
            print(f"  stderr: {record['whykey_stderr'].strip()}")
    if args.keep:
        print("\nkept containers:", " ".join(r["container"] for r in records if r.get("container")))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
