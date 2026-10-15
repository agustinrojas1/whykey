# Lab image "gnome" (lab/images.json): headless mutter, gnome-shell and
# gnome-settings-daemon. Built FROM the base image (ARG BASE, see
# tools/lab_images.py).
ARG BASE
FROM ${BASE}

COPY packages/_install.sh /tmp/packages/
COPY packages/gnome.* /tmp/packages/
RUN sh /tmp/packages/_install.sh gnome
