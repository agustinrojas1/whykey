# Whykey support matrix

This file is generated from [`support-matrix.json`](support-matrix.json) by
[`tools/generate_support_matrix.py`](tools/generate_support_matrix.py).
Do not edit it by hand. Runtime availability is environment-dependent and
is reported by `whykey capabilities`; this matrix records the implementation
contract shared by the capability registry and tests.

Verification is what reality confirmed: *lab-verified* entries name the
[lab](lab/run.py) scenarios where the real program agreed with whykey (see
[Lab evidence](#lab-evidence)). A scenario counts only when a positive
control observed the key (it fired, reached a probe, or reached the layer)
and whykey's verdict matched exactly; a tolerated uncertain answer (PASS~)
never counts. In an X11 session whykey's root-window grab probe
(`compositor.x11-grab`) decides most verdicts; an adapter is credited
there only for naming the binding that owns the grab, and its own
consume, release, and mode logic counts only in scenarios it decided.
*live-verified* entries were checked by hand on the
maintainer's desktop; everything else is *unverified*, however well its
fixtures pass.

| ID | Area | Implementation | Verification | Scope |
| --- | --- | --- | --- | --- |
| `cli.inspect.combination` | cli | Implemented | unverified | Inspect one normalized key combination. |
| `cli.inspect.sequence` | cli | Implemented | unverified | Inspect a bounded key sequence, carrying Readline, tmux, and GNU Screen prefix state between steps. |
| `cli.inspect.pid` | cli | Implemented | unverified | Select an interactive target by process ancestry. |
| `cli.inspect.focused` | cli | Implemented | unverified | Discover a focused-window process when the compositor exposes it. |
| `cli.bindings` | cli | Implemented | unverified | Enumerate effective bindings from supported adapters. |
| `cli.conflicts` | cli | Implemented | unverified | Compare bindings within matching source and context. |
| `cli.replay` | cli | Implemented | unverified | Render saved schema-v1 and schema-v2 reports without input injection. |
| `cli.snapshot` | cli | Implemented | unverified | Save a versioned static diagnostic snapshot without capturing or injecting input. |
| `cli.diff` | cli | Implemented | unverified | Compare saved snapshots or reports without querying the desktop. |
| `cli.output.schema-v2` | cli | Implemented | unverified | Emit structured context and evidence while preserving schema v1. |
| `cli.extensions` | cli | Implemented | live-verified on maintainer machine: Neovim extension (whykey-nvim) answering from a running nvim | Run one explicitly selected adapter through the bounded JSON protocol. |
| `cli.extensions-compositor` | cli | Implemented | unverified | Use opt-in compositor manifests for bounded read-only binding and focus queries. |
| `capture.terminal` | capture | Implemented | unverified | Capture terminal input when a controlling terminal is available. |
| `capture.native-compositor` | capture | Implemented | lab-verified (1 scenario) | Capture through the native compositor backend when Hyprland socket2 IPC is available. |
| `capture.native-sway` | capture | Implemented | unverified | Observe Sway binding events through IPC in pass-through mode; suppression is not exposed by Sway IPC. |
| `capture.probe-window` | capture | Implemented | lab-verified (8 scenarios) | Open a focused Wayland or X11 probe window and report whether the desktop delivered the key to a client or consumed it; the default listen chain uses it when native compositor capture is unavailable. |
| `capture.ndjson` | capture | Implemented | unverified | Emit one compact schema-v2 record per captured event. |
| `capture.export` | capture | Implemented | unverified | Write replayable JSON or NDJSON capture records to an explicit file. |
| `capture.evdev` | capture | Implemented | unverified | Capture Linux evdev keyboard events when a readable device is available. |
| `remapper.pre-compositor` | input | Implemented | unverified | Detect common pre-compositor remappers; evaluate the queried key against keyd, kanata, kmonad, input-remapper, and xremap configurations so remaps redirect and layer or command actions consume, keeping timing-dependent actions and kmonad or input-remapper rules for unproven devices conditional. |
| `input.ime-context` | input | Implemented | unverified | Detect IBus/Fcitx5 session context. |
| `input.ime-runtime-engine` | input | Implemented | unverified | Query an active IBus/Fcitx5 engine through a read-only API when possible. |
| `input.ime.fcitx5-safe-query` | safety | Implemented | unverified | Use bounded read-only Fcitx5 D-Bus calls without launching client helpers. |
| `compositor.hyprland` | compositor | Implemented | lab-verified (15 scenarios); live-verified on maintainer machine: static `whykey <key>` inspection against the running Hyprland session | Inspect Hyprland bindings, submaps, devices, and instance-scoped IPC. |
| `compositor.sway` | compositor | Implemented | lab-verified (9 scenarios) | Inspect Sway bindings by parsing the running configuration from get_config IPC and matching the get_binding_state mode. |
| `compositor.i3` | compositor | Implemented | lab-verified (5 scenarios); named the grabbing binding in 3 scenarios where the X11 grab probe decided | Inspect i3 bindings by parsing the running configuration (including included_configs) from get_config IPC and matching the get_binding_state mode. |
| `compositor.kde` | compositor | Implemented | lab-verified (3 scenarios); named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect KDE Plasma global shortcuts by asking the running kglobalaccel daemon which components use a key and whether they are active. |
| `compositor.xfce` | compositor | Implemented | unverified | Inspect Xfce keyboard-shortcut channel values. |
| `compositor.cinnamon` | compositor | Implemented | unverified | Inspect Cinnamon global keybinding schemas. |
| `compositor.mate` | compositor | Implemented | unverified | Inspect MATE Marco and SettingsDaemon keybinding schemas. |
| `compositor.niri` | compositor | Implemented | unverified | Inspect literal Niri binds configuration. |
| `compositor.dwl` | compositor | Implemented | unverified | Inspect literal dwl config.h key mappings conditionally; dwl has no stable capture IPC. |
| `compositor.herbstluftwm` | compositor | Implemented | unverified | Inspect effective herbstluftwm key bindings through bounded herbstclient IPC. |
| `compositor.river` | compositor | Implemented | unverified | Inspect literal River init mappings. |
| `compositor.wayfire` | compositor | Implemented | unverified | Inspect literal Wayfire binding settings. |
| `compositor.labwc` | compositor | Implemented | unverified | Inspect labwc Openbox-compatible keybind configuration. |
| `compositor.bspwm-sxhkd` | compositor | Implemented | lab-verified (2 scenarios); named the grabbing binding in 5 scenarios where the X11 grab probe decided | Inspect sxhkd hotkeys from SXHKD_CONFIG, the running sxhkd's -c and extra configuration files, or the default sxhkdrc, expanding {…} sequences and ranges in lockstep with their commands; a ~ (replay) binding passes the key on and an @ (release) binding withholds the press. |
| `compositor.openbox` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect Openbox keybind XML. |
| `compositor.gnome` | compositor | Implemented | lab-verified (4 scenarios; mutter-owned bindings only (org.gnome.desktop.wm.keybindings, mutter run headless); GNOME Shell and gnome-settings-daemon behavior not verified) | Inspect GNOME global shortcuts through read-only GSettings queries. |
| `compositor.programmable-x11` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect literal AwesomeWM, Qtile, and XMonad mappings. |
| `compositor.x11.xbindkeys` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect literal X11 xbindkeys configuration. |
| `compositor.portal-globalshortcuts` | compositor | Implemented | unverified | Probe read-only xdg-desktop-portal GlobalShortcuts availability; the API does not expose an existing binding inventory. |
| `compositor.kglobalaccel-runtime` | compositor | Implemented | lab-verified (3 scenarios); named the grabbing binding in 1 scenario where the X11 grab probe decided | Ask the running kglobalaccel over D-Bus (globalShortcutsByKey, isActive, allShortcutInfos) without auto-starting it; kglobalshortcutsrc is a configured-only fallback. |
| `compositor.gnome-gsettings` | compositor | Implemented | lab-verified (4 scenarios; mutter-owned bindings only (org.gnome.desktop.wm.keybindings, mutter run headless); GNOME Shell and gnome-settings-daemon behavior not verified) | Read the live GNOME keybinding schemas (mutter, GNOME Shell, gnome-settings-daemon media and custom keys) with gsettings. |
| `compositor.x11-grab` | compositor | Implemented | lab-verified (22 scenarios) | In an X11 session, observe passive key grabs on the root window (the X server refuses a second grab with BadAccess) to decide whether a window manager or hotkey daemon takes the key; the desktop adapter names the binding, and only an adapter that knows the owner passes the key on (sxhkd ~) keeps a pass-through verdict. |
| `compositor.generic` | compositor | Implemented | unverified | Report generic desktop/session context with opaque global bindings. |
| `compositor.additional-desktops` | compositor | Planned | unverified | Additional desktop integrations beyond the supported adapter set. |
| `terminal.dedicated` | terminal | Implemented | unverified | Detect and query a dedicated terminal adapter when available. |
| `shell.runtime-snapshot` | shell | Implemented | lab-verified (13 scenarios); live-verified on maintainer machine: Bash with `whykey shell-init bash` loaded | Use a live Bash, Zsh, or Fish binding snapshot when shell-init supplied one. |
| `session.multiplexers` | session | Implemented | lab-verified (5 scenarios) | Detect tmux, GNU Screen, and Zellij session context. |
| `input.ime-limits` | input | Implemented | unverified | Expose typed committed-text, Compose/dead-key, and application-preedit observability limits; passive capture does not reconstruct text history. |
| `safety.read-only` | safety | Implemented | unverified | Keep adapters read-only and never execute the investigated action. |

## Lab evidence

- `capture.native-compositor`: `hyprland-native-capture-lua`
- `capture.probe-window`: `hyprland-listen-conf-fallback`, `hyprland-window-bound`, `hyprland-window-unbound`, `i3-window-bound`, `i3-window-super-alone`, `i3-window-unbound`, `sway-window-bound`, `sway-window-unbound`
- `compositor.hyprland`: `hyprland-bindn`, `hyprland-code-key`, `hyprland-keysym-comma`, `hyprland-lua-bind`, `hyprland-lua-code-key`, `hyprland-lua-submap-active`, `hyprland-lua-submap-inactive`, `hyprland-lua-unbound`, `hyprland-native-capture-lua`, `hyprland-plain-bind`, `hyprland-release-bind`, `hyprland-source-include`, `hyprland-submap-active`, `hyprland-submap-inactive`, `hyprland-unbound`
- `compositor.sway`: `sway-include`, `sway-mode-active`, `sway-mode-inactive`, `sway-plain-bind`, `sway-plain-bind-wtype`, `sway-release-bind`, `sway-to-code`, `sway-unbound`, `sway-variable-mod`
- `compositor.i3`: `i3-mode-active-no-display`, `i3-mode-inactive-no-display`, `i3-plain-bind-no-display`, `i3-release-bind-no-display`, `i3-unbound-no-display`
- `compositor.i3` (binding only, X11 grab probe decided): `i3-mode-active`, `i3-plain-bind`, `i3-release-bind`
- `compositor.kde`: `kde-inactive-component-no-display`, `kde-launch-bind-no-display`, `kde-unbound-no-display`
- `compositor.kde` (binding only, X11 grab probe decided): `kde-launch-bind`
- `compositor.bspwm-sxhkd`: `sxhkd-replay`, `sxhkd-replay-plain`
- `compositor.bspwm-sxhkd` (binding only, X11 grab probe decided): `sxhkd-bind`, `sxhkd-brace`, `sxhkd-brace-range`, `sxhkd-plain-key`, `sxhkd-release`
- `compositor.openbox` (binding only, X11 grab probe decided): `openbox-keybind`
- `compositor.gnome`: `gnome-changed-bound`, `gnome-changed-unbound`, `gnome-default-bound`, `gnome-default-unbound`
- `compositor.programmable-x11` (binding only, X11 grab probe decided): `awesome-bind`
- `compositor.x11.xbindkeys` (binding only, X11 grab probe decided): `xbindkeys-bind`
- `compositor.kglobalaccel-runtime`: `kde-inactive-component-no-display`, `kde-launch-bind-no-display`, `kde-unbound-no-display`
- `compositor.kglobalaccel-runtime` (binding only, X11 grab probe decided): `kde-launch-bind`
- `compositor.gnome-gsettings`: `gnome-changed-bound`, `gnome-changed-unbound`, `gnome-default-bound`, `gnome-default-unbound`
- `compositor.x11-grab`: `awesome-bind`, `awesome-unbound`, `i3-mode-active`, `i3-mode-inactive`, `i3-plain-bind`, `i3-release-bind`, `i3-unbound`, `kde-inactive-component`, `kde-launch-bind`, `kde-unbound`, `openbox-keybind`, `openbox-unbound`, `sxhkd-bind`, `sxhkd-brace`, `sxhkd-brace-range`, `sxhkd-plain-key`, `sxhkd-release`, `sxhkd-replay`, `sxhkd-replay-plain`, `sxhkd-unbound`, `xbindkeys-bind`, `xbindkeys-unbound`
- `shell.runtime-snapshot`: `bash-alt-f-init`, `bash-default-transpose-init`, `bash-user-bind-init`, `fish-default-transpose-init`, `fish-user-bind-init`, `fish-vi-default-mode-bind-init`, `fish-vi-insert-bind-init`, `tmux-fish-vi-insert-bind`, `tmux-zsh-user-bind`, `zsh-bind-before-vi-init`, `zsh-default-transpose-init`, `zsh-user-bind-init`, `zsh-vi-user-bind-init`
- `session.multiplexers`: `screen-prefix-fish`, `tmux-prefix`, `tmux-prefix-zsh`, `tmux-root-bind`, `tmux-unbound`
