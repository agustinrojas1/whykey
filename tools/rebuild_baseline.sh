#!/usr/bin/env bash
set -euo pipefail

# tools/rebuild_baseline.sh
# Rebuild a baseline whykey binary from a verified Git commit with recorded provenance.

usage() {
    cat <<'EOF'
Usage: tools/rebuild_baseline.sh [options] <git-revision> <destination-binary>

Arguments:
  <git-revision>        Git revision (commit, tag, branch) to build
  <destination-binary>  Output path for the baseline executable

Options:
  --profile <release|debug>  Cargo build profile (default: release)
  --toolchain <name>         Explicit rustup toolchain (default: 1.98.1)
  --force                    Overwrite destination binary if it already exists
  -h, --help                 Show this help message

Prerequisites & Scope Notice:
  - Rebuilding the baseline requires a full Git repository checkout where the
    target commit is available. A source distribution archive without .git
    history does not contain historical commits and cannot reconstruct the baseline.
  - The verified baseline identity is recorded in tools/baseline_manifest.json
    and is strictly scoped to the x86_64-unknown-linux-gnu target platform.
EOF
    exit 1
}

PROFILE="release"
TOOLCHAIN="1.98.1"
FORCE=0
REVISION=""
DEST_BINARY=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --profile)
            PROFILE="$2"
            shift 2
            ;;
        --toolchain)
            TOOLCHAIN="$2"
            shift 2
            ;;
        --force)
            FORCE=1
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
            if [[ -z "$REVISION" ]]; then
                REVISION="$1"
            elif [[ -z "$DEST_BINARY" ]]; then
                DEST_BINARY="$1"
            else
                echo "Error: Unexpected argument $1" >&2
                usage
            fi
            shift
            ;;
    esac
done

if [[ -z "$REVISION" || -z "$DEST_BINARY" ]]; then
    echo "Error: Both <git-revision> and <destination-binary> are required." >&2
    usage
fi

if [[ -e "$DEST_BINARY" && "$FORCE" -ne 1 ]]; then
    echo "Error: Destination '$DEST_BINARY' already exists. Use --force to overwrite." >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Resolve revision to full commit hash
COMMIT_ID=$(git -C "$REPO_ROOT" rev-parse --verify "${REVISION}^{commit}")
COMMIT_SUMMARY=$(git -C "$REPO_ROOT" log -1 --format="%s" "$COMMIT_ID")

echo "Resolving baseline revision: $REVISION -> $COMMIT_ID ($COMMIT_SUMMARY)"

WORK_DIR="$(mktemp -d -t whykey-baseline-build-XXXXXX)"
WORKTREE_DIR="$WORK_DIR/src"
TARGET_DIR="$WORK_DIR/target"

cleanup() {
    local exit_code=$?
    echo "Cleaning up temporary build resources..."
    if [[ -d "$WORKTREE_DIR" ]]; then
        git -C "$REPO_ROOT" worktree remove --force "$WORKTREE_DIR" >/dev/null 2>&1 || rm -rf "$WORKTREE_DIR"
    fi
    rm -rf "$WORK_DIR"
    exit "$exit_code"
}
trap cleanup EXIT INT TERM

# Create isolated worktree
mkdir -p "$TARGET_DIR"
git -C "$REPO_ROOT" worktree add --detach "$WORKTREE_DIR" "$COMMIT_ID" >/dev/null

# Verify worktree is clean
WORKTREE_STATUS=$(git -C "$WORKTREE_DIR" status --porcelain)
if [[ -n "$WORKTREE_STATUS" ]]; then
    echo "Error: Worktree at $COMMIT_ID is not clean:" >&2
    echo "$WORKTREE_STATUS" >&2
    exit 1
fi

# Toolchain validation
CARGO_CMD=(cargo "+$TOOLCHAIN")
RUSTC_V_V=$("${CARGO_CMD[@]}" rustc -- -Vv 2>/dev/null || rustc "+$TOOLCHAIN" -Vv)
CARGO_V=$("${CARGO_CMD[@]}" --version)

# Record Cargo.lock sha256 before build
CARGO_LOCK_SHA256=$(sha256sum "$WORKTREE_DIR/Cargo.lock" | awk '{print $1}')

# Build arguments
BUILD_ARGS=(build --locked)
if [[ "$PROFILE" == "release" ]]; then
    BUILD_ARGS+=(--release)
elif [[ "$PROFILE" != "debug" ]]; then
    BUILD_ARGS+=(--profile "$PROFILE")
fi

BUILD_COMMAND="cargo +$TOOLCHAIN ${BUILD_ARGS[*]}"
echo "Building baseline with: $BUILD_COMMAND (CARGO_TARGET_DIR=$TARGET_DIR)"

(
    cd "$WORKTREE_DIR"
    CARGO_TARGET_DIR="$TARGET_DIR" "${CARGO_CMD[@]}" "${BUILD_ARGS[@]}"
)

BUILT_BIN="$TARGET_DIR/$PROFILE/whykey"
if [[ ! -x "$BUILT_BIN" ]]; then
    echo "Error: Built binary not found at $BUILT_BIN" >&2
    exit 1
fi

BINARY_SHA256=$(sha256sum "$BUILT_BIN" | awk '{print $1}')
BUILD_DATE_UTC=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
UNAME_A=$(uname -a)

# Ensure destination directory exists
DEST_DIR="$(dirname "$DEST_BINARY")"
mkdir -p "$DEST_DIR"

# Copy binary to destination
cp "$BUILT_BIN" "$DEST_BINARY"
chmod 755 "$DEST_BINARY"

# Record provenance alongside binary
PROVENANCE_FILE="${DEST_BINARY}.provenance.json"
cat > "$PROVENANCE_FILE" <<EOF
{
  "source_commit": "$COMMIT_ID",
  "source_summary": "$COMMIT_SUMMARY",
  "clean_worktree": true,
  "build_command": "$BUILD_COMMAND",
  "profile": "$PROFILE",
  "toolchain": "$TOOLCHAIN",
  "cargo_version": "$CARGO_V",
  "rustc_verbose_version": $(echo "$RUSTC_V_V" | jq -R -s '.'),
  "cargo_lock_sha256": "$CARGO_LOCK_SHA256",
  "binary_sha256": "$BINARY_SHA256",
  "build_timestamp_utc": "$BUILD_DATE_UTC",
  "platform": "$UNAME_A",
  "reproducibility_notice": "Baseline artifact built from explicit commit in isolated temporary worktree. Cross-machine bit-for-bit identity is not claimed; runtime behavioral equivalence under controlled tests is evaluated."
}
EOF

echo "Baseline successfully built and verified:"
echo "  Binary:     $DEST_BINARY (SHA256: $BINARY_SHA256)"
echo "  Provenance: $PROVENANCE_FILE"

# Validate against tracked baseline manifest if building reference revision on x86_64 Linux
MANIFEST_FILE="$REPO_ROOT/tools/baseline_manifest.json"
if [[ -f "$MANIFEST_FILE" && "$COMMIT_ID" =~ ^fe52e36 && "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]]; then
    MANIFEST_SHA=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["binary_sha256"])' "$MANIFEST_FILE" 2>/dev/null || true)
    if [[ -n "$MANIFEST_SHA" ]]; then
        if [[ "$BINARY_SHA256" == "$MANIFEST_SHA" ]]; then
            echo "  Manifest:   Matches recorded baseline manifest in tools/baseline_manifest.json"
        else
            echo "Error: Built reference baseline binary SHA-256 ($BINARY_SHA256) does not match recorded manifest in tools/baseline_manifest.json ($MANIFEST_SHA)." >&2
            exit 1
        fi
    fi
fi
