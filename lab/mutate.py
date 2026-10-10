#!/usr/bin/env python3
"""Mutation check: which lab scenarios catch each deliberate whykey bug.

    python3 lab/mutate.py MUTATIONS.json [--only GLOB]... [-j N] [--vms N]

MUTATIONS.json is a list of {"name", "file", "search", "replace"} objects;
"search" must occur exactly once in "file" (repo-relative). The tool copies
the tracked sources once into target/mutate/src, then per mutation: restores
the pristine file, applies the replacement, cleans the whykey crate (cargo
otherwise may keep a stale artifact from the previous mutant), builds with
CARGO_TARGET_DIR=target/mutate/target, checks the binary differs from the
unmutated one, and runs lab/run.py against it. An unmutated baseline run
comes first; scenarios failing there are reported as noise, not catches.
A mutation no scenario catches marks a lab coverage gap.
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

LAB = Path(__file__).resolve().parent
ROOT = LAB.parent
WORK = ROOT / "target" / "mutate"
SRC = WORK / "src"
TARGET = WORK / "target"


def build(name: str) -> Path:
    env = {**os.environ, "CARGO_TARGET_DIR": str(TARGET)}
    subprocess.run(["cargo", "clean", "--release", "-p", "whykey"], cwd=SRC, env=env, check=True,
                   capture_output=True)
    subprocess.run(["cargo", "build", "--release", "--locked", "--quiet"], cwd=SRC, env=env, check=True)
    binary = WORK / "bin" / name
    binary.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(TARGET / "release" / "whykey", binary)
    return binary


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def failing(binary: Path, name: str, lab_args: list[str]) -> tuple[list[str], list[str]]:
    """Scenarios this binary fails, and scenarios the lab could not judge.

    run.py writes its results file only when it finishes, so a stale file
    from an earlier run is removed first and a missing one is fatal. A
    lab-error record (timeout, Docker failure, failed positive control)
    says nothing about the binary and never counts as a failure."""
    results = WORK / f"{name}.json"
    results.unlink(missing_ok=True)
    run = subprocess.run([sys.executable, str(LAB / "run.py"), "--binary", str(binary), "--results", str(results),
                          *lab_args], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if run.returncode not in (0, 1) or not results.exists():
        raise SystemExit(f"{name}: lab/run.py exited with status {run.returncode} without results:\n{run.stderr[-2000:]}")
    records = json.loads(results.read_text(encoding="utf-8"))
    failed = sorted(r["scenario"] for r in records if not r["pass"] and not r.get("error"))
    errors = sorted(r["scenario"] for r in records if r.get("error"))
    return failed, errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("mutations", type=Path)
    parser.add_argument("--only", action="append", default=[], metavar="GLOB")
    parser.add_argument("-j", "--jobs", type=int)
    parser.add_argument("--vms", type=int)
    parser.add_argument("--rev", metavar="REV",
                        help="mutate this git revision instead of the working tree's tracked files")
    args = parser.parse_args()
    mutations = json.loads(args.mutations.read_text(encoding="utf-8"))
    lab_args = [flag for glob in args.only for flag in ("--only", glob)]
    if args.jobs:
        lab_args += ["-j", str(args.jobs)]
    if args.vms:
        lab_args += ["--vms", str(args.vms)]

    if SRC.exists():
        shutil.rmtree(SRC)
    SRC.mkdir(parents=True)
    if args.rev:
        archive = subprocess.run(["git", "archive", args.rev], cwd=ROOT, check=True, capture_output=True).stdout
        subprocess.run(["tar", "-x", "-C", str(SRC)], input=archive, check=True)
    else:
        files = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True).stdout
        for name in filter(None, files.decode().split("\0")):
            source = ROOT / name
            if source.is_file():
                (SRC / name).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, SRC / name)
    for mutation in mutations:
        text = (SRC / mutation["file"]).read_text(encoding="utf-8")
        if text.count(mutation["search"]) != 1:
            raise SystemExit(f"{mutation['name']}: search text must occur exactly once in {mutation['file']}")

    print("baseline: building", flush=True)
    baseline = build("baseline")
    noise, baseline_errors = failing(baseline, "baseline", lab_args)
    noise = set(noise) | set(baseline_errors)
    print(f"baseline: failing {sorted(noise) or 'none'}", flush=True)

    gaps = []
    for mutation in mutations:
        pristine = (SRC / mutation["file"]).read_text(encoding="utf-8")
        target = SRC / mutation["file"]
        target.write_text(pristine.replace(mutation["search"], mutation["replace"]), encoding="utf-8")
        try:
            binary = build(mutation["name"])
        finally:
            target.write_text(pristine, encoding="utf-8")
        if digest(binary) == digest(baseline):
            print(f"{mutation['name']}: binary identical to baseline (mutation compiled away?)", flush=True)
            gaps.append(mutation["name"])
            continue
        failed, errors = failing(binary, mutation["name"], lab_args)
        caught = [s for s in failed if s not in noise]
        unjudged = [s for s in errors if s not in noise]
        print(f"{mutation['name']}: caught by {', '.join(caught) if caught else 'NONE'}"
              + (f"; lab errors (not counted): {', '.join(unjudged)}" if unjudged else ""), flush=True)
        if not caught:
            gaps.append(mutation["name"])
    shutil.rmtree(SRC)
    print(f"\n{len(mutations) - len(gaps)}/{len(mutations)} mutations caught; uncaught: {', '.join(gaps) or 'none'}")
    return 1 if gaps else 0


if __name__ == "__main__":
    sys.exit(main())
