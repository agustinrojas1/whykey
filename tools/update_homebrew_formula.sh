#!/usr/bin/env bash
set -euo pipefail

# Point the agustinrojas1/homebrew-tap formula at a published release: rewrite
# each release URL to VERSION and the sha256 after it to that archive's
# checksum from the release .sha256 file.
#
#   tools/update_homebrew_formula.sh VERSION FORMULA
#
# FORMULA is the path to Formula/whykey.rb in a checkout of the tap.

if [[ $# -ne 2 ]]; then
  echo "usage: $0 VERSION FORMULA" >&2
  exit 2
fi
version=$1
formula=$2
release_url="https://github.com/agustinrojas1/whykey/releases/download/v${version}"

declare -A sums
for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  sum=$(curl -fsSL "${release_url}/whykey-${version}-${target}.tar.gz.sha256" | awk '{print $1}')
  if [[ ! "$sum" =~ ^[0-9a-f]{64}$ ]]; then
    echo "no valid checksum for ${target} in release v${version}" >&2
    exit 1
  fi
  sums[$target]=$sum
done

updated=$(mktemp)
trap 'rm -f "$updated"' EXIT
awk -v version="$version" -v x86="${sums[x86_64-unknown-linux-musl]}" \
  -v arm="${sums[aarch64-unknown-linux-musl]}" '
  /^ *url "https:\/\/github.com\/agustinrojas1\/whykey\/releases\/download\// {
    gsub(/\/v[0-9][^\/]*\/whykey-[0-9][^-]*-/, "/v" version "/whykey-" version "-")
    pending = /x86_64-unknown-linux-musl/ ? x86 : /aarch64-unknown-linux-musl/ ? arm : ""
    replaced += pending != ""
    print
    next
  }
  pending != "" && /^ *sha256 "/ {
    sub(/"[0-9a-f]+"/, "\"" pending "\"")
    pending = ""
  }
  { print }
  END { if (replaced != 2) exit 1 }
' "$formula" >"$updated" || {
  echo "$formula does not have exactly the x86_64 and aarch64 musl URLs" >&2
  exit 1
}
cat "$updated" >"$formula"
