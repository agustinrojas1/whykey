#!/bin/sh
# Installs the pre-compositor remappers into /usr/local/bin. Every upstream
# artifact is pinned by version and sha256; a mismatch fails the image build.
#   kanata 1.12.0      jtroo/kanata  linux-binaries-x64.zip
#   xremap 0.15.14     xremap/xremap x86_64 "hypr" build (evdev grab)
#   kmonad 0.4.5       kmonad/kmonad static linux binary
#   keyd 2.6.0         rvaiya/keyd   source tarball, built with make (no Fedora package)
set -eu
work=$(mktemp -d)
fetch() { # URL SHA256 FILE
    curl -fsSL -o "$work/$3" "$1"
    echo "$2  $work/$3" | sha256sum -c -
}
# _install.sh runs this before the dnf packages, so use python3 (in base)
# rather than unzip.
unzip_member() { # ZIP MEMBER DIR
    python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extract(sys.argv[2], sys.argv[3])' "$1" "$2" "$3"
}

fetch https://github.com/jtroo/kanata/releases/download/v1.12.0/linux-binaries-x64.zip \
    0bedd91567c5d7c54679061baadc37e4f83fb71750003999bc1d11f2c9754f36 kanata.zip
unzip_member "$work/kanata.zip" kanata_linux_x64 "$work"
install -m755 "$work/kanata_linux_x64" /usr/local/bin/kanata

fetch https://github.com/xremap/xremap/releases/download/v0.15.14/xremap-linux-x86_64-hypr.zip \
    387ac2163b297b8df72207e1bedd47915099576b8f49af8b2e2cf871bc797b63 xremap.zip
unzip_member "$work/xremap.zip" xremap "$work"
install -m755 "$work/xremap" /usr/local/bin/xremap

fetch https://github.com/kmonad/kmonad/releases/download/0.4.5/kmonad \
    0640b411338d2a94a759919089203d9d376e447554613b0914569f56e46afab7 kmonad
install -m755 "$work/kmonad" /usr/local/bin/kmonad

fetch https://github.com/rvaiya/keyd/archive/refs/tags/v2.6.0.tar.gz \
    697089681915b89d9e98caf93d870dbd4abce768af8a647d54650a6a90744e26 keyd.tar.gz
tar -xzf "$work/keyd.tar.gz" -C "$work"
make -C "$work/keyd-2.6.0" -s
install -m755 "$work/keyd-2.6.0/bin/keyd" /usr/local/bin/keyd
mkdir -p /etc/keyd

rm -rf "$work"
