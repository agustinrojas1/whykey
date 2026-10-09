# whykey specification

## Platform scope

Whykey is a Linux-only program. The supported binary depends on Linux evdev,
PTY/termios, XDG session paths, and Linux compositor/session IPC. Windows and
macOS are outside this specification; non-Linux targets fail at compile time
with an explicit message. A future portable `whykey-core` would be a separate
workspace decision and is not implied by this binary.

## Goal

Given one key combination, report how the running Linux compositor, terminal and downstream session handle it. Hyprland, Sway, i3, herbstluftwm, GNOME, KDE Plasma, Xfce, Cinnamon, MATE, Niri, dwl, River, Wayfire, labwc, bspwm/sxhkd, Openbox, X11 `xbindkeys`, AwesomeWM, Qtile, and XMonad have read-only desktop adapters; unsupported desktops are reported as an explicit conditional context.

```text
whykey ctrl+left
whykey inspect ctrl+x ctrl+s
whykey extension ./my-editor-whykey ctrl+x
```

`whykey` inspects state and prints an explanation. It never edits configuration files and never executes the inspected shortcut. Static inspection never changes compositor state or reloads Hyprland. `listen` with native Hyprland capture installs a temporary Lua hook. The default pass-through hook only observes; `--suppress` also activates a private submap and restores the recorded submap before reporting. Each hook carries a 5-second lease that `listen` renews every 2 seconds. If renewals stop, the hook removes itself, and a suppressing hook also restores the submap. Compositor extension manifests and `whykey extension` run the programs the user configured or named.

Dependency versions are pinned by `Cargo.lock`; registry checksums and the
toolchains/host covered by repository verification are recorded in
`DEPENDENCY_PROVENANCE.md`. This provenance does not claim that every native
distribution package or release target has been built.

Capture mode is available with `whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--suppress|--no-suppress|--pass-through] [--dry-run] [--explain-capture] [--terminal] [--ndjson] [--output PATH]` and an optional Linux-only `whykey listen --evdev [--device /dev/input/eventN]`. Native compositor capture (Hyprland, or Sway IPC) is preferred when compositor IPC is reachable and observes pass-through by default. `--no-suppress` states that default explicitly; `--pass-through` is its compatibility alias. `--suppress` opts into temporary suppression through a private Hyprland submap. It is rejected with exit status 2 when combined with `--terminal`, `--evdev`, `--device`, or `--no-suppress`. When native suppression is unavailable, including on Sway (whose IPC has no suppression hook), it fails with exit status 1 instead of falling back. Without `--suppress`, `listen` falls back to terminal capture of the event that reaches `whykey` when native capture is unavailable; pass `--terminal` to force terminal-only capture. `listen --evdev` observes read-only `EV_KEY` events before compositor processing and never grabs the device. `--timeout` is a wall-clock deadline for the capture and `--count` stops after the requested number of reports. `--events all` includes modifier-only and release events in native compositor or evdev mode. A key consumed before it reaches the terminal cannot be observed by terminal capture, so native capture or the static command remains the fallback for those shortcuts. Capture observations and normal downstream bytes are separate values. Captured JSON uses `Terminal`, the v1-compatible `"Hyprland"` string, canonical v2 `{"kind":"compositor-native","backend":"Hyprland"}`, or `Evdev` with device name/path. Reading the legacy `"Hyprland"` string normalizes it to the same in-memory native source as the v2 object; schema-v1 output writes the string again. Other native backends, such as Sway, keep the canonical compositor-native object in schema-v1 output until a backend-specific compatibility form is defined. Text output leads with a shortcut/operation header, diagnosis, practical effect, relevant layers, and uncertainty where needed. The diagnosis says a layer has a matching key only when that layer reported binding evidence. When no inspected layer reports a binding, the diagnosis says so and names the layers whose handling is unknown. Binds known only by their modifiers (unresolved keys) are reported as possible matches, and shell answers built from defaults are labelled unverified. Rows for layers that pass the key without a binding appear only with `--verbose`. Raw events, probe bytes, encoding internals, full modifier state, XKB candidates, keyboard inventory, inactive submaps, and alternate keys also stay behind `--verbose`; JSON always carries the full evidence. `whykey shell-init <bash|zsh|fish>` can install an optional wrapper that passes current shell editing snapshots through environment variables. Add `--json` to emit machine-readable reports; interactive diagnostics remain on stderr in JSON capture mode. `--ndjson` implies JSON and emits one compact schema-v2 `listen` record per stdout line. Human output accepts `--color=auto|always|never`; auto respects `NO_COLOR`, non-TTY output, and `TERM=dumb`, while explicit `always` and `never` take precedence. JSON, NDJSON, shell-init, and completion output never contain styling.

When more than one compositor adapter is applicable, candidates are scored from the one environment snapshot: reachable read-only IPC contributes 3 points, an explicit compositor environment signature contributes 2, and a matching desktop/session hint contributes 1. Configuration-only evidence contributes 0. Candidates are sorted by descending score with registry order breaking ties; the selected compositor remains the first candidate for compatibility.

Capture is observe-only by default. `--suppress` opts into temporary native
compositor suppression on Hyprland. `--dry-run` selects a backend without arming
it, and reports `capture backend: none` when `--suppress` cannot be honored.
`--explain-capture` prints the detection evidence. Sway IPC is pass-through-only.

### Compositor extension manifests

Users can add a Tier 4 adapter by placing a validated manifest in
`~/.config/whykey/adapters/`, or by setting `WHYKEY_ADAPTER_DIR`. The manifest
contains a slug `id`, display name, `tier = "extension"`, environment and
desktop hints, and an argv-only `bindings_cmd`. `focused_cmd` is optional.
Unknown keys and malformed entries become doctor warnings. Manifest commands
run only when the adapter is applicable; inspection runs only the
highest-scored applicable manifest. Commands go through the bounded two-second,
one-megabyte command runner. The command returns either a JSON array of
`key`/`action` records or literal `key action` lines. Invalid records are
skipped and counted. Actions are display data; whykey never executes them.

Extension candidates use the same score weights as compiled-in adapters. A
manifest environment hint contributes 2 and a desktop hint contributes 1.
No IPC score is claimed before a command has run. A compiled-in adapter wins
ties. Tier 4 adapters provide static inspection, inventory, and optional
focused-PID evidence only; `listen` does not capture through a manifest.

JSON reports have a top-level `schema_version` field. Version 1 remains the
default for compatibility. `--json --schema-version 2` (or `--json-v2`) opts
into the version-2 shape, which adds an operation name, session context,
structured input/observation fields, and per-layer evidence while preserving
the same diagnostic conclusions. Extension stdin/stdout remains protocol v1
until a separately negotiated extension protocol is introduced.

`whykey doctor` runs read-only prerequisite checks for the controlling TTY, compositor context, terminal adapter, XKB compiler, shell, SSH, multiplexer context, common remappers, and IBus/Fcitx5 session context. When the Hyprland signature is absent, it discovers an instance through `hyprctl instances -j`, preferring the instance whose Wayland socket matches `WAYLAND_DISPLAY` and accepting a sole running instance. It reports the selected dedicated terminal adapter when one is detected. Before querying Fcitx5, the adapter verifies the session D-Bus and the `org.fcitx.Fcitx5` well-known name; it then uses the read-only `org.fcitx.Fcitx.Controller1` methods directly rather than launching `fcitx5-remote`, whose client may abort when the bus is unavailable. A failed preflight is reported as unavailable runtime evidence. Hyprland, Sway, i3, GNOME, KDE, Xfce, Cinnamon, MATE, Niri, River, Wayfire, labwc, bspwm/sxhkd, Openbox, X11 xbindkeys, and programmable X11 WMs expose separate `applicable` and `ipc` fields; other detected desktops are described through a generic compositor context with their desktop/session/display-server identity. The JSON form includes `schema_version: 1`, matching key reports. It exits `0` when TTY and an applicable compositor context or SSH session are available and either Ghostty is inspectable or a different terminal is detected for the generic terminal fallback; XKB is reported as an optional precision feature.

The doctor JSON defaults to schema v1 for compatibility; `--schema-version 2`
wraps the same checks in the v2 operation/context envelope.

## Input

The legacy top-level form accepts exactly one combination. Modifiers are
separated with `+`; the final key may itself be `+` or contain `=` (for
example `ctrl++` and `ctrl+=`). Hyprland numeric keycodes use `code:<n>`, for
example `ctrl+code:30`.

Modifier names are `ctrl`/`control`/`ctl`, `alt`/`mod1`, `shift`,
`super`/`meta`/`win`/`mod4`, `caps`/`capslock`/`lock`, `num`/`numlock`/`mod2`,
`hyper`/`mod3`, and `altgr`/`iso_level3_shift`/`mod5`. The key must be one of
the following: a single Unicode character; a named key (arrows,
`return`/`enter`, `escape`, `space`, `tab`, `backspace`, `delete`, `page_up`,
keypad and lock keys, or keysym names such as `comma`, `slash`, and `minus`);
`f1`–`f35`; an `XF86*` media key; `code:<n>`; or a Hyprland `mouse:<n>`.
Unknown names are rejected with exit status 2. When an edit-distance match is
close, the error suggests it (`ctrl+retrun` → `return`). A combination
contains exactly one non-modifier key. Configuration adapters parse keys
leniently, so unusual names in user configuration are still compared.

`whykey inspect <combination-or-sequence>` accepts one or more combinations
separated by whitespace, for example `whykey inspect ctrl+x ctrl+e`. Each step
is retained as its own text or JSON report, and all steps share one environment
snapshot. Steps carry prefix state. Each layer receives only the earlier keys
that actually reached it, meaning that no earlier layer consumed or redirected
them. Readline replays those keys the way Readline reads them: a complete
binding runs and resets, a prefix such as `\C-x` stays pending, and an unbound
key after a prefix is discarded with a bell. Zsh and Fish try the concatenated
sequence first, then the last key alone. tmux switches to the prefix table
after the prefix key, and to the table named by `switch-client -T`. A binding
returns the client to root (or to the named table). An unbound key in a
non-root table is retried in root and otherwise discarded. A bare key is
looked up only in the active (or root) table. GNU Screen resolves the key after
its command escape against `bind`, and a doubled escape is a literal. If an
earlier step's bytes are unknown, the shell step is reported as unknown.
`--pid <pid>` changes application-layer discovery to the
selected process and its ancestors; a target that disappears is reported as an
unavailable layer. `--focused` resolves a process through the active Hyprland,
Sway, or i3 tree API and uses that process for application and shell evidence;
other desktops report the missing focused-window API. A matching compositor
manifest can supply conditional focused-PID evidence when no compiled-in focus
API is available. Sequences are bounded to 64 steps so malformed or
unbounded command-line input cannot grow the report without limit.

`--instance <id>` explicitly selects a Hyprland instance for the inspection
when automatic discovery is ambiguous. The selection is process-local and is
not persisted in the user's environment.

`doctor --json` includes `evdev.devices`, a read-only list of candidate keyboard event paths with their kernel-reported name, keyboard capability, readability, and an optional opening error. This makes `--device` selection and input-group permission failures discoverable without touching device state.

`whykey capabilities` exposes the support matrix as a versioned report. Each
entry has an identifier, area, `implemented` boolean, `availability` value
(`available`, `unavailable`, or `planned`), and evidence text. Implemented does
not imply that the integration is available in the current session; planned
entries are listed so automation can distinguish a known limitation from a
permission or environment failure.

The repository's human-readable [support matrix](SUPPORT_MATRIX.md) is
generated from `support-matrix.json`. The Rust tests compare that manifest with
the capability registry, so documentation and tests use the same contract.
Behavior gaps and their regression evidence are tracked separately in
`DEFECT_REGISTER.md`.
The per-adapter implementation inventory is maintained in
`ADAPTER_INVENTORY.md`; unknown version or runtime state is recorded as
unknown rather than inferred.

The declared minimum compiler is Rust 1.85.0; the locked dependency set and
the full test suite are checked with that toolchain in CI.

`whykey bindings` emits a versioned read-only inventory from the detected
compositor adapters. Entries contain source, key, action, context, certainty,
and optional typed `device` and `submap` fields read from the adapter payload,
never inferred from context strings. `--key`, `--action`, `--source`,
`--device`, and `--submap` filter case-insensitively with AND logic; missing
metadata never matches an active filter. `complete` is intentionally false:
application/plugin bindings, input inhibitors, and firmware transformations
can remain outside the observable desktop APIs. Adapter errors are listed
under `unavailable`.

`whykey conflicts` consumes the same inventory and groups entries only when
source, key, and context all match. Entries whose key the source does not
report (Hyprland binds with key `""`) are never grouped, and their count is
listed as a limitation. Different actions are reported as possible conflicts
because ordering and runtime precedence are not universally observable.
Hyprland is the exception: same-key binds are classified as firing together,
because Hyprland runs every matching bind, unless one of them is a `submap`
bind, which stops the binds after it. Identical shortcuts from different
applications are not combined. `whykey bindings --key` also lists, under
`possible_matches`, same-modifier entries whose key the source does not report.

`doctor` and `capabilities` detect common pre-compositor remappers by process
and conventional configuration paths. Static literal keyd mappings are
reported as evidence, while virtual-device routing, timing-dependent
tap-hold, macros, and firmware transformations remain conditional.
Literal input-remapper JSON pairs, xremap YAML `remap` entries, and
Kanata/KMonad aliases, layers, and tap-hold forms are also surfaced when their
configuration is available; dynamic expressions and active-layer selection
remain conditional.

`whykey replay <file>` accepts one or more concatenated schema-v1 JSON reports,
including the pretty-printed documents emitted by `listen --json`. It validates
the report shape and renders stored evidence only; it never opens an input
device, executes a configured action, or injects a key. Files are limited to
16 MiB. `whykey snapshot <combination> --output <file>` stores one redacted
schema-v2 report for later replay and comparison. A snapshot preserves the
observed conclusion, not the raw IPC or configuration inputs that adapter
analysis would need to run again offline; replaying it never queries the
desktop. Snapshots redact shell identity, private paths, and secret-bearing
assignments mechanically. Inspect a snapshot before sharing it.
`whykey diff <before> <after>` compares two stored snapshots or reports
without querying the desktop.

`whykey extension <program> <combination>` runs one explicitly selected
application extension. Whykey writes a schema-v1 `inspect` request to the
child's stdin and validates its schema-v1 response from stdout. The response
declares zero or more capabilities (`list_bindings`, `query_mode`,
`identify_action`, and `observe_receipt`) plus one result with `status`,
`propagation`, `summary`, and optional `details`. Unknown capability or result
values are rejected; a valid response becomes an `External extension` layer.
The child receives no key event to execute, is isolated in a process group,
and is subject to the normal two-second/one-megabyte limits. Extensions are
never discovered or invoked automatically.

### First-party extensions

- `extensions/whykey-nvim`: stdlib Python 3 script querying running Neovim via `--remote-expr`. Declares `query_mode` and `identify_action` capabilities. Connects via `NVIM_LISTEN_ADDRESS` or scans `/proc` for active `nvim --listen` / `--server` instances. Returns `status: "unavailable"` when no running Neovim server is found. Converts `KeyCombo` to Vim notation (`<C-x>`, `<C-Left>`, `<C-M-CR>`), evaluates `mode()` and `maparg()`, and reports handling, target action, and `noremap` status.
- `extensions/whykey-vscode`: stdlib Python 3 script querying user `keybindings.json` across Code, Code - OSS, and VSCodium. Declares `identify_action` capability. Normalizes keys and resolves chords, exact bindings, and unbind removals (`"command": "-id"`). Exact matches with unevaluated `when` clauses yield `handled` status with `indeterminate` propagation; unbound or unmapped combinations return `not_handled` (product defaults not evaluated).
- `extensions/whykey-emacs`: stdlib Python 3 script querying running Emacs via `emacsclient --eval`. Declares `query_mode` and `identify_action` capabilities. Connects via `EMACS_SERVER_FILE`, `EMACS_SOCKET_NAME`, or default daemon socket. Returns `status: "unavailable"` when no running daemon is reachable with remediation to run `emacs --daemon` or `(server-start)`. Evaluates `key-binding`, `current-active-maps`, and `major-mode`, reporting the winning map (`global` vs `major` vs `minor`).

Parsing is case-insensitive. JSON keeps the numeric modifier mask. Text output orders modifiers `Ctrl`, `Alt`, `Shift`, `Super`, `Caps`, `Mod2`, `Mod3`, `AltGr` (Mod5), then the key. Kitty's Hyper/Meta/Num modifier bits are represented by the existing `MOD3`/`MOD5`/`MOD2` masks when a distinct public name is unavailable. As a result, a Mod5/AltGr query is currently predicted as Kitty Meta (`DEFECT_REGISTER.md` D-046).

## Hyprland inspection

1. Run `hyprctl binds -j` to read the effective binding set.
2. Run `hyprctl submap -j` to read the active submap.
3. Match the normalized key name and exact modifier mask. Keysym names in bindings (`comma`, `slash`, `minus`, `Prior`) are normalized the same way as the query. Hyprland reports some runtime-built Lua binds with key `""` and keycode 0. Such a bind takes its key from literal Lua hints with the same modmask and description, but only when all of those hints agree (for example `SUPER + code:20`). Otherwise the key is unresolved. An unresolved bind in the active submap with the query's modifiers is reported as a possible match (`UnresolvedBindingKey`), never as an exact one.
4. Consider a binding active when it belongs to the active submap or has the universal-submap flag. Keep same-key bindings from inactive submaps in the evidence so mode-dependent shortcuts are visible without treating them as active.
5. Determine handling and propagation separately. Use Hyprland's `non_consuming`, `auto_consuming`, `release`, and `longPress` flags, the `pass` and `submap` dispatchers, and any dispatcher result visible through the runtime API. A `pass` binding is reported as redirected because its target window is not necessarily the next layer in the inspected chain. `__lua` and plugin dispatchers follow the same rule as every other dispatcher. Hyprland consumes the key unless the bind is `non_consuming`; an `auto_consuming` bind stays uncertain. A Lua callback can forward the key only by returning `pass_event`, which whykey cannot observe, so the evidence says so.
6. Include active same-key bindings with different modifiers as possible matches unless their `ignore_mods` value is known. Read `ignore_mods` from the runtime JSON field when present. Otherwise, use a Lua hint and then a declarative `.conf` hint for the exact modifier mask and key. A hint for another binding on the same key never applies, and a binding with no hint stays a possible match. For declarative config, follow literal `source`/`include` files and variables. The longest variable name wins; unknown names stay literal, and the bind is skipped. Read bind flags from the keyword suffix (`bindi`, `bindln`; letters `lremnatisodpcgukx`), skip multi-key `s` binds, and apply `unbind = MODS, KEY` in order. In discovered Lua files, scan balanced single- and multi-line literal `o.bind`/`hl.bind` calls in file order, without executing Lua. They supply source and description evidence and a literal `ignore_mods` value, including the helper default of `false`. `hl.unbind("COMBO")` removes earlier hints for that combo, a later bind adds it back, `hl.unbind("all")` clears every hint, and a non-literal unbind changes nothing. Globs and dynamic key or options expressions remain possible matches. Surface `locked`, `dont_inhibit`, and `allow_input_capture` when present, because they qualify inhibitor and capture behavior.
7. Report every active exact match, including its dispatcher, argument, and description when present. Never execute `__lua` or plugin dispatchers; the evidence states that their runtime action is not executed by whykey.
8. Query `hyprctl devices -j` when available and include active keyboard names, main-device status, and keymap in the evidence. Accept both legacy string and structured per-device fields in `hyprctl binds -j`. Kernel keycodes enter as evdev and convert once to the XKB namespace (`+8`); Hyprland socket events and numeric binding fields are treated as already XKB. For events from the native capture hook, this has not been confirmed against a live Hyprland (`DEFECT_REGISTER.md` D-042). Missing RMLVO or layout data yields explicit uncertainty, never a guessed US layout.

## Sway and i3 inspection

Sway reads its running main configuration with `swaymsg -t get_config -r`. i3 reads `i3-msg -t get_config`, including `included_configs` when i3 reports them. Sway does not report its includes, so `include` lines are resolved from disk, relative to the including file, with `~`, `$ENV`, and last-component globs. Those files may differ from what Sway loaded. Both adapters parse `set` variables (longest name wins), `set_from_resource` fallbacks (X resources are not read), modes, `{}` blocks, line continuations, and comments. They also parse `bindsym`/`bindcode`/`unbindsym`/`unbindcode` with `--release`, `--locked`, `--inhibited`, and `--input-device`, and skip mouse buttons. They match symbolic names and explicit `code:<n>` queries with exact modifiers against the mode reported by `get_binding_state`. When that query fails, they use the default mode and mark the result uncertain. A `nop` binding still grabs the key. A Sway `--release` binding forwards the press and runs its command on release. On i3, whether the press is withheld has not been verified, so it is reported as uncertain. The focused PID comes from `get_tree`. KDE Plasma reads the INI-style `kglobalshortcutsrc` file (or `KGLOBALSHORTCUTS_CONFIG`), and separately probes `org.kde.KGlobalAccel` through read-only D-Bus introspection; the file remains the inventory source and live-vs-file provenance is explicit. The portal `org.freedesktop.portal.GlobalShortcuts` and GNOME Shell D-Bus readers likewise perform read-only introspection only: both APIs expose registration/session or shell operations rather than a read-only dump of existing shortcuts, so their result is reachability or `api-does-not-expose`, never an invented binding list. Xfce reads `xfconf-query -c xfce4-keyboard-shortcuts -l -v`, covering literal custom/default command properties and their current channel values. Cinnamon reads `gsettings list-recursively org.cinnamon.desktop.keybindings` and reports the schemas and accelerator values returned by that live query. MATE reads its Marco and SettingsDaemon global-keybinding schemas through GSettings. Niri reads literal `binds` blocks from `NIRI_CONFIG` or `~/.config/niri/config.kdl`; runtime reload, active mode, and inhibitor state remain conditional. dwl reads literal `static const Key keys[]` rows from `DWL_CONFIG` or `~/.config/dwl/config.h`; dwl has no stable global-shortcut IPC, so function/action matches remain conditional. River reads literal `riverctl map` commands from `RIVER_INIT` or `~/.config/river/init`, preserving mode and reload uncertainty. Wayfire reads literal `binding_*` accelerator values from `WAYFIRE_CONFIG` or `~/.config/wayfire.ini`, preserving plugin activation and reload uncertainty. labwc reuses the Openbox-compatible `<keybind>` format from `LABWC_CONFIG` or `~/.config/labwc/rc.xml`; runtime reload remains conditional. bspwm/sxhkd follows literal key/command pairs in `SXHKD_CONFIG` or `~/.config/sxhkd/sxhkdrc`, preserving conditional runtime activation and reload state. Openbox reads `<keybind>` XML entries from `OPENBOX_CONFIG` or `~/.config/openbox/rc.xml`, preserving conditional runtime reload state. On X11, `xbindkeys` reads literal command/key pairs from `XBINDKEYSRC` or `~/.xbindkeysrc`; daemon activation and precedence against the window manager or other clients remain conditional. AwesomeWM parses literal `awful.key` declarations from `AWESOME_CONFIG` or `~/.config/awesome/rc.lua`, Qtile parses literal `Key([...], key, ...)` entries from `QTILE_CONFIG` or `~/.config/qtile/config.py`, and XMonad parses literal `xK_*` entries from `XMONAD_CONFIG` or `~/.xmonad/xmonad.hs`. These executable configurations are never evaluated; helpers, dynamic variables, modes, and reload state remain conditional. The adapters remain read-only and preserve conditional status when the compositor does not expose enough runtime state. A desktop without one of these adapters is represented by a generic conditional compositor layer rather than a false Hyprland error.

## GNOME inspection

GNOME uses `gsettings list-recursively org.gnome.settings-daemon.plugins.media-keys` for standard shortcuts and reads each path listed in `custom-keybindings` for custom bindings. Literal GNOME accelerator notation such as `<Super>l` and `<Primary><Alt>t` is normalized to `KeyCombo`. Matching global shortcuts are reported as consuming; configured commands are displayed as evidence but never executed.

Malformed individual binding entries do not discard the valid remainder: they are counted in the report and force the Hyprland result to remain conditional because an omitted entry could still capture the key.

Bindings that use numeric keycodes are inspectable with an explicit `code:<n>` query. When `xkbcli`, the main keyboard's RMLVO data, and its active layout index are available, the effective XKB symbol group is compiled and compared against numeric bindings. Compose sequences, physical keys, and layout-specific edge cases remain conditional when the full state cannot be determined.

## Ghostty inspection

1. Prefer `ghostty +list-keybinds --plain`, which reports effective built-in and user keybinds. If Ghostty is unavailable, prefer `$XDG_CONFIG_HOME/ghostty/config.ghostty` and fall back to the legacy `config`, or the equivalent files under `~/.config`.
2. Follow `config-file` includes, including optional `?` includes and `~` expansion. Included files are applied after the containing file, matching Ghostty's precedence rules.
3. Apply `keybind` entries in order, including `clear` and `unbind`; later entries override earlier entries for the same trigger.
4. Report explicit Ghostty actions as consuming, terminal-output, or conditional. When no binding matches, report the default terminal encoding for supported common keys.
5. If `TERM_PROGRAM` identifies a non-Ghostty terminal, mark this layer not applicable and use only a clearly labeled generic byte fallback for static downstream analysis; do not present it as a Ghostty binding.
6. Preserve physical or multi-key triggers that cannot be represented by `KeyCombo` as unresolved configuration details.
7. If an unresolved physical trigger normalizes to the requested logical key, report Ghostty handling as indeterminate instead of claiming that the key is forwarded.

The same chain may use dedicated Kitty, Alacritty, Foot, WezTerm, or Konsole adapters when the terminal identity is available through identity variables or terminal-specific session variables. WezTerm is queried through `wezterm show-keys` so the adapter consumes effective key tables without implementing a Lua evaluator; this command loads the active WezTerm config. Bindings in non-default WezTerm tables remain conditional because the active table is runtime state, and they must not replace the normal PTY input prediction. Konsole resolves the active profile's keytab and only treats modifier-only rules as exact; screen/keypad/ANSI mode-dependent rules stay conditional. Adapter action tables distinguish known PTY-forwarding actions (for example `ReceiveChar`/`sendText`) from known terminal-consuming actions. An adapter may replace the downstream `TerminalInput` only when it can parse an explicit output sequence; otherwise it reports the binding as conditional and preserves a generic fallback where safe.

If no terminal identity is exported, the implementation must not assume Ghostty merely because its executable or configuration is present. It uses the generic terminal encoding and reports terminal-specific binding inspection as unavailable.

## TTY and shell inspection

1. Read the active terminal settings with `tcgetattr` against `/dev/tty`.
2. Inspect the normal bytes produced by the selected terminal adapter, not the Kitty probe bytes used by `listen`. When the terminal has no explicit binding for the key, the dedicated Kitty, Alacritty, Foot, WezTerm, and Konsole adapters use the shared default encoding. Ctrl with a letter, `@ [ \ ] ^ _`, or Space produces the legacy control byte, and Ctrl+Alt adds an ESC prefix.
3. Report `ISIG` signal characters (`VINTR`, `VQUIT`, `VSUSP`) only while `ISIG` is set, and `IXON` flow-control characters (`VSTOP`, `VSTART`) only while `IXON` is set, each under its own label. For `ctrl+z` with `ISIG` and `VSUSP` set to `^Z`, explain the resulting `SIGTSTP`. When a byte is shared with another key, say so: 0x1b is also Escape, 0x09 Tab, 0x0d Return, 0x00 both Ctrl+@ and Ctrl+Space, and 0x08 Ctrl+H.
4. Bash is detected from the `--pid` target's parent shell or from `SHELL`. When `shell-init` supplied a live `bind -P`, `bind -S`, `bind -X`, and `bind -V` snapshot, that snapshot is the only source. Function, macro, and `bind -x` entries are parsed, and `bind -x` entries are labelled `shell command (bind -x)`. Without a snapshot, check the selected `INPUTRC`, `~/.inputrc`, or `/etc/inputrc`. Follow literal `$include` directives and `set keymap` sections, apply later matching definitions as overrides, and fall back to GNU Readline's emacs defaults only when the active mode is Emacs. Such answers carry the `UnverifiedDefault` reason, because `~/.bashrc` `bind` commands may override them, and the report suggests `eval "$(whykey shell-init bash)"`. Prefix keys such as `\C-x` and `\e` are reported as waiting for the next key, with their continuations listed. Static and captured paths must not claim an Emacs default while the active keymap is vi. For Zsh and Fish, inspect live snapshots supplied by `shell-init` when available and compare arbitrary captured sequences against bindings declared in `.zshrc`/`.zshenv` or `config.fish`. Fish also receives `fish_bind_mode` from the wrapper and only selects mode-specific bindings when that mode is known; otherwise those bindings remain conditional. Without a runtime snapshot, common control-key defaults are reported as unverified defaults, and the report suggests the matching `shell-init` command.

## Exit status

- `0`: inspection completed, whether or not a binding matched.
- `1`: a required layer or prerequisite is unavailable. A detected Hyprland IPC failure is represented as an indeterminate compositor result so downstream layers can still be inspected; the report remains conditional rather than claiming that the shortcut is unbound.
- `2`: invalid CLI input. This includes an unknown key name, more than one non-modifier key, and `--suppress` combined with `--terminal`, `--evdev`, `--device`, or `--no-suppress`.

`whykey listen` uses the same statuses; `--suppress` without native suppression exits `1`. `listen` restores the terminal settings before returning, including after normal errors, Escape, and Ctrl+C. A signal guard is installed before raw mode is entered and outlives the capture session, so `SIGTERM` and `SIGHUP` also restore the terminal. On `SIGTSTP`, `listen` restores the terminal, pops the Kitty flags, removes any leased native hook, and then stops; after `SIGCONT` it re-arms capture. `SIGTSTP` arrives from Ctrl+Z only on terminals using the legacy encoding (where the TTY turns `^Z` into the signal), or from an external `kill -TSTP`. With the Kitty keyboard protocol active (Kitty, Ghostty, foot, WezTerm), Ctrl+Z is a CSI `u` key report, so it is captured and explained like any other key. The Hyprland native backend restores the recorded submap before reporting and retries a failed restoration twice; a restoration that still fails stays pending with an explicit error, and a final best-effort attempt on exit warns instead of failing silently. A process killed with `SIGKILL` cannot run restoration code. In that case the hook stays installed until its 5-second lease expires; a suppressing hook then restores the submap.

## Interactive capture

0. Prefer native compositor capture when its IPC is reachable. The Hyprland backend installs a token-owned Lua hook. In pass-through mode, the hook only listens to keyboard events and reports them to whykey as socket2 events. With `--suppress`, the backend also records the active submap and activates the private `__whykey_capture` submap. The hook counts as armed only after dispatcher success, the observed submap postcondition, and the token-specific armed event. Every hook carries a 5-second lease that is renewed every 2 seconds. If the lease expires, the hook removes itself, and a suppressing hook also restores the submap. Remove the hook, or restore the submap, before reporting each event, and re-arm only for the next one. A stale token never restores another session or revives an expired hook. The Sway backend subscribes to binding events over IPC and is pass-through-only. It decodes the modifier names in `event_state_mask` into the modifier mask.
1. Open `/dev/tty`, save its `termios`, and temporarily disable canonical input, echo, extended input processing, and software flow control. `ISIG` stays enabled so legacy-encoded Ctrl+C and Ctrl+Z still raise `SIGINT` and `SIGTSTP`.
2. Push all currently specified Kitty capture enhancements (`CSI > 31 u`: disambiguation, event types, alternate keys, all-keys reporting, and associated text) when the terminal supports them, and pop them with `CSI < u` on exit. Legacy terminals simply ignore these private control sequences and continue with their normal encoding. The original flags are retained separately for downstream byte prediction.
3. Read one event. Escape or Ctrl+C ends capture. CSI and Kitty protocol sequences are buffered for a short bounded interval, and consecutive literal Escape bytes stay separate events.
4. Decode control bytes, UTF-8 text, Alt-prefixed bytes, common cursor/navigation CSI sequences, and Kitty `u` sequences (including Unicode/function/keypad/media keys, alternate key fields, associated text, and press/repeat/release events) into `KeyCombo`. Preserve unrecognized input as a raw observed key with an explicit unknown-origin warning rather than aborting capture; mark UTF-8 text as potentially produced by Compose/dead-key/layout processing.
5. Remove or restore the native hook before rendering each report, whether it is pass-through or suppressing, and re-arm it only for the next captured event. Keep terminal capture mode active while rendering repeated reports, inspecting the TTY layer against the saved `termios` snapshot. This prevents capture mode from changing the explanation of `Ctrl+Z` and avoids a restore/reopen gap between reports. Restore the terminal and keyboard protocol on normal exit, cancellation, handled signals (`SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`), and errors. On `SIGTSTP`, restore before stopping and re-arm after `SIGCONT`. Evdev capture keeps `--events all` across re-arming. A process killed with `SIGKILL` cannot run restoration code.
6. Run the route in `ROUTE_ORDER`: remapper, compositor, terminal, input method, TTY, session/multiplexer, interactive application, and shell. A consuming layer stops downstream analysis. When a layer is uncertain, later results are retained but marked conditional by the report. The observed event proves that it reached the terminal, so compositor and terminal forwarding is reported as observed. Capture mode does not prove forwarding through the TTY, multiplexer, application, or shell. The terminal adapter resolves a separate normal terminal input for the downstream layers. If that input cannot be inferred, the report labels the downstream result as based on captured bytes or unknown.
7. When an ancestor is Neovim or Vim, query the current `mode()` and live `maparg()` state through the editor's remote-expression interface when a server address is available; otherwise inspect common mappings in the relevant init/vimrc files. For Emacs, query `key-binding` through `emacsclient` when a server is available, then inspect common `global-set-key` and `define-key` declarations in standard init files. For Helix, Micro, and Kakoune, inspect their conventional static keymap files. Detect common full-screen TUIs such as fzf, less, lazygit, btop, ranger, and yazi as intermediate applications even when their mappings are not parsed. For VS Code/VSCodium, parse literal single-key entries from user `keybindings.json`; for recognized JetBrains IDEs, parse literal `keyboard-shortcut` entries from the selected XML keymap. Chords, `when` clauses, plugins, modes, and runtime overrides remain indeterminate. Plugin precedence and mappings that are not visible through the available RPC endpoint or static parser remain indeterminate.

The listener queries the terminal's existing Kitty keyboard flags before enabling its capture flags. A known previous state controls the normal encoding used by Ghostty. A terminal that does not answer the query uses a legacy fallback with inferred confidence. The evdev backend keeps pressed modifier keys per device, reads the initial pressed-key bitmap and lock LEDs when the kernel permits it, resynchronizes after `SYN_DROPPED`, and removes descriptors reported as disconnected. Evdev observations expose pressed left/right modifier names and locked LED names separately from the compatibility bitmask; XKB-style latched state remains explicitly unavailable. The chain also reports detected tmux, GNU Screen, Zellij, and SSH context. tmux root bindings, the active client key table when available, both configured prefixes, and the `prefix`/`prefix2` tables are read. A bare key is looked up only in the active table, or in root. A prefix-table binding applies only in an `inspect` sequence whose earlier step is the prefix. A bare prefix is reported as `tmux prefix key`, and summaries name the table that matched. Screen reads its system file (`SYSTEM_SCREENRC` or common system paths) followed by the user file (`SCREENRC`, or `~/.screenrc`), preserving the effective override order. It inspects the command prefix and literal `bind`/`bindkey` directives and compares direct sequences with common terminal encodings. In an `inspect` sequence, a key that follows the command prefix is resolved against `bind`. Zellij merges dumped installed defaults with `ZELLIJ_CONFIG_FILE`/`config.kdl`, applying user overrides last; `keybinds clear-defaults=true` disables that merge. If defaults cannot be dumped or the KDL shape is unsupported, an unmatched key remains conditional. `WHYKEY_ZELLIJ_MODE` can provide a known runtime mode; without it, the active Zellij mode remains an explicit assumption. If multiple multiplexer markers are present, each adapter is inspected independently and the nesting order is marked conditional. If capture temporarily bypasses a normal TTY consumer, the report stops at that TTY result instead of claiming that the shell received the key.

## Architecture

`KeyCombo` and `KeySequence` own parsing and normalization. `TerminalInput` carries the bytes sent to the child PTY, along with their source and confidence. Each concrete layer reports one typed `LayerResult` whose internal state is a `LayerId` identity plus a single `Outcome` control value; the legacy `status`/`propagation` JSON pair is derived during serialization so schema v1 and v2 keep their exact meaning. One static route order (`ROUTE_ORDER`) sequences the remapper, compositor, terminal, IME (before the TTY stage), TTY, multiplexer/session, application, and shell stages. A remapper result that consumes the key stops the route. If a redirect's target parses as a key, the later layers re-run on that key. If it does not parse, their outcomes are made conditional. The current remapper adapters return only pass or uncertain results (`DEFECT_REGISTER.md` D-043). The IME layer is included whenever an IME is detected, right after the terminal. An inactive IME, or one whose engine is a plain keyboard layout (fcitx5 `keyboard-*`, IBus `xkb:*`), passes keys through. The selected terminal adapter (Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, or generic fallback) produces `TerminalInput` before TTY, session, application, and shell layers inspect it. JSON reports retain the numeric modifier mask, expose a human-readable `key_display`, and summarize overall confidence as `configured`, `confirmed`, or `conditional`. Captured observations keep associated text separate from the logical key and preserve alternate-key metadata. If propagation is uncertain, later layers are still inspected and the report preserves that uncertainty as a conditional result.

Desktop adapters are registered once in `src/registry.rs`. Each entry supplies its detection and IPC probes plus the operations that vary by desktop: key inspection, binding enumeration when supported, focused-window PID lookup when supported, and capability/doctor metadata. Route dispatch, `bindings`, focused-window lookup, `doctor`, `capabilities`, and the support-matrix drift test all read this registry; each command takes one `Environment` snapshot per run, so desktop detection runs once and is shared instead of being repeated per adapter.

Future versions can add more terminal protocols and shell layers. A layer may stop the explanation when it captures a key or pass the key to the next layer.

External integration commands use a bounded executor with a two-second per-query timeout and a one-megabyte stdout/stderr limit. A timeout, read failure, or oversized response remains an adapter error and does not block the rest of the report. Static inspections share a ten-second wall-clock budget across all layers and sequence steps; when it expires, the report retains completed layers and adds an unavailable `Diagnostic budget` result. `WHYKEY_COMMAND_TIMEOUT_MS`, `WHYKEY_COMMAND_MAX_OUTPUT`, and `WHYKEY_DIAGNOSTIC_TIMEOUT_MS` override these defaults for a session.
