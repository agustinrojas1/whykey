#!/bin/sh
# Zellij is not packaged for Fedora 44, so install the pinned upstream release
# binary (v0.45.1, x86_64 musl build). The tarball's sha256 is pinned; the
# binary inside matches the release's own published .sha256sum
# (d006c521dcb475a6005d741e9dd7c5758e5a23b28dd60a5c10cebfa4876319dd).
set -eu
version=0.45.1
sha256=40bcc2e03f5d5ae8e054e39f676081fe12ab70871506996ba595834c3718eefc
work=$(mktemp -d)
curl -fsSL -o "$work/zellij.tar.gz" \
    "https://github.com/zellij-org/zellij/releases/download/v$version/zellij-x86_64-unknown-linux-musl.tar.gz"
echo "$sha256  $work/zellij.tar.gz" | sha256sum -c -
tar -xzf "$work/zellij.tar.gz" -C "$work"
install -m 0755 "$work/zellij" /usr/local/bin/zellij
rm -rf "$work"
zellij --version
