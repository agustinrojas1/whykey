#!/usr/bin/env bash
set -euo pipefail

# Write AUR-ready package directories for a published release: whykey (built
# from the offline source archive) and whykey-bin (prebuilt binaries). The
# upstream recipes keep SKIP checksums because the release archives contain
# packaging/; this fills them in from the release .sha256 files and writes
# .SRCINFO. Each output directory is the content of one AUR Git repository.
#
#   tools/prepare_aur.sh [VERSION] [OUTDIR]
#
# VERSION defaults to the Cargo.toml version, OUTDIR to target/aur.

for command_name in curl makepkg; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "prepare_aur requires: $command_name" >&2
    exit 77
  fi
done

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root_dir/Cargo.toml" | head -n 1)}
out_dir=${2:-$root_dir/target/aur}
release_url="https://github.com/agustinrojas1/whykey/releases/download/v${version}"

release_sha256() {
  local asset=$1 sum
  sum=$(curl -fsSL "${release_url}/${asset}.sha256" | awk '{print $1}')
  if [[ ! "$sum" =~ ^[0-9a-f]{64}$ ]]; then
    echo "no valid checksum for ${asset} in release v${version}" >&2
    exit 1
  fi
  printf '%s' "$sum"
}

# Writes $2/PKGBUILD from the upstream recipe $1 at $version, replacing each
# "name=('SKIP')" given as name=sum in the remaining arguments.
write_package() {
  local recipe=$1 dir=$2
  shift 2
  mkdir -p "$dir"
  sed -e "s/^pkgver=.*/pkgver=${version}/" -e 's/^pkgrel=.*/pkgrel=1/' \
    "$root_dir/packaging/$recipe" >"$dir/PKGBUILD"
  local pair
  for pair in "$@"; do
    sed -i "s/^${pair%%=*}=('SKIP')/${pair%%=*}=('${pair#*=}')/" "$dir/PKGBUILD"
  done
  if grep -q "'SKIP'" "$dir/PKGBUILD"; then
    echo "$dir/PKGBUILD still has a SKIP checksum" >&2
    exit 1
  fi
  (cd "$dir" && makepkg --printsrcinfo >.SRCINFO)
  echo "wrote $dir"
}

write_package PKGBUILD "$out_dir/whykey" \
  "sha256sums=$(release_sha256 "whykey-${version}-source.tar.gz")"
write_package PKGBUILD-bin "$out_dir/whykey-bin" \
  "sha256sums_x86_64=$(release_sha256 "whykey-${version}-x86_64-unknown-linux-gnu.tar.gz")" \
  "sha256sums_aarch64=$(release_sha256 "whykey-${version}-aarch64-unknown-linux-musl.tar.gz")"
