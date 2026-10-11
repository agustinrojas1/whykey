# Fedora retired its Hyprland package; the sdegler/hyprland COPR builds the
# upstream releases for Fedora 44. If the COPR drops the pinned build, the
# image build fails rather than switching versions.
dnf -y -q install --setopt=install_weak_deps=False dnf-plugins-core
dnf -y -q copr enable sdegler/hyprland
