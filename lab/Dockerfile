# whykey reality lab: real compositors, window managers, hotkey daemons,
# desktop settings stores and shells, driven by synthetic input inside the
# container only. Build: docker build -t whykey-lab lab/
FROM registry.fedoraproject.org/fedora:44

# Every package below is available in Fedora 44 (checked 2026-10-10).
# "gnome" scenarios run mutter headless (GNOME Shell aborts without
# systemd-logind) and type through mutter's RemoteDesktop D-Bus API with
# python3-dbus (lab/rdkbd.py); gnome-shell is installed for its GSettings
# schemas, which whykey reads.
RUN dnf -y -q install --setopt=install_weak_deps=False \
        sway wtype foot wev libxkbcommon-utils \
        xorg-x11-server-Xvfb xdotool xev xinput setxkbmap xkbcomp xterm \
        i3 openbox xbindkeys sxhkd awesome \
        gsettings-desktop-schemas gnome-shell mutter gnome-settings-daemon dconf dbus-daemon \
        kf6-kglobalaccel kglobalacceld \
        python3 python3-dbus mesa-dri-drivers \
        bash zsh fish tmux screen neovim \
        procps-ng util-linux-script libcap acl \
    && dnf clean all \
    && for binary in /usr/bin/* /usr/libexec/*; do \
           if [ -f "$binary" ] && getcap "$binary" 2>/dev/null | grep -q .; then \
               setcap -r "$binary"; \
           fi; \
       done \
    && glib-compile-schemas /usr/share/glib-2.0/schemas

# Hyprland: Fedora retired its package; the sdegler/hyprland COPR builds the
# upstream releases for Fedora 44 (0.56.2 = the main user's desktop; both the
# hyprlang .conf and the Lua config are supported by this release). The
# version is pinned so a COPR update cannot silently change what the lab
# verifies. aquamarine needs a KMS device even for headless outputs, so
# Hyprland scenarios boot this image's kernel in a KVM guest (virtme-ng with
# the container as 9p root, virtio-gpu for KMS, llvmpipe via kms_swrast); see
# lab/driver.py. file and binutils are virtme-ng's kernel-version probes.
RUN dnf -y -q install --setopt=install_weak_deps=False dnf-plugins-core \
    && dnf -y -q copr enable sdegler/hyprland \
    && dnf -y -q install --setopt=install_weak_deps=False \
        hyprland-0.56.2 \
        virtme-ng qemu-system-x86-core qemu-device-display-virtio-gpu-pci \
        kernel-core busybox file binutils \
    && dnf clean all

# File capabilities (sway ships cap_sys_nice) make exec fail with EPERM in an
# unprivileged container; the loop above strips them. Scenarios run as root
# inside a throwaway container, with whykey mounted at /w/whykey.
ENV XDG_RUNTIME_DIR=/tmp/rt LANG=C.UTF-8
WORKDIR /root
CMD ["/bin/bash"]
