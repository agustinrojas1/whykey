#!/usr/bin/env python3
"""Run the complete, sanitized differential matrix for Whykey binaries."""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests/fixtures/differential"


@dataclass(frozen=True)
class Scenario:
    name: str
    args: tuple[str, ...]
    env: tuple[tuple[str, str], ...] = ()
    normalize_pid: bool = False
    expected_rule: str | None = None
    expected_description: str = ""


@dataclass
class Result:
    code: int | None
    timed_out: bool
    stdout: bytes
    stderr: bytes
    start_error: str | None = None


def scenarios() -> list[Scenario]:
    f = str(FIXTURES)
    cases = [
        ("version", ("--version",)), ("help", ("--help",)),
        ("help inspect", ("inspect", "--help")), ("help listen", ("listen", "--help")),
        ("help doctor", ("doctor", "--help")), ("help capabilities", ("capabilities", "--help")),
        ("capabilities text", ("capabilities",)), ("capabilities json", ("capabilities", "--json")),
        ("capabilities json-v2", ("capabilities", "--json-v2")),
        ("invalid subcommand", ("invalid_cmd",)), ("inspect without arg", ("inspect",)),
        ("replay without arg", ("replay",)), ("extension without arg", ("extension",)),
        ("diff without arg", ("diff",)), ("replay nonexistent", ("replay", "nonexistent_file_404.json")),
        ("diff single file", ("diff", f + "/replay-compositor-consumed.json")),
    ]
    for label, file_name in [
        ("compositor consumed", "replay-compositor-consumed.json"),
        ("terminal consumed", "replay-terminal-consumed.json"),
        ("tty signal", "replay-tty-signal.json"),
        ("shell readline", "replay-shell-readline.json"),
        ("v1 unhandled", "replay-v1-unhandled.json"),
    ]:
        cases += [(f"replay {label} text", ("replay", f + "/" + file_name))]
        cases += [(f"replay {label} json", ("replay", f + "/" + file_name, "--json"))]
    cases += [
        ("diff compositor vs terminal text", ("diff", f + "/replay-compositor-consumed.json", f + "/replay-terminal-consumed.json")),
        ("diff compositor vs terminal json", ("diff", f + "/replay-compositor-consumed.json", f + "/replay-terminal-consumed.json", "--json")),
        ("diff tty vs readline text", ("diff", f + "/replay-tty-signal.json", f + "/replay-shell-readline.json")),
        ("diff tty vs readline json", ("diff", f + "/replay-tty-signal.json", f + "/replay-shell-readline.json", "--json")),
    ]
    return [Scenario(name, args) for name, args in cases]


def inspect_scenarios(path: str, pid: str | None = None) -> list[Scenario]:
    env = tuple(sorted({"PATH": path, "HYPRLAND_INSTANCE_SIGNATURE": "test", "XDG_CURRENT_DESKTOP": "Hyprland", "XDG_SESSION_TYPE": "wayland"}.items()))
    if pid is None:
        return [Scenario("inspect upstream consumption text", ("super+q",), env), Scenario("inspect upstream consumption verbose", ("super+q", "--verbose"), env), Scenario("inspect upstream consumption json-v2", ("super+q", "--json-v2"), env)]
    return [Scenario("inspect uncertain upstream continues text", ("ctrl+c",), env, True), Scenario("inspect uncertain upstream continues json-v2", ("ctrl+c", "--json-v2"), env, True, "json_v2_modifier_ambiguity", "typed ModifierAmbiguity on Hyprland binding in schema-v2")]


def isolated_env(extra: tuple[tuple[str, str], ...], root: Path) -> dict[str, str]:
    env = {"PATH": "/usr/bin:/bin", "HOME": str(root)}
    env.update(dict(extra))
    return env


def stop_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()


def run(binary: str, args: tuple[str, ...], env: dict[str, str], timeout: float) -> Result:
    try:
        process = subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   env=env, start_new_session=True)
    except OSError as exc:
        return Result(None, False, b"", b"", f"{type(exc).__name__}: {exc}")
    try:
        out, err = process.communicate(timeout=timeout)
        return Result(process.returncode, False, out, err)
    except subprocess.TimeoutExpired:
        stop_group(process)
        out, err = process.communicate()
        return Result(124, True, out, err)


def slug(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_-]", "", re.sub(r"[ /:]", "_", name))


def normalize(data: bytes, enabled: bool) -> bytes:
    if not enabled:
        return data
    return re.sub(rb"target (?:pid|process)[: ]+\d+|\(\d+\)", lambda m: re.sub(rb"\d+", b"<PID>", m.group()), data)


def diff_text(a: bytes, b: bytes, path: Path, limit: int) -> str:
    import difflib
    text = "".join(difflib.unified_diff(a.decode(errors="replace").splitlines(True), b.decode(errors="replace").splitlines(True), fromfile="baseline", tofile="candidate"))
    path.write_text(text, encoding="utf-8")
    lines = text.splitlines()
    return "\n".join(lines[:limit]) + (f"\n  ... (diff truncated; full diff at {path})" if len(lines) > limit else "")


def write_metadata(path: Path, result: Result) -> None:
    path.write_text(json.dumps({"exit_code": result.code, "timed_out": result.timed_out,
                                "start_error": result.start_error}, indent=2) + "\n", encoding="utf-8")


def verify_expected(rule: str, base: Path, cand: Path) -> tuple[bool, str]:
    """Approve exactly one typed schema-v2 extension; reject anything else."""
    if rule != "json_v2_modifier_ambiguity":
        return False, f"Unknown expected difference rule: {rule!r}"
    try:
        base_doc = json.loads(base.read_text(encoding="utf-8"), object_pairs_hook=strict_pairs)
    except Exception as exc:
        return False, f"Error parsing baseline JSON '{base}': {exc}"
    try:
        cand_doc = json.loads(cand.read_text(encoding="utf-8"), object_pairs_hook=strict_pairs)
    except Exception as exc:
        return False, f"Error parsing candidate JSON '{cand}': {exc}"
    if cand_doc.get("schema_version") != 2 or base_doc.get("schema_version") != 2:
        return False, (
            "expected schema_version == 2 in both documents, "
            f"got base={base_doc.get('schema_version')!r}, cand={cand_doc.get('schema_version')!r}"
        )
    try:
        cand_layers = cand_doc.get("path")
        base_layers = base_doc.get("path")
        if not isinstance(cand_layers, list) or not isinstance(base_layers, list) or not cand_layers or not base_layers:
            return False, "non-empty 'path' array missing in JSON v2 report"
        if cand_layers[0].get("layer") != "Hyprland" or base_layers[0].get("layer") != "Hyprland":
            return False, (
                "expected path[0].layer == 'Hyprland', "
                f"got base={base_layers[0].get('layer')!r}, cand={cand_layers[0].get('layer')!r}"
            )
        cand_binding = cand_layers[0].get("binding")
        base_binding = base_layers[0].get("binding")
        if not isinstance(cand_binding, dict) or not isinstance(base_binding, dict):
            return False, "path[0].binding object missing"
    except (KeyError, IndexError, TypeError) as exc:
        return False, f"structure error: {exc}"
    if cand_binding.get("uncertainty") != "ModifierAmbiguity":
        return False, (
            "expected path[0].binding.uncertainty == 'ModifierAmbiguity', "
            f"got {cand_binding.get('uncertainty')!r}"
        )
    if base_binding.get("uncertainty") is not None:
        return False, f"baseline already has uncertainty: {base_binding.get('uncertainty')!r}"
    del cand_binding["uncertainty"]
    if cand_doc != base_doc:
        return False, "candidate has unexpected differences beyond path[0].binding.uncertainty"
    return True, ""


def make_mock_bin(directory: Path) -> str:
    directory.mkdir(parents=True, exist_ok=True)
    def put(name: str, body: str) -> None:
        path = directory / name
        path.write_text("#!/bin/sh\n" + body, encoding="utf-8")
        path.chmod(0o755)
    put("pgrep", "exit 1\n")
    put("hyprctl", """case "$*" in
  *binds*) printf '%s\\n' '[{"modmask":64,"key":"q","key_code":0,"catch_all":false,"description":"kill","dispatcher":"killactive","arg":"","submap":"","submap_universal":false}]' ;;
  *submap*) printf '%s\\n' '""' ;;
  *devices*) printf '%s\\n' '{"keyboards":[{"name":"main-keyboard","rules":"","model":"","layout":"us","variant":"","options":"","active_keymap":"English (US)","main":true}]}' ;;
  *) exit 0 ;; esac
""")
    put("tmux", """case "$*" in
  *list-keys*root*) printf '%s\\n' 'bind-key -T root M-Enter split-window -v' ;;
  *list-keys*) printf '%s\\n' '' ;;
  *display-message*) printf '%s\\n' 'root' ;;
  *) exit 0 ;; esac
""")
    put("ghostty", "exit 0\n")
    return str(directory) + ":/usr/bin:/bin"


def run_case(s: Scenario, base: str, cand: str, root: Path, out: Path, timeout: float, max_lines: int) -> tuple[str, bool, bool]:
    directory = out / slug(s.name)
    directory.mkdir(parents=True, exist_ok=True)
    env = isolated_env(s.env, root)
    baseline = run(base, s.args, env, timeout)
    candidate = run(cand, s.args, env, timeout)
    for label, result in (("base", baseline), ("cand", candidate)):
        (directory / f"{label}.out").write_bytes(result.stdout)
        (directory / f"{label}.err").write_bytes(result.stderr)
        write_metadata(directory / f"{label}.meta.json", result)
    if baseline.start_error or candidate.start_error:
        errors = "; ".join(x for x in (f"baseline: {baseline.start_error}" if baseline.start_error else "", f"candidate: {candidate.start_error}" if candidate.start_error else "") if x)
        return f"FAIL [{s.name}]: harness could not start process ({errors})", False, False
    if baseline.timed_out or candidate.timed_out:
        return f"FAIL [{s.name}]: execution timed out after {timeout:g}s (baseline timed out: {int(baseline.timed_out)}, candidate timed out: {int(candidate.timed_out)})", False, False
    if baseline.code != candidate.code:
        return f"FAIL [{s.name}]: exit code mismatch (baseline {baseline.code} vs candidate {candidate.code})", False, False
    if baseline.stderr != candidate.stderr:
        detail = diff_text(baseline.stderr, candidate.stderr, directory / "stderr.diff", max_lines)
        return f"FAIL [{s.name}]: stderr mismatch\n{detail}", False, False
    base_out = normalize(baseline.stdout, s.normalize_pid)
    cand_out = normalize(candidate.stdout, s.normalize_pid)
    if s.normalize_pid:
        (directory / "base.normalized.out").write_bytes(base_out)
        (directory / "cand.normalized.out").write_bytes(cand_out)
    if base_out != cand_out:
        detail = diff_text(base_out, cand_out, directory / "stdout.diff", max_lines)
        if s.expected_rule:
            base_json = directory / "base.normalized.out"
            cand_json = directory / "cand.normalized.out"
            if not s.normalize_pid:
                base_json.write_bytes(base_out); cand_json.write_bytes(cand_out)
            ok, diagnostic = verify_expected(s.expected_rule, base_json, cand_json)
            if ok:
                return f"PASS [{s.name}] (with expected difference: {s.expected_description})", True, True
            return f"FAIL [{s.name}]: stdout has unexpected differences beyond: {s.expected_description}\n  Verifier diagnostic: {diagnostic}\n{detail}", False, False
        return f"FAIL [{s.name}]: stdout mismatch\n{detail}", False, False
    return f"PASS [{s.name}]", True, False


def strict_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate key {key!r}")
        result[key] = value
    return result


def validate_baseline(path: Path, allow_custom: bool) -> str | None:
    if allow_custom:
        return None
    manifest = Path(os.environ.get("WHYKEY_BASELINE_MANIFEST", ROOT / "tools/baseline_manifest.json"))
    if not manifest.is_file():
        return f"Error: Baseline manifest '{manifest}' is missing."
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"), object_pairs_hook=strict_pairs)
        if not isinstance(data, dict):
            raise ValueError("top level must be a JSON object")
        expected = data.get("binary_sha256")
        if not isinstance(expected, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", expected):
            raise ValueError("does not contain a valid 64-character hexadecimal 'binary_sha256'")
    except json.JSONDecodeError as exc:
        return f"Error: Baseline manifest '{manifest}' is not valid JSON ({exc})."
    except (ValueError, TypeError) as exc:
        return f"Error: Baseline manifest '{manifest}' {exc}."
    if not (sys.platform.startswith("linux") and os.uname().machine == "x86_64"):
        return "Error: Current platform does not match recorded baseline platform (Linux x86_64)."
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected.lower():
        return f"Error: Baseline binary '{path}' SHA-256 does not match recorded manifest in tools/baseline_manifest.json."
    return None


def positive(value: str) -> float:
    number = float(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return number


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Compare Whykey binaries across the complete sanitized scenario matrix.")
    parser.add_argument("baseline", nargs="?", default="target/baseline/whykey")
    parser.add_argument("candidate", nargs="?", default="target/debug/whykey")
    parser.add_argument("--timeout", type=positive, default=5)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--max-diff-lines", type=int, default=20)
    parser.add_argument("--allow-custom-baseline", action="store_true")
    parser.add_argument("--allow-identical-binaries", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser


def _script(path: Path, body: str) -> str:
    path.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
    path.chmod(0o755)
    return str(path)


def self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="whykey-comparator-selftest-") as temporary:
        root = Path(temporary)
        good = _script(root / "good", 'printf \'{"ok":true}\\n\'; printf \'warning\\n\' >&2')
        allowed = _script(root / "allowed", 'printf \'{"schema_version":2,"path":[{"layer":"Hyprland","binding":{"action":"echo","uncertainty":"ModifierAmbiguity"}}]}\\n\'; printf \'warning\\n\' >&2')
        bad_exit = _script(root / "bad-exit", 'exit 3')
        bad_err = _script(root / "bad-err", 'printf \'{"ok":true}\\n\'; printf \'different\\n\' >&2')
        bad_multi = _script(root / "bad-multi", 'if [ "$#" -eq 0 ]; then printf \'divergent\\n\'; else printf \'{"ok":true}\\n\'; fi; printf \'warning\\n\' >&2')
        hang = _script(root / "hang", 'exec sleep 10')
        pid_base = _script(root / "pid-base", 'printf \'target pid: 101\\n\'')
        pid_cand = _script(root / "pid-cand", 'printf \'target pid: 202\\n\'')
        base_json = _script(root / "base-json", 'printf \'{"schema_version":2,"path":[{"layer":"Hyprland","binding":{"action":"echo"}}]}\\n\'; printf \'warning\\n\' >&2')
        cases = 0
        def check(condition: bool, label: str) -> None:
            nonlocal cases
            cases += 1
            print(f"Self-test {cases}: {label}... {'PASSED' if condition else 'FAILED'}")
            if not condition:
                raise AssertionError(label)
        env = isolated_env((), root)
        check(run_case(Scenario("allowed JSON", (), (), False, "json_v2_modifier_ambiguity"), base_json, allowed, root, root / "artifacts", 2, 20)[1], "run_case accepts only approved JSON extension")
        verifier_base = root / "verifier-base.json"
        verifier_cand = root / "verifier-cand.json"
        verifier_base.write_text('{"schema_version":2,"path":[{"layer":"Hyprland","binding":{"action":"echo"}}]}', encoding="utf-8")
        for content, label in [
            ('{"schema_version":1}', "wrong schema"),
            ('{"schema_version":2,"path":[{"layer":"Hyprland","binding":{"action":"echo","uncertainty":"Wrong"}}]}', "wrong approved value"),
            ('{"schema_version":2,"path":[{"layer":"Hyprland","binding":{"action":"echo","uncertainty":"ModifierAmbiguity"}}],"extra":true}', "unexpected extra"),
            ('{"schema_version":2,"path":[]}', "missing path"),
        ]:
            verifier_cand.write_text(content, encoding="utf-8")
            check(not verify_expected("json_v2_modifier_ambiguity", verifier_base, verifier_cand)[0], f"verifier rejects {label}")
        check(not run_case(Scenario("wrong exit", ()), good, bad_exit, root, root / "artifacts", 2, 20)[1], "run_case rejects wrong exit")
        check(not run_case(Scenario("wrong stderr", ()), good, bad_err, root, root / "artifacts", 2, 20)[1], "run_case rejects wrong stderr")
        check(run(hang, (), env, .1).timed_out, "timeout is recorded and process group is stopped")
        check(not run_case(Scenario("wrong timeout", ()), hang, good, root, root / "artifacts", .1, 20)[1], "run_case rejects timeout")
        check(run_case(Scenario("pid normalization", (), (), True), pid_base, pid_cand, root, root / "artifacts", 2, 20)[1], "PID normalization accepts equivalent output")
        artifact = root / "artifacts" / slug("allowed JSON")
        check((artifact / "base.meta.json").is_file() and json.loads((artifact / "base.meta.json").read_text())["exit_code"] == 0, "exit metadata persists")
        pid_artifact = root / "artifacts" / slug("pid normalization")
        check((pid_artifact / "base.out").read_bytes() != (pid_artifact / "base.normalized.out").read_bytes(), "raw output is not replaced by normalization")
        invalid = root / "invalid.json"
        old_manifest = os.environ.get("WHYKEY_BASELINE_MANIFEST")
        try:
            os.environ["WHYKEY_BASELINE_MANIFEST"] = str(invalid)
            for content, label in [("[]", "manifest top-level type"), ('{"binary_sha256":"' + "0" * 64 + '","binary_sha256":"' + "1" * 64 + '"}', "manifest duplicate key"), ("not json", "malformed manifest")]:
                invalid.write_text(content, encoding="utf-8")
                check(validate_baseline(Path(good), False) is not None, f"rejects {label}")
            invalid.write_text('{"binary_sha256":"' + "0" * 64 + '"}', encoding="utf-8")
            check("does not match" in (validate_baseline(Path(good), False) or ""), "rejects wrong manifest hash")
        finally:
            if old_manifest is None: os.environ.pop("WHYKEY_BASELINE_MANIFEST", None)
            else: os.environ["WHYKEY_BASELINE_MANIFEST"] = old_manifest
        with contextlib.redirect_stdout(io.StringIO()) as stdout, contextlib.redirect_stderr(io.StringIO()) as stderr:
            sys.argv = ["differential_compare.py", str(root / "missing"), good, "--allow-custom-baseline"]
            check(main() == 2 and "not found" in stderr.getvalue(), "CLI rejects missing binary")
            sys.argv = ["differential_compare.py", good, good, "--allow-custom-baseline"]
            check(main() == 2 and "exact same file" in stderr.getvalue(), "CLI rejects identical binaries")
        original_scenarios, original_inspect = scenarios, inspect_scenarios
        try:
            globals()["scenarios"] = lambda: [Scenario("mismatch one", ()), Scenario("mismatch two", ())]
            globals()["inspect_scenarios"] = lambda path, pid=None: []
            with contextlib.redirect_stdout(io.StringIO()) as output:
                sys.argv = ["differential_compare.py", good, bad_multi, "--allow-custom-baseline", "--output-dir", str(root / "main-out")]
                result = main()
            check(result == 1 and "2 failed" in output.getvalue() and "summary" in output.getvalue().lower(), "CLI reports multi-case mismatches in final summary")
        finally:
            globals()["scenarios"], globals()["inspect_scenarios"] = original_scenarios, original_inspect
        check(build_parser().parse_args(["--timeout", "1"]).timeout == 1, "positive timeout is accepted")
        try:
            build_parser().parse_args(["--timeout", "0"])
        except SystemExit:
            check(True, "non-positive timeout is rejected")
        else:
            check(False, "non-positive timeout is rejected")
        print(f"=== Self-Test Summary: {cases} passed, 0 failed ===")
    return 0


def main() -> int:
    args = build_parser().parse_args()
    if args.self_test:
        return self_test()
    base, cand = Path(args.baseline), Path(args.candidate)
    for label, path in (("Baseline", base), ("Candidate", cand)):
        if not path.is_file() or not os.access(path, os.X_OK):
            print(f"Error: {label} binary '{path}' not found or not executable.", file=sys.stderr)
            return 2
    if not args.allow_identical_binaries and base.resolve() == cand.resolve():
        print(f"Error: Baseline binary '{base}' and candidate binary '{cand}' point to the exact same file.", file=sys.stderr)
        return 2
    error = validate_baseline(base, args.allow_custom_baseline)
    if error:
        print(error, file=sys.stderr)
        return 2
    if args.output_dir is None:
        target = ROOT / "target"
        target.mkdir(parents=True, exist_ok=True)
        output_root = Path(tempfile.mkdtemp(prefix="whykey-diff-", dir=target))
    else:
        output_root = args.output_dir
    output_root.mkdir(parents=True, exist_ok=True)
    out = output_root / "diffs"
    out.mkdir(exist_ok=True)
    passed = expected = failed = 0
    app = shell = None
    try:
        mock = make_mock_bin(output_root / "mock_bin")
        all_cases = scenarios() + inspect_scenarios(mock)
        uncertain_mock = output_root / "uncertain_bin"
        uncertain_path = make_mock_bin(uncertain_mock)
        (uncertain_mock / "hyprctl").write_text("#!/bin/sh\ncase \"$*\" in *binds*) printf '%s\\n' '[{\"modmask\":0,\"key\":\"c\",\"key_code\":0,\"catch_all\":false,\"description\":\"possible\",\"dispatcher\":\"exec\",\"arg\":\"echo\",\"submap\":\"\",\"submap_universal\":false}]' ;; *submap*) printf '%s\\n' '\"\"' ;; *devices*) printf '%s\\n' '{\"keyboards\":[]}' ;; *) exit 0 ;; esac\n", encoding="utf-8")
        (uncertain_mock / "hyprctl").chmod(0o755)
        hypr_env = tuple(sorted({"PATH": uncertain_path, "HYPRLAND_INSTANCE_SIGNATURE": "test", "XDG_CURRENT_DESKTOP": "Hyprland", "XDG_SESSION_TYPE": "wayland"}.items()))
        all_cases += [Scenario("inspect uncertain upstream continues text", ("ctrl+c",), hypr_env, True), Scenario("inspect uncertain upstream continues json-v2", ("ctrl+c", "--json-v2"), hypr_env, True, "json_v2_modifier_ambiguity", "typed ModifierAmbiguity on Hyprland binding in schema-v2")]
        tmux_env = tuple(sorted({"PATH": mock, "TMUX": "/tmp/mock-tmux,1,0", "TMUX_PANE": "%0", "TERM_PROGRAM": "ghostty"}.items()))
        all_cases += [Scenario("inspect tmux candidate unpredicted bytes text", ("alt+return", "--verbose"), tmux_env, True), Scenario("inspect tmux candidate unpredicted bytes json-v2", ("alt+return", "--json-v2"), tmux_env, True)]
        term_env = tuple(sorted({"PATH": mock, "TERM_PROGRAM": "ghostty"}.items()))
        all_cases += [Scenario(f"inspect predicted terminal bytes {kind}", ("ctrl+z", *args), term_env, True) for kind, args in (("text", ()), ("verbose", ("--verbose",)), ("json-v2", ("--json-v2",)))]
        all_cases += [Scenario("inspect missing shell context text", ("ctrl+z",), tuple(sorted({"PATH": mock, "SHELL": ""}.items())), True), Scenario("inspect missing shell context json-v2", ("ctrl+z", "--json-v2"), tuple(sorted({"PATH": mock, "SHELL": ""}.items())), True), Scenario("inspect session multiplexer context text", ("ctrl+b",), tmux_env, True)]
        app = subprocess.Popen(["sleep", "45"], start_new_session=True)
        app_env = tuple(sorted({"PATH": mock, "SHELL": "/bin/bash", "WHYKEY_READLINE_BINDINGS": "unix-word-rubout"}.items()))
        all_cases += [Scenario("inspect selected app non-shell text", ("inspect", "--pid", str(app.pid), "ctrl+w", "--verbose"), app_env, True), Scenario("inspect selected app non-shell json-v2", ("inspect", "--pid", str(app.pid), "ctrl+w", "--json-v2"), app_env, True)]
        shell = subprocess.Popen(["bash", "-c", "trap 'exit 0' TERM INT; while :; do sleep 1; done"], start_new_session=True)
        shell_env = tuple(sorted({"PATH": mock, "SHELL": "/bin/bash", "WHYKEY_READLINE_BINDINGS": "reverse-search-history"}.items()))
        all_cases += [Scenario("inspect selected shell target text", ("inspect", "--pid", str(shell.pid), "ctrl+r", "--verbose"), shell_env, True), Scenario("inspect selected shell target json-v2", ("inspect", "--pid", str(shell.pid), "ctrl+r", "--json-v2"), shell_env, True)]
        budget = tuple(sorted({"PATH": mock, "WHYKEY_DIAGNOSTIC_TIMEOUT_MS": "0"}.items()))
        all_cases += [Scenario("inspect diagnostic budget exhausted text", ("ctrl+z",), budget), Scenario("inspect diagnostic budget exhausted json-v2", ("ctrl+z", "--json-v2"), budget)]
        for scenario in all_cases:
            message, ok, documented = run_case(scenario, str(base), str(cand), output_root, out, args.timeout, args.max_diff_lines)
            print(message)
            if ok:
                passed += 1; expected += int(documented)
            else:
                failed += 1
        identical = passed - expected
        print("---------------------------------------------------------")
        print(f"Differential comparison summary: {passed} passed ({identical} identical, {expected} with documented expected difference), {failed} failed.")
        print(f"Results and detailed diffs stored in: {out}")
        return 1 if failed else 0
    finally:
        for process in (app, shell):
            if process is not None:
                stop_group(process)


if __name__ == "__main__":
    raise SystemExit(main())
