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
| `capture.native-compositor` | capture | Implemented | lab-verified (7 scenarios) | Capture through the native compositor backend when Hyprland socket2 IPC is available. |
| `capture.native-sway` | capture | Implemented | lab-verified (4 scenarios) | Observe Sway binding events through IPC in pass-through mode; suppression is not exposed by Sway IPC. |
| `capture.probe-window` | capture | Implemented | lab-verified (13 scenarios) | Open a focused Wayland or X11 probe window and report whether the desktop delivered the key to a client or consumed it; the default listen chain uses it when native compositor capture is unavailable. |
| `capture.ndjson` | capture | Implemented | unverified | Emit one compact schema-v2 record per captured event. |
| `capture.export` | capture | Implemented | unverified | Write replayable JSON or NDJSON capture records to an explicit file. |
| `capture.evdev` | capture | Implemented | unverified | Capture Linux evdev keyboard events when a readable device is available. |
| `remapper.pre-compositor` | input | Implemented | lab-verified (11 scenarios) | Detect common pre-compositor remappers; evaluate the queried key against keyd, kanata, kmonad, input-remapper, and xremap configurations so remaps redirect and layer or command actions consume, keeping timing-dependent actions and kmonad or input-remapper rules for unproven devices conditional. |
| `input.ime-context` | input | Implemented | lab-verified (6 scenarios) | Detect IBus/Fcitx5 session context. |
| `input.ime-runtime-engine` | input | Implemented | lab-verified (6 scenarios) | Query an active IBus/Fcitx5 engine through a read-only API when possible. |
| `input.ime.fcitx5-safe-query` | safety | Implemented | unverified | Use bounded read-only Fcitx5 D-Bus calls without launching client helpers. |
| `compositor.hyprland` | compositor | Implemented | lab-verified (37 scenarios); live-verified on maintainer machine: static `whykey <key>` inspection against the running Hyprland session | Inspect Hyprland bindings, submaps, devices, and instance-scoped IPC. |
| `compositor.sway` | compositor | Implemented | lab-verified (18 scenarios) | Inspect Sway bindings by parsing the running configuration from get_config IPC and matching the get_binding_state mode. |
| `compositor.i3` | compositor | Implemented | lab-verified (5 scenarios); named the grabbing binding in 4 scenarios where the X11 grab probe decided | Inspect i3 bindings by parsing the running configuration (including included_configs) from get_config IPC and matching the get_binding_state mode. |
| `compositor.kde` | compositor | Implemented | lab-verified (8 scenarios); named the grabbing binding in 2 scenarios where the X11 grab probe decided | Inspect KDE Plasma global shortcuts by asking the running kglobalaccel daemon which components use a key and whether they are active. |
| `compositor.xfce` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect Xfce keyboard-shortcut channel values. |
| `compositor.cinnamon` | compositor | Implemented | named the grabbing binding in 2 scenarios where the X11 grab probe decided | Inspect Cinnamon global keybinding schemas. |
| `compositor.mate` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect MATE Marco and SettingsDaemon keybinding schemas. |
| `compositor.niri` | compositor | Implemented | lab-verified (3 scenarios) | Inspect literal Niri binds configuration. |
| `compositor.dwl` | compositor | Implemented | lab-verified (2 scenarios) | Inspect literal dwl config.h key mappings conditionally; dwl has no stable capture IPC. |
| `compositor.herbstluftwm` | compositor | Implemented | unverified | Inspect effective herbstluftwm key bindings through bounded herbstclient IPC. |
| `compositor.river` | compositor | Implemented | unverified | Inspect literal River init mappings. |
| `compositor.wayfire` | compositor | Implemented | lab-verified (8 scenarios) | Inspect literal Wayfire binding settings. |
| `compositor.labwc` | compositor | Implemented | lab-verified (2 scenarios) | Inspect labwc Openbox-compatible keybind configuration. |
| `compositor.bspwm-sxhkd` | compositor | Implemented | lab-verified (2 scenarios); named the grabbing binding in 6 scenarios where the X11 grab probe decided | Inspect sxhkd hotkeys from SXHKD_CONFIG, the running sxhkd's -c and extra configuration files, or the default sxhkdrc, expanding {…} sequences and ranges in lockstep with their commands; a ~ (replay) binding passes the key on and an @ (release) binding withholds the press. |
| `compositor.openbox` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect Openbox keybind XML. |
| `compositor.gnome` | compositor | Implemented | lab-verified (8 scenarios; mutter-owned bindings on headless mutter, plus GNOME Shell's overview binding and gsd-media-keys custom shortcuts (GNOME Shell on headless mutter with a minimal logind); other gnome-settings-daemon plugins and GNOME Shell defaults not verified) | Inspect GNOME global shortcuts through read-only GSettings queries. |
| `compositor.programmable-x11` | compositor | Implemented | named the grabbing binding in 3 scenarios where the X11 grab probe decided | Inspect literal AwesomeWM, Qtile, and XMonad mappings. |
| `compositor.x11.xbindkeys` | compositor | Implemented | named the grabbing binding in 1 scenario where the X11 grab probe decided | Inspect literal X11 xbindkeys configuration. |
| `compositor.portal-globalshortcuts` | compositor | Implemented | unverified | Probe read-only xdg-desktop-portal GlobalShortcuts availability; the API does not expose an existing binding inventory. |
| `compositor.kglobalaccel-runtime` | compositor | Implemented | lab-verified (8 scenarios); named the grabbing binding in 2 scenarios where the X11 grab probe decided | Ask the running kglobalaccel over D-Bus (globalShortcutsByKey, isActive, allShortcutInfos) without auto-starting it; kglobalshortcutsrc is a configured-only fallback. |
| `compositor.gnome-gsettings` | compositor | Implemented | lab-verified (8 scenarios; mutter-owned bindings on headless mutter, plus GNOME Shell's overview binding and gsd-media-keys custom shortcuts (GNOME Shell on headless mutter with a minimal logind); other gnome-settings-daemon plugins and GNOME Shell defaults not verified) | Read the live GNOME keybinding schemas (mutter, GNOME Shell, gnome-settings-daemon media and custom keys) with gsettings. |
| `compositor.x11-grab` | compositor | Implemented | lab-verified (40 scenarios) | In an X11 session, observe passive key grabs on the root window (the X server refuses a second grab with BadAccess) to decide whether a window manager or hotkey daemon takes the key; the desktop adapter names the binding, and only an adapter that knows the owner passes the key on (sxhkd ~) keeps a pass-through verdict. |
| `compositor.generic` | compositor | Implemented | unverified | Report generic desktop/session context with opaque global bindings. |
| `compositor.additional-desktops` | compositor | Planned | unverified | Additional desktop integrations beyond the supported adapter set. |
| `terminal.dedicated` | terminal | Implemented | lab-verified (16 scenarios) | Detect and query a dedicated terminal adapter when available. |
| `shell.runtime-snapshot` | shell | Implemented | lab-verified (16 scenarios); live-verified on maintainer machine: Bash with `whykey shell-init bash` loaded | Use a live Bash, Zsh, or Fish binding snapshot when shell-init supplied one. |
| `session.multiplexers` | session | Implemented | lab-verified (19 scenarios) | Detect tmux, GNU Screen, and Zellij session context. |
| `input.ime-limits` | input | Implemented | unverified | Expose typed committed-text, Compose/dead-key, and application-preedit observability limits; passive capture does not reconstruct text history. |
| `safety.read-only` | safety | Implemented | unverified | Keep adapters read-only and never execute the investigated action. |

## Lab evidence

- `capture.native-compositor`: `hyprland-native-capture-lua`, `hyprland-native-capture-wtype`, `hyprland-qkbd-listen-bound`, `hyprland-qkbd-listen-unbound`, `hyprland-suppress-bound`, `hyprland-suppress-restored`, `hyprland-suppress-unbound`
- `capture.native-sway`: `sway-native-capture-bindcode`, `sway-native-capture-bound`, `sway-native-capture-mode`, `sway-native-capture-release`
- `capture.probe-window`: `hyprland-listen-conf-fallback`, `hyprland-qkbd-evdev-unbound`, `hyprland-qkbd-window-bound`, `hyprland-qkbd-window-unbound`, `hyprland-suppress-escape`, `hyprland-window-bound`, `hyprland-window-unbound`, `i3-window-bound`, `i3-window-super-alone`, `i3-window-unbound`, `sway-native-capture-unbound`, `sway-window-bound`, `sway-window-unbound`
- `remapper.pre-compositor`: `remap-input-remapper-unbound-passes`, `remap-kanata-redirect-bound`, `remap-kanata-unbound-passes`, `remap-keyd-command-consumed`, `remap-keyd-ids-mismatch-passes`, `remap-keyd-redirect-bound`, `remap-keyd-redirect-modifier-bound`, `remap-keyd-unbound-passes`, `remap-kmonad-unbound-passes`, `remap-xremap-redirect-bound`, `remap-xremap-unbound-passes`
- `input.ime-context`: `ime-fcitx5-sway-inactive`, `ime-fcitx5-sway-keyboard-us`, `ime-fcitx5-x11-keyboard-us-xim`, `ime-fcitx5-x11-no-input-context`, `ime-ibus-x11-no-engine-xim`, `ime-ibus-x11-xkb-us-xim`
- `input.ime-runtime-engine`: `ime-fcitx5-sway-inactive`, `ime-fcitx5-sway-keyboard-us`, `ime-fcitx5-x11-keyboard-us-xim`, `ime-fcitx5-x11-no-input-context`, `ime-ibus-x11-no-engine-xim`, `ime-ibus-x11-xkb-us-xim`
- `compositor.hyprland`: `hyprland-bindn`, `hyprland-code-key`, `hyprland-keypad-enter`, `hyprland-keypad-enter-not-return`, `hyprland-keysym-comma`, `hyprland-lua-bind`, `hyprland-lua-code-key`, `hyprland-lua-submap-active`, `hyprland-lua-submap-inactive`, `hyprland-lua-unbound`, `hyprland-native-capture-lua`, `hyprland-native-capture-wtype`, `hyprland-plain-bind`, `hyprland-qkbd-listen-bound`, `hyprland-qkbd-listen-unbound`, `hyprland-release-and-press-bind`, `hyprland-release-bind`, `hyprland-return-not-keypad-enter`, `hyprland-source-include`, `hyprland-submap-active`, `hyprland-submap-break`, `hyprland-submap-break-catchall`, `hyprland-submap-inactive`, `hyprland-suppress-bound`, `hyprland-suppress-restored`, `hyprland-suppress-unbound`, `hyprland-unbound`, `remap-input-remapper-unbound-passes`, `remap-kanata-redirect-bound`, `remap-kanata-unbound-passes`, `remap-keyd-ids-mismatch-passes`, `remap-keyd-redirect-bound`, `remap-keyd-redirect-modifier-bound`, `remap-keyd-unbound-passes`, `remap-kmonad-unbound-passes`, `remap-xremap-redirect-bound`, `remap-xremap-unbound-passes`
- `compositor.sway`: `sway-include`, `sway-keypad-digit`, `sway-keypad-digit-mod2`, `sway-keypad-enter`, `sway-keypad-enter-not-return`, `sway-mode-active`, `sway-mode-inactive`, `sway-native-capture-bindcode`, `sway-native-capture-bound`, `sway-native-capture-mode`, `sway-native-capture-release`, `sway-plain-bind`, `sway-plain-bind-wtype`, `sway-release-bind`, `sway-return-not-keypad-enter`, `sway-to-code`, `sway-unbound`, `sway-variable-mod`
- `compositor.i3`: `i3-mode-active-no-display`, `i3-mode-inactive-no-display`, `i3-plain-bind-no-display`, `i3-release-bind-no-display`, `i3-unbound-no-display`
- `compositor.i3` (binding only, X11 grab probe decided): `i3-keypad-digit-numlock`, `i3-mode-active`, `i3-plain-bind`, `i3-release-bind`
- `compositor.kde`: `kde-inactive-component-no-display`, `kde-launch-bind-no-display`, `kde-launch5-key`, `kde-unbound-no-display`, `kwin-default-close`, `kwin-kglobalacceld-owns-name`, `kwin-window-close-bound`, `kwin-window-close-unbound`
- `compositor.kde` (binding only, X11 grab probe decided): `kde-launch-bind`, `kde-shift-digit-symbol`
- `compositor.xfce` (binding only, X11 grab probe decided): `xfce-bind`
- `compositor.cinnamon` (binding only, X11 grab probe decided): `cinnamon-custom-bound`, `cinnamon-wm-maximize-bound`
- `compositor.mate` (binding only, X11 grab probe decided): `mate-bind`
- `compositor.niri`: `niri-mod-nested-super-unbound`, `niri-no-binds-delivered`, `niri-unbound-delivered`
- `compositor.dwl`: `dwl-bind-consumed`, `dwl-unbound-delivered`
- `compositor.wayfire`: `wayfire-bind-alternative`, `wayfire-bind-consumed`, `wayfire-bind-disabled`, `wayfire-bind-modifiers`, `wayfire-default-bind-consumed`, `wayfire-default-bind-disabled`, `wayfire-plugin-not-loaded`, `wayfire-unbound-delivered`
- `compositor.labwc`: `labwc-bind-consumed`, `labwc-unbound-delivered`
- `compositor.bspwm-sxhkd`: `sxhkd-replay`, `sxhkd-replay-plain`
- `compositor.bspwm-sxhkd` (binding only, X11 grab probe decided): `sxhkd-bind`, `sxhkd-brace`, `sxhkd-brace-range`, `sxhkd-non-ascii-command`, `sxhkd-plain-key`, `sxhkd-release`
- `compositor.openbox` (binding only, X11 grab probe decided): `openbox-keybind`
- `compositor.gnome`: `gnome-changed-bound`, `gnome-changed-unbound`, `gnome-default-bound`, `gnome-default-unbound`, `gnome-shell-gsd-custom-bound`, `gnome-shell-gsd-custom-not-running`, `gnome-shell-overview-bound`, `gnome-shell-overview-unbound`
- `compositor.programmable-x11` (binding only, X11 grab probe decided): `awesome-bind`, `qtile-bind`, `xmonad-bind`
- `compositor.x11.xbindkeys` (binding only, X11 grab probe decided): `xbindkeys-bind`
- `compositor.kglobalaccel-runtime`: `kde-inactive-component-no-display`, `kde-launch-bind-no-display`, `kde-launch5-key`, `kde-unbound-no-display`, `kwin-default-close`, `kwin-kglobalacceld-owns-name`, `kwin-window-close-bound`, `kwin-window-close-unbound`
- `compositor.kglobalaccel-runtime` (binding only, X11 grab probe decided): `kde-launch-bind`, `kde-shift-digit-symbol`
- `compositor.gnome-gsettings`: `gnome-changed-bound`, `gnome-changed-unbound`, `gnome-default-bound`, `gnome-default-unbound`, `gnome-shell-gsd-custom-bound`, `gnome-shell-gsd-custom-not-running`, `gnome-shell-overview-bound`, `gnome-shell-overview-unbound`
- `compositor.x11-grab`: `awesome-bind`, `awesome-unbound`, `cinnamon-custom-bound`, `cinnamon-custom-unbound`, `cinnamon-wm-maximize-bound`, `cinnamon-wm-unbound`, `i3-keypad-digit-numlock`, `i3-keypad-digit-without-numlock-modifier`, `i3-mode-active`, `i3-mode-inactive`, `i3-plain-bind`, `i3-release-bind`, `i3-unbound`, `kde-inactive-component`, `kde-launch-bind`, `kde-shift-digit-symbol`, `kde-unbound`, `mate-bind`, `mate-unbound`, `openbox-keybind`, `openbox-unbound`, `qtile-bind`, `qtile-unbound`, `sxhkd-bind`, `sxhkd-brace`, `sxhkd-brace-range`, `sxhkd-continuation-past-text`, `sxhkd-continuation-shared-offset`, `sxhkd-non-ascii-command`, `sxhkd-plain-key`, `sxhkd-release`, `sxhkd-replay`, `sxhkd-replay-plain`, `sxhkd-unbound`, `xbindkeys-bind`, `xbindkeys-unbound`, `xfce-bind`, `xfce-unbound`, `xmonad-bind`, `xmonad-unbound`
- `terminal.dedicated`: `alacritty-alt-x`, `alacritty-command-shortcut`, `alacritty-ctrl-left`, `foot-alt-x`, `foot-ctrl-left`, `foot-pipe-shortcut`, `ghostty-ctrl-left`, `ghostty-fullscreen-shortcut`, `kitty-alt-x`, `kitty-ctrl-left`, `kitty-launch-shortcut`, `konsole-ctrl-left`, `konsole-fullscreen-shortcut`, `wezterm-alt-x`, `wezterm-callback-shortcut`, `wezterm-ctrl-left`
- `shell.runtime-snapshot`: `bash-alt-f-init`, `bash-default-transpose-init`, `bash-unbound-shift-up-init`, `bash-user-bind-init`, `fish-default-transpose-init`, `fish-preset-alt-left-init`, `fish-user-bind-init`, `fish-vi-default-mode-bind-init`, `fish-vi-insert-bind-init`, `tmux-fish-vi-insert-bind`, `tmux-zsh-user-bind`, `zsh-bind-before-vi-init`, `zsh-default-transpose-init`, `zsh-unbound-ctrl-left-init`, `zsh-user-bind-init`, `zsh-vi-user-bind-init`
- `session.multiplexers`: `screen-prefix-fish`, `tmux-prefix`, `tmux-prefix-before-pane-tty`, `tmux-prefix-dash`, `tmux-prefix-escaped-percent`, `tmux-prefix-escaped-semicolon`, `tmux-prefix-lowercase`, `tmux-prefix-uppercase`, `tmux-prefix-zsh`, `tmux-root-alt-escape`, `tmux-root-bind`, `tmux-root-btab`, `tmux-root-quoted-brace`, `tmux-unbound`, `zellij-default-alt-n`, `zellij-locked-normal-bind`, `zellij-pane-mode-bind`, `zellij-pane-mode-only-bind`, `zellij-unbound-ctrl-k`
