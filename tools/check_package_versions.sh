#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root_dir/Cargo.toml" | head -n 1)

check() {
    local file=$1
    local actual=$2
    if [[ "$actual" != "$version" ]]; then
        echo "$file has version $actual, expected $version" >&2
        exit 1
    fi
}

check packaging/PKGBUILD "$(sed -n 's/^pkgver=//p' "$root_dir/packaging/PKGBUILD")"
check packaging/whykey.spec "$(sed -n 's/^Version:[[:space:]]*//p' "$root_dir/packaging/whykey.spec")"
check packaging/whykey.nix "$(sed -n 's/^[[:space:]]*version = "\([^"]*\)";.*/\1/p' "$root_dir/packaging/whykey.nix")"
check debian/changelog "$(sed -n '1s/.*(\([^)]*\)).*/\1/p' "$root_dir/debian/changelog")"
