# Whykey adapter inventory

This inventory describes the adapters currently present in the diagnostic
engine. “Unknown” is intentional: whykey records when a version, runtime
registration, active mode, generated include, or application-side action is
not observable. Adapter applicability never means that every binding is known.

## Desktop and compositor adapters

Native capture backends are separate from static desktop adapters. Hyprland
uses socket2 IPC and a temporary Lua hook (`capture.native-compositor`). The
default pass-through hook only observes. `--suppress` also installs the private
`__whykey_capture` submap and restores the recorded submap before each report.
Both hooks carry a 5-second lease that whykey renews every 2 seconds, so a
listener killed without cleanup cannot leave them behind. Sway reports binding
events through IPC in pass-through mode (`capture.native-sway`). Pass-through
is the default there too. Sway IPC has no suppression hook, so an explicit
`--suppress` fails and points to `--no-suppress`. Other desktops keep their
static inspection adapters and fall back to terminal or evdev capture.

Support tiers are T1 live IPC, T2 live query, T3 literal configuration, and
T4 manifest extension. T4 manifests are opt-in, bounded, and conditional. They
add static inspection, binding inventory, and optional focus lookup; they do
not add a capture backend.

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Hyprland | `HYPRLAND_INSTANCE_SIGNATURE`, instance discovery, or Hyprland IPC; skipped for remote SSH | `hyprctl binds -j/submap/devices/activewindow`, plus the text `hyprctl binds` listing when a bind has an unreported key; literal config and Lua files; bounded `source=` chains (with `*`/`?` globs, sorted like glob(3)) retain file/line hints | IPC availability; version is not persisted | bindings, submaps, device scopes, keymaps, focused PID, Lua/config hints; keysym-named keys are normalized; `__lua`/plugin dispatchers consume unless the callback returns `pass_event`; runtime-built Lua binds that `-j` reports with key `""`/keycode 0 take their key from the text listing (joined on dispatcher, argument, modmask, submap, and description); `code:N` binds are shown in `bindings`/`conflicts` as their key in the active XKB layout | dynamic Lua, inhibitors, generated config, runtime-vs-disk divergence, and `source=` bracket globs (`[…]`, skipped). An empty-key bind that the text listing cannot resolve (multi-key or ambiguous) takes its key only from a literal Lua `code:` hint; otherwise it stays a possible match for its modifiers, and `conflicts` counts it without comparing it |
| Sway | `SWAYSOCK` or Sway desktop identity; skipped for SSH | `swaymsg -t get_config -r` (running main config), `get_binding_state`, `get_tree`; `include` files are read from disk (`~`, `$ENV`, last-component globs) | native version probe for applicability | `set`/`set_from_resource` variables (resource fallback only), modes, `bindsym`/`bindcode`/`unbind*` with `--release`/`--locked`/`--inhibited`/`--input-device`, exact modifiers, focused PID; a `nop` binding still grabs the key; `--release` binds forward the press | included files on disk may differ from what Sway loaded; runtime `bindsym` via IPC, external grabs, and unknown binding mode (assumed default when `get_binding_state` fails) |
| i3 | `I3SOCK` or i3 desktop identity; skipped for SSH | `i3-msg -t get_config` (running config plus `included_configs`), `get_binding_state`, `get_tree` | native version probe for applicability | variables, modes, `bindsym`/`bindcode`/`unbind*` flags, exact modifiers, focused PID; a `nop` binding still grabs the key; a `--release` binding consumes the press, as i3's source shows (passive grab for every binding, press never replayed) | runtime `bindsym` via IPC, external clients, unreported grabs, no live i3 run of the `--release` behavior, and unknown binding mode (assumed default when `get_binding_state` fails) |
| GNOME | GNOME desktop/session identity or `gsettings` availability | `gsettings` media-key and custom-keybinding schemas | no version claim | standard/custom literal accelerators and configured command text | daemon runtime registration, extensions, and command execution |
| GNOME Shell runtime | GNOME Shell session D-Bus introspection | `org.gnome.Shell` read-only introspection | service reachability only | live API reachability; no full keybinding dump is claimed | Shell exposes no read-only full shortcut inventory; extensions and runtime state remain unknown |
| KDE Plasma | KDE desktop identity or `kglobalshortcutsrc` path | `kglobalshortcutsrc` (or `KGLOBALSHORTCUTS_CONFIG`) plus read-only `org.kde.KGlobalAccel` introspection | service reachability only | literal file bindings with live-vs-file provenance | the bounded D-Bus spike found no stable, read-only enumeration contract for `allComponents`/`getGlobalShortcuts`; live registrations, runtime reload, and plugin shortcuts remain conditional |
| xdg-desktop-portal GlobalShortcuts | session D-Bus portal introspection | `org.freedesktop.portal.GlobalShortcuts` | service reachability only | portal API reachability; no existing binding inventory is claimed | portal exposes registration/session APIs, not a read-only list of another client’s shortcuts |
| Xfce | Xfce desktop identity or `xfconf-query` | live `xfconf-query -c xfce4-keyboard-shortcuts` | no version claim | current custom/default channel values | daemon races, plugins, and competing clients |
| Cinnamon | Cinnamon identity or `CINNAMON_VERSION`/GSettings | `gsettings list-recursively org.cinnamon.desktop.keybindings` | environment version is detection evidence only | literal schema accelerators | custom schemas not returned and runtime plugin state |
| MATE | MATE identity or session variables | MATE Marco and SettingsDaemon GSettings schemas | no version claim | literal global accelerators and schema sources | runtime registrations and custom extensions |
| Niri | `NIRI_SOCKET`/desktop identity or config file | literal `NIRI_CONFIG`/`config.kdl` `binds` blocks | no version claim | literal mappings and modes | active mode, reload state, inhibitors, and dynamic KDL |
| River | River identity or `RIVER_INIT`/init file | literal `riverctl map` commands | no version claim | literal mappings and modes | executable shell logic, reload state, and runtime mode |
| dwl | dwl desktop identity or `DWL_CONFIG`/`~/.config/dwl/config.h` | literal `static const Key keys[]` entries | no version claim | key/modifier/function rows from `config.h` | dwl has no stable global-shortcut IPC; runtime activation, reload state, generated config, and custom macros |
| herbstluftwm | `HERBSTLUFTWM_SOCKET` or desktop identity | bounded `herbstclient list_keybinds` | command reachability only | effective key/action rows and command provenance | runtime hooks, reload races, generated commands, and focused-PID activation remain conditional |
| Wayfire | Wayfire identity or config path | literal `binding_*` values in `WAYFIRE_CONFIG`/`wayfire.ini` | no version claim | literal plugin binding values | plugin activation, reload state, and dynamic values |
| labwc | labwc identity or config path | Openbox-compatible `<keybind>` XML | no version claim | literal keybind/action nodes | runtime reload, scripts, and dynamic XML generation |
| bspwm/sxhkd | bspwm/sxhkd identity or `SXHKD_CONFIG` | `SXHKD_CONFIG`; otherwise the `-c` file and extra configuration arguments of the running sxhkd (`/proc/<pid>/cmdline`, relative to its `/proc/<pid>/cwd`); otherwise the default `sxhkdrc`. sxhkd has no include directive, so none is followed; paths that are not regular files are reported as unreadable | no version claim | literal chords and commands per file, with file/line context | reload state, shell expansion, generated content, and ordering; without a running sxhkd the default path comes from whykey's own `XDG_CONFIG_HOME` |
| Openbox | Openbox identity or config path | Openbox `<keybind>` XML | no version claim | literal keybind/action nodes | runtime reload, scripts, and competing X11 clients |
| X11 xbindkeys | X11 session (`DISPLAY` without Wayland) or config path | `XBINDKEYSRC`/`.xbindkeysrc` | no version claim | literal command/key pairs | daemon activation, WM precedence, and other X11 grabs |
| Programmable X11 | AwesomeWM, Qtile, or XMonad identity/config detection | literal `rc.lua`, `config.py`, or `xmonad.hs` declarations | no version claim; executable config is never evaluated | literal Awesome `awful.key`, Qtile `Key`, and XMonad `xK_*` mappings | helpers, variables, modes, reload state, and evaluated runtime tables |
| Generic compositor | desktop/session/display-server context without a dedicated adapter | environment identity only | not applicable | session context and an opaque compositor layer | global bindings and focused PID are unavailable unless another adapter provides them |
| Manifest extension (T4) | `WHYKEY_ADAPTER_DIR` or `~/.config/whykey/adapters` plus env/desktop hint | bounded `bindings_cmd`, optional `focused_cmd` | command reachability only; generation is optional | script-reported bindings and optional focused PID | runtime activation, reload state, and command output outside the bounded snapshot |

## Terminal adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Ghostty | `TERM_PROGRAM`/`LC_TERMINAL` identity | `ghostty +list-keybinds --plain`, config files, includes | command availability only | effective keybind overrides, output actions, normal terminal bytes; AltGr text from the active XKB layout's level 3/4, and dead keys reported as starting composition | physical/multi-key triggers and dynamic config; AltGr text without a keymap, without a level 3, or combined with Ctrl/Alt/Super |
| Kitty | `TERM_PROGRAM`, `KITTY_WINDOW_ID`, or protocol response | Kitty key tables where available; terminal keyboard protocol during capture | protocol response flags | key actions and negotiated capture flags | every negotiated capability, runtime window state, and app-side handling |
| Alacritty | terminal identity or `ALACRITTY_SOCKET` | TOML/YAML config and key bindings | identity variables only | literal actions, forwarding, and consuming actions | dynamic config and runtime overrides |
| Foot | terminal identity | `foot.ini` key bindings | identity variables only | literal key bindings | generated config and runtime reload |
| WezTerm | terminal identity or `WEZTERM_PANE` | effective `wezterm show-keys` output | command availability only | effective default/table key assignments | active non-default table and Lua conditions |
| Konsole | `KONSOLE_PROFILE_NAME`/`KONSOLE_VERSION` or terminal identity | active profile `.keytab` | environment version only | literal keytab actions and sequences | screen/keypad modes and runtime profile reload |
| Generic terminal | no dedicated identity, or unsupported terminal | effective `termios` plus safe byte encoding fallback | not applicable | predicted terminal bytes and confidence; AltGr level-3/4 text and dead keys resolved as for the named terminals | terminal-specific key tables and pre-terminal grabs |

## Shell, TTY, and multiplexer adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Linux TTY | readable `/dev/tty` | `termios` and terminal adapter bytes | kernel behavior is not version-claimed | `ISIG` signal characters (`VINTR`/`VQUIT`/`VSUSP`) and `IXON` flow control (`VSTOP`/`VSTART`) reported separately; bytes shared with other keys (Escape, Tab, Return, Ctrl+@/Space, Ctrl+H) are called out | kernel line discipline details outside the snapshot |
| Bash/Readline | `SHELL` (or the `--pid` target's parent shell) plus shell-init snapshot or inputrc paths | runtime `bind -P/-S/-X` snapshot when present (then authoritative); otherwise `INPUTRC`, `~/.inputrc`, `/etc/inputrc` plus GNU Readline's emacs defaults | Bash version is not claimed | active map, prefix keys (`\C-x`, `\e`) and their continuations, multi-key sequences across `inspect` steps, macros, `bind -x` shell commands | without a snapshot, answers are reported as unverified because `~/.bashrc` `bind` commands may override them; unobserved runtime mode and generated includes |
| Zsh/ZLE | `SHELL` plus shell-init snapshot/config | `WHYKEY_ZLE_BINDINGS`, `.zshrc`/`.zshenv` | Zsh version is not claimed | active keymap and literal sequences | plugin-generated maps and runtime mode races |
| Fish | `SHELL` plus shell-init snapshot/config | `WHYKEY_FISH_BINDINGS`, `config.fish` | Fish version is not claimed | active bind mode and literal sequences | dynamic functions, generated bindings, and unknown mode |
| tmux | `TMUX`/`TMUX_PANE` | `tmux list-keys` root/active/prefix tables | command output only | a bare key is looked up in the active (or root) table only; across `inspect` steps the prefix and `switch-client -T` select the next table, and an unbound key after the prefix is reported as discarded | nested server races and hooks not visible through tables |
| GNU Screen | `STY` or screen process/config | system then user `screenrc`, `bind`, `bindkey` | command/config evidence only | effective prefix and literal bindings; a key right after the command escape in an `inspect` sequence is resolved against `bind` | nested sessions, runtime reload, and shell expansion |
| Zellij | `ZELLIJ` markers or config | dumped defaults plus `config.kdl` overrides | installed command availability only | mode tables, defaults, user overrides, clear-defaults | active mode unless explicitly supplied and runtime reload |
| SSH boundary | `SSH_CONNECTION`/`SSH_TTY` | local process ancestry, PTY, and environment | not applicable | local/remote boundary and conditional remote leg | remote whykey state and remote application internals |

## Input, remapping, and IME adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| evdev | readable keyboard event device or explicit `--device` | `/dev/input/event*`, capabilities, key bitmap (sysfs words read most significant first), LEDs | kernel device metadata only | physical code, device name, pressed/locked modifiers, SYN_DROPPED recovery | denied devices, latched state, firmware, and complete XKB translation |
| keyd | process (`keyd`) and `/etc/keyd/*.conf` or `WHYKEY_KEYD_CONFIG` | `include`, `[ids]`, `[aliases]`, `[global] default_layout`, `[main]`, the modifier layers for the held modifiers and their composites; last binding wins | process/config evidence only | a plain key or `C-x` combo redirects; `layer(x)` on a one-modifier layer acts as that modifier; other layer actions, `macro`, `command`, and `noop` consume | `overload`, `oneshot`, `timeout`, and `lettermod` stay conditional; listed or excluded `[ids]` may leave the keyboard unmanaged; printable keys assume their US position unless the layout is `us` |
| kanata | process (`kanata`) and its `-c`/`--cfg` file (relative to `/proc/<pid>/cwd`), else `KANATA_CONFIG`, `~/.config/kanata/kanata.kbd` | `defsrc` with the first `deflayer`/`deflayermap`, `_`, `XX`, aliases to plain keys, `C-`/`S-`/`A-`/`M-`/`RA-` prefixes, `multi`, `defoverrides`, `process-unmapped-keys`, `linux-dev` | process/config evidence only | position remaps redirect; `layer-switch`/`layer-while-held`, macros, and `cmd` consume | tap-hold, tap-dance, one-shot, `fork`, and `switch` stay conditional; a file without `defsrc` is not evaluated; nesting beyond the parser's depth cap is reported as not evaluated |
| kmonad | process/config detection | literal config aliases/layers/tap-hold forms (listed by `doctor`) | process/config evidence only | conservative literal mappings as evidence; the queried key is not evaluated | the route stays uncertain whenever kmonad runs: input device, layer stack, and tap-hold are not parsed |
| input-remapper | process/config detection | bounded JSON mapping pairs (listed by `doctor`) | process/config evidence only | literal pairs as evidence; the queried key is not evaluated | the active per-device preset is unknown, so the route stays uncertain |
| xremap | process (`xremap`) with its configuration paths and `--device`, else `XREMAP_CONFIG`, `~/.config/xremap/*.yml`/`*.yaml` | YAML subset: `modmap` feeding `keymap`, exact then extra-modifier matches, `exact_match` | process/config evidence only | keymap combos redirect; modmap remaps feed keymap | application/window/device/mode conditions, side-specific modifiers, and multi-purpose keys stay conditional; anchors, aliases, tags, block scalars, and nesting beyond the parser's depth cap make the file not evaluated |
| Fcitx5 | session variables/process and D-Bus preflight | bounded read-only `org.fcitx.Fcitx.Controller1` calls | service presence only | active engine/context when query succeeds; an inactive context or a plain `keyboard-*` engine passes keys through | committed text, application preedit, input-method hotkeys, and unavailable D-Bus |
| IBus | session variables/process and safe API availability | read-only IBus context where available | service presence only | detected IME context and limited engine state; an inactive context or an `xkb:*` engine passes keys through | committed text, dead-key/Compose history, input-method hotkeys, and app preedit |
| XKB/layout | device keymap fields and optional `xkbcli` | RMLVO/keymap data, compiled `symbols[N]` (or libxkbcommon ≤ 1.7 `symbols[GroupN]`) groups, `xkb_types` and per-key `type`, `NoSymbol` level positions, optional `WHYKEY_XKB_GROUP`, and a keysym-to-Unicode table from `keysymdef.h` | compiler availability only | symbolic/code candidates; level selection by key type (CapsLock only where the type maps Lock, Shift+AltGr to level 4 on four-level types); AltGr level-3/4 text and dead keysyms for terminal prediction | compositor-independent Compose history and missing group/RMLVO state remain conditional; CapsLock and NumLock for AltGr text come from the query, not the live LEDs |

The route stops at a remapper result that consumes the key. A redirect sends
the target key to the later layers, or makes them conditional when the target
cannot be parsed. keyd, kanata, and xremap reach both paths with real
configurations. A capture from a physical evdev device names the exact key;
otherwise layout-independent keys map to fixed codes, and printable keys use
their US position and stay uncertain unless `WHYKEY_XKB_LAYOUT` or
`XKB_DEFAULT_LAYOUT` is `us` with no variant. Input that the compositor or a
remapper's virtual device already delivered stays uncertain. A configuration
found without a running process, findings that disagree, or several remappers
acting on the key make the result uncertain; an unreadable configuration is
unavailable.
An Input method row is shown whenever an IME is detected, right after the
Terminal row.

## Interactive application adapters

| Adapter | Detection | Source/API | Version evidence | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- | --- |
| Neovim/Vim | process ancestry or selected PID | RPC/server expression when available; init/vimrc fallback | process identity only | mode and common/live mappings | plugins, runtime precedence, and unavailable server |
| Emacs | process ancestry or selected PID | `emacsclient` active map; init files fallback | process identity only | active/common global maps | package maps, buffers, and unavailable server |
| Helix | process ancestry/config path | `config.toml` literal keymap | process identity only | literal mappings and mode hints | runtime commands, plugins, and generated config |
| Micro | process ancestry/config path | `bindings.json` literal mappings | process identity only | literal mappings | plugins and runtime overrides |
| Kakoune | process ancestry/config path | `kakrc` literal mappings | process identity only | literal mappings | hooks, scripts, and runtime mode |
| VS Code/VSCodium | process name and user keybindings path | `keybindings.json` literal single-key entries | process identity only | command, key, and `when` evidence | chords, extensions, `when` evaluation, and runtime context; `whykey-vscode` below models VS Code's resolver |
| JetBrains | recognized IDE process and XML keymap path | XML keymaps | process identity only | literal shortcut entries | plugins, IDE modes, and runtime overrides |
| fzf/less/lazygit/btop | process ancestry/name | static process identity and common config where supported | process identity only | intermediate application boundary and common mappings | plugin/runtime maps and application state |
| ranger/yazi | process ancestry/name | common config where supported | process identity only | intermediate application boundary and common mappings | plugin/runtime maps and application state |
| Unknown application | selected PID/focus ancestry without a recognized adapter | process tree and endpoints | not applicable | selected target and exact evidence boundary | application internals and unobservable receipt/action |

## First-party application extensions

These run only through an explicit `whykey extension <program> <combination>`
and answer with extension protocol v1. They are separate from the built-in
application adapters above.

| Extension | Discovery | Source/API | Parsed or reported | Explicit unknown boundary |
| --- | --- | --- | --- | --- |
| `whykey-nvim` | `NVIM_LISTEN_ADDRESS`, `--listen`/`--server` processes, and default `$XDG_RUNTIME_DIR/nvim.<pid>.0` sockets (newest first) | one `luaeval` query over `--remote-expr`: `nvim_get_mode`, `maparg`, and the buffer and global keymaps for longer mappings | Lua-callback mappings (with `desc`), `<Plug>` chains up to 5 levels, buffer-local vs global, `expr`/`noremap`, prefix-only keys and the `timeoutlen` wait, `<C-Bslash>` in terminal mode | built-in commands and `vim.on_key`/`getchar()` handlers (no mapping is uncertain); terminal mode without a mapping is uncertain/continues; several servers without one named in the environment make the answer uncertain; AltGr and `CODE:N` queries |
| `whykey-vscode` | `$XDG_CONFIG_HOME`, then `~/.config`, for Code, Code - Insiders, Code - OSS, VSCodium, and Flatpak paths | user `keybindings.json` (JSONC, BOM) with VS Code's resolver order: the last matching entry without a `when` clause wins; `-command` removes defaults only | `when` clauses named but not evaluated (uncertain); a chord prefix consumes; an empty command passes the key; `^command` runs and passes it; VS Code key syntax (`cmd`/`win`/`meta` = Super, `[KeyA]`, multi-chord) | VS Code's default keybindings are not bundled, so keys missing from the user file are uncertain; profile keybindings or several builds make answers uncertain; extensions' contributed keybindings; the integrated terminal's `commandsToSkipShell`; AltGr and `CODE:N` queries |
| `whykey-emacs` | `EMACS_SERVER_FILE`, `EMACS_SOCKET_NAME`, or the default daemon socket | `emacsclient --eval` in the selected window's buffer: `key-binding` at point and `current-active-maps` | winning map (global, major, minor mode, or overriding/emulation/text-property), prefix keys, explicit `undefined`, GUI function-key events before their ASCII forms, shift translation | `key-translation-map`, `input-decode-map`, and pre-command hooks (no binding is uncertain); a timeout or evaluation error is uncertain; not run against a live Emacs |

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
