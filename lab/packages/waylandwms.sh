# dwl has no Fedora package and configures its keys at compile time, so the
# image builds the pinned upstream release (codeberg.org/dwl/dwl v0.9, which
# targets wlroots 0.20) with its default config.h. Scenarios that need other
# bindings rebuild it from their own config.h at container start.
dnf -y -q install --setopt=install_weak_deps=False \
    curl gcc make pkgconf-pkg-config wayland-devel wayland-protocols-devel \
    libxkbcommon-devel libinput-devel wlroots-devel
mkdir -p /usr/local/src/dwl
curl -fsSL https://codeberg.org/dwl/dwl/archive/v0.9.tar.gz |
    tar -xz -C /usr/local/src/dwl --strip-components=1
cp /usr/local/src/dwl/config.def.h /usr/local/src/dwl/config.h
make -C /usr/local/src/dwl
make -C /usr/local/src/dwl install
# wayfire refuses to start as root ("Unable to drop root"), and the lab runs as
# root, so its plugin runs wayfire as this user with its own runtime directory.
id wayfire >/dev/null 2>&1 || useradd --create-home wayfire
