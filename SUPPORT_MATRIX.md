# Whykey support matrix

This file is generated from [`support-matrix.json`](support-matrix.json) by
[`tools/generate_support_matrix.py`](tools/generate_support_matrix.py).
Do not edit it by hand. Runtime availability is environment-dependent and
is reported by `whykey capabilities`; this matrix records the implementation
contract shared by the capability registry and tests.

| ID | Area | Implementation | Scope |
| --- | --- | --- | --- |
| `cli.inspect.combination` | cli | Implemented | Inspect one normalized key combination. |
| `cli.inspect.sequence` | cli | Implemented | Inspect a bounded sequence of independent key steps. |
| `cli.inspect.pid` | cli | Implemented | Select an interactive target by process ancestry. |
| `cli.inspect.focused` | cli | Implemented | Discover a focused-window process when the compositor exposes it. |
| `cli.bindings` | cli | Implemented | Enumerate effective bindings from supported adapters. |
| `cli.conflicts` | cli | Implemented | Compare bindings within matching source and context. |
| `cli.replay` | cli | Implemented | Render saved schema-v1 and schema-v2 reports without input injection. |
| `cli.snapshot` | cli | Implemented | Save a versioned static diagnostic snapshot without capturing or injecting input. |
| `cli.diff` | cli | Implemented | Compare saved snapshots or reports without querying the desktop. |
| `cli.output.schema-v2` | cli | Implemented | Emit structured context and evidence while preserving schema v1. |
| `cli.extensions` | cli | Implemented | Run one explicitly selected adapter through the bounded JSON protocol. |
| `cli.extensions-compositor` | cli | Implemented | Use opt-in compositor manifests for bounded read-only binding and focus queries. |
| `capture.terminal` | capture | Implemented | Capture terminal input when a controlling terminal is available. |
| `capture.native-compositor` | capture | Implemented | Capture through the native compositor backend when Hyprland socket2 IPC is available. |
| `capture.native-sway` | capture | Implemented | Observe Sway binding events through IPC in pass-through mode; suppression is not exposed by Sway IPC. |
| `capture.ndjson` | capture | Implemented | Emit one compact schema-v2 record per captured event. |
| `capture.export` | capture | Implemented | Write replayable JSON or NDJSON capture records to an explicit file. |
| `capture.evdev` | capture | Implemented | Capture Linux evdev keyboard events when a readable device is available. |
| `remapper.pre-compositor` | input | Implemented | Detect common pre-compositor remappers and preserve conditional transformations. |
| `input.ime-context` | input | Implemented | Detect IBus/Fcitx5 session context. |
| `input.ime-runtime-engine` | input | Implemented | Query an active IBus/Fcitx5 engine through a read-only API when possible. |
| `input.ime.fcitx5-safe-query` | safety | Implemented | Use bounded read-only Fcitx5 D-Bus calls without launching client helpers. |
| `compositor.hyprland` | compositor | Implemented | Inspect Hyprland bindings, submaps, devices, and instance-scoped IPC. |
| `compositor.sway` | compositor | Implemented | Inspect Sway bindings through its read-only IPC. |
| `compositor.i3` | compositor | Implemented | Inspect i3 bindings through its read-only IPC. |
| `compositor.kde` | compositor | Implemented | Inspect KDE Plasma shortcut configuration without claiming runtime activation. |
| `compositor.xfce` | compositor | Implemented | Inspect Xfce keyboard-shortcut channel values. |
| `compositor.cinnamon` | compositor | Implemented | Inspect Cinnamon global keybinding schemas. |
| `compositor.mate` | compositor | Implemented | Inspect MATE Marco and SettingsDaemon keybinding schemas. |
| `compositor.niri` | compositor | Implemented | Inspect literal Niri binds configuration. |
| `compositor.dwl` | compositor | Implemented | Inspect literal dwl config.h key mappings conditionally; dwl has no stable capture IPC. |
| `compositor.herbstluftwm` | compositor | Implemented | Inspect effective herbstluftwm key bindings through bounded herbstclient IPC. |
| `compositor.river` | compositor | Implemented | Inspect literal River init mappings. |
| `compositor.wayfire` | compositor | Implemented | Inspect literal Wayfire binding settings. |
| `compositor.labwc` | compositor | Implemented | Inspect labwc Openbox-compatible keybind configuration. |
| `compositor.bspwm-sxhkd` | compositor | Implemented | Inspect literal sxhkd mappings associated with bspwm. |
| `compositor.openbox` | compositor | Implemented | Inspect Openbox keybind XML. |
| `compositor.gnome` | compositor | Implemented | Inspect GNOME global shortcuts through read-only GSettings queries. |
| `compositor.programmable-x11` | compositor | Implemented | Inspect literal AwesomeWM, Qtile, and XMonad mappings. |
| `compositor.x11.xbindkeys` | compositor | Implemented | Inspect literal X11 xbindkeys configuration. |
| `compositor.portal-globalshortcuts` | compositor | Implemented | Probe read-only xdg-desktop-portal GlobalShortcuts availability; the API does not expose an existing binding inventory. |
| `compositor.kglobalaccel-runtime` | compositor | Implemented | Probe live org.kde.KGlobalAccel availability; no read-only runtime enumeration is claimed, so kglobalshortcutsrc remains the static inventory source. |
| `compositor.gnome-shell-runtime` | compositor | Implemented | Probe GNOME Shell runtime reachability; Shell does not expose a read-only full keybinding dump. |
| `compositor.generic` | compositor | Implemented | Report generic desktop/session context with opaque global bindings. |
| `compositor.additional-desktops` | compositor | Planned | Additional desktop integrations beyond the supported adapter set. |
| `terminal.dedicated` | terminal | Implemented | Detect and query a dedicated terminal adapter when available. |
| `shell.runtime-snapshot` | shell | Implemented | Use a live Bash, Zsh, or Fish binding snapshot when shell-init supplied one. |
| `session.multiplexers` | session | Implemented | Detect tmux, GNU Screen, and Zellij session context. |
| `input.ime-limits` | input | Implemented | Expose typed committed-text, Compose/dead-key, and application-preedit observability limits; passive capture does not reconstruct text history. |
| `safety.read-only` | safety | Implemented | Keep adapters read-only and never execute the investigated action. |
