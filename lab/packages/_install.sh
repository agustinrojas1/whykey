#!/bin/sh
# Installs one package family of the lab image (lab/Dockerfile):
#   sh _install.sh FAMILY
# runs FAMILY.sh first when it exists (e.g. enable a COPR), installs the dnf
# packages listed in FAMILY.txt (one per line, # comments), then strips file
# capabilities and recompiles the GSettings schemas.
set -eu
dir=$(dirname "$0")
family=$1

if [ -f "$dir/$family.sh" ]; then
    sh -eu "$dir/$family.sh"
fi
packages=$(sed 's/#.*//' "$dir/$family.txt")
# Word splitting of $packages is intended: one argument per package.
# shellcheck disable=SC2086
dnf -y -q install --setopt=install_weak_deps=False $packages
dnf clean all

# File capabilities (sway ships cap_sys_nice) make exec fail with EPERM in an
# unprivileged container. Scenarios run as root inside a throwaway container.
for binary in /usr/bin/* /usr/libexec/*; do
    if [ -f "$binary" ] && getcap "$binary" 2>/dev/null | grep -q .; then
        setcap -r "$binary"
    fi
done

if ls /usr/share/glib-2.0/schemas/*.gschema.xml >/dev/null 2>&1; then
    glib-compile-schemas /usr/share/glib-2.0/schemas
fi
