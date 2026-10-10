# Distribution packages

The release includes a source archive with the exact Cargo dependencies needed
for an offline build. It is the input for the Arch, Debian, and RPM recipes.
Verify its accompanying `.sha256` file before building a package.

## Arch Linux

Copy `PKGBUILD` next to the source archive and run `makepkg -si` as a regular
user. The upstream recipe uses `SKIP` because a source archive cannot contain
its own checksum. Before publishing it to the AUR, replace `SKIP` with the
checksum from the release.

`bash tools/arch_package_smoke.sh` builds the recipe, extracts the package, and
runs the installed-binary smoke test. It exits with status 77 outside an Arch
build environment.

## Fedora COPR

`whykey.spec` builds from the same offline source release with Fedora's Rust
macros. It is suitable for a Fedora COPR project. COPR is not the official
Fedora repository, and no COPR project has been published yet.

For a local RPM build, place the source archive in the RPM source directory and
run `rpmbuild -ba packaging/whykey.spec`. The spec records bundled Rust crates
for vulnerability tracking.

## Debian and Ubuntu

Extract the source release, then run `dpkg-buildpackage -us -uc -b`. The
native Debian package definition is in the source root, as Debian expects. It
needs Cargo, Rust 1.85 or newer, and debhelper 13. The release source already
has the vendor directory needed for an offline build.

## Nix and NixOS

From a checkout, run `nix-build packaging/whykey.nix`. Nix fetches the exact
crates from `Cargo.lock` through its fixed-output dependency builder.

None of these recipes install a service, udev rule, input-group membership, or
desktop autostart entry. Physical capture remains an explicit read-only action.

Every recipe also installs the first-party `whykey-nvim`, `whykey-vscode`, and
`whykey-emacs` extensions. They are `#!/usr/bin/env python3` scripts, so the
recipes declare `python3` as optional for them (Debian and RPM `Recommends`,
Arch `optdepends`). The Nix recipe patches their shebangs to its `python3`.
The `whykey` binary itself needs no Python.

The Wayland and X11 clients (`wayland-client`, `x11rb`) use their pure-Rust
backends, so the binary links no libwayland or libxcb at runtime, and
packages need no extra runtime dependencies for the probe window or the X11
grab probe. The vendor directory in the offline source archive is about
65 MB; about 18 MB of that is `windows-sys`, which is vendored but never
compiled on Linux.
The `lab/` directory is excluded from the crate and is not needed to build.

The PKGBUILD lists `x86_64` and `aarch64` because it compiles the vendored
source archive. Prebuilt release binaries are only published for
`x86_64-unknown-linux-gnu`.
