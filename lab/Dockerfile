# whykey reality lab: real compositors, window managers, hotkey daemons,
# desktop settings stores and shells, driven by synthetic input inside the
# container only. Build: docker build -t whykey-lab lab/
FROM registry.fedoraproject.org/fedora:44

# Package families: lab/packages/<family>.txt lists dnf packages (every one
# available in Fedora 44 or its family's COPR, checked 2026-10-10), with an
# optional <family>.sh run first; lab/packages/_install.sh installs a family
# and strips file capabilities. One layer per family: base, then the others
# sorted by name. A new family adds its COPY + RUN pair at its sorted place.
COPY packages/_install.sh packages/base.* /tmp/packages/
RUN sh /tmp/packages/_install.sh base
COPY packages/editors.* /tmp/packages/
RUN sh /tmp/packages/_install.sh editors
COPY packages/gnome.* /tmp/packages/
RUN sh /tmp/packages/_install.sh gnome
COPY packages/hyprland.* /tmp/packages/
RUN sh /tmp/packages/_install.sh hyprland
COPY packages/ime.* /tmp/packages/
RUN sh /tmp/packages/_install.sh ime
COPY packages/kde.* /tmp/packages/
RUN sh /tmp/packages/_install.sh kde
COPY packages/kwin.* /tmp/packages/
RUN sh /tmp/packages/_install.sh kwin
COPY packages/shells.* /tmp/packages/
RUN sh /tmp/packages/_install.sh shells
COPY packages/termapps.* /tmp/packages/
RUN sh /tmp/packages/_install.sh termapps
COPY packages/terminals.* /tmp/packages/
RUN sh /tmp/packages/_install.sh terminals
COPY packages/wayland.* /tmp/packages/
RUN sh /tmp/packages/_install.sh wayland
COPY packages/waylandwms.* /tmp/packages/
RUN sh /tmp/packages/_install.sh waylandwms
COPY packages/x11.* /tmp/packages/
RUN sh /tmp/packages/_install.sh x11
COPY packages/x11desktops.* /tmp/packages/
RUN sh /tmp/packages/_install.sh x11desktops

# Scenarios run as root inside a throwaway container, with whykey mounted at
# /w/whykey and lab/ at /lab.
ENV XDG_RUNTIME_DIR=/tmp/rt LANG=C.UTF-8
WORKDIR /root
CMD ["/bin/bash"]
