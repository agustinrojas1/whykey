# Distribution packages

The release includes a source archive with the exact Cargo dependencies needed
for an offline build. It is the input for the Arch, Debian, and RPM recipes.
The archive holds only files tracked by Git, plus `vendor/` and the
`.cargo/config.toml` that points Cargo at it. Verify its accompanying `.sha256`
file before building a package.

Every recipe runs the test suite during the build. The suite does not depend
on the build host's process tree, so it passes in containers and in the Nix
sandbox. It needs `python3` to run the extension tests.

## Arch Linux

There are two recipes. `PKGBUILD` builds `whykey` from the offline source
archive; `PKGBUILD-bin` packages the prebuilt release binaries as `whykey-bin`
(the glibc 2.28 build on x86_64, the static musl build on aarch64). Both
install the same files.

To build locally, copy `PKGBUILD` next to the source archive and run
`makepkg -si` as a regular user. The upstream recipes use `SKIP` because the
release archives contain `packaging/` and so cannot contain their own
checksums.

`tools/prepare_aur.sh [VERSION]` writes `target/aur/whykey` and
`target/aur/whykey-bin`: each holds a `PKGBUILD` with the release checksums
filled in and its `.SRCINFO`, ready to commit to the matching AUR Git
repository.

`bash tools/arch_package_smoke.sh` builds the recipe, extracts the package, and
runs the installed-binary smoke test. It exits with status 77 outside an Arch
build environment.

## Fedora and openSUSE

`whykey.spec` builds from the same offline source release. On Fedora it uses
Fedora's Rust macros and records the bundled Rust crates for vulnerability
tracking; on other RPM distributions (openSUSE) it calls Cargo directly. It
drops the automatic `/usr/bin/python3` requirement that the extension shebangs
would add, so `python3` stays a `Recommends`.

For a local RPM build, place the source archive in the RPM source directory and
run `rpmbuild -ba packaging/whykey.spec`.

## openSUSE Build Service

The [home:agustinrojas/whykey](https://build.opensuse.org/package/show/home:agustinrojas/whykey)
package builds the spec and the Debian recipe for Fedora 43, 44 and Rawhide,
openSUSE Tumbleweed and Leap 16.0, Debian 13, and Ubuntu 26.04 and 26.10, on
x86_64 and aarch64. Older Debian and Ubuntu releases ship a Rust older than
1.85.

`tools/prepare_obs.sh [VERSION]` writes the package files to `target/obs`:
the release source archive, the spec, and a Debian native source package
(`.dsc` and `.tar.xz`). The spec and `debian/` come from the checkout, the
code from the release archive. Copy them into an `osc checkout` of the
package, then run `osc addremove` and `osc commit`.

## Debian and Ubuntu

Extract the source release, then run `dpkg-buildpackage -us -uc -b`. The
native Debian package definition is in the source root, as Debian expects. It
needs Cargo, Rust 1.85 or newer, debhelper 13, and `python3` and `bsdutils`
(for `script`) for the tests (`<!nocheck>`; skip them and the tests with the
`nocheck` build profile).
`apt-get build-dep ./` installs exactly these. The release source already has
the vendor directory needed for an offline build. The upstream `CHANGELOG.md`
is installed as `/usr/share/doc/whykey/CHANGELOG.md.gz`, next to the Debian
`changelog.gz`.

## Homebrew

The [agustinrojas1/homebrew-tap](https://github.com/agustinrojas1/homebrew-tap)
formula installs the static musl release binaries and the extensions on Linux.
Update its URLs and checksums after each release.

## Nix and NixOS

From a checkout, run `nix-build packaging` (it uses `packaging/default.nix`
with `<nixpkgs>`), or call the recipe from your own Nix expression with
`callPackage ./packaging/whykey.nix { }`. Nix fetches the exact crates from
`Cargo.lock` through its fixed-output dependency builder. The build copies the
checkout without `target/` and `result`.

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
source archive. Prebuilt release binaries are published for
`x86_64-unknown-linux-gnu` (glibc 2.28 or newer), and as fully static binaries
for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.
