#!/usr/bin/env bash
set -euo pipefail

# Create the source archive consumed by the native package recipes.  It has the
# project files, the locked Cargo dependency set, and a local Cargo
# configuration so downstream builders never contact crates.io.

if [[ $# -gt 1 ]]; then
    echo "usage: $0 [output-directory]" >&2
    exit 2
fi

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$root_dir"}
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root_dir/Cargo.toml" | head -n 1)
archive_root="whykey-${version}"
source_date_epoch=$(git -C "$root_dir" log -1 --format=%ct)
work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

mkdir -p "$output_dir" "$work_dir/$archive_root/.cargo"

(
    cd "$root_dir"
    while IFS= read -r -d '' path; do
        if [[ -e "$path" || -L "$path" ]]; then
            printf '%s\0' "$path"
        fi
    done < <(git ls-files -co --exclude-standard -z) \
        | tar --null --files-from=- --create --file=- \
        | tar --extract --file=- --directory="$work_dir/$archive_root"
)

(
    cd "$root_dir"
    if ! cargo vendor --locked "$work_dir/$archive_root/vendor" >"$work_dir/vendor.log" 2>&1; then
        cat "$work_dir/vendor.log" >&2
        exit 1
    fi
)

cat >"$work_dir/$archive_root/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

archive="$output_dir/${archive_root}-source.tar.gz"
checksum="$archive.sha256"
tar --create --gzip --file="$archive" \
    --directory="$work_dir" \
    --sort=name \
    --mtime="@${source_date_epoch}" \
    --owner=0 --group=0 --numeric-owner \
    "$archive_root"
(cd "$output_dir" && sha256sum "$(basename "$archive")") >"$checksum"

echo "Created $archive"
echo "Created $checksum"
