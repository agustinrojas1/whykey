# whykey v1.2.1 specification

## Goal

Given one key combination, report how the running Linux compositor, terminal and downstream session handle it. Hyprland, Sway, i3, GNOME, KDE Plasma, Xfce, Cinnamon, MATE, Niri, River, Wayfire, labwc, bspwm/sxhkd, Openbox, X11 `xbindkeys`, AwesomeWM, Qtile, and XMonad have read-only desktop adapters; unsupported desktops are reported as an explicit conditional context.

```text
whykey ctrl+left
whykey inspect ctrl+x ctrl+s
whykey extension ./my-editor-whykey ctrl+x
```

`whykey` inspects state and prints an explanation. Static inspection never changes configuration or reloads Hyprland. `listen` with native compositor capture (currently the Hyprland backend) temporarily installs a private submap to observe the event, then restores the recorded submap before reporting; configuration files are never edited.

Dependency versions are pinned by `Cargo.lock`; registry checksums and the
toolchains/host covered by repository verification are recorded in
`DEPENDENCY_PROVENANCE.md`. This provenance does not claim that every native
distribution package or release target has been built.

Capture mode is available with `whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--suppress|--no-suppress|--pass-through] [--ndjson]` and an optional Linux-only `whykey listen --evdev [--device /dev/input/eventN]`. Native compositor capture, currently the Hyprland backend, is preferred when compositor IPC is reachable: it temporarily suppresses the shortcut via a private submap and restores the recorded submap before reporting. Suppression is the safe default; `--suppress` makes it explicit, while `--no-suppress` captures without suppression and `--pass-through` remains its compatibility alias. When native capture is unavailable, `listen` falls back to terminal capture of the event that reaches `whykey`; pass `--terminal` to force terminal-only capture. `listen --evdev` observes read-only `EV_KEY` events before compositor processing and never grabs the device. `--timeout` is a wall-clock deadline for the capture and `--count` stops after the requested number of reports. `--events all` includes modifier-only and release events in evdev mode. A key consumed before it reaches the terminal cannot be observed by terminal capture, so native capture or the static command remains the fallback for those shortcuts. Capture observations and normal downstream bytes are separate values. Captured JSON uses `Terminal`, the v1-compatible `"Hyprland"` string, canonical v2 `{"kind":"compositor-native","backend":"Hyprland"}`, or `Evdev` with device name/path. Reading the legacy `"Hyprland"` string normalizes it to the same in-memory native source as the v2 object; schema-v1 output writes the string again. For future native backends, schema-v1 output keeps the canonical compositor-native object until a backend-specific compatibility form is defined. Text output labels the event source explicitly. Normal text reports fit one screen: key, assessment, one result statement, capture source, matching layer, binding action, and concrete uncertainty. Raw events, probe bytes, encoding internals, full modifier state, XKB candidates, keyboard inventory, inactive submaps, and alternate keys stay behind `--verbose`; JSON always carries the full evidence. `whykey shell-init <bash|zsh|fish>` can install an optional wrapper that passes current shell editing snapshots through environment variables. Add `--json` to emit machine-readable reports; interactive diagnostics remain on stderr in JSON capture mode. `--ndjson` implies JSON and emits one compact schema-v2 `listen` record per stdout line.

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

`whykey inspect <combination-or-sequence>` accepts one or more combinations
separated by whitespace, for example `whykey inspect ctrl+x ctrl+s`. Each step
is analyzed independently with the default chain and is retained as its own
text or JSON report. `--pid <pid>` changes application-layer discovery to the
selected process and its ancestors; a target that disappears is reported as an
unavailable layer. `--focused` resolves a process through the active Hyprland,
Sway, or i3 tree API and uses that process for application and shell evidence;
other desktops report the missing focused-window API. The current
implementation does not infer temporal prefixes or mode changes between
steps; those remain explicit limits even though `whykey conflicts` is
available for enumerated bindings. Sequences are bounded to 64 steps so malformed or
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
source, key, and context all match. Different actions are reported as
possible conflicts because ordering and runtime precedence are not universally
observable; identical shortcuts from different applications are not combined.

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

Parsing is case-insensitive. The display order is `CTRL`, `ALT`, `SHIFT`, `SUPER`, `CAPS`, `MOD2`, `MOD3`, `MOD5`, then the key. Kitty's Hyper/Meta/Num modifier bits are represented by the existing `MOD3`/`MOD5`/`MOD2` masks when a distinct public name is unavailable.

## Hyprland inspection

1. Run `hyprctl binds -j` to read the effective binding set.
2. Run `hyprctl submap -j` to read the active submap.
3. Match the normalized key name and exact modifier mask.
4. Consider a binding active when it belongs to the active submap or has the universal-submap flag. Keep same-key bindings from inactive submaps in the evidence so mode-dependent shortcuts are visible without treating them as active.
5. Determine handling and propagation separately. Use Hyprland's `non_consuming`, `auto_consuming`, `release`, and `longPress` flags, the `pass` and `submap` dispatchers, and any dispatcher result visible through the runtime API. A `pass` binding is reported as redirected because its target window is not necessarily the next layer in the inspected chain.
6. Include active same-key bindings with different modifiers as possible matches. `hyprctl binds -j` does not expose whether those bindings have `ignore_mods` enabled. When a conventional declarative config is available, follow literal `source`/`include` files and simple variables to recover an explicit `bind[i]` hint. Scan balanced single- and multi-line literal `o.bind`/`hl.bind` calls in discovered Lua files for source/description evidence and a literal `ignore_mods` value, including the helper default of `false`, without executing Lua. Unresolved variables, globs, and dynamic key or options expressions remain possible matches. Surface `locked`, `dont_inhibit`, and `allow_input_capture` when present because they qualify inhibitor/capture behavior.
7. Report every active exact match, including its dispatcher, argument, and description when present. Mark `__lua` and plugin-style dispatchers as runtime-opaque and never execute them; their handling/propagation remains conditional.
8. Query `hyprctl devices -j` when available and include active keyboard names, main-device status, and keymap in the evidence. Accept both legacy string and structured per-device fields in `hyprctl binds -j`. Kernel keycodes enter as evdev and convert once to the XKB namespace (`+8`); Hyprland socket events and numeric binding fields are already XKB. Missing RMLVO or layout data yields explicit uncertainty, never a guessed US layout.

## Sway and i3 inspection

Sway uses `swaymsg -t get_bindings -r`; i3 uses `i3-msg -t get_bindings`. Both adapters match symbolic names and explicit `code:<n>` queries, apply the compositor's modifier mask and `exact` flag, skip release-only bindings for a press query, and classify `nop` as forwarding and `pass` as redirected. KDE Plasma reads the INI-style `kglobalshortcutsrc` file (or `KGLOBALSHORTCUTS_CONFIG`), reports matching actions as configured handlers, and leaves propagation indeterminate because static configuration does not prove the runtime registration. Xfce reads `xfconf-query -c xfce4-keyboard-shortcuts -l -v`, covering literal custom/default command properties and their current channel values. Cinnamon reads `gsettings list-recursively org.cinnamon.desktop.keybindings` and reports the schemas and accelerator values returned by that live query. MATE reads its Marco and SettingsDaemon global-keybinding schemas through GSettings. Niri reads literal `binds` blocks from `NIRI_CONFIG` or `~/.config/niri/config.kdl`; runtime reload, active mode, and inhibitor state remain conditional. River reads literal `riverctl map` commands from `RIVER_INIT` or `~/.config/river/init`, preserving mode and reload uncertainty. Wayfire reads literal `binding_*` accelerator values from `WAYFIRE_CONFIG` or `~/.config/wayfire.ini`, preserving plugin activation and reload uncertainty. labwc reuses the Openbox-compatible `<keybind>` format from `LABWC_CONFIG` or `~/.config/labwc/rc.xml`; runtime reload remains conditional. bspwm/sxhkd follows literal key/command pairs in `SXHKD_CONFIG` or `~/.config/sxhkd/sxhkdrc`, preserving conditional runtime activation and reload state. Openbox reads `<keybind>` XML entries from `OPENBOX_CONFIG` or `~/.config/openbox/rc.xml`, preserving conditional runtime reload state. On X11, `xbindkeys` reads literal command/key pairs from `XBINDKEYSRC` or `~/.xbindkeysrc`; daemon activation and precedence against the window manager or other clients remain conditional. AwesomeWM parses literal `awful.key` declarations from `AWESOME_CONFIG` or `~/.config/awesome/rc.lua`, Qtile parses literal `Key([...], key, ...)` entries from `QTILE_CONFIG` or `~/.config/qtile/config.py`, and XMonad parses literal `xK_*` entries from `XMONAD_CONFIG` or `~/.xmonad/xmonad.hs`. These executable configurations are never evaluated; helpers, dynamic variables, modes, and reload state remain conditional. The adapters remain read-only and preserve conditional status when the compositor does not expose enough runtime state. A desktop without one of these adapters is represented by a generic conditional compositor layer rather than a false Hyprland error.

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
2. Inspect the bytes produced by the effective Ghostty configuration, not the Kitty probe bytes used by `listen`.
3. For `ctrl+z`, report whether `ISIG` is enabled and whether `VSUSP` is `^Z`; when both are true, explain the resulting `SIGTSTP`.
4. When the current shell is Bash, inspect the live `bind -P`, `bind -S`, `bind -X`, and `bind -V` snapshots when available. Parse function, macro, and shell-command formats, then check the selected `INPUTRC`, `~/.inputrc`, or `/etc/inputrc`, follow literal `$include` directives, and apply later matching definitions as overrides. Otherwise use Readline's common Emacs defaults only when the active mode is Emacs. Static and captured paths must not claim an Emacs default while the active keymap is vi. For Zsh and Fish, inspect live snapshots supplied by `shell-init` when available and compare arbitrary captured sequences against bindings declared in `.zshrc`/`.zshenv` or `config.fish`. Fish also receives `fish_bind_mode` from the wrapper and only selects mode-specific bindings when that mode is known; otherwise those bindings remain conditional. Common control-key defaults are reported as conditional when runtime state cannot establish the active mode.

## Exit status

- `0`: inspection completed, whether or not a binding matched.
- `1`: a required layer or prerequisite is unavailable. A detected Hyprland IPC failure is represented as an indeterminate compositor result so downstream layers can still be inspected; the report remains conditional rather than claiming that the shortcut is unbound.
- `2`: invalid CLI input.

`whykey listen` uses the same statuses. It restores the terminal settings before returning, including after normal errors and when the user presses `Esc` twice. The Hyprland native backend restores the recorded submap before reporting and retries a failed restoration twice; a restoration that still fails stays pending with an explicit error, and a final best-effort attempt on exit warns instead of failing silently. A process killed with `SIGKILL` cannot run restoration code, in which case the compositor keeps the private submap until the capture lease expires or the user resets it manually.

## Interactive capture

0. Prefer native compositor capture when its IPC is reachable. The Hyprland backend records the active submap, installs the private `__whykey_capture` submap, and considers the hook armed only after dispatcher success, the observed submap postcondition, and the token-specific armed event. Restore the recorded submap before reporting each event and re-arm only for the next one. A stale token never restores another session.
1. Open `/dev/tty`, save its `termios`, and temporarily disable canonical input, echo, signal processing, and software flow control.
2. Push all currently specified Kitty capture enhancements (`CSI > 31 u`: disambiguation, event types, alternate keys, all-keys reporting, and associated text) when the terminal supports them, and pop them with `CSI < u` on exit. Legacy terminals simply ignore these private control sequences and continue with their normal encoding. The original flags are retained separately for downstream byte prediction.
3. Read one event. A lone `Esc` is an Escape key; `Esc Esc` exits within a 300 ms cancellation window. CSI and Kitty protocol sequences are buffered for a short bounded interval.
4. Decode control bytes, UTF-8 text, Alt-prefixed bytes, common cursor/navigation CSI sequences, and Kitty `u` sequences (including Unicode/function/keypad/media keys, alternate key fields, associated text, and press/repeat/release events) into `KeyCombo`. Preserve unrecognized input as a raw observed key with an explicit unknown-origin warning rather than aborting capture; mark UTF-8 text as potentially produced by Compose/dead-key/layout processing.
5. Restore the Hyprland hook before rendering each report and re-arm it only for the next captured event, but keep terminal capture mode active while rendering repeated reports, inspecting the TTY layer against the saved `termios` snapshot. This prevents capture mode from changing the explanation of `Ctrl+Z` and avoids a restore/reopen gap between reports. Restore the terminal and keyboard protocol on normal exit, cancellation, handled signals, and errors. A process killed with `SIGKILL` cannot run restoration code.
6. Run the Hyprland → Ghostty → TTY → session/multiplexer → interactive application → selected shell adapter chain. A consuming layer stops downstream analysis. When a layer is uncertain, later results are retained but marked conditional by the report. The observed event proves that it reached the terminal, so Hyprland/Ghostty forwarding is reported as observed; capture mode does not prove forwarding through the TTY, multiplexer, application, or shell. Ghostty resolves a separate normal terminal input for the downstream layers. If that input cannot be inferred, the report labels the downstream result as based on captured bytes or unknown.
7. When an ancestor is Neovim or Vim, query the current `mode()` and live `maparg()` state through the editor's remote-expression interface when a server address is available; otherwise inspect common mappings in the relevant init/vimrc files. For Emacs, query `key-binding` through `emacsclient` when a server is available, then inspect common `global-set-key` and `define-key` declarations in standard init files. For Helix, Micro, and Kakoune, inspect their conventional static keymap files. Detect common full-screen TUIs such as fzf, less, lazygit, btop, ranger, and yazi as intermediate applications even when their mappings are not parsed. For VS Code/VSCodium, parse literal single-key entries from user `keybindings.json`; for recognized JetBrains IDEs, parse literal `keyboard-shortcut` entries from the selected XML keymap. Chords, `when` clauses, plugins, modes, and runtime overrides remain indeterminate. Plugin precedence and mappings that are not visible through the available RPC endpoint or static parser remain indeterminate.

The listener queries the terminal's existing Kitty keyboard flags before enabling its capture flags. A known previous state controls the normal encoding used by Ghostty. A terminal that does not answer the query uses a legacy fallback with inferred confidence. The evdev backend keeps pressed modifier keys per device, reads the initial pressed-key bitmap and lock LEDs when the kernel permits it, resynchronizes after `SYN_DROPPED`, and removes descriptors reported as disconnected. Evdev observations expose pressed left/right modifier names and locked LED names separately from the compatibility bitmask; XKB-style latched state remains explicitly unavailable. The chain also reports detected tmux, GNU Screen, Zellij, and SSH context; tmux root bindings, the active client key table when available, both configured prefixes, and the `prefix`/`prefix2` tables are inspected. A known non-root active table takes precedence, while a prefix-table match is marked as requiring the prefix only when the active table is unknown. Screen reads its system file (`SYSTEM_SCREENRC` or common system paths) followed by the user file (`SCREENRC`, or `~/.screenrc`), preserving the effective override order; it inspects the command prefix and literal `bind`/`bindkey` directives, compares direct sequences with common terminal encodings, and keeps prefix bindings conditional until the preceding prefix is known. Zellij merges dumped installed defaults with `ZELLIJ_CONFIG_FILE`/`config.kdl`, applying user overrides last; `keybinds clear-defaults=true` disables that merge. If defaults cannot be dumped or the KDL shape is unsupported, an unmatched key remains conditional. `WHYKEY_ZELLIJ_MODE` can provide a known runtime mode; without it, the active Zellij mode remains an explicit assumption. If multiple multiplexer markers are present, each adapter is inspected independently and the nesting order is marked conditional. If capture temporarily bypasses a normal TTY consumer, the report stops at that TTY result instead of claiming that the shell received the key.

## Architecture

`KeyCombo` and `KeySequence` own parsing and normalization. `TerminalInput` carries the bytes sent to the child PTY, along with their source and confidence. Each concrete layer reports one typed `LayerResult` whose internal state is a `LayerId` identity plus a single `Outcome` control value; the legacy `status`/`propagation` JSON pair is derived during serialization so schema v1 and v2 keep their exact meaning. One static route order (`ROUTE_ORDER`) sequences the remapper, compositor, terminal, IME (before the TTY stage), TTY, multiplexer/session, application, and shell stages. The selected terminal adapter (Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, or generic fallback) produces `TerminalInput` before TTY, session, application, and shell layers inspect it. JSON reports retain the numeric modifier mask, expose a human-readable `key_display`, and summarize overall confidence as `configured`, `confirmed`, or `conditional`. Captured observations keep associated text separate from the logical key and preserve alternate-key metadata. If propagation is uncertain, later layers are still inspected and the report preserves that uncertainty as a conditional result.

Desktop adapters are registered once in `src/registry.rs`. Each entry supplies its detection and IPC probes plus the operations that vary by desktop: key inspection, binding enumeration when supported, focused-window PID lookup when supported, and capability/doctor metadata. Route dispatch, `bindings`, focused-window lookup, `doctor`, `capabilities`, and the support-matrix drift test all read this registry; each command takes one `Environment` snapshot per run, so desktop detection runs once and is shared instead of being repeated per adapter.

Future versions can add more terminal protocols and shell layers. A layer may stop the explanation when it captures a key or pass the key to the next layer.

External integration commands use a bounded executor with a two-second per-query timeout and a one-megabyte stdout/stderr limit. A timeout, read failure, or oversized response remains an adapter error and does not block the rest of the report. Static inspections share a ten-second wall-clock budget across all layers and sequence steps; when it expires, the report retains completed layers and adds an unavailable `Diagnostic budget` result. `WHYKEY_COMMAND_TIMEOUT_MS`, `WHYKEY_COMMAND_MAX_OUTPUT`, and `WHYKEY_DIAGNOSTIC_TIMEOUT_MS` override these defaults for a session.
