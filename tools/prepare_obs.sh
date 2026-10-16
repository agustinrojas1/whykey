#!/usr/bin/env bash
set -euo pipefail

# Write the files of the home:agustinrojas/whykey package on the openSUSE Build
# Service for a published release: the RPM spec and source archive, and a
# Debian native source package (.dsc and .tar.xz) made from the same archive.
#
#   tools/prepare_obs.sh [VERSION] [OUTDIR]
#
# VERSION defaults to the Cargo.toml version, OUTDIR to target/obs. Copy the
# files into an `osc checkout` of the package, then `osc addremove` and
# `osc commit`.
#
# The Debian tarball is packed with tar rather than by dpkg-source, which
# drops .git* files; vendored crates list such files in .cargo-checksum.json
# (vendor/yaml-rust2/.gitmodules), and Cargo refuses a vendor directory with a
# missing file.

for command_name in curl dpkg-source sha256sum tar xz; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "prepare_obs requires: $command_name" >&2
    exit 77
  fi
done

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root_dir/Cargo.toml" | head -n 1)}
out_dir=${2:-$root_dir/target/obs}
archive="whykey-${version}-source.tar.gz"
release_url="https://github.com/agustinrojas1/whykey/releases/download/v${version}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out_dir"

curl -fsSL -o "$out_dir/$archive" "$release_url/$archive"
expected=$(curl -fsSL "$release_url/$archive.sha256" | awk '{print $1}')
echo "$expected  $out_dir/$archive" | sha256sum -c --quiet -

sed -e "s/^Version:\([[:space:]]*\).*/Version:\1${version}/" \
  "$root_dir/packaging/whykey.spec" >"$out_dir/whykey.spec"

tar -xzf "$out_dir/$archive" -C "$work"
# Like the spec, the Debian recipe comes from this checkout, so packaging
# fixes reach OBS without a new release.
debian_version=$(sed -n '1s/.*(\([^)]*\)).*/\1/p' "$root_dir/debian/changelog")
if [[ "$debian_version" != "$version" ]]; then
  echo "debian/changelog is at $debian_version, expected $version" >&2
  exit 1
fi
rm -rf "$work/whykey-${version}/debian"
cp -a "$root_dir/debian" "$work/whykey-${version}/debian"
(cd "$work" && dpkg-source --build "whykey-${version}" >/dev/null)
debian_tar="whykey_${version}.tar.xz"
tar -C "$work" --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 \
  -cJf "$out_dir/$debian_tar" "whykey-${version}"

size=$(stat -c %s "$out_dir/$debian_tar")
md5=$(md5sum "$out_dir/$debian_tar" | cut -d ' ' -f 1)
sha1=$(sha1sum "$out_dir/$debian_tar" | cut -d ' ' -f 1)
sha256=$(sha256sum "$out_dir/$debian_tar" | cut -d ' ' -f 1)
awk -v sha1="$sha1" -v sha256="$sha256" -v md5="$md5" -v line="$size $debian_tar" '
  section == "Checksums-Sha1:" { print " " sha1 " " line; section = ""; next }
  section == "Checksums-Sha256:" { print " " sha256 " " line; section = ""; next }
  section == "Files:" { print " " md5 " " line; section = ""; next }
  /^(Checksums-Sha1|Checksums-Sha256|Files):$/ { section = $0 }
  { print }
' "$work/whykey_${version}.dsc" >"$out_dir/whykey_${version}.dsc"

echo "wrote $out_dir"
