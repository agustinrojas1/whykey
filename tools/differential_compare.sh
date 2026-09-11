#!/usr/bin/env bash
set -uo pipefail

# tools/differential_compare.sh
# Public regression comparator for Whykey refactoring and optimization.
# Compares a candidate binary against a baseline binary across CLI contracts,
# sanitized replay/diff fixtures, and controlled inspection routes.

usage() {
    cat <<'EOF'
Usage: tools/differential_compare.sh [options] [baseline_binary] [candidate_binary]

Arguments:
  baseline_binary   Path to verified baseline binary (default: target/baseline/whykey)
  candidate_binary  Path to candidate binary under test (default: target/debug/whykey)

Options:
  --timeout <sec>        Per-command execution timeout in seconds (default: 5)
  --output-dir <dir>     Directory where full diffs and outputs are saved (default: auto-created temp dir)
  --max-diff-lines <N>       Maximum diff lines to print per stream mismatch (default: 20)
  --allow-custom-baseline    Allow non-standard baseline without matching tools/baseline_manifest.json
  --allow-identical-binaries Allow candidate and baseline to point to the same binary path (for testing)
  --self-test                Run comparator automated self-tests (including negative tests) and exit
  -h, --help                 Show this help message
Documentation & Details:
  - Required tools: bash, diff, cmp, timeout, sed, mktemp, python3
  - Building baseline: tools/rebuild_baseline.sh --toolchain 1.98.1 <commit> <output-path>
  - Building candidate: cargo build --locked
  - Guarantees: Does not touch live desktop compositors, input devices, or user configs.
  - Baseline identity is documented in tools/baseline_manifest.json (x86_64-unknown-linux-gnu).
EOF
    exit 1
}

CMD_TIMEOUT=5
OUTPUT_DIR=""
MAX_DIFF_LINES=20
SELF_TEST=0
ALLOW_CUSTOM_BASELINE=0
ALLOW_IDENTICAL_BINARIES=0
BASELINE="target/baseline/whykey"
CANDIDATE="target/debug/whykey"
POSITIONAL=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --timeout)
            CMD_TIMEOUT="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --max-diff-lines)
            MAX_DIFF_LINES="$2"
            shift 2
            ;;
        --allow-custom-baseline)
            ALLOW_CUSTOM_BASELINE=1
            shift
            ;;
        --allow-identical-binaries)
            ALLOW_IDENTICAL_BINARIES=1
            shift
            ;;
        --self-test)
            SELF_TEST=1
            shift
            ;;
        -h|--help)
            usage
            ;;
        -*)
            echo "Error: Unknown option $1" >&2
            usage
            ;;
        *)
            POSITIONAL+=("$1")
            shift
            ;;
    esac
done

if [[ ${#POSITIONAL[@]} -ge 1 ]]; then
    BASELINE="${POSITIONAL[0]}"
fi
if [[ ${#POSITIONAL[@]} -ge 2 ]]; then
    CANDIDATE="${POSITIONAL[1]}"
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Self-test harness
run_self_tests() {
    echo "=== Running Differential Comparator Self-Tests ==="
    ST_DIR="$(mktemp -d -t whykey-comparator-selftest-XXXXXX)"
    trap 'rm -rf "$ST_DIR"' EXIT INT TERM

    local st_passed=0
    local st_failed=0

    # 1. Identical baseline and candidate pass
    echo -n "Self-test 1: Identical baseline and candidate pass... "
    local bin_good="$ST_DIR/bin_good"
    cat > "$bin_good" <<'EOF'
#!/bin/sh
echo "standard output"
echo "standard error" >&2
exit 0
EOF
    chmod +x "$bin_good"

    local out1="$ST_DIR/test1.log"
    if bash "$0" --timeout 2 --allow-identical-binaries --allow-custom-baseline "$bin_good" "$bin_good" >"$out1" 2>&1; then
        if grep -q "0 failed" "$out1"; then
            echo "PASSED"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (unexpected output)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (exit non-zero)"
        st_failed=$((st_failed + 1))
    fi

    # 2. An intentionally incorrect candidate fails
    echo -n "Self-test 2: Intentionally incorrect candidate fails... "
    local bin_bad="$ST_DIR/bin_bad"
    cat > "$bin_bad" <<'EOF'
#!/bin/sh
echo "divergent output"
exit 0
EOF
    chmod +x "$bin_bad"

    local out2="$ST_DIR/test2.log"
    if ! bash "$0" --timeout 2 --allow-custom-baseline "$bin_good" "$bin_bad" >"$out2" 2>&1; then
        if grep -q "FAIL" "$out2" && grep -q "failed" "$out2"; then
            echo "PASSED"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (expected failure log not found)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (comparator exited zero on mismatch)"
        st_failed=$((st_failed + 1))
    fi

    # 3. Multiple mismatches are all collected and summarized (no early exit)
    echo -n "Self-test 3: Multiple mismatches are collected and summarized... "
    local out3="$ST_DIR/test3.log"
    if ! bash "$0" --timeout 2 --allow-custom-baseline "$bin_good" "$bin_bad" >"$out3" 2>&1; then
        local fail_count
        fail_count=$(grep -c "FAIL \[" "$out3" || true)
        if [[ "$fail_count" -ge 1 ]] && grep -q "Differential comparison summary:" "$out3"; then
            echo "PASSED (collected $fail_count mismatches before summary)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (missing summary or failure collection)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (comparator exited zero)"
        st_failed=$((st_failed + 1))
    fi

    # 4. Missing prerequisites produce an explicit failure
    echo -n "Self-test 4: Missing prerequisites produce explicit failure... "
    local out4="$ST_DIR/test4.log"
    if ! bash "$0" --allow-custom-baseline "$ST_DIR/nonexistent_baseline" "$bin_good" >"$out4" 2>&1; then
        if grep -q "Error: Baseline binary.*not found or not executable" "$out4"; then
            echo "PASSED"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (missing explicit diagnostic)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (missing binary did not fail)"
        st_failed=$((st_failed + 1))
    fi

    # 5. A hanging candidate times out with a useful diagnostic
    echo -n "Self-test 5: Hanging candidate times out with diagnostic... "
    local bin_hang="$ST_DIR/bin_hang"
    cat > "$bin_hang" <<'EOF'
#!/bin/sh
sleep 10
EOF
    chmod +x "$bin_hang"

    local out5="$ST_DIR/test5.log"
    if ! bash "$0" --timeout 1 --allow-custom-baseline "$bin_good" "$bin_hang" >"$out5" 2>&1; then
        if grep -q "execution timed out" "$out5"; then
            echo "PASSED"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (timeout diagnostic missing)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (hanging candidate did not fail)"
        st_failed=$((st_failed + 1))
    fi

    # Direct unit tests for verify_expected_diff.py
    local py_verifier="$REPO_ROOT/tools/verify_expected_diff.py"
    local f_base="$ST_DIR/base.json"
    local f_cand="$ST_DIR/cand.json"

    cat > "$f_base" <<'EOF'
{"schema_version": 2, "path": [{"layer": "Hyprland", "binding": {"action": "echo"}}]}
EOF

    # 6. Valid expected JSON addition passes
    echo -n "Self-test 6: Valid expected JSON addition passes... "
    cat > "$f_cand" <<'EOF'
{"schema_version": 2, "path": [{"layer": "Hyprland", "binding": {"action": "echo", "uncertainty": "ModifierAmbiguity"}}]}
EOF
    if python3 "$py_verifier" json_v2_modifier_ambiguity "$f_base" "$f_cand" >/dev/null 2>&1; then
        echo "PASSED"
        st_passed=$((st_passed + 1))
    else
        echo "FAILED"
        st_failed=$((st_failed + 1))
    fi

    # 7. Negative: Exit code mismatch takes precedence over expected difference
    echo -n "Self-test 7 (negative): Exit code mismatch fails before exception... "
    local bin_json_base="$ST_DIR/bin_json_base"
    cat > "$bin_json_base" <<EOF
#!/bin/sh
cat "$f_base"
exit 0
EOF
    chmod +x "$bin_json_base"

    local bin_json_cand_badcode="$ST_DIR/bin_json_cand_badcode"
    cat > "$bin_json_cand_badcode" <<EOF
#!/bin/sh
cat "$f_cand"
exit 1
EOF
    chmod +x "$bin_json_cand_badcode"

    local out7="$ST_DIR/test7.log"
    if ! bash "$0" --timeout 2 --allow-custom-baseline "$bin_json_base" "$bin_json_cand_badcode" >"$out7" 2>&1; then
        if grep -q "exit code mismatch" "$out7"; then
            echo "PASSED (exit code mismatch enforced)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (exit code mismatch not reported)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (exit code mismatch permitted)"
        st_failed=$((st_failed + 1))
    fi

    # 8. Negative: Stderr mismatch takes precedence over expected difference
    echo -n "Self-test 8 (negative): Stderr mismatch fails before exception... "
    local bin_json_cand_baderr="$ST_DIR/bin_json_cand_baderr"
    cat > "$bin_json_cand_baderr" <<EOF
#!/bin/sh
cat "$f_cand"
echo "unexpected stderr" >&2
exit 0
EOF
    chmod +x "$bin_json_cand_baderr"

    local out8="$ST_DIR/test8.log"
    if ! bash "$0" --timeout 2 --allow-custom-baseline "$bin_json_base" "$bin_json_cand_baderr" >"$out8" 2>&1; then
        if grep -q "stderr mismatch" "$out8"; then
            echo "PASSED (stderr mismatch enforced)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (stderr mismatch not reported)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (stderr mismatch permitted)"
        st_failed=$((st_failed + 1))
    fi

    # 9. Negative: Timeout takes precedence over expected difference
    echo -n "Self-test 9 (negative): Timeout fails before exception... "
    local bin_json_cand_hang="$ST_DIR/bin_json_cand_hang"
    cat > "$bin_json_cand_hang" <<'EOF'
#!/bin/sh
sleep 10
EOF
    chmod +x "$bin_json_cand_hang"

    local out9="$ST_DIR/test9.log"
    if ! bash "$0" --timeout 1 --allow-custom-baseline "$bin_json_base" "$bin_json_cand_hang" >"$out9" 2>&1; then
        if grep -q "execution timed out" "$out9"; then
            echo "PASSED (timeout enforced)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (timeout not reported)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (hang permitted)"
        st_failed=$((st_failed + 1))
    fi

    # 10. Negative: Duplicate key in candidate JSON rejected
    echo -n "Self-test 10 (negative): Duplicate JSON key rejected... "
    cat > "$f_cand" <<'EOF'
{"schema_version": 2, "path": [{"layer": "Hyprland", "binding": {"action": "echo", "uncertainty": "ModifierAmbiguity"}}], "dup": 1, "dup": 2}
EOF
    local out10
    out10=$(python3 "$py_verifier" json_v2_modifier_ambiguity "$f_base" "$f_cand" 2>&1 || true)
    if echo "$out10" | grep -q "Duplicate key detected"; then
        echo "PASSED (duplicate key rejected)"
        st_passed=$((st_passed + 1))
    else
        echo "FAILED: $out10"
        st_failed=$((st_failed + 1))
    fi

    # 11. Negative: Wrong value or wrong path rejected
    echo -n "Self-test 11 (negative): Wrong uncertainty value rejected... "
    cat > "$f_cand" <<'EOF'
{"schema_version": 2, "path": [{"layer": "Hyprland", "binding": {"action": "echo", "uncertainty": "UnrelatedValue"}}]}
EOF
    local out11
    out11=$(python3 "$py_verifier" json_v2_modifier_ambiguity "$f_base" "$f_cand" 2>&1 || true)
    if echo "$out11" | grep -q "got 'UnrelatedValue'"; then
        echo "PASSED (wrong value rejected)"
        st_passed=$((st_passed + 1))
    else
        echo "FAILED: $out11"
        st_failed=$((st_failed + 1))
    fi

    # 12. Negative: Unexpected difference elsewhere rejected
    echo -n "Self-test 12 (negative): Extra difference elsewhere rejected... "
    cat > "$f_cand" <<'EOF'
{"schema_version": 2, "path": [{"layer": "Hyprland", "binding": {"action": "echo", "uncertainty": "ModifierAmbiguity"}}], "extra_field": "unexpected"}
EOF
    local out12
    out12=$(python3 "$py_verifier" json_v2_modifier_ambiguity "$f_base" "$f_cand" 2>&1 || true)
    if echo "$out12" | grep -q "unexpected differences beyond"; then
        echo "PASSED (extra difference rejected)"
        st_passed=$((st_passed + 1))
    else
        echo "FAILED: $out12"
        st_failed=$((st_failed + 1))
    fi

    # 13. Negative: Wrong schema_version rejected by verifier
    echo -n "Self-test 13 (negative): Wrong schema_version rejected... "
    local f_base_v1="$ST_DIR/base_v1.json"
    local f_cand_v1="$ST_DIR/cand_v1.json"
    cat > "$f_base_v1" <<'EOF'
{"schema_version": 1, "path": [{"layer": "Hyprland", "binding": {"action": "echo"}}]}
EOF
    cat > "$f_cand_v1" <<'EOF'
{"schema_version": 1, "path": [{"layer": "Hyprland", "binding": {"action": "echo", "uncertainty": "ModifierAmbiguity"}}]}
EOF
    local out13
    out13=$(python3 "$py_verifier" json_v2_modifier_ambiguity "$f_base_v1" "$f_cand_v1" 2>&1 || true)
    if echo "$out13" | grep -q "expected schema_version == 2"; then
        echo "PASSED (wrong schema rejected)"
        st_passed=$((st_passed + 1))
    else
        echo "FAILED: $out13"
        st_failed=$((st_failed + 1))
    fi

    # 14. Negative: Wrong layer name rejected by verifier
    echo -n "Self-test 14 (negative): Wrong layer name rejected... "
    cat > "$f_cand" <<'EOF'
{"schema_version": 2, "path": [{"layer": "NotHyprland", "binding": {"action": "echo", "uncertainty": "ModifierAmbiguity"}}]}
EOF
    local out14
    out14=$(python3 "$py_verifier" json_v2_modifier_ambiguity "$f_base" "$f_cand" 2>&1 || true)
    if echo "$out14" | grep -q "expected path\[0\].layer == 'Hyprland'"; then
        echo "PASSED (wrong layer rejected)"
        st_passed=$((st_passed + 1))
    else
        echo "FAILED: $out14"
        st_failed=$((st_failed + 1))
    fi

    # 15. Negative: Comparing binary against itself rejected by default
    echo -n "Self-test 15 (negative): Accidental self-comparison rejected... "
    local out15="$ST_DIR/test15.log"
    if ! bash "$0" "$bin_good" "$bin_good" >"$out15" 2>&1; then
        if grep -q "point to the exact same file" "$out15"; then
            echo "PASSED (self-comparison rejected)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (missing self-comparison diagnostic)"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (self-comparison permitted without flag)"
        st_failed=$((st_failed + 1))
    fi

    # 16. Negative: Baseline manifest SHA-256 mismatch rejected on Linux x86_64
    echo -n "Self-test 16 (negative): Unverified baseline manifest mismatch rejected... "
    local out16="$ST_DIR/test16.log"
    if ! bash "$0" "$bin_good" "$bin_bad" >"$out16" 2>&1; then
        if grep -q "SHA-256 does not match recorded manifest" "$out16"; then
            echo "PASSED (manifest mismatch rejected)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (manifest mismatch not enforced): $(cat "$out16")"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (unverified baseline permitted)"
        st_failed=$((st_failed + 1))
    fi
    # 17. Negative: Missing baseline manifest rejected without --allow-custom-baseline
    echo -n "Self-test 17 (negative): Missing baseline manifest rejected... "
    local out17="$ST_DIR/test17.log"
    if ! WHYKEY_BASELINE_MANIFEST="$ST_DIR/missing_manifest.json" bash "$0" "$bin_good" "$bin_bad" >"$out17" 2>&1; then
        if grep -q "is missing" "$out17"; then
            echo "PASSED (missing manifest rejected)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (missing manifest not detected): $(cat "$out17")"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (missing manifest permitted without flag)"
        st_failed=$((st_failed + 1))
    fi

    # 18. Negative: Malformed baseline manifest JSON rejected without --allow-custom-baseline
    echo -n "Self-test 18 (negative): Malformed baseline manifest JSON rejected... "
    local bad_manifest="$ST_DIR/bad_manifest.json"
    echo "not-valid-json{{{" > "$bad_manifest"
    local out18="$ST_DIR/test18.log"
    if ! WHYKEY_BASELINE_MANIFEST="$bad_manifest" bash "$0" "$bin_good" "$bin_bad" >"$out18" 2>&1; then
        if grep -q "not valid JSON" "$out18"; then
            echo "PASSED (malformed manifest rejected)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (malformed manifest not detected): $(cat "$out18")"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (malformed manifest permitted without flag)"
        st_failed=$((st_failed + 1))
    fi

    # 19. Negative: Manifest with invalid SHA-256 rejected without --allow-custom-baseline
    echo -n "Self-test 19 (negative): Manifest with invalid SHA-256 rejected... "
    local invalid_sha_manifest="$ST_DIR/invalid_sha_manifest.json"
    echo '{"binary_sha256": "not-a-valid-sha"}' > "$invalid_sha_manifest"
    local out19="$ST_DIR/test19.log"
    if ! WHYKEY_BASELINE_MANIFEST="$invalid_sha_manifest" bash "$0" "$bin_good" "$bin_bad" >"$out19" 2>&1; then
        if grep -q "does not contain a valid 64-character" "$out19"; then
            echo "PASSED (invalid SHA rejected)"
            st_passed=$((st_passed + 1))
        else
            echo "FAILED (invalid SHA not detected): $(cat "$out19")"
            st_failed=$((st_failed + 1))
        fi
    else
        echo "FAILED (invalid SHA permitted without flag)"
        st_failed=$((st_failed + 1))
    fi

    echo "=== Self-Test Summary: $st_passed passed, $st_failed failed ==="
    if [[ "$st_failed" -gt 0 ]]; then
        return 1
    fi
    return 0
}

if [[ "$SELF_TEST" -eq 1 ]]; then
    run_self_tests
    exit $?
fi

# Validate binaries
if [[ ! -x "$BASELINE" ]]; then
    echo "Error: Baseline binary '$BASELINE' not found or not executable." >&2
    echo "Hint: Build baseline with: tools/rebuild_baseline.sh --toolchain 1.98.1 fe52e36 $BASELINE" >&2
    exit 2
fi

if [[ ! -x "$CANDIDATE" ]]; then
    echo "Error: Candidate binary '$CANDIDATE' not found or not executable." >&2
    echo "Hint: Build candidate with: cargo build --locked" >&2
    exit 2
fi

# 1. Prevent accidental comparison of candidate against itself
if [[ "$ALLOW_IDENTICAL_BINARIES" -ne 1 ]]; then
    base_real="$(realpath "$BASELINE" 2>/dev/null || echo "$BASELINE")"
    cand_real="$(realpath "$CANDIDATE" 2>/dev/null || echo "$CANDIDATE")"
    if [[ "$base_real" == "$cand_real" ]]; then
        echo "Error: Baseline binary '$BASELINE' and candidate binary '$CANDIDATE' point to the exact same file." >&2
        echo "Accidentally comparing the candidate against itself would report false passing results." >&2
        echo "Hint: Specify a verified baseline binary or pass --allow-identical-binaries if testing harness logic." >&2
        exit 2
    fi
fi

# 2. Enforce baseline manifest verification unless explicitly exempted with --allow-custom-baseline
MANIFEST_FILE="${WHYKEY_BASELINE_MANIFEST:-$REPO_ROOT/tools/baseline_manifest.json}"
if [[ "$ALLOW_CUSTOM_BASELINE" -ne 1 ]]; then
    # 2.1 Manifest file existence
    if [[ ! -f "$MANIFEST_FILE" ]]; then
        echo "Error: Baseline manifest '$MANIFEST_FILE' is missing." >&2
        echo "Hint: Restore tools/baseline_manifest.json, or pass --allow-custom-baseline to bypass manifest verification." >&2
        exit 2
    fi

    # 2.2 Manifest JSON parsing and SHA-256 validation
    manifest_err=""
    expected_sha=""
    manifest_info=$(python3 -c '
import json, sys, re
try:
    with open(sys.argv[1], "r", encoding="utf-8") as f:
        data = json.load(f)
except Exception as e:
    sys.stderr.write(f"INVALID_JSON: {e}\n")
    sys.exit(1)

sha = data.get("binary_sha256")
if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", sha.strip()):
    sys.stderr.write("INVALID_SHA: missing or malformed 64-character hexadecimal binary_sha256\n")
    sys.exit(1)

print(sha.strip().lower())
' "$MANIFEST_FILE" 2>&1) || manifest_err="$manifest_info"

    if [[ -n "$manifest_err" ]]; then
        if [[ "$manifest_err" =~ INVALID_JSON ]]; then
            echo "Error: Baseline manifest '$MANIFEST_FILE' is not valid JSON ($manifest_err)." >&2
        else
            echo "Error: Baseline manifest '$MANIFEST_FILE' does not contain a valid 64-character hexadecimal 'binary_sha256' ($manifest_err)." >&2
        fi
        echo "Hint: Check tools/baseline_manifest.json, or pass --allow-custom-baseline." >&2
        exit 2
    fi
    expected_sha="$manifest_info"

    # 2.3 Platform compatibility check
    host_os="$(uname -s)"
    host_arch="$(uname -m)"
    if [[ "$host_os" != "Linux" || "$host_arch" != "x86_64" ]]; then
        echo "Error: Current platform ($host_os $host_arch) does not match recorded baseline platform (Linux x86_64)." >&2
        echo "The recorded baseline binary in tools/baseline_manifest.json was built for x86_64-unknown-linux-gnu." >&2
        echo "Hint: Pass --allow-custom-baseline to compare against a locally rebuilt baseline on this platform." >&2
        exit 2
    fi

    # 2.4 Binary checksum verification
    actual_sha=$(sha256sum "$BASELINE" | awk '{print $1}')
    if [[ "$actual_sha" != "$expected_sha" ]]; then
        echo "Error: Baseline binary '$BASELINE' SHA-256 does not match recorded manifest in tools/baseline_manifest.json." >&2
        echo "  Recorded manifest SHA-256: $expected_sha" >&2
        echo "  Actual baseline SHA-256:   $actual_sha" >&2
        echo "Hint: Rebuild verified baseline with: tools/rebuild_baseline.sh --toolchain 1.98.1 fe52e36 $BASELINE" >&2
        echo "      Or pass --allow-custom-baseline if testing a locally rebuilt baseline on a different platform." >&2
        exit 2
    fi
fi
# Working directory setup
if [[ -z "$OUTPUT_DIR" ]]; then
    TMP_ROOT="$(mktemp -d -t whykey-diff-XXXXXX)"
    CLEANUP_TMP=1
    DIFF_DIR="$TMP_ROOT/diffs"
else
    mkdir -p "$OUTPUT_DIR"
    TMP_ROOT="$OUTPUT_DIR"
    CLEANUP_TMP=0
    DIFF_DIR="$OUTPUT_DIR/diffs"
fi
mkdir -p "$DIFF_DIR"

cleanup() {
    local exit_code=$?
    if [[ "$CLEANUP_TMP" -eq 1 ]]; then
        rm -rf "$TMP_ROOT"
    fi
    exit "$exit_code"
}
trap cleanup EXIT INT TERM

passed=0
failed=0
expected_diffs=0

slugify() {
    echo "$1" | tr ' /:' '___' | tr -cd 'A-Za-z0-9_-'
}

# Core runner
# Arguments:
#   $1: case name
#   $2: extra env assignment string (e.g. "KEY=VAL KEY2=VAL2")
#   $3: normalization sed script or empty string
#   $4: expected difference rule name (or empty string)
#   $5: expected difference description (or empty string)
#   $6...: arguments to pass to whykey
run_check_advanced() {
    local name="$1"
    local extra_env="$2"
    local norm_sed="$3"
    local expected_rule="$4"
    local expected_desc="$5"
    shift 5

    local slug
    slug="$(slugify "$name")"
    local case_dir="$DIFF_DIR/$slug"
    mkdir -p "$case_dir"

    local base_out="$case_dir/base.out"
    local cand_out="$case_dir/cand.out"
    local base_err="$case_dir/base.err"
    local cand_err="$case_dir/cand.err"

    local base_code=0
    local cand_code=0
    local base_timed_out=0
    local cand_timed_out=0

    # Build isolated environment
    local env_cmd=(env -i "PATH=/usr/bin:/bin" "HOME=$TMP_ROOT")
    if [[ -n "$extra_env" ]]; then
        read -r -a custom_vars <<< "$extra_env"
        env_cmd+=("${custom_vars[@]}")
    fi

    # Run baseline with timeout
    set +e
    timeout "$CMD_TIMEOUT" "${env_cmd[@]}" "$BASELINE" "$@" >"$base_out" 2>"$base_err"
    base_code=$?
    if [[ "$base_code" -eq 124 ]]; then
        base_timed_out=1
    fi

    # Run candidate with timeout
    timeout "$CMD_TIMEOUT" "${env_cmd[@]}" "$CANDIDATE" "$@" >"$cand_out" 2>"$cand_err"
    cand_code=$?
    if [[ "$cand_code" -eq 124 ]]; then
        cand_timed_out=1
    fi
    set -e

    # Precedence check 1: Timeout
    if [[ "$base_timed_out" -eq 1 || "$cand_timed_out" -eq 1 ]]; then
        echo "FAIL [$name]: execution timed out after ${CMD_TIMEOUT}s (baseline timed out: $base_timed_out, candidate timed out: $cand_timed_out)"
        failed=$((failed + 1))
        return 0
    fi

    # Precedence check 2: Exit code mismatch
    if [[ "$base_code" -ne "$cand_code" ]]; then
        echo "FAIL [$name]: exit code mismatch (baseline $base_code vs candidate $cand_code)"
        failed=$((failed + 1))
        return 0
    fi

    # Precedence check 3: Stderr mismatch
    if ! cmp -s "$base_err" "$cand_err"; then
        echo "FAIL [$name]: stderr mismatch:"
        local diff_err="$case_dir/stderr.diff"
        diff -u "$base_err" "$cand_err" > "$diff_err" || true
        head -n "$MAX_DIFF_LINES" "$diff_err"
        if [[ $(wc -l < "$diff_err") -gt "$MAX_DIFF_LINES" ]]; then
            echo "  ... (diff truncated; full diff at $diff_err)"
        fi
        failed=$((failed + 1))
        return 0
    fi

    # Apply text normalization if specified (only after stderr/status verified)
    if [[ -n "$norm_sed" ]]; then
        sed -E "$norm_sed" "$base_out" > "$base_out.norm" && mv "$base_out.norm" "$base_out"
        sed -E "$norm_sed" "$cand_out" > "$cand_out.norm" && mv "$cand_out.norm" "$cand_out"
    fi

    # Precedence check 4: Stdout comparison with syntax-aware exception verification
    if ! cmp -s "$base_out" "$cand_out"; then
        local diff_file="$case_dir/stdout.diff"
        diff -u "$base_out" "$cand_out" > "$diff_file" || true

        if [[ -n "$expected_rule" && -n "$expected_desc" ]]; then
            local verifier_out
            if verifier_out=$(python3 "$REPO_ROOT/tools/verify_expected_diff.py" "$expected_rule" "$base_out" "$cand_out" 2>&1); then
                echo "PASS [$name] (with expected difference: $expected_desc)"
                expected_diffs=$((expected_diffs + 1))
                passed=$((passed + 1))
                return 0
            else
                echo "FAIL [$name]: stdout has unexpected differences beyond: $expected_desc"
                echo "  Verifier diagnostic: $verifier_out"
                head -n "$MAX_DIFF_LINES" "$diff_file"
                if [[ $(wc -l < "$diff_file") -gt "$MAX_DIFF_LINES" ]]; then
                    echo "  ... (diff truncated; full diff at $diff_file)"
                fi
                failed=$((failed + 1))
                return 0
            fi
        else
            echo "FAIL [$name]: stdout mismatch:"
            head -n "$MAX_DIFF_LINES" "$diff_file"
            if [[ $(wc -l < "$diff_file") -gt "$MAX_DIFF_LINES" ]]; then
                echo "  ... (diff truncated; full diff at $diff_file)"
            fi
            failed=$((failed + 1))
            return 0
        fi
    fi

    echo "PASS [$name]"
    passed=$((passed + 1))
    return 0
}

run_check() {
    local name="$1"
    shift
    run_check_advanced "$name" "" "" "" "" "$@"
}

echo "Running differential comparisons between $BASELINE and $CANDIDATE..."

# --- 1. CLI Metadata & Help ---
run_check "version" --version
run_check "help" --help
run_check "help inspect" inspect --help
run_check "help listen" listen --help
run_check "help doctor" doctor --help
run_check "help capabilities" capabilities --help
run_check "capabilities text" capabilities
run_check "capabilities json" capabilities --json
run_check "capabilities json-v2" capabilities --json-v2

# --- 2. Error handling ---
run_check "invalid subcommand" invalid_cmd
run_check "inspect without arg" inspect
run_check "replay without arg" replay
run_check "extension without arg" extension
run_check "diff without arg" diff
run_check "replay nonexistent" replay nonexistent_file_404.json
run_check "diff single file" diff tests/fixtures/differential/replay-compositor-consumed.json

# --- 3. Sanitized Replay Fixtures ---
FIXTURES_DIR="$REPO_ROOT/tests/fixtures/differential"
run_check "replay compositor consumed text" replay "$FIXTURES_DIR/replay-compositor-consumed.json"
run_check "replay compositor consumed json" replay "$FIXTURES_DIR/replay-compositor-consumed.json" --json
run_check "replay terminal consumed text" replay "$FIXTURES_DIR/replay-terminal-consumed.json"
run_check "replay terminal consumed json" replay "$FIXTURES_DIR/replay-terminal-consumed.json" --json
run_check "replay tty signal text" replay "$FIXTURES_DIR/replay-tty-signal.json"
run_check "replay tty signal json" replay "$FIXTURES_DIR/replay-tty-signal.json" --json
run_check "replay shell readline text" replay "$FIXTURES_DIR/replay-shell-readline.json"
run_check "replay shell readline json" replay "$FIXTURES_DIR/replay-shell-readline.json" --json
run_check "replay v1 unhandled text" replay "$FIXTURES_DIR/replay-v1-unhandled.json"
run_check "replay v1 unhandled json" replay "$FIXTURES_DIR/replay-v1-unhandled.json" --json

# --- 4. Sanitized Diff Fixtures ---
run_check "diff compositor vs terminal text" diff "$FIXTURES_DIR/replay-compositor-consumed.json" "$FIXTURES_DIR/replay-terminal-consumed.json"
run_check "diff compositor vs terminal json" diff "$FIXTURES_DIR/replay-compositor-consumed.json" "$FIXTURES_DIR/replay-terminal-consumed.json" --json
run_check "diff tty vs readline text" diff "$FIXTURES_DIR/replay-tty-signal.json" "$FIXTURES_DIR/replay-shell-readline.json"
run_check "diff tty vs readline json" diff "$FIXTURES_DIR/replay-tty-signal.json" "$FIXTURES_DIR/replay-shell-readline.json" --json

# --- 5. Controlled Inspection Routes (Step 5) ---

MOCK_DIR="$TMP_ROOT/mock_bin"
mkdir -p "$MOCK_DIR"

cat > "$MOCK_DIR/pgrep" <<'EOF'
#!/bin/sh
exit 1
EOF
chmod +x "$MOCK_DIR/pgrep"

MOCK_PATH="$MOCK_DIR:/usr/bin:/bin"

# 5.1 Upstream consumption and route stopping
cat > "$MOCK_DIR/hyprctl" <<'EOF'
#!/bin/sh
case "$*" in
  *binds*) printf '%s\n' '[{"modmask":64,"key":"q","key_code":0,"catch_all":false,"description":"kill","dispatcher":"killactive","arg":"","submap":"","submap_universal":false}]' ;;
  *submap*) printf '%s\n' '""' ;;
  *devices*) printf '%s\n' '{"keyboards":[{"name":"main-keyboard","rules":"","model":"","layout":"us","variant":"","options":"","active_keymap":"English (US)","main":true}]}' ;;
  *) exit 0 ;;
esac
EOF
chmod +x "$MOCK_DIR/hyprctl"

run_check_advanced "inspect upstream consumption text" \
    "PATH=$MOCK_PATH HYPRLAND_INSTANCE_SIGNATURE=test XDG_CURRENT_DESKTOP=Hyprland XDG_SESSION_TYPE=wayland" \
    "" "" "" \
    super+q

run_check_advanced "inspect upstream consumption verbose" \
    "PATH=$MOCK_PATH HYPRLAND_INSTANCE_SIGNATURE=test XDG_CURRENT_DESKTOP=Hyprland XDG_SESSION_TYPE=wayland" \
    "" "" "" \
    super+q --verbose

run_check_advanced "inspect upstream consumption json-v2" \
    "PATH=$MOCK_PATH HYPRLAND_INSTANCE_SIGNATURE=test XDG_CURRENT_DESKTOP=Hyprland XDG_SESSION_TYPE=wayland" \
    "" "" "" \
    super+q --json-v2

# 5.2 Uncertain upstream propagation followed by downstream inspection
cat > "$MOCK_DIR/hyprctl" <<'EOF'
#!/bin/sh
case "$*" in
  *binds*) printf '%s\n' '[{"modmask":0,"key":"c","key_code":0,"catch_all":false,"description":"possible","dispatcher":"exec","arg":"echo","submap":"","submap_universal":false}]' ;;
  *submap*) printf '%s\n' '""' ;;
  *devices*) printf '%s\n' '{"keyboards":[{"name":"main-keyboard","rules":"","model":"","layout":"us","variant":"","options":"","active_keymap":"English (US)","main":true}]}' ;;
  *) exit 0 ;;
esac
EOF
chmod +x "$MOCK_DIR/hyprctl"

PID_NORM="s/target pid: [0-9]+/target pid: <PID>/g"

run_check_advanced "inspect uncertain upstream continues text" \
    "PATH=$MOCK_PATH HYPRLAND_INSTANCE_SIGNATURE=test XDG_CURRENT_DESKTOP=Hyprland XDG_SESSION_TYPE=wayland" \
    "$PID_NORM" "" "" \
    ctrl+c

# Documented expected difference: Candidate refactor emits typed UncertaintyReason::ModifierAmbiguity
# on the Hyprland binding evidence in schema-v2 to replace summary text parsing.
run_check_advanced "inspect uncertain upstream continues json-v2" \
    "PATH=$MOCK_PATH HYPRLAND_INSTANCE_SIGNATURE=test XDG_CURRENT_DESKTOP=Hyprland XDG_SESSION_TYPE=wayland" \
    "$PID_NORM" \
    "json_v2_modifier_ambiguity" \
    "typed ModifierAmbiguity on Hyprland binding in schema-v2" \
    ctrl+c --json-v2

# 5.3 Missing terminal bytes with a logical tmux candidate
cat > "$MOCK_DIR/tmux" <<'EOF'
#!/bin/sh
case "$*" in
  *list-keys*root*) printf '%s\n' 'bind-key -T root M-Enter split-window -v' ;;
  *list-keys*) printf '%s\n' '' ;;
  *display-message*) printf '%s\n' 'root' ;;
  *) exit 0 ;;
esac
EOF
chmod +x "$MOCK_DIR/tmux"

cat > "$MOCK_DIR/ghostty" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$MOCK_DIR/ghostty"

run_check_advanced "inspect tmux candidate unpredicted bytes text" \
    "PATH=$MOCK_PATH TMUX=/tmp/mock-tmux,1,0 TMUX_PANE=%0 TERM_PROGRAM=ghostty" \
    "$PID_NORM" "" "" \
    alt+return --verbose

run_check_advanced "inspect tmux candidate unpredicted bytes json-v2" \
    "PATH=$MOCK_PATH TMUX=/tmp/mock-tmux,1,0 TMUX_PANE=%0 TERM_PROGRAM=ghostty" \
    "$PID_NORM" "" "" \
    alt+return --json-v2

# 5.4 Predicted terminal bytes
run_check_advanced "inspect predicted terminal bytes text" \
    "PATH=$MOCK_PATH TERM_PROGRAM=ghostty" \
    "$PID_NORM" "" "" \
    ctrl+z

run_check_advanced "inspect predicted terminal bytes verbose" \
    "PATH=$MOCK_PATH TERM_PROGRAM=ghostty" \
    "$PID_NORM" "" "" \
    ctrl+z --verbose

run_check_advanced "inspect predicted terminal bytes json-v2" \
    "PATH=$MOCK_PATH TERM_PROGRAM=ghostty" \
    "$PID_NORM" "" "" \
    ctrl+z --json-v2

# 5.5 Selected application versus parent shell (--pid on non-shell target)
sleep 45 &
TARGET_APP_PID=$!

APP_PID_NORM="s/target pid: ${TARGET_APP_PID}/target pid: <TARGET_PID>/g; s/target process ${TARGET_APP_PID}/target process <TARGET_PID>/g; s/\(${TARGET_APP_PID}\)/(<TARGET_PID>)/g"

run_check_advanced "inspect selected app non-shell text" \
    "PATH=$MOCK_PATH SHELL=/bin/bash WHYKEY_READLINE_BINDINGS=unix-word-rubout" \
    "$APP_PID_NORM" "" "" \
    inspect --pid "$TARGET_APP_PID" ctrl+w --verbose

run_check_advanced "inspect selected app non-shell json-v2" \
    "PATH=$MOCK_PATH SHELL=/bin/bash WHYKEY_READLINE_BINDINGS=unix-word-rubout" \
    "$APP_PID_NORM" "" "" \
    inspect --pid "$TARGET_APP_PID" ctrl+w --json-v2

kill "$TARGET_APP_PID" 2>/dev/null || true

# 5.6 An explicitly selected shell (--pid on bash process)
bash < <(sleep 45) &
TARGET_SHELL_PID=$!

SHELL_PID_NORM="s/target pid: ${TARGET_SHELL_PID}/target pid: <TARGET_PID>/g; s/target process ${TARGET_SHELL_PID}/target process <TARGET_PID>/g; s/\(${TARGET_SHELL_PID}\)/(<TARGET_PID>)/g"

run_check_advanced "inspect selected shell target text" \
    "PATH=$MOCK_PATH SHELL=/bin/bash WHYKEY_READLINE_BINDINGS=reverse-search-history" \
    "$SHELL_PID_NORM" "" "" \
    inspect --pid "$TARGET_SHELL_PID" ctrl+r --verbose

run_check_advanced "inspect selected shell target json-v2" \
    "PATH=$MOCK_PATH SHELL=/bin/bash WHYKEY_READLINE_BINDINGS=reverse-search-history" \
    "$SHELL_PID_NORM" "" "" \
    inspect --pid "$TARGET_SHELL_PID" ctrl+r --json-v2

kill "$TARGET_SHELL_PID" 2>/dev/null || true

# 5.7 Missing optional shell context with useful upstream evidence
run_check_advanced "inspect missing shell context text" \
    "PATH=$MOCK_PATH SHELL=" \
    "$PID_NORM" "" "" \
    ctrl+z

run_check_advanced "inspect missing shell context json-v2" \
    "PATH=$MOCK_PATH SHELL=" \
    "$PID_NORM" "" "" \
    ctrl+z --json-v2

# 5.8 Multiplexer / session context reuse
run_check_advanced "inspect session multiplexer context text" \
    "PATH=$MOCK_PATH TMUX=/tmp/mock-tmux,1,0 TMUX_PANE=%0" \
    "$PID_NORM" "" "" \
    ctrl+b

# 5.9 Diagnostic budget exhaustion
run_check_advanced "inspect diagnostic budget exhausted text" \
    "PATH=$MOCK_PATH WHYKEY_DIAGNOSTIC_TIMEOUT_MS=0" \
    "" "" "" \
    ctrl+z

run_check_advanced "inspect diagnostic budget exhausted json-v2" \
    "PATH=$MOCK_PATH WHYKEY_DIAGNOSTIC_TIMEOUT_MS=0" \
    "" "" "" \
    ctrl+z --json-v2

echo "---------------------------------------------------------"
identical_count=$((passed - expected_diffs))
if [[ "$expected_diffs" -gt 0 ]]; then
    echo "Differential comparison summary: $passed passed ($identical_count identical, $expected_diffs with documented expected difference), $failed failed."
else
    echo "Differential comparison summary: $passed passed ($identical_count identical), $failed failed."
fi
echo "Results and detailed diffs stored in: $DIFF_DIR"

if [[ "$failed" -gt 0 ]]; then
    exit 1
fi
exit 0
