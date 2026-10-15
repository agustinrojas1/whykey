# Lab image "x11" (lab/images.json): the X11 desktops and window managers whose
# bindings live in their own configs (Qtile, XMonad, Xfce, MATE, Cinnamon).
# Built FROM the base image (ARG BASE, see tools/lab_images.py).
ARG BASE
FROM ${BASE}

COPY packages/_install.sh /tmp/packages/
COPY packages/x11desktops.* /tmp/packages/
RUN sh /tmp/packages/_install.sh x11desktops
