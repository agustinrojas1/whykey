#!/bin/sh
# Fedora does not package ghostty or wezterm; enable the COPRs that do.
set -eu
dnf -y -q install --setopt=install_weak_deps=False dnf-plugins-core
dnf -y -q copr enable scottames/ghostty
dnf -y -q copr enable wezfurlong/wezterm-nightly
