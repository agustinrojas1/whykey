# Whykey adapter inventory

This inventory describes the adapters currently present in the diagnostic
engine. “Unknown” is intentional: whykey records when a version, runtime
registration, active mode, generated include, or application-side action is
not observable. Adapter applicability never means that every binding is known.

The **Verification** column says what reality has confirmed. *lab* names the
[lab](lab/README.md) scenarios (under `lab/scenarios/`) in which the real
program reacted to synthetic input the way whykey predicted. *live* names hand
checks on the maintainer's desktop. *unverified* means the adapter was built
from documentation, upstream source, and fixtures only. Scenarios that pass
only as honest uncertainty (`PASS~`) do not count. The generated
[support matrix](SUPPORT_MATRIX.md) is the authoritative list.

## Desktop and compositor adapters

`whykey listen` tries capture backends in order: native compositor capture,
then a probe window, then the terminal. Hyprland native capture uses socket2
IPC and a temporary Lua hook (`capture.native-compositor`) and needs a Lua
configuration; under a `.conf` configuration Hyprland rejects `hyprctl eval`
and the chain moves on. The default pass-through hook only observes.
`--suppress` also installs the private `__whykey_capture` submap and restores
the recorded submap before each report. Both hooks carry a 5-second lease
that whykey renews every 2 seconds, so a listener killed without cleanup
cannot leave them behind. Sway reports binding events through IPC in
pass-through mode (`capture.native-sway`), so it sees only keys that ran a
Sway binding. Sway IPC has no suppression hook, so an explicit `--suppress`
fails and points to `--no-suppress`. The probe window
(`capture.probe-window`) is a whykey-owned Wayland or X11 client that works
on every desktop with a usable display outside SSH. It shows whether the
desktop delivered the key to a client. When only modifiers arrive, it infers
"not delivered", a conditional verdict, because a lone modifier tap looks
the same. evdev capture is opt-in.

Support tiers are T1 live IPC, T2 live query, T3 literal configuration, and
T4 manifest extension. T4 manifests are opt-in, bounded, and conditional. They
add static inspection, binding inventory, and optional focus lookup; they do
not add a capture backend.

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary | Verification |
| --- | --- | --- | --- | --- | --- | --- |
| Hyprland | `HYPRLAND_INSTANCE_SIGNATURE`, instance discovery, or Hyprland IPC; skipped for remote SSH | `hyprctl binds -j/submap/devices/activewindow`, plus the text `hyprctl binds` listing when a bind has an unreported key; literal config and Lua files; bounded `source=` chains (with `*`/`?` globs, sorted like glob(3)) retain file/line hints | IPC availability; version is not persisted | bindings, submaps, device scopes, keymaps, focused PID, Lua/config hints; keysym-named keys are normalized; `__lua`/plugin dispatchers consume unless the callback returns `pass_event`; runtime-built Lua binds that `-j` reports with key `""`/keycode 0 take their key from the text listing (joined on dispatcher, argument, modmask, submap, and description); `code:N` binds are shown in `bindings`/`conflicts` as their key in the active XKB layout | dynamic Lua, inhibitors, generated config, runtime-vs-disk divergence, and POSIX named classes or backslash escapes in `source=` bracket globs. An empty-key bind that the text listing cannot resolve (multi-key or ambiguous) takes its key only from a literal Lua `code:` hint; otherwise it stays a possible match for its modifiers, and `conflicts` counts it without comparing it | lab: `scenarios/hyprland/` (22, Hyprland 0.56.2 `.conf` and Lua, including `hyprland-release-and-press-bind`, `hyprland-keypad-enter`, `hyprland-keypad-enter-not-return`, `hyprland-return-not-keypad-enter`, `hyprland-submap-break`, `hyprland-submap-break-catchall`), `hyprland-window-bound`, `hyprland-window-unbound`; live: static inspection on the maintainer's desktop |
| Sway | `SWAYSOCK` or Sway desktop identity; skipped for SSH | `swaymsg -t get_config -r` (running main config), `get_binding_state`, `get_tree`; `include` files are read from disk (`~`, `$ENV`, last-component globs) | native version probe for applicability | `set`/`set_from_resource` variables (resource fallback only), modes, `bindsym`/`bindcode`/`unbind*` with `--release`/`--locked`/`--inhibited`/`--input-device`, exact modifiers, focused PID; a `nop` binding still grabs the key; `--release` binds forward the press; a keypad digit matches only a binding without `Mod2` (Sway consumes NumLock) | included files on disk may differ from what Sway loaded; runtime `bindsym` via IPC, external grabs, and unknown binding mode (assumed default when `get_binding_state` fails) | lab: `scenarios/sway/` (14, Sway 1.11, including `sway-keypad-digit`, `sway-keypad-digit-mod2`, `sway-keypad-enter`, `sway-keypad-enter-not-return`, `sway-return-not-keypad-enter`), `sway-window-bound`, `sway-window-unbound` |
| i3 | `I3SOCK` or i3 desktop identity; skipped for SSH | `i3-msg -r -t get_config` (running config plus `included_configs`; plain text from i3-msg older than 4.20 is accepted), `get_binding_state`, `get_tree`, `get_version` for `loaded_config_file_name`; on native X11 the X11 grab probe refines the result | native version probe for applicability | variables, modes, `bindsym`/`bindcode`/`unbind*` flags, exact modifiers, focused PID; a `nop` binding still grabs the key; a `--release` binding consumes the key: the focused client receives neither press nor release; a keypad digit with NumLock on matches only a binding that includes `Mod2` | runtime `bindsym` via IPC, external clients, and unknown binding mode (assumed default when `get_binding_state` fails) | lab: `scenarios/i3/` (12, i3 4.25.1, including `i3-keypad-digit-numlock`, `i3-keypad-digit-without-numlock-modifier`; the five `i3-*-no-display` scenarios run whykey without `DISPLAY`, so the adapter alone decides), `i3-window-bound`, `i3-window-unbound`, `i3-window-super-alone` |
| GNOME | GNOME desktop/session identity or `gsettings` availability | live `gsettings list-recursively` over `org.gnome.desktop.wm.keybindings`, `org.gnome.mutter.keybindings`, `org.gnome.mutter.wayland.keybindings`, `org.gnome.shell.keybindings`, `org.gnome.settings-daemon.plugins.media-keys`, and each `custom-keybinding:<path>`; doctor reader `gnome-gsettings` | no version claim | string accelerators parsed by mutter's rules (`Primary`, `Above_Tab`, `0xNN` keycodes, `disabled`), custom command text; non-string values (`volume-step`) are ignored; a missing schema is a note | `<Meta>` is not compared; the memory backend (defaults only) makes the layer unavailable; extensions and grabs Mutter takes outside these schemas | lab: `scenarios/gnome/` (4), headless mutter 50: mutter-owned bindings only; GNOME Shell and gnome-settings-daemon behaviour not verified |
| KDE Plasma | KDE desktop identity, the kglobalaccel D-Bus name, or `kglobalshortcutsrc` | `busctl --user --auto-start=no` calls to the running kglobalaccel: `globalShortcutsByKey` with the Qt key code, `getComponent`, `Component.isActive`; inventory from `allComponents`/`allShortcutInfos`; doctor reader `kglobalaccel-runtime`. Without the daemon, `kglobalshortcutsrc` (or `KGLOBALSHORTCUTS_CONFIG`) with KConfig escapes | daemon reachability only | an active component's shortcut consumes the key; an inactive one passes it ("registered but inactive"); a file-only match is handled with unknown forwarding, never consumed | activity is per component (L-012); the daemon is never auto-started; `Key_Launch0`–`9`, `Key_MediaTogglePlayPause`, and Shift+digit stored as a symbol are not matched (D-093) | lab: `kde-launch-bind`, `kde-unbound`, `kde-inactive-component`, and their `-no-display` variants without the X11 grab probe (kglobalacceld 6.7.5 on Xvfb; no Plasma session) |
| X11 grab probe | `DISPLAY` set and the session not remote; full effect only on native X11 (`XDG_SESSION_TYPE`, else a `WAYLAND_DISPLAY` socket that accepts a connection, decides XWayland) | x11rb: core `GrabKey` and XInput2 `XIPassiveGrabDevice` on the root window with Lock/NumLock variants; `BadAccess` means another client holds the grab; successful grabs are released at once | not applicable | a grab confirms the claiming adapter's binding as Consumed (an adapter that knows the key is replayed, sxhkd `~`, keeps handled-and-passed with the grab evidence), or adds a neutral "X11 key grab" layer; a free key makes a configured binding inactive; under XWayland a grab only adds a note | only root-window passive grabs are seen; the grab owner is not reported by the X server; runs under the diagnostic budget on a helper thread; a stalled X server makes it unavailable; legacy non-Latin-1 keysyms are not resolved (grab state unknown) | lab: `scenarios/{i3,openbox,xbindkeys,sxhkd,awesome,kde}/` (Xvfb); support-matrix entry `compositor.x11-grab` is credited with the verdicts the grab decided; `sxhkd-replay`, `sxhkd-replay-plain` check that a replaying adapter keeps handled-and-passed |
| xdg-desktop-portal GlobalShortcuts | session D-Bus portal introspection | `org.freedesktop.portal.GlobalShortcuts` | service reachability only | portal API reachability; no existing binding inventory is claimed | portal exposes registration/session APIs, not a read-only list of another client’s shortcuts | unverified |
| Xfce | Xfce desktop identity or `xfconf-query` | live `xfconf-query -c xfce4-keyboard-shortcuts` | no version claim | current custom/default channel values | daemon races, plugins, and competing clients | unverified |
| Cinnamon | Cinnamon identity or `CINNAMON_VERSION`/GSettings | `gsettings list-recursively org.cinnamon.desktop.keybindings` | environment version is detection evidence only | literal schema accelerators | custom schemas not returned and runtime plugin state | unverified |
| MATE | MATE identity or session variables | MATE Marco and SettingsDaemon GSettings schemas | no version claim | literal global accelerators and schema sources | runtime registrations and custom extensions | unverified |
| Niri | `NIRI_SOCKET`/desktop identity or config file | literal `NIRI_CONFIG`/`config.kdl` `binds` blocks | no version claim | literal mappings and modes | active mode, reload state, inhibitors, and dynamic KDL | unverified |
| River | River identity or `RIVER_INIT`/init file | literal `riverctl map` commands | no version claim | literal mappings and modes | executable shell logic, reload state, and runtime mode | unverified |
| dwl | dwl desktop identity or `DWL_CONFIG`/`~/.config/dwl/config.h` | literal `static const Key keys[]` entries | no version claim | key/modifier/function rows from `config.h` | dwl has no stable global-shortcut IPC; runtime activation, reload state, generated config, and custom macros | unverified |
| herbstluftwm | `HERBSTLUFTWM_SOCKET` or desktop identity | bounded `herbstclient list_keybinds` | command reachability only | effective key/action rows and command provenance | runtime hooks, reload races, generated commands, and focused-PID activation remain conditional | unverified |
| Wayfire | Wayfire identity or config path | literal `binding_*` values in `WAYFIRE_CONFIG`/`wayfire.ini` | no version claim | literal plugin binding values | plugin activation, reload state, and dynamic values | unverified |
| labwc | labwc identity or config path | Openbox-compatible `<keybind>` XML | no version claim | literal keybind/action nodes | runtime reload, scripts, and dynamic XML generation | unverified |
| bspwm/sxhkd | bspwm/sxhkd identity or `SXHKD_CONFIG` | `SXHKD_CONFIG`; otherwise the `-c` file and extra configuration arguments of the running sxhkd (`/proc/<pid>/cmdline`, relative to its `/proc/<pid>/cwd`); otherwise the default `sxhkdrc`. sxhkd has no include directive, so none is followed; paths that are not regular files are reported as unreadable | no version claim | chords and commands per file, with file/line context, parsed like sxhkd 0.6.1: `~` replay (handled-and-passed), `@` release, `{a,b}`/`{1-9}`/`_` expansion in lockstep with the command, plain keys, `meta`/`mode_switch`, uppercase letter as Shift | continuations follow sxhkd's shared hotkey/command byte offset; non-ASCII line edges are trimmed as in sxhkd's C locale; a line with more than 4096 renderings is skipped with a detail; chord chains (`;`/`:`), pointer buttons, and `any` are skipped; reload state, shell expansion, generated content, and ordering; without a running sxhkd the default path comes from whykey's own `XDG_CONFIG_HOME` | lab: `sxhkd-bind`, `sxhkd-unbound`, `sxhkd-replay`, `sxhkd-replay-plain`, `sxhkd-brace`, `sxhkd-brace-range`, `sxhkd-plain-key`, `sxhkd-release`, `sxhkd-continuation-shared-offset`, `sxhkd-continuation-past-text`, `sxhkd-non-ascii-command` (sxhkd 0.6.1; bspwm not run) |
| Openbox | Openbox identity or config path | Openbox `<keybind>` XML | no version claim | literal keybind/action nodes | runtime reload, scripts, and competing X11 clients | lab: `openbox-keybind`, `openbox-unbound` (Openbox 3.6.1) |
| X11 xbindkeys | X11 session (`DISPLAY` without Wayland) or config path | `XBINDKEYSRC`/`.xbindkeysrc` | no version claim | literal command/key pairs | daemon activation, WM precedence, and other X11 grabs | lab: `xbindkeys-bind`, `xbindkeys-unbound` (xbindkeys 1.8.7) |
| Programmable X11 | AwesomeWM, Qtile, or XMonad identity/config detection | literal `rc.lua`, `config.py`, or `xmonad.hs` declarations | no version claim; executable config is never evaluated | literal Awesome `awful.key`, Qtile `Key`, and XMonad `xK_*` mappings | helpers, variables, modes, reload state, and evaluated runtime tables | lab: `awesome-bind`, `awesome-unbound` (AwesomeWM 4.3); Qtile and XMonad unverified |
| Generic compositor | desktop/session/display-server context without a dedicated adapter | environment identity only | not applicable | session context and an opaque compositor layer | global bindings and focused PID are unavailable unless another adapter provides them | unverified |
| Manifest extension (T4) | `WHYKEY_ADAPTER_DIR` or `~/.config/whykey/adapters` plus env/desktop hint | bounded `bindings_cmd`, optional `focused_cmd` | command reachability only; generation is optional | script-reported bindings and optional focused PID | runtime activation, reload state, and command output outside the bounded snapshot | unverified |

## Terminal adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary | Verification |
| --- | --- | --- | --- | --- | --- | --- |
| Ghostty | `TERM_PROGRAM`/`LC_TERMINAL` identity | `ghostty +list-keybinds --plain`, config files, includes | command availability only | effective keybind overrides, output actions, normal terminal bytes; AltGr text from the active XKB layout's level 3/4, and dead keys reported as starting composition | physical/multi-key triggers and dynamic config; AltGr text without a keymap, without a level 3, or combined with Ctrl/Alt/Super | unverified |
| Kitty | `TERM_PROGRAM`, `KITTY_WINDOW_ID`, or protocol response | Kitty key tables where available; terminal keyboard protocol during capture | protocol response flags | key actions and negotiated capture flags | every negotiated capability, runtime window state, and app-side handling | unverified |
| Alacritty | terminal identity or `ALACRITTY_SOCKET` | TOML/YAML config and key bindings | identity variables only | literal actions, forwarding, and consuming actions | dynamic config and runtime overrides | unverified |
| Foot | terminal identity | `foot.ini` key bindings | identity variables only | literal key bindings | generated config and runtime reload | unverified |
| WezTerm | terminal identity or `WEZTERM_PANE` | effective `wezterm show-keys` output | command availability only | effective default/table key assignments | active non-default table and Lua conditions | unverified |
| Konsole | `KONSOLE_PROFILE_NAME`/`KONSOLE_VERSION` or terminal identity | active profile `.keytab` | environment version only | literal keytab actions and sequences | screen/keypad modes and runtime profile reload | unverified |
| Generic terminal | no dedicated identity, or unsupported terminal | effective `termios` plus safe byte encoding fallback | not applicable | predicted terminal bytes and confidence; AltGr level-3/4 text and dead keys resolved as for the named terminals | terminal-specific key tables and pre-terminal grabs | unverified |

## Shell, TTY, and multiplexer adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary | Verification |
| --- | --- | --- | --- | --- | --- | --- |
| Linux TTY | readable `/dev/tty` | `termios` and terminal adapter bytes | kernel behavior is not version-claimed | `ISIG` signal characters (`VINTR`/`VQUIT`/`VSUSP`) and `IXON` flow control (`VSTOP`/`VSTART`) reported separately; bytes shared with other keys (Escape, Tab, Return, Ctrl+@/Space, Ctrl+H) are called out | kernel line discipline details outside the snapshot | unverified |
| Bash/Readline | nearest interactive shell in the process tree (or above the `--pid` target), else `SHELL` | runtime `bind -p` (per keymap) `/-S/-X/-V` snapshot from `shell-init` (then authoritative); otherwise `bind`, `bind -x`, `bind -m`, `bind -f`, and `set -o vi` from `/etc/bash.bashrc` and `~/.bashrc`, then `INPUTRC`, `~/.inputrc`, `/etc/inputrc`, plus the Readline 8.3 defaults of a pristine bash 5.3.9 (emacs, or vi-insert in vi mode) | Bash version is not claimed | active map, prefix keys (`\C-x`, `\e`) and their continuations, multi-key sequences across `inspect` steps, macros, `bind -x` shell commands | without a snapshot, answers are unverified: sourced files and plugins are not followed; vi mode assumes the vi-insert keymap; an unbound key reports Readline's effect (bell, inserted leftover bytes) because the shell is the endpoint | lab: `bash-*-init` (4, bash 5.3.9, including `bash-unbound-shift-up-init` for the unbound-key effect); live: with `shell-init` on the maintainer's desktop. `bash-*-noinit` pass only as honest uncertainty |
| Zsh/ZLE | nearest interactive shell in the process tree (login shells as `-zsh` included), else `SHELL` | `WHYKEY_ZLE_BINDINGS` from `shell-init` (`bindkey -lL main; bindkey -L; bindkey -LM vicmd`); otherwise the installed `zsh -f` defaults plus `bindkey` lines from `/etc/zshenv`, `/etc/zsh/zshenv`, `~/.zshenv`, `/etc/zshrc`, `/etc/zsh/zshrc`, `~/.zshrc` | Zsh version is not claimed | `bindkey -L` output with `-R`, `-a`, `-s`, `-r`, `-A`; bindings follow the keymap `main` points to when declared; ZLE key reading (an unbound escape sequence leaves its tail on the line; ESC then a key in vi insert mode) | plugins, sourced files, and conditionals without `shell-init`; a `vicmd` state with no captured `vicmd` bindings is unknown | lab: `zsh-*-init` (including `zsh-unbound-ctrl-left-init`), `tmux-zsh-user-bind` (zsh 5.9). `zsh-*-noinit` pass only as honest uncertainty |
| Fish | nearest interactive shell in the process tree, else `SHELL` | `WHYKEY_FISH_BINDINGS` (`bind --all \| string collect`) and `WHYKEY_FISH_MODE` from `shell-init`; otherwise `fish --no-config` presets for the preset in `fish_variables` plus `bind` lines from `conf.d`, `config.fish`, `fish_user_key_bindings.fish` | Fish version is not claimed | fish quoting and escapes, fish 4 key names and fish 3 `-k` names, `--preset` vs user precedence, `-M`/`-m`/`-e`; a bind without `-M` is mode `default` (vi normal); Kitty and SS3 input decoded to legacy bytes | functions, generated bindings, and unknown mode without `shell-init` | lab: `fish-*-init` (including `fish-preset-alt-left-init`), `tmux-fish-vi-insert-bind` (fish 4.6.0). `fish-*-noinit` pass only as honest uncertainty |
| tmux | `TMUX`/`TMUX_PANE` | `tmux list-keys` root/active/prefix tables | command output only | a bare key is looked up in the active (or root) table only; across `inspect` steps the prefix and `switch-client -T` select the next table, and an unbound key after the prefix is reported as discarded; `list-keys` keys are case-sensitive (`C` is Shift+c, `BTab` is Shift+Tab), escaped, quoted, and `-` keys are read; inside a multiplexer the tmux layer runs before the pane TTY driver | nested server races and hooks not visible through tables | lab: `tmux-prefix`, `tmux-root-bind`, `tmux-unbound`, `tmux-prefix-zsh`, `tmux-prefix-lowercase`, `tmux-prefix-uppercase`, `tmux-prefix-escaped-percent`, `tmux-prefix-escaped-semicolon`, `tmux-prefix-dash`, `tmux-prefix-before-pane-tty`, `tmux-root-quoted-brace`, `tmux-root-btab` (tmux 3.7c); `tmux-root-alt-escape` passes only as honest uncertainty (D-124) |
| GNU Screen | `STY` or screen process/config | system then user `screenrc`, `bind`, `bindkey` | command/config evidence only | effective prefix and literal bindings; a key right after the command escape in an `inspect` sequence is resolved against `bind` | nested sessions, runtime reload, and shell expansion | lab: `screen-prefix-fish` (Screen 5.0.1) |
| Zellij | `ZELLIJ` markers or config | dumped defaults plus `config.kdl` overrides | installed command availability only | mode tables, defaults, user overrides, clear-defaults | active mode unless explicitly supplied and runtime reload | unverified |
| SSH boundary | `SSH_CONNECTION`/`SSH_TTY` | local process ancestry, PTY, and environment | not applicable | local/remote boundary and conditional remote leg | remote whykey state and remote application internals | unverified |

## Input, remapping, and IME adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary | Verification |
| --- | --- | --- | --- | --- | --- | --- |
| evdev | readable keyboard event device or explicit `--device` | `/dev/input/event*`, capabilities, key bitmap (sysfs words read most significant first), LEDs | kernel device metadata only | physical code, device name, pressed/locked modifiers, SYN_DROPPED recovery | denied devices, latched state, firmware, and complete XKB translation | unverified |
| keyd | process (`keyd`) and `/etc/keyd/*.conf` or `WHYKEY_KEYD_CONFIG` | `include`, `[ids]`, `[aliases]`, `[global] default_layout`, `[main]`, the modifier layers for the held modifiers and their composites; last binding wins | process/config evidence only | a plain key or `C-x` combo redirects; `layer(x)` on a one-modifier layer acts as that modifier; other layer actions, `macro`, `command`, and `noop` consume | `overload`, `oneshot`, `timeout`, and `lettermod` stay conditional; listed or excluded `[ids]` may leave the keyboard unmanaged; printable keys assume their US position unless the layout is `us` | unverified |
| kanata | process (`kanata`) and its `-c`/`--cfg` file (relative to `/proc/<pid>/cwd`), else `KANATA_CONFIG`, `~/.config/kanata/kanata.kbd` | `defsrc` with the first `deflayer`/`deflayermap`, `_`, `XX`, aliases to plain keys, `C-`/`S-`/`A-`/`M-`/`RA-` prefixes, `multi`, `defoverrides`, `process-unmapped-keys`, `linux-dev` | process/config evidence only | position remaps redirect; `layer-switch`/`layer-while-held`, macros, and `cmd` consume | tap-hold, tap-dance, one-shot, `fork`, and `switch` stay conditional; a file without `defsrc` is not evaluated; nesting beyond the parser's depth cap is reported as not evaluated | unverified |
| kmonad | process/config detection | config path from the running process's command line (and its `--fallthrough`/`--input`/`--implicit-around`), else `KMONAD_CONFIG` or `~/.config/kmonad/*.kbd` when no kmonad runs | process/config evidence only | the queried key through `defcfg` (`device-file`, `fallthrough`, `implicit-around`), named/unnamed `defsrc`, the first `deflayer`, aliases, kmonad key and shifted names, modifier prefixes, and `around` forms; remaps redirect, layer/macro/command/compose/dead-key buttons and dropped keys consume | tap, multi-tap, sticky, stepped, and timeout buttons; later layers; conditional unless the captured device is proven to be the `device-file` | unverified |
| input-remapper | process/config detection | `INPUT_REMAPPER_CONFIG`, `$XDG_CONFIG_HOME/input-remapper-2`, or `~/.config/input-remapper-2`: `config.json` autoload entries (input-remapper device groups, covering every evdev node of the group) and their presets (list and older `mapping` formats), `xmodmap.json` symbols | process/config evidence only | the queried key in each autoloaded preset; key or combination outputs redirect, `disable` and macros consume | conditional macros (`if_tap`, `mod_tap`, …), multi-key inputs, symbols that need `xmodmap.json`; presets applied from the GUI without autoload are unknown; conditional unless the captured device matches the preset's device | unverified |
| xremap | process (`xremap`) with its configuration paths and `--device`, else `XREMAP_CONFIG`, `~/.config/xremap/*.yml`/`*.yaml` | YAML subset: `modmap` feeding `keymap`, exact then extra-modifier matches, `exact_match` | process/config evidence only | keymap combos redirect; modmap remaps feed keymap | application/window/device/mode conditions, side-specific modifiers, and multi-purpose keys stay conditional; anchors, aliases, tags, block scalars, and nesting beyond the parser's depth cap make the file not evaluated | unverified |
| Fcitx5 | session variables/process and D-Bus preflight | bounded read-only `org.fcitx.Fcitx.Controller1` calls | service presence only | active engine/context when query succeeds; an inactive context or a plain `keyboard-*` engine passes keys through | committed text, application preedit, input-method hotkeys, and unavailable D-Bus | unverified |
| IBus | session variables/process and safe API availability | read-only IBus context where available | service presence only | detected IME context and limited engine state; an inactive context or an `xkb:*` engine passes keys through | committed text, dead-key/Compose history, input-method hotkeys, and app preedit | unverified |
| XKB/layout | device keymap fields and optional `xkbcli` | RMLVO/keymap data, compiled `symbols[N]` (or libxkbcommon ≤ 1.7 `symbols[GroupN]`) groups, `xkb_types` and per-key `type`, `NoSymbol` level positions, optional `WHYKEY_XKB_GROUP`, and a keysym-to-Unicode table from `keysymdef.h` | compiler availability only | symbolic/code candidates; level selection by key type (CapsLock only where the type maps Lock, Shift+AltGr to level 4 on four-level types); AltGr level-3/4 text and dead keysyms for terminal prediction | compositor-independent Compose history and missing group/RMLVO state remain conditional; CapsLock and NumLock for AltGr text come from the query, not the live LEDs | unverified |

The route stops at a remapper result that consumes the key. A redirect sends
the target key to the later layers, or makes them conditional when the target
cannot be parsed. keyd, kanata, and xremap reach both paths with real
configurations. A capture from a physical evdev device names the exact key;
otherwise layout-independent keys map to fixed codes (keypad keys to the
keypad's own codes, such as keyd `kpenter` and `kp1`; a main key never
matches a keypad code; a remap to a keypad key leaves the downstream key
unknown, D-127), and printable keys use
their US position and stay uncertain unless `WHYKEY_XKB_LAYOUT` or
`XKB_DEFAULT_LAYOUT` is `us` with no variant. Input that the compositor or a
remapper's virtual device already delivered stays uncertain. A configuration
found without a running process, findings that disagree, or several remappers
acting on the key make the result uncertain; an unreadable configuration is
unavailable.
An Input method row is shown whenever an IME is detected, right after the
Terminal row.

## Interactive application adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary | Verification |
| --- | --- | --- | --- | --- | --- | --- |
| Neovim/Vim | process ancestry or selected PID | RPC/server expression when available; init/vimrc fallback | process identity only | mode and common/live mappings | plugins, runtime precedence, and unavailable server | unverified |
| Emacs | process ancestry or selected PID | `emacsclient` active map; init files fallback | process identity only | active/common global maps | package maps, buffers, and unavailable server | unverified |
| Helix | process ancestry/config path | `config.toml` literal keymap | process identity only | literal mappings and mode hints | runtime commands, plugins, and generated config | unverified |
| Micro | process ancestry/config path | `bindings.json` literal mappings | process identity only | literal mappings | plugins and runtime overrides | unverified |
| Kakoune | process ancestry/config path | `kakrc` literal mappings | process identity only | literal mappings | hooks, scripts, and runtime mode | unverified |
| VS Code/VSCodium | process name and user keybindings path | `keybindings.json` literal single-key entries | process identity only | command, key, and `when` evidence | chords, extensions, `when` evaluation, and runtime context; `whykey-vscode` below models VS Code's resolver | unverified |
| JetBrains | recognized IDE process and XML keymap path | XML keymaps | process identity only | literal shortcut entries | plugins, IDE modes, and runtime overrides | unverified |
| fzf/less/lazygit/btop | process ancestry/name | static process identity and common config where supported | process identity only | intermediate application boundary and common mappings | plugin/runtime maps and application state | unverified |
| ranger/yazi | process ancestry/name | common config where supported | process identity only | intermediate application boundary and common mappings | plugin/runtime maps and application state | unverified |
| Unknown application | selected PID/focus ancestry without a recognized adapter | process tree and endpoints | not applicable | selected target and exact evidence boundary | application internals and unobservable receipt/action | unverified |

## First-party application extensions

These run only through an explicit `whykey extension <program> <combination>`
and answer with extension protocol v1. They are separate from the built-in
application adapters above.

| Extension | Discovery | Source/API | Parsed or reported | Explicit unknown boundary | Verification |
| --- | --- | --- | --- | --- | --- |
| `whykey-nvim` | `NVIM_LISTEN_ADDRESS`, `--listen`/`--server` processes, and default `$XDG_RUNTIME_DIR/nvim.<pid>.0` sockets (newest first) | one `luaeval` query over `--remote-expr`: `nvim_get_mode`, `maparg`, and the buffer and global keymaps for longer mappings | Lua-callback mappings (with `desc`), `<Plug>` chains up to 5 levels, buffer-local vs global, `expr`/`noremap`, prefix-only keys and the `timeoutlen` wait, `<C-Bslash>` in terminal mode | built-in commands and `vim.on_key`/`getchar()` handlers (no mapping is uncertain); terminal mode without a mapping is uncertain/continues; several servers without one named in the environment make the answer uncertain; AltGr and `CODE:N` queries | live: Neovim 0.12.5 on the maintainer's desktop |
| `whykey-vscode` | `$XDG_CONFIG_HOME`, then `~/.config`, for Code, Code - Insiders, Code - OSS, VSCodium, and Flatpak paths | user `keybindings.json` (JSONC, BOM) with VS Code's resolver order: the last matching entry without a `when` clause wins; `-command` removes defaults only | `when` clauses named but not evaluated (uncertain); a chord prefix consumes; an empty command passes the key; `^command` runs and passes it; VS Code key syntax (`cmd`/`win`/`meta` = Super, `[KeyA]`, multi-chord) | VS Code's default keybindings are not bundled, so keys missing from the user file are uncertain; profile keybindings or several builds make answers uncertain; extensions' contributed keybindings; the integrated terminal's `commandsToSkipShell`; AltGr and `CODE:N` queries | unverified (D-052) |
| `whykey-emacs` | `EMACS_SERVER_FILE`, `EMACS_SOCKET_NAME`, or the default daemon socket | `emacsclient --eval` in the selected window's buffer: `key-binding` at point and `current-active-maps` | winning map (global, major, minor mode, or overriding/emulation/text-property), prefix keys, explicit `undefined`, GUI function-key events before their ASCII forms, shift translation | `key-translation-map`, `input-decode-map`, and pre-command hooks (no binding is uncertain); a timeout or evaluation error is uncertain; not run against a live Emacs | unverified (D-055) |

Adding a new adapter requires updating this inventory, the generated support
matrix where its capability is user-visible, and a test or documented
limitation for each advertised behavior.

Listener internals keep terminal protocol decoding (legacy escape sequences
and Kitty keyboard protocol) behind `listen_protocol`; terminal restoration,
evdev reads, and report rendering remain separate transport responsibilities.

Hyprland inspection has the same boundary: IPC/instance/config acquisition is
kept in the Hyprland adapter, while `hyprland_matching` analyzes an already
collected JSON snapshot. The analysis boundary never invokes `hyprctl`, reads
config files, or mutates compositor state.

The deterministic desktop-session fixtures and their CLI-test mapping are in
[`tests/fixtures/sessions/matrix.json`](tests/fixtures/sessions/matrix.json).
They cover the adapter shapes without claiming live runtime or version state.

## C2 adapter-dedup inventory (issue #72)

The post-guard inventory below records the remaining repeated shapes and the
semantic reason each is intentionally local. A helper is safe only when its
success values, failure values, and evidence ordering are identical; a shorter
implementation is not sufficient evidence for a shared contract.

| Shape | Consumers | Finding | Decision |
| --- | --- | --- | --- |
| Remote-session guard | Desktop adapters | `compositor::remote_session()` is the one shared guard; it preserves the SSH boundary before any adapter-specific probe. | Already centralized; no second helper. |
| GSettings accelerator decoding | Cinnamon, GNOME, MATE, XFCE | The common GSettings forms use `gsettings::parse_accelerator` (GNOME uses its explicit historical variant); malformed and modifier-only values have shared tests. | Already shared in `gsettings.rs`; no duplicate extraction. |
| Openbox XML keybind parsing | Openbox, labwc | labwc deliberately reuses `openbox::parse_bindings`, including action ordering and malformed-node filtering. | Already shared; no duplicate extraction. |
| Layer-result construction | Cinnamon, KDE, i3, Sway, tiling, X11, programmable, and others | Labels, `LayerId`, propagation, evidence detail, and unavailable wording differ by adapter. Collapsing constructors would either add match-adapter switches or reorder evidence. | Keep local; semantics are not identical. |
| Key/value splitters | X11, sxhkd, River, Niri, Wayfire, Xfce, KDE | Split rules intentionally differ: X11 brace/release rejection, sxhkd hotkey lines with indented command lines (one file at a time; sxhkd has no includes), River literal command tokens, Niri KDL comments, Wayfire INI comments, and KDE comma fields. | Do not unify; contracts and edge cases differ. |
| Config normalization | Programmable X11, Hyprland Lua, terminal adapters | Lua literal scanning, Python/HS declaration scanning, shell-safe command parsing, and terminal protocol decoding each reject different dynamic forms. | Keep bounded normalizers local; no identical failure contract. |
| Binding collection order | All adapters | Some sources use last-wins maps, others preserve wire/file order or emit possible matches beside exact matches. | Keep collection logic local to preserve evidence-array order. |

Conclusion: after the shared remote-session guard and the existing parser
reuse above, no remaining candidate has identical semantics and identical
failure strings. Issue #72 is therefore closed as an inventory-backed wontfix;
the contract-differing splitters are explicitly excluded from future generic
helpers. The standard fixture gate remains the regression check for this
boundary.
