#!/usr/bin/env python3
"""Select the reality-lab scenarios a change can affect, and split them into CI shards (stdlib only).

    git diff --name-only BASE...HEAD | python3 tools/lab_select.py
    python3 tools/lab_select.py --full
    python3 tools/lab_select.py --changed FILE --github-output "$GITHUB_OUTPUT" --summary "$GITHUB_STEP_SUMMARY"

Changed paths come one per line on stdin (or from --changed FILE). The rules
are lab/affected.json (its "_about" key documents them). The result is JSON on
stdout:

    {"full": bool, "selected": N, "total": N, "reasons": [...], "shards": [...]}

Each shard is {"name", "image", "shard": "i/n", "only": null | [globs], "count",
"args"}: "args" are the lab/run.py arguments that run exactly this shard
(--group IMAGE --shard i/n [--only GLOB]...). "only" is null when the shard's
image is run whole. Shards split the selected scenarios of an image, sorted by
id, round-robin; lab/run.py --shard uses the same split (shard_slice).

With --github-output, the file gets count=<shards>, shards=<json>, full=,
selected=, total=. --summary writes a Markdown table for the job summary.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import math
import re
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))
import lab_images  # noqa: E402  (tools/lab_images.py: lab/images.json, environment -> image)

LAB = ROOT / "lab"
SCENARIOS = LAB / "scenarios"
AFFECTED = LAB / "affected.json"
BASE = lab_images.BASE


class SelectError(Exception):
    """lab/affected.json or the scenario files are inconsistent."""


def glob_regex(pattern: str) -> re.Pattern:
    """A path glob: * and ? stay in one segment, ** crosses segments ("**/" may match none)."""
    out = []
    index = 0
    while index < len(pattern):
        if pattern.startswith("**/", index):
            out.append("(?:.*/)?")
            index += 3
        elif pattern.startswith("**", index):
            out.append(".*")
            index += 2
        elif pattern[index] == "*":
            out.append("[^/]*")
            index += 1
        elif pattern[index] == "?":
            out.append("[^/]")
            index += 1
        else:
            out.append(re.escape(pattern[index]))
            index += 1
    return re.compile("^" + "".join(out) + "$")


def shard_slice(items: list, index: int, count: int) -> list:
    """Shard `index` (1-based) of `count` of items already sorted by id: round-robin,
    so slow and fast scenarios spread over the shards. lab/run.py --shard uses this too."""
    if not 1 <= index <= count:
        raise ValueError(f"shard {index}/{count}: index must be between 1 and {count}")
    return items[index - 1::count]


def parse_shard(text: str) -> tuple[int, int]:
    match = re.fullmatch(r"(\d+)/(\d+)", text)
    if not match:
        raise ValueError(f"--shard expects I/N, not {text!r}")
    index, count = int(match.group(1)), int(match.group(2))
    if count < 1 or not 1 <= index <= count:
        raise ValueError(f"--shard {text}: need 1 <= I <= N")
    return index, count


@dataclass(frozen=True)
class Scenario:
    rel: str           # lab/scenarios-relative path, e.g. "sway/sway-plain-bind.json"
    id: str
    environment: str
    image: str         # the group image it runs in (its "image" field, else its environment's)


@dataclass(frozen=True)
class Rule:
    paths: tuple[re.Pattern, ...]
    scenarios: tuple[str, ...]


@dataclass
class Config:
    shard_size: dict[str, int]
    core: tuple[re.Pattern, ...]
    ignore: tuple[re.Pattern, ...]
    rules: tuple[Rule, ...]


def load_scenarios() -> list[Scenario]:
    manifest = lab_images.load_manifest()
    env_image = lab_images.environment_images(manifest)
    found = []
    for path in sorted(SCENARIOS.rglob("*.json")):
        scenario = json.loads(path.read_text(encoding="utf-8"))
        environment = scenario.get("environment")
        if environment not in env_image:
            raise SelectError(f"{path}: environment {environment!r} is in no image (lab/images.json)")
        image = scenario.get("image") or env_image[environment]
        found.append(Scenario(path.relative_to(SCENARIOS).as_posix(), scenario["id"], environment, image))
    return found


def load_config(scenarios: list[Scenario]) -> Config:
    data = json.loads(AFFECTED.read_text(encoding="utf-8"))
    manifest = lab_images.load_manifest()
    groups = set(manifest["images"])
    sizes = data.get("shard_size", {})
    if not isinstance(sizes.get("default"), int) or sizes["default"] < 1:
        raise SelectError('lab/affected.json: shard_size needs an integer "default"')
    for name in sizes:
        if not name.startswith("_") and name != "default" and name not in groups:
            raise SelectError(f"lab/affected.json: shard_size names unknown image {name!r}")

    def compile_globs(patterns: list[str], where: str) -> tuple[re.Pattern, ...]:
        for pattern in patterns:
            if not isinstance(pattern, str) or not pattern:
                raise SelectError(f"lab/affected.json: {where} has a bad pattern {pattern!r}")
        return tuple(glob_regex(pattern) for pattern in patterns)

    core = compile_globs(data.get("core", []), "core")
    ignore = compile_globs(data.get("ignore", []), "ignore")
    rules = []
    for index, rule in enumerate(data.get("rules", [])):
        paths = compile_globs(rule.get("paths", []), f"rule {index}")
        if not paths:
            raise SelectError(f"lab/affected.json: rule {index} has no paths")
        tokens = tuple(rule.get("scenarios", []))
        for token in tokens:
            if token.startswith("@"):
                if token not in ("@self", "@env", "@image", "@family"):
                    raise SelectError(f"lab/affected.json: rule {index} has unknown token {token!r}")
            elif token.startswith("image:"):
                if token[len("image:"):] not in groups:
                    raise SelectError(f"lab/affected.json: rule {index} names unknown image {token!r}")
            elif not any(fnmatch.fnmatch(s.id, token) or fnmatch.fnmatch(s.rel, token) for s in scenarios):
                raise SelectError(f"lab/affected.json: rule {index} glob {token!r} matches no scenario")
        rules.append(Rule(paths, tokens))
    return Config(dict(sizes), core, ignore, tuple(rules))


def matches(patterns: tuple[re.Pattern, ...], path: str) -> bool:
    return any(pattern.match(path) for pattern in patterns)


class Unmapped(Exception):
    """A changed file whose scenarios the rules cannot name: everything runs."""


def token_hits(token: str, path: str, scenarios: list[Scenario], manifest: dict) -> list[tuple[int, str]]:
    """(scenario index, the pattern that selected it) for one rule token."""
    everything = [(i, s.rel) for i, s in enumerate(scenarios)]
    if token == "@self":
        match = re.match(r"^lab/scenarios/([^/]+/[^/]+\.json)$", path)
        if not match:
            return []
        return [(i, s.rel) for i, s in enumerate(scenarios) if s.rel == match.group(1)]
    if token == "@env":
        name = Path(path).stem
        return [(i, s.rel) for i, s in enumerate(scenarios) if s.environment == name]
    if token == "@image":
        name = Path(path).name.split(".")[0]
        if name == BASE:
            return everything
        if name not in manifest["images"]:
            raise Unmapped(f"{path}: no image named {name!r}")
        return [(i, s.rel) for i, s in enumerate(scenarios) if s.image == name]
    if token == "@family":
        family = Path(path).name.split(".")[0]
        if family in manifest["base"]:
            return everything  # every group image is built FROM the base
        for name, spec in manifest["images"].items():
            if family in spec["families"]:
                return [(i, s.rel) for i, s in enumerate(scenarios) if s.image == name]
        raise Unmapped(f"{path}: package family {family!r} is in no image")
    if token.startswith("image:"):
        name = token[len("image:"):]
        return [(i, s.rel) for i, s in enumerate(scenarios) if s.image == name]
    return [(i, token) for i, s in enumerate(scenarios)
            if fnmatch.fnmatch(s.id, token) or fnmatch.fnmatch(s.rel, token)]


def resolve(changed: list[str], scenarios: list[Scenario], config: Config,
            manifest: dict) -> tuple[bool, dict[int, set[str]], list[str]]:
    """(full, selected {scenario index: patterns that chose it}, reasons)."""
    selected: dict[int, set[str]] = defaultdict(set)
    reasons: list[str] = []
    for path in changed:
        if matches(config.core, path):
            reasons.append(f"{path}: shared by every scenario")
            return True, {}, reasons
        hits = [rule for rule in config.rules if matches(rule.paths, path)]
        if hits:
            names = []
            try:
                for rule in hits:
                    for token in rule.scenarios:
                        for index, pattern in token_hits(token, path, scenarios, manifest):
                            selected[index].add(pattern)
                        names.append(token)
            except Unmapped as error:
                reasons.append(f"{error}: unmapped, every scenario")
                return True, {}, reasons
            if names:
                reasons.append(f"{path}: {', '.join(sorted(set(names)))}")
            else:
                reasons.append(f"{path}: no lab scenario covers it")
            continue
        if matches(config.ignore, path):
            reasons.append(f"{path}: cannot change a lab verdict")
            continue
        reasons.append(f"{path}: not mapped in lab/affected.json, every scenario")
        return True, {}, reasons
    return False, selected, reasons


def make_plan(changed: list[str] | None, full: bool) -> dict:
    manifest = lab_images.load_manifest()
    scenarios = load_scenarios()
    config = load_config(scenarios)
    if full:
        is_full, selected, reasons = True, {}, ["--full: every scenario"]
    else:
        is_full, selected, reasons = resolve(changed or [], scenarios, config, manifest)
    if is_full:
        selected = {i: {s.rel} for i, s in enumerate(scenarios)}
    shards = []
    images = {}
    for image in manifest["images"]:
        members = [i for i, s in enumerate(scenarios) if s.image == image]
        if not members:
            continue
        chosen = sorted((i for i in members if i in selected), key=lambda i: scenarios[i].id)
        images[image] = {"selected": len(chosen), "total": len(members), "shards": 0}
        if not chosen:
            continue
        limit = config.shard_size.get(image, config.shard_size["default"])
        count = math.ceil(len(chosen) / limit)
        only = None if len(chosen) == len(members) else sorted(
            set().union(*(selected[i] for i in chosen)))
        images[image]["shards"] = count
        for index in range(1, count + 1):
            args = ["--group", image, "--shard", f"{index}/{count}"]
            for pattern in only or []:
                args += ["--only", pattern]
            shards.append({
                "name": f"{image}-{index}of{count}",
                "image": image,
                "shard": f"{index}/{count}",
                "only": only,
                "count": len(shard_slice(chosen, index, count)),
                "args": args,
            })
    return {
        "full": is_full,
        "selected": sum(info["selected"] for info in images.values()),
        "total": len(scenarios),
        "images": images,
        "reasons": reasons,
        "shards": shards,
    }


def write_outputs(plan: dict, github_output: Path | None, summary: Path | None) -> None:
    if github_output:
        with github_output.open("a", encoding="utf-8") as handle:
            handle.write(f"count={len(plan['shards'])}\n")
            handle.write(f"shards={json.dumps(plan['shards'], separators=(',', ':'))}\n")
            handle.write(f"full={'true' if plan['full'] else 'false'}\n")
            handle.write(f"selected={plan['selected']}\n")
            handle.write(f"total={plan['total']}\n")
    if summary:
        lines = ["## Lab scenario selection", ""]
        if plan["selected"] == 0:
            lines.append("No lab scenario is affected by this change; nothing to run.")
        else:
            lines.append(f"{plan['selected']} of {plan['total']} scenarios"
                         + (" (full run)" if plan["full"] else "") + ".")
            lines += ["", "| image | selected | total | shards |", "| --- | ---: | ---: | ---: |"]
            for image, info in plan["images"].items():
                lines.append(f"| {image} | {info['selected']} | {info['total']} | {info['shards']} |")
        lines += ["", "Reasons:"] + [f"- {reason}" for reason in plan["reasons"][:200]]
        with summary.open("a", encoding="utf-8") as handle:
            handle.write("\n".join(lines) + "\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--full", action="store_true", help="select every scenario")
    parser.add_argument("--changed", type=Path, metavar="FILE",
                        help="changed paths, one per line (default: stdin)")
    parser.add_argument("--github-output", type=Path, metavar="FILE",
                        help="append count=, shards=, full=, selected=, total= (GITHUB_OUTPUT)")
    parser.add_argument("--summary", type=Path, metavar="FILE",
                        help="append a Markdown summary (GITHUB_STEP_SUMMARY)")
    args = parser.parse_args(argv)
    changed: list[str] | None = None
    if not args.full:
        text = args.changed.read_text(encoding="utf-8") if args.changed else sys.stdin.read()
        changed = [line.strip().removeprefix("./") for line in text.splitlines() if line.strip()]
    try:
        plan = make_plan(changed, args.full)
    except (SelectError, lab_images.ImageError, ValueError) as error:
        print(f"lab_select: {error}", file=sys.stderr)
        return 2
    write_outputs(plan, args.github_output, args.summary)
    print(json.dumps(plan, indent=2))
    print(f"lab_select: {plan['selected']} of {plan['total']} scenarios in {len(plan['shards'])} shards",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
