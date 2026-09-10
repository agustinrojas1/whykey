#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 /absolute/path/to/whykey" >&2
    exit 2
fi

binary=$1
if [[ ! -x "$binary" ]]; then
    echo "not an executable: $binary" >&2
    exit 2
fi

root=$(mktemp -d "${TMPDIR:-/tmp}/whykey-clean-install.XXXXXX")
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/home" "$root/config" "$root/data" "$root/cache" "$root/runtime"

clean_env=(
    env -i
    HOME="$root/home"
    PATH="/usr/bin:/bin"
    XDG_CONFIG_HOME="$root/config"
    XDG_DATA_HOME="$root/data"
    XDG_CACHE_HOME="$root/cache"
    XDG_RUNTIME_DIR="$root/runtime"
)

version=$("${clean_env[@]}" "$binary" --version)
[[ "$version" == whykey\ * ]]

capabilities="$root/capabilities.json"
"${clean_env[@]}" "$binary" --json capabilities >"$capabilities"
python3 -c '
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    report = json.load(stream)
assert report["schema_version"] == 1
assert isinstance(report["capabilities"], list)
assert report["capabilities"]
' "$capabilities"

inspection="$root/inspection.json"
set +e
"${clean_env[@]}" "$binary" --json ctrl+z >"$inspection"
inspection_status=$?
set -e
[[ "$inspection_status" -eq 0 || "$inspection_status" -eq 1 ]]
python3 -c '
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    report = json.load(stream)
assert report["schema_version"] == 1
assert report["key_display"] == "CTRL + Z"
' "$inspection"

echo "clean install smoke passed: $version"
