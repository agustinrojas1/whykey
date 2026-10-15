# Lab image "tty" (lab/images.json): terminal emulators, shells, multiplexers,
# terminal editors, Emacs and Neovim. Built FROM the base image (ARG BASE, see
# tools/lab_images.py); one COPY + RUN pair per family, sorted by name.
ARG BASE
FROM ${BASE}

COPY packages/_install.sh /tmp/packages/
COPY packages/editors.* /tmp/packages/
RUN sh /tmp/packages/_install.sh editors
COPY packages/shells.* /tmp/packages/
RUN sh /tmp/packages/_install.sh shells
COPY packages/termapps.* /tmp/packages/
RUN sh /tmp/packages/_install.sh termapps
COPY packages/terminals.* /tmp/packages/
RUN sh /tmp/packages/_install.sh terminals
