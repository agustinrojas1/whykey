# Lab image "wayland" (lab/images.json): compositors with their own keybinding
# configs (niri, labwc, wayfire, dwl) and the input methods (fcitx5, IBus).
# Built FROM the base image (ARG BASE, see tools/lab_images.py); one COPY + RUN
# pair per family, sorted by name, as in base.Dockerfile.
ARG BASE
FROM ${BASE}

COPY packages/_install.sh /tmp/packages/
COPY packages/ime.* /tmp/packages/
RUN sh /tmp/packages/_install.sh ime
COPY packages/waylandwms.* /tmp/packages/
RUN sh /tmp/packages/_install.sh waylandwms
