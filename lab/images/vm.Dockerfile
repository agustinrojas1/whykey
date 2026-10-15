# Lab image "vm" (lab/images.json): everything that boots the KVM guest with
# the image's own kernel (virtme-ng; hyprland.txt holds the VM tooling):
# Hyprland, KWin with its kglobalacceld (also run by the x11 "kde" scenarios),
# and the remappers. Built FROM the base image (ARG BASE, see
# tools/lab_images.py); one COPY + RUN pair per family, sorted by name.
ARG BASE
FROM ${BASE}

COPY packages/_install.sh /tmp/packages/
COPY packages/hyprland.* /tmp/packages/
RUN sh /tmp/packages/_install.sh hyprland
COPY packages/kde.* /tmp/packages/
RUN sh /tmp/packages/_install.sh kde
COPY packages/kwin.* /tmp/packages/
RUN sh /tmp/packages/_install.sh kwin
COPY packages/remap.* /tmp/packages/
RUN sh /tmp/packages/_install.sh remap
