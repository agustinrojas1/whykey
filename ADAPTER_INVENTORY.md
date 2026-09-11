# Whykey adapter inventory

This inventory describes the adapters currently present in the diagnostic
engine. “Unknown” is intentional: whykey records when a version, runtime
registration, active mode, generated include, or application-side action is
not observable. Adapter applicability never means that every binding is known.

## Desktop and compositor adapters

Native capture backends are separate from static desktop adapters. Hyprland
uses socket2 IPC and a temporary Lua hook, restores the recorded submap before
reporting, and exposes `capture.native-compositor`. Sway exposes read-only
binding notifications through IPC in pass-through mode (`capture.native-sway`);
Sway IPC has no suppression/restoration hook, so default suppression fails
closed and users must select `--no-suppress`. Other desktops keep their static
inspection adapters and fall back to terminal or evdev capture.

Support tiers are T1 live IPC, T2 live query, T3 literal configuration, and
T4 manifest extension. T4 manifests are opt-in, bounded, and conditional. They
add static inspection, binding inventory, and optional focus lookup; they do
not add a capture backend.

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Hyprland | `HYPRLAND_INSTANCE_SIGNATURE`, instance discovery, or Hyprland IPC; skipped for remote SSH | `hyprctl binds/submap/devices/activewindow`, literal config and Lua files | IPC availability; version is not persisted | bindings, submaps, device scopes, keymaps, focused PID, Lua/config hints | dynamic Lua, inhibitors, generated config, and runtime-vs-disk divergence |
| Sway | `SWAYSOCK` or Sway desktop identity; skipped for SSH | `swaymsg -t get_bindings/get_tree` | native version probe for applicability | bindings, release/exact/pass behavior, focused PID | runtime ordering outside IPC and external grabs |
| i3 | `I3SOCK` or i3 desktop identity; skipped for SSH | `i3-msg -t get_bindings/get_tree` | native version probe for applicability | bindings, release/exact/nop/pass behavior, focused PID | external clients, ordering races, and unreported grabs |
| GNOME | GNOME desktop/session identity or `gsettings` availability | `gsettings` media-key and custom-keybinding schemas | no version claim | standard/custom literal accelerators and configured command text | daemon runtime registration, extensions, and command execution |
| KDE Plasma | KDE desktop identity or `kglobalshortcutsrc` path | `kglobalshortcutsrc` (or `KGLOBALSHORTCUTS_CONFIG`) | no version claim | enabled literal global shortcuts and action names | live D-Bus registrations, runtime reload, and plugin shortcuts |
| Xfce | Xfce desktop identity or `xfconf-query` | live `xfconf-query -c xfce4-keyboard-shortcuts` | no version claim | current custom/default channel values | daemon races, plugins, and competing clients |
| Cinnamon | Cinnamon identity or `CINNAMON_VERSION`/GSettings | `gsettings list-recursively org.cinnamon.desktop.keybindings` | environment version is detection evidence only | literal schema accelerators | custom schemas not returned and runtime plugin state |
| MATE | MATE identity or session variables | MATE Marco and SettingsDaemon GSettings schemas | no version claim | literal global accelerators and schema sources | runtime registrations and custom extensions |
| Niri | `NIRI_SOCKET`/desktop identity or config file | literal `NIRI_CONFIG`/`config.kdl` `binds` blocks | no version claim | literal mappings and modes | active mode, reload state, inhibitors, and dynamic KDL |
| River | River identity or `RIVER_INIT`/init file | literal `riverctl map` commands | no version claim | literal mappings and modes | executable shell logic, reload state, and runtime mode |
| dwl | dwl desktop identity or `DWL_CONFIG`/`~/.config/dwl/config.h` | literal `static const Key keys[]` entries | no version claim | key/modifier/function rows from `config.h` | dwl has no stable global-shortcut IPC; runtime activation, reload state, generated config, and custom macros |
| Wayfire | Wayfire identity or config path | literal `binding_*` values in `WAYFIRE_CONFIG`/`wayfire.ini` | no version claim | literal plugin binding values | plugin activation, reload state, and dynamic values |
| labwc | labwc identity or config path | Openbox-compatible `<keybind>` XML | no version claim | literal keybind/action nodes | runtime reload, scripts, and dynamic XML generation |
| bspwm/sxhkd | bspwm/sxhkd identity or `SXHKD_CONFIG` | literal sxhkd key/command pairs with bounded nested `include`/`source` resolution | no version claim | literal chords and commands with file-chain context | process activation, reload, shell expansion, generated content, and ordering |
| Openbox | Openbox identity or config path | Openbox `<keybind>` XML | no version claim | literal keybind/action nodes | runtime reload, scripts, and competing X11 clients |
| X11 xbindkeys | X11 session (`DISPLAY` without Wayland) or config path | `XBINDKEYSRC`/`.xbindkeysrc` | no version claim | literal command/key pairs | daemon activation, WM precedence, and other X11 grabs |
| Programmable X11 | AwesomeWM, Qtile, or XMonad identity/config detection | literal `rc.lua`, `config.py`, or `xmonad.hs` declarations | no version claim; executable config is never evaluated | literal Awesome `awful.key`, Qtile `Key`, and XMonad `xK_*` mappings | helpers, variables, modes, reload state, and evaluated runtime tables |
| Generic compositor | desktop/session/display-server context without a dedicated adapter | environment identity only | not applicable | session context and an opaque compositor layer | global bindings and focused PID are unavailable unless another adapter provides them |
| Manifest extension (T4) | `WHYKEY_ADAPTER_DIR` or `~/.config/whykey/adapters` plus env/desktop hint | bounded `bindings_cmd`, optional `focused_cmd` | command reachability only; generation is optional | script-reported bindings and optional focused PID | runtime activation, reload state, and command output outside the bounded snapshot |

## Terminal adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Ghostty | `TERM_PROGRAM`/`LC_TERMINAL` identity | `ghostty +list-keybinds --plain`, config files, includes | command availability only | effective keybind overrides, output actions, normal terminal bytes | physical/multi-key triggers and dynamic config |
| Kitty | `TERM_PROGRAM`, `KITTY_WINDOW_ID`, or protocol response | Kitty key tables where available; terminal keyboard protocol during capture | protocol response flags | key actions and negotiated capture flags | every negotiated capability, runtime window state, and app-side handling |
| Alacritty | terminal identity or `ALACRITTY_SOCKET` | TOML/YAML config and key bindings | identity variables only | literal actions, forwarding, and consuming actions | dynamic config and runtime overrides |
| Foot | terminal identity | `foot.ini` key bindings | identity variables only | literal key bindings | generated config and runtime reload |
| WezTerm | terminal identity or `WEZTERM_PANE` | effective `wezterm show-keys` output | command availability only | effective default/table key assignments | active non-default table and Lua conditions |
| Konsole | `KONSOLE_PROFILE_NAME`/`KONSOLE_VERSION` or terminal identity | active profile `.keytab` | environment version only | literal keytab actions and sequences | screen/keypad modes and runtime profile reload |
| Generic terminal | no dedicated identity, or unsupported terminal | effective `termios` plus safe byte encoding fallback | not applicable | predicted terminal bytes and confidence | terminal-specific key tables and pre-terminal grabs |

## Shell, TTY, and multiplexer adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Linux TTY | readable `/dev/tty` | `termios` and terminal adapter bytes | kernel behavior is not version-claimed | `ISIG`, `VSUSP`, canonical/flow-control evidence | kernel line discipline details outside the snapshot |
| Bash/Readline | `SHELL` plus shell-init snapshot or inputrc paths | runtime `bind` snapshot, `INPUTRC`, `/etc/inputrc` | Bash version is not claimed | active map, macros, shell commands, literal overrides | unobserved runtime mode and generated includes |
| Zsh/ZLE | `SHELL` plus shell-init snapshot/config | `WHYKEY_ZLE_BINDINGS`, `.zshrc`/`.zshenv` | Zsh version is not claimed | active keymap and literal sequences | plugin-generated maps and runtime mode races |
| Fish | `SHELL` plus shell-init snapshot/config | `WHYKEY_FISH_BINDINGS`, `config.fish` | Fish version is not claimed | active bind mode and literal sequences | dynamic functions, generated bindings, and unknown mode |
| tmux | `TMUX`/`TMUX_PANE` | `tmux list-keys` root/active/prefix tables | command output only | active table, prefixes, bindings, action classification | nested server races and hooks not visible through tables |
| GNU Screen | `STY` or screen process/config | system then user `screenrc`, `bind`, `bindkey` | command/config evidence only | effective prefix and literal bindings | nested sessions, runtime reload, and shell expansion |
| Zellij | `ZELLIJ` markers or config | dumped defaults plus `config.kdl` overrides | installed command availability only | mode tables, defaults, user overrides, clear-defaults | active mode unless explicitly supplied and runtime reload |
| SSH boundary | `SSH_CONNECTION`/`SSH_TTY` | local process ancestry, PTY, and environment | not applicable | local/remote boundary and conditional remote leg | remote whykey state and remote application internals |

## Input, remapping, and IME adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| evdev | readable keyboard event device or explicit `--device` | `/dev/input/event*`, capabilities, key bitmap, LEDs | kernel device metadata only | physical code, device name, pressed/locked modifiers, SYN_DROPPED recovery | denied devices, latched state, firmware, and complete XKB translation |
| keyd | process/config detection | process list and conventional config | process/config evidence only | literal mappings | virtual routing, timing, layers, and macros |
| kanata | process/config detection | literal config aliases/layers/tap-hold forms | process/config evidence only | conservative literal mappings | active layer and timing-dependent behavior |
| kmonad | process/config detection | literal config aliases/layers/tap-hold forms | process/config evidence only | conservative literal mappings | active layer and timing-dependent behavior |
| input-remapper | process/config detection | bounded JSON mapping pairs | process/config evidence only | literal pairs | profiles, timing, and virtual-device routing |
| xremap | process/config detection | bounded YAML `remap` entries | process/config evidence only | literal remaps | device matching, layers, and generated config |
| Fcitx5 | session variables/process and D-Bus preflight | bounded read-only `org.fcitx.Fcitx.Controller1` calls | service presence only | active engine/context when query succeeds | committed text, application preedit, and unavailable D-Bus |
| IBus | session variables/process and safe API availability | read-only IBus context where available | service presence only | detected IME context and limited engine state | committed text, dead-key/Compose history, and app preedit |
| XKB/layout | device keymap fields and optional `xkbcli` | RMLVO/keymap data, compiled `symbols[N]` groups, `NoSymbol` level positions, and optional `WHYKEY_XKB_GROUP` | compiler availability only | symbolic/code candidates plus typed Shift/AltGr/Caps/NumLock/latched level state when supplied | compositor-independent Compose history and missing group/RMLVO state remain conditional |

## Interactive application adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Neovim/Vim | process ancestry or selected PID | RPC/server expression when available; init/vimrc fallback | process identity only | mode and common/live mappings | plugins, runtime precedence, and unavailable server |
| Emacs | process ancestry or selected PID | `emacsclient` active map; init files fallback | process identity only | active/common global maps | package maps, buffers, and unavailable server |
| Helix | process ancestry/config path | `config.toml` literal keymap | process identity only | literal mappings and mode hints | runtime commands, plugins, and generated config |
| Micro | process ancestry/config path | `bindings.json` literal mappings | process identity only | literal mappings | plugins and runtime overrides |
| Kakoune | process ancestry/config path | `kakrc` literal mappings | process identity only | literal mappings | hooks, scripts, and runtime mode |
| VS Code/VSCodium | process name and user keybindings path | `keybindings.json` literal single-key entries | process identity only | command, key, and `when` evidence | chords, extensions, `when` evaluation, and runtime context |
| JetBrains | recognized IDE process and XML keymap path | XML keymaps | process identity only | literal shortcut entries | plugins, IDE modes, and runtime overrides |
| fzf/less/lazygit/btop | process ancestry/name | static process identity and common config where supported | process identity only | intermediate application boundary and common mappings | plugin/runtime maps and application state |
| ranger/yazi | process ancestry/name | common config where supported | process identity only | intermediate application boundary and common mappings | plugin/runtime maps and application state |
| Unknown application | selected PID/focus ancestry without a recognized adapter | process tree and endpoints | not applicable | selected target and exact evidence boundary | application internals and unobservable receipt/action |

Adding a new adapter requires updating this inventory, the generated support
matrix where its capability is user-visible, and a test or documented
limitation for each advertised behavior.

The deterministic desktop-session fixtures and their CLI-test mapping are in
[`tests/fixtures/sessions/matrix.json`](tests/fixtures/sessions/matrix.json).
They cover the adapter shapes without claiming live runtime or version state.
