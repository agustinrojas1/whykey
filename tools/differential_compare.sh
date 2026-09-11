#!/usr/bin/env bash
set -euo pipefail

# Keep the public entry point stable; comparison, isolation, cleanup, and the
# declarative scenario matrix live in the standard-library Python runner.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$SCRIPT_DIR/differential_compare.py" "$@"