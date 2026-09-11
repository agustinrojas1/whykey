#!/usr/bin/env bash
set -euo pipefail

# Build the Arch recipe from a source tarball, extract the resulting package
# without installing system-wide, and run the same isolated binary smoke gate
# used for Cargo installs. Run this on an Arch build host as a non-root user.

if [[ "$(id -u)" == 0 ]]; then
  echo "arch package smoke must not run as root" >&2
  exit 2
fi

for command_name in makepkg fakeroot bsdtar cargo; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "arch package smoke requires: $command_name" >&2
    exit 77
  fi
done

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root_dir/Cargo.toml" | head -n 1)
package_name="whykey-${version}"
build_root=$(mktemp -d)
trap 'rm -rf "$build_root"' EXIT
build_dir="$build_root/$package_name"
mkdir -p "$build_dir"

cp "$root_dir/packaging/PKGBUILD" "$build_dir/PKGBUILD"
"$root_dir/tools/assemble_source_archive.sh" "$build_dir"

(
  cd "$build_dir"
  makepkg --nodeps --noconfirm --cleanbuild
)

package_file=$(find "$build_dir" -maxdepth 1 -type f \
  -name "whykey-*-x86_64.pkg.tar.*" \
  ! -name '*-debug-*' -print -quit)
if [[ -z "$package_file" ]]; then
  echo "makepkg did not produce the expected x86_64 package" >&2
  exit 1
fi

install_root="$build_root/extracted"
mkdir -p "$install_root"
bsdtar -xf "$package_file" -C "$install_root"
test -x "$install_root/usr/bin/whykey"
test -x "$install_root/usr/bin/whykey-nvim"
test -x "$install_root/usr/bin/whykey-vscode"
test -x "$install_root/usr/bin/whykey-emacs"
test -s "$install_root/usr/share/doc/whykey/DEPENDENCY_PROVENANCE.md"
test -s "$install_root/usr/share/doc/whykey/SUPPORT_MATRIX.md"
bash "$root_dir/tools/clean_install_smoke.sh" "$install_root/usr/bin/whykey"
echo "Arch package smoke passed: $package_file"
