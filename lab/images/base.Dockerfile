# Base of every lab image (lab/images.json): Fedora 44 with the families that
# all groups build on. Every group image is FROM this image (ARG BASE).
#
# Package families: lab/packages/<family>.txt lists dnf packages (every one
# available in Fedora 44 or its family's COPR, checked 2026-10-10), with an
# optional <family>.sh run first; lab/packages/_install.sh installs a family
# and strips file capabilities. One COPY + RUN pair per family, sorted by name.
# base: python3, dbus, Mesa; wayland: sway and the Wayland probe (foot, wtype);
# x11: Xvfb, xdotool, xev and the X11 window managers that need no desktop.
FROM registry.fedoraproject.org/fedora:44

COPY packages/_install.sh packages/base.* /tmp/packages/
RUN sh /tmp/packages/_install.sh base
COPY packages/wayland.* /tmp/packages/
RUN sh /tmp/packages/_install.sh wayland
COPY packages/x11.* /tmp/packages/
RUN sh /tmp/packages/_install.sh x11

# Scenarios run as root inside a throwaway container, with whykey mounted at
# /w/whykey and lab/ at /lab.
ENV XDG_RUNTIME_DIR=/tmp/rt LANG=C.UTF-8
WORKDIR /root
CMD ["/bin/bash"]
