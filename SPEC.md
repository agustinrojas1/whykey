# whykey specification

## Platform scope

Whykey is a Linux-only program. The supported binary depends on Linux evdev,
PTY/termios, XDG session paths, and Linux compositor/session IPC. Windows and
macOS are outside this specification; non-Linux targets fail at compile time
with an explicit message. A future portable `whykey-core` would be a separate
workspace decision and is not implied by this binary.

## Goal

Given one key combination, report how the running Linux compositor, terminal and downstream session handle it. Hyprland, Sway, i3, herbstluftwm, GNOME, KDE Plasma, Xfce, Cinnamon, MATE, Niri, dwl, River, Wayfire, labwc, bspwm/sxhkd, Openbox, X11 `xbindkeys`, AwesomeWM, Qtile, and XMonad have read-only desktop adapters; unsupported desktops are reported as an explicit conditional context.

Answers come from the strongest source available, in this order: observing the key (`whykey listen`), asking the running program (compositor IPC, the X server's grab state, GSettings, kglobalaccel over D-Bus, tmux, a shell's live bindings), and reading configuration files or built-in defaults. Each layer result records which of these produced it (see Evidence tiers).

```text
whykey ctrl+left
whykey inspect ctrl+x ctrl+s
whykey extension ./my-editor-whykey ctrl+x
```

`whykey` inspects state and prints an explanation. It never edits configuration files and never executes the inspected shortcut. Static inspection never changes compositor state or reloads Hyprland. On a native X11 session it takes passive key grabs on the root window for a moment and releases them at once, to learn whether another client holds the key (see X11 grab probe). `listen` with native Hyprland capture installs a temporary Lua hook. The default pass-through hook only observes; `--suppress` also activates a private submap and restores the recorded submap before reporting. Each hook carries a 5-second lease that `listen` renews every 2 seconds. If renewals stop, the hook removes itself, and a suppressing hook also restores the submap. `listen` with the probe window maps one small window and asks for focus. Compositor extension manifests and `whykey extension` run the programs the user configured or named.

Dependency versions are pinned by `Cargo.lock`; registry checksums and the
toolchains/host covered by repository verification are recorded in
`DEPENDENCY_PROVENANCE.md`. This provenance does not claim that every native
distribution package or release target has been built.

Capture mode is available with `whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--suppress|--no-suppress|--pass-through] [--dry-run] [--explain-capture] [--window|--terminal] [--ndjson] [--output PATH]` and an optional Linux-only `whykey listen --evdev [--device /dev/input/eventN]`. Without a forcing option, `listen` uses the first backend that works, in this order: (1) native compositor capture (Hyprland with a Lua configuration, or Sway IPC); (2) a probe window, when `WAYLAND_DISPLAY` or `DISPLAY` is usable and the session is not remote (SSH), because a remote display is not the user's desktop; (3) terminal capture. When a stage fails to open, the reason goes to stderr and the next stage is tried. `--window` (`-w`), `--terminal`, `--evdev`, and `--device` force one backend; `--window` cannot be combined with the other three. Native capture observes pass-through by default. `--no-suppress` states that default explicitly; `--pass-through` is its compatibility alias. `--suppress` opts into temporary suppression through a private Hyprland submap. It is rejected with exit status 2 when combined with `--terminal`, `--window`, `--evdev`, `--device`, or `--no-suppress`. When native suppression is unavailable, including on Sway (whose IPC has no suppression hook), it fails with exit status 1 and never falls back. `listen --evdev` observes read-only `EV_KEY` events before compositor processing and never grabs the device. `--timeout` is a wall-clock deadline for the capture and `--count` stops after the requested number of reports. `--events all` includes modifier-only and release events in native compositor, probe window, or evdev mode; in the default chain it is an error only when neither native capture nor the probe window can be used. Terminal capture cannot observe a key consumed before it reaches the terminal; the probe window and native capture can. Capture observations and normal downstream bytes are separate values. Captured JSON uses `Terminal`, the v1-compatible `"Hyprland"` string, canonical v2 `{"kind":"compositor-native","backend":"Hyprland"}`, `{"kind":"probe-window"}`, or `Evdev` with device name/path. Reading the legacy `"Hyprland"` string normalizes it to the same in-memory native source as the v2 object; schema-v1 output writes the string again. Other native backends, such as Sway, keep the canonical compositor-native object in schema-v1 output until a backend-specific compatibility form is defined. Text output leads with a shortcut/operation header, diagnosis, practical effect, relevant layers with their evidence tier, uncertainty where needed, and a Next section that names the step that would strengthen the evidence (observing with `listen`, or loading `shell-init`). The diagnosis says a layer has a matching key only when that layer reported binding evidence. When no inspected layer reports a binding, the diagnosis says so and names the layers whose handling is unknown. Binds known only by their modifiers (unresolved keys) are reported as possible matches, and shell answers built from defaults are labelled unverified. Rows for layers that pass the key without a binding appear only with `--verbose`. Raw events, probe bytes, encoding internals, full modifier state, XKB candidates, keyboard inventory, inactive submaps, and alternate keys also stay behind `--verbose`; JSON always carries the full evidence. `whykey shell-init <bash|zsh|fish>` can install an optional wrapper that passes current shell editing snapshots through environment variables. Add `--json` to emit machine-readable reports; interactive diagnostics remain on stderr in JSON capture mode. `--ndjson` implies JSON and emits one compact schema-v2 `listen` record per stdout line. Human output accepts `--color=auto|always|never`; auto respects `NO_COLOR`, non-TTY output, and `TERM=dumb`, while explicit `always` and `never` take precedence. JSON, NDJSON, shell-init, and completion output never contain styling.

When more than one compositor adapter is applicable, candidates are scored from the one environment snapshot: reachable read-only IPC contributes 3 points, an explicit compositor environment signature contributes 2, and a matching desktop/session hint contributes 1. Configuration-only evidence contributes 0. Candidates are sorted by descending score with registry order breaking ties; the selected compositor remains the first candidate for compatibility.

Capture is observe-only by default. `--suppress` opts into temporary native
compositor suppression on Hyprland. `--dry-run` prints the numbered chain with
the reason each stage was selected, skipped, or not needed, then the selected
backend, without arming it; it reports `capture backend: none` when no stage
is usable or `--suppress` cannot be honored. `--explain-capture` prints the
detection evidence and the same chain. Sway IPC is pass-through-only and
reports only keys that ran a Sway binding.

### Probe window

The probe window is a whykey-owned client: a Wayland `xdg_toplevel` (app_id
`whykey-probe`) when `WAYLAND_DISPLAY` is usable, otherwise an X11 window
(`WM_CLASS` `whykey-probe`/`whykey`, `_NET_WM_PID` set), titled
"whykey: press a shortcut here (Esc exits)". It needs no permission beyond a
display connection. It reads the keymap the compositor or X server gives it
and reports keycodes as XKB keycodes with X11-style modifier bits. Escape or
Ctrl+C in the window ends capture.

- A non-modifier key press that arrives is *delivered*: the compositor or
  window manager, any hotkey daemon, and any input method passed it to a
  focused client. A Remapper or Compositor layer that reported Consumed or
  Redirected is upgraded to HandledAndPassed with the note "observed key
  reached the probe window". Downstream Terminal, TTY, and shell rows still
  describe the user's usual focused application, not the probe.
- When modifiers become held and then return to none with no non-modifier
  key in between, the observation is *not delivered* (`not_delivered`).
  Either something above the window consumed the key, or the modifier was
  pressed alone (or with the mouse); the window cannot tell these apart. The
  observation's confidence is therefore `conditional`, never `confirmed`.
  The human report says, for example, "Super was pressed and released and no
  other key reached this window: either the key you pressed with Super was
  consumed by the compositor/desktop or an input method, or Super was
  pressed alone (or with the mouse)." Caps Lock and Num Lock bits are
  ignored for this rule.
- The window cannot name the program that took the key; the inspection that
  follows names the likely owner. With a virtual keyboard, the recorded
  keycode is that device's own keycode.
- In an SSH session (`SSH_CONNECTION` or `SSH_TTY`) the default chain skips
  the window ("remote session; terminal capture used"); `--window` still
  forces it. When both a Wayland socket and `DISPLAY` exist, `--dry-run`
  names "Wayland (falls back to X11 if the connection fails)", and the live
  message "Opened a probe window (Wayland)" or "(X11)" names the protocol
  actually used.

The window stays open across repeated captures. In schema v1 the
observation carries `disposition` only when it is `not_delivered`; schema v2
always carries it.

### X11 grab probe

When `DISPLAY` is set and the session is not remote (SSH), inspection asks
the X server whether another client holds a passive grab on the queried key.
It resolves the key to a keycode through `GetKeyboardMapping` (a match in
the shifted column adds Shift, except a keypad keysym there, which is the
NumLock level and needs no Shift; `code:N` is keycode N) and attempts a core
`GrabKey` and an XInput2 `XIPassiveGrabDevice` on the root window. Each grab
is tried with the requested mask alone and with Lock, NumLock, and both,
with the NumLock bit read from the modifier map. A `BadAccess` (or a failed
XI2 status) means another client holds the grab. Grabs that succeed are
released immediately.

- Native X11 session: `XDG_SESSION_TYPE` decides when it is `x11` or
  `wayland`; otherwise the session counts as XWayland only if the
  `WAYLAND_DISPLAY` socket accepts a connection, so a stale or inherited
  `WAYLAND_DISPLAY` does not hide a native X11 session. On native X11: when the key is grabbed and the selected desktop adapter
  claims it, that adapter's result (layer, binding, mode) is kept and
  forced to Consumed, with the observed `BadAccess` evidence first. A static
  conditional claim gets the summary "an X11 client holds a grab on this
  shortcut on the root window (WM or hotkey daemon)". A grab that no adapter claims becomes a
  neutral "X11 key grab" compositor layer that consumes the key. A free key
  ("no grab on the root window (WM and hotkey daemons grab there)") means
  only that no client holds a passive grab for it on the root window; the
  evidence notes that a grab an application holds on its own window is not
  visible, nor is a client that reads keys some other way. The key passes, and a
  configured binding is reported as not active.
- Exception: when the claiming adapter knows that the grabbing program
  replays the key to the focused client (an sxhkd `~` binding gives
  handled-and-passed), the adapter's outcome and summary are kept. The grab
  evidence is added with the detail "the grab delivers the key to <layer>
  first, which passes it on to the focused client", and the tier becomes
  live query. Only a consuming or undecided claim is forced to Consumed.
- XWayland: a grab only adds a conditional detail to the Wayland
  compositor's result, because the compositor sees the key first.
- The probe runs on a helper thread under the diagnostic budget. An X
  server that does not answer in time gives "X11 grab probe unavailable:
  the X server did not answer within N ms".
- A probe failure, or a key with no keycode in the current mapping, leaves
  the adapter's result unchanged with a note. The X server does not reveal
  which client holds a grab, so the owner's name comes only from the
  selected adapter.

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
the same diagnostic conclusions. Version-2 and NDJSON path entries carry an
optional `dead_key` keysym when a terminal layer consumes the key to start
dead-key composition; version 1 is unchanged. Selecting two different versions
(`--json-v2` with `--schema-version 1`, or two different `--schema-version`
values) is a usage error; repeating the same selection is accepted. Extension
stdin/stdout remains protocol v1 until a separately negotiated extension
protocol is introduced.

`whykey completions bash|zsh|fish` prints a completion script generated from
the same command and option table as `--help` and the usage errors, so each
command completes only the options its parser accepts. `--pass-through` is
accepted but not completed, and `replay` and `diff` complete file names.

`whykey doctor` runs read-only prerequisite checks for the controlling TTY, compositor context, terminal adapter, XKB compiler, shell, SSH, multiplexer context, common remappers, and IBus/Fcitx5 session context. When the Hyprland signature is absent, it discovers an instance through `hyprctl instances -j`, preferring the instance whose Wayland socket matches `WAYLAND_DISPLAY` and accepting a sole running instance. It reports the selected dedicated terminal adapter when one is detected. Before querying Fcitx5, the adapter verifies the session D-Bus and the `org.fcitx.Fcitx5` well-known name; it then uses the read-only `org.fcitx.Fcitx.Controller1` methods directly rather than launching `fcitx5-remote`, whose client may abort when the bus is unavailable. A failed preflight is reported as unavailable runtime evidence. The runtime readers are `kglobalaccel-runtime` (the `allComponents` D-Bus call; the daemon is never auto-started), `gnome-gsettings` (the GSettings read used for inspection), and the xdg-desktop-portal GlobalShortcuts probe, which asks `NameHasOwner` first so that introspection never activates the portal. Hyprland, Sway, i3, GNOME, KDE, Xfce, Cinnamon, MATE, Niri, River, Wayfire, labwc, bspwm/sxhkd, Openbox, X11 xbindkeys, and programmable X11 WMs expose separate `applicable` and `ipc` fields; other detected desktops are described through a generic compositor context with their desktop/session/display-server identity. The JSON form includes `schema_version: 1`, matching key reports. It exits `0` when TTY and an applicable compositor context or SSH session are available and either Ghostty is inspectable or a different terminal is detected for the generic terminal fallback; XKB is reported as an optional precision feature.

On Hyprland, `doctor` and `capabilities` check native capture the way `listen` does: after socket2 answers, they send the same no-op `hyprctl eval "return nil"`, which changes no compositor state. If Hyprland rejects it (a hyprlang `.conf` configuration), native capture is reported unavailable with the cause "the Hyprland key hook needs a Lua config (hyprland.lua)"; JSON reports `api-does-not-expose` with the reason in `detail` and in `native_attempts[].reason`.

Human doctor output adds two sections. "Observation taps (what whykey can watch now)" lists, in `listen` chain order, native compositor capture, the probe window (Wayland and/or X11), the X11 grab probe (native X11; "X11 clients only" under XWayland; unavailable when remote or without `DISPLAY`), terminal capture, and evdev. Each tap has its availability, a detail, and, when one exists, the exact step that enables it. "Layer sources (live query, config file, or none)" says, for the remapper, compositor/desktop, X11 key grabs, input method, terminal, multiplexer, shell, and editor layers, whether this session's answer will come from a live query, a configuration file, or nothing.

The doctor JSON defaults to schema v1 for compatibility, and v1 is unchanged.
`--schema-version 2` wraps the same checks in the v2 operation/context
envelope and adds `observation_taps` (`id` = `native-compositor`,
`probe-window`, `x11-grab`, `terminal`, or `evdev`, plus `name`, `available`,
`detail`, and an optional `enable`) and `layer_sources` (`layer`, `source` =
`live_query`, `config_file`, or `none`, and `detail`).

## Evidence tiers

Each layer result carries the tier of evidence it rests on, derived from the
layer data, strongest first:

| Tier | Human label | JSON `tier` | Assigned when |
| --- | --- | --- | --- |
| Observed | `observed` | `observed` | The capture covers the layer. The probe window covers the remapper and compositor. Terminal capture covers the remapper, compositor, and terminal. Native capture covers the compositor. |
| Live query | `live query` | `live_query` | The running program answered: the X11 grab probe, Hyprland, Sway, or i3 IPC, GSettings, kglobalaccel, xfconf, herbstclient, `wezterm show-keys`, `ghostty +list-keybinds`, `tmux list-keys`, termios, an IME daemon's runtime state, or a bash, zsh, or fish `shell-init` dump. |
| Config file | `config file (static)` | `config_file` | Configuration files were read: user startup files that change shell bindings, INPUTRC or bashrc, terminal, editor, and remapper configs, Konsole keytabs, `config.kdl`, compositor extension manifests, and static desktop configs. |
| Default | `default (unverified)` | `default` | Only built-in defaults answered: Readline's table, `zsh -f` or `fish --no-config` defaults with no user file changing them, or `zellij setup --dump-config` alone. |

Unavailable layers, and layers with no recorded source, have no tier. Human
rows end with the tier in brackets (`[live query]`). Schema-v2 and NDJSON
path entries carry `tier`; schema v1 is unchanged. Adapters record the
source of their answer as a typed field on the layer result, and the tier is
derived from it together with the observation; it is not inferred from the
wording of detail lines.

## Input

The legacy top-level form accepts exactly one combination. Modifiers are
separated with `+`; the final key may itself be `+` or contain `=` (for
example `ctrl++` and `ctrl+=`). Hyprland numeric keycodes use `code:<n>`, for
example `ctrl+code:30`.

Modifier names are `ctrl`/`control`/`ctl`, `alt`/`mod1`, `shift`,
`super`/`meta`/`win`/`mod4`, `caps`/`capslock`/`lock`, `num`/`numlock`/`mod2`,
`hyper`/`mod3`, and `altgr`/`iso_level3_shift`/`mod5`. `meta-key` is not a
modifier name; a combination containing it is a usage error. The key must be one of
the following: a single Unicode character; a named key (arrows,
`return`/`enter`, `escape`, `space`, `tab`, `backspace`, `delete`, `page_up`,
keypad and lock keys, or keysym names such as `comma`, `slash`, and `minus`);
`f1`–`f35`; an `XF86*` media key; `code:<n>`; or a Hyprland `mouse:<n>`.
Unknown names are rejected with exit status 2. When an edit-distance match is
close, the error suggests it (`ctrl+retrun` → `return`). A combination
contains exactly one non-modifier key. Configuration adapters parse keys
leniently, so unusual names in user configuration are still compared.

Keypad keys are distinct from the main keys at every layer: `kp_enter`
(alias `kp_return`) is not `return`, and `kp_0`–`kp_9`, `kp_home`, `kp_end`,
`kp_page_up` (alias `kp_prior`), `kp_page_down` (alias `kp_next`), `kp_add`,
`kp_subtract`, `kp_multiply`, `kp_divide`, `kp_equal`, and `kp_decimal` are
not the corresponding main keys. Hyprland keycode binds resolve them to the
keypad keycodes (KP_Enter is XKB 104, Return is 36), and terminal prediction
encodes them as keypad keys (Kitty codes 57399–57426; in legacy mode the
main key's bytes, with the application-keypad `ESC O x` alternative noted).
Remapper rules on keypad keys (keyd `kpenter`, `kp1`) use the keypad's own
physical codes, and a main key never matches a keypad code. i3 and Sway
treat NumLock oppositely for keypad digits (`kp_0`–`kp_9`, `kp_decimal`): i3
resolves the keysym under the binding's own modifiers, so `bindsym
$mod+Mod2+KP_1` fires with NumLock on and `$mod+KP_1` never fires; Sway
consumes NumLock, so `Mod4+KP_1` fires and `Mod4+Mod2+KP_1` does not.
whykey matches each compositor's rule; keypad navigation keysyms (`KP_End`)
need no `Mod2` on either.

Every argument except a file path must be valid UTF-8. File arguments
(`replay`, `diff`, `snapshot --output`, and the `extension` program) are
taken as raw paths; any other non-UTF-8 argument is a usage error
("argument X is not valid UTF-8", exit status 2). Error messages echo at
most 64 characters of an input token, and a combination with many `+`
segments is summarized after the first three.

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
looked up only in the active (or root) table. tmux keys come from
`list-keys`, where the key is the token after `-T TABLE`. They are matched
case-sensitively (`C` is Shift+c, `^B` is Ctrl+Shift+b, `BTab` is
Shift+Tab); escaped (`\%`, `\;`) and quoted (`"M-{"`) keys and the `-` key
are unescaped, and `Escape`, `BSpace`, `Tab`, `Enter`, `PgUp`, `PgDn`, and
the `KP` names map to whykey keys. A punctuation query drops Shift
(`shift+percent` is `%`); a Shift+digit query is not mapped to its US
symbol. The prefix comparison uses the same rules. GNU Screen resolves the key after
its command escape against `bind`, and a doubled escape is a literal. If an
earlier step's bytes are unknown, the shell step is reported as unknown.
`--pid <pid>` changes application-layer discovery to the
selected process and its ancestors; a target that disappears is reported as an
unavailable layer. `--focused` resolves a process through the active Hyprland,
Sway, or i3 tree API and uses that process for application and shell evidence;
other desktops report the missing focused-window API. A matching compositor
manifest can supply conditional focused-PID evidence when no compiled-in focus
API is available. When the focused process has no controlling terminal but
its children use exactly one pty (a terminal emulator), whykey targets that
pty's foreground process-group leader, so the application and shell layers
describe the program that reads the key; the source notes "foreground
process of pid N's terminal". When the emulator serves several ptys, the
target stays the emulator and the focused terminal is reported as unknown.
When the target is not on whykey's own terminal, stderr notes that the
terminal-emulator and TTY rows describe whykey's terminal. An empty
`TERM_PROGRAM` or `LC_TERMINAL` counts as unset. Sequences are bounded to 64 steps so malformed or
unbounded command-line input cannot grow the report without limit.

`--instance <id>` explicitly selects a Hyprland instance for the inspection
when automatic discovery is ambiguous. The selection is process-local and is
not persisted in the user's environment.

`doctor --json` includes `evdev.devices`, a read-only list of candidate keyboard event paths with their kernel-reported name, keyboard capability, readability, and an optional opening error. This makes `--device` selection and permission failures discoverable without touching device state. A device is a keyboard when its sysfs key bitmap, read most significant word first as the kernel prints it, reports keyboard keys. When evdev is unavailable, `evdev.cause`, `evdev.detail`, and `evdev.next_check` name one of four cases: `/dev/input` is missing (`not-installed`), `/dev/input` cannot be listed (`permission-missing`), no keyboard event devices exist (`api-does-not-expose`), or keyboard devices exist but none can be opened (`permission-missing`). In the last case the next check is a temporary, least-privilege grant for exactly the unreadable keyboard nodes, `sudo setfacl -m u:"$USER":r '<node>'…` with shell-quoted paths. The ACL disappears when the device is replugged or the system reboots (or sooner with `setfacl -x`). Joining the `input` group is named only as the permanent alternative that grants every input device. Doctor never changes permissions itself.

`whykey capabilities` exposes the support matrix as a versioned report. Each
entry has an identifier, area, `implemented` boolean, `availability` value
(`available`, `unavailable`, or `planned`), and evidence text. Implemented does
not imply that the integration is available in the current session; planned
entries are listed so automation can distinguish a known limitation from a
permission or environment failure.

The repository's human-readable [support matrix](SUPPORT_MATRIX.md) is
generated from `support-matrix.json`. The Rust tests compare that manifest with
the capability registry, so documentation and tests use the same contract.
Each entry also records its verification: the [lab](lab/README.md) scenarios
in which the real program agreed with whykey (`lab`), hand checks on the
maintainer's machine (`live`), or neither (unverified). Only scenarios that
pass without the honest-uncertainty tolerance count.
`tools/generate_support_matrix.py --lab-results FILE` merges a lab run into
the manifest.
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
`--device`, and `--submap` filter with AND logic; missing metadata never
matches an active filter. `--key` text that parses as a key query is compared
by normalized key, as inspect does: a bare key matches under any modifiers, a
combination needs exactly its modifiers, and `code:N` also matches a Hyprland
record listed as its layout symbol. `--key` text that is not a key query
(for example `shift`), and the other filters, match case-insensitive
substrings. `conflicts --key` uses the same matcher, and `conflicts` lists
actions in the order the source reports the binds. `complete` is intentionally false:
application/plugin bindings, input inhibitors, and firmware transformations
can remain outside the observable desktop APIs. Adapter errors are listed
under `unavailable`. A Hyprland bind written with `code:N` is listed as the
key it produces in the active XKB layout, and its certainty says
"code:N shown as its key in the active XKB layout".

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
and conventional configuration paths, and list literal keyd, kanata, kmonad,
input-remapper, and xremap mappings as evidence.

Inspection evaluates the queried key against keyd, kanata, kmonad,
input-remapper, and xremap configurations. kanata and kmonad share one
s-expression reader with a 64-level nesting cap. keyd reads `/etc/keyd/*.conf` (or the `WHYKEY_KEYD_CONFIG`
file or directory) with `include`, `[ids]`, `[aliases]`, and `[global]
default_layout`. Only `[main]`, the built-in layers for the held modifiers,
their composite layers, and the default layout apply, and the last binding
wins. kanata reads the `-c`/`--cfg` file of the running process (relative to
its `/proc/<pid>/cwd`), otherwise `KANATA_CONFIG`, then
`~/.config/kanata/kanata.kbd`. It evaluates `defsrc` with the first
`deflayer` or `deflayermap`, `_`, `XX`, aliases naming plain keys,
`C-`/`S-`/`A-`/`M-`/`RA-` prefixes, `multi`, and `defoverrides`, and respects
`process-unmapped-keys` and `linux-dev`. xremap reads the configuration paths
and `--device` of the running process, otherwise `XREMAP_CONFIG` or
`~/.config/xremap/*.yml`/`*.yaml`; `modmap` feeds `keymap`, which prefers an
exact modifier match and then one with extra modifiers unless `exact_match`
is set. A plain key or combination is a redirect. Layer switches, macros,
commands, and `noop` consume the key. Tap-hold, tap-dance, one-shot,
`overload`, `fork`/`switch`, application, window, device, or mode conditions,
and side-specific modifiers are conditional. A file without `defsrc`, or YAML
with anchors, aliases, tags, or block scalars, or a kanata or xremap file nested
more than 64 levels deep, is reported as not evaluated.

kmonad reads the configuration file from each running process's command line
(relative to its `/proc/<pid>/cwd`) and applies its `--fallthrough`,
`--input`, and `--implicit-around` options; `KMONAD_CONFIG` and
`~/.config/kmonad/*.kbd` are used only when no kmonad runs. It evaluates
`defcfg` (`device-file` input, `fallthrough`, `implicit-around`), named and
unnamed `defsrc`, the first `deflayer`, aliases, kmonad's key names and
shifted names, `C-`/`S-`/`A-`/`M-` and right-side prefixes, and `around`
forms. Layer, macro, command, pause, press-/release-only, `around-next`,
compose, and dead-key buttons consume the key, as do `XX` and, without
fallthrough, `_` or keys outside `defsrc`. Tap, multi-tap, sticky, stepped,
and timeout buttons are conditional. input-remapper reads
`INPUT_REMAPPER_CONFIG` (a directory or its `config.json`), otherwise
`$XDG_CONFIG_HOME/input-remapper-2` or `~/.config/input-remapper-2`, and treats
each `autoload` entry as the active preset for its device. Both preset
formats are read. Symbols resolve through `xmodmap.json`, then `KEY_*` names;
without `xmodmap.json` only layout-invariant keysyms resolve. A key or
combination output is a redirect; `disable` and macros consume, except
conditional macros (`if_tap`, `mod_tap`, …). Presets applied from the GUI
without autoload are unknown, so a running service without autoload stays
uncertain. For both, the result is conditional unless the captured device is
proven to be the configured one.

A key captured from a physical evdev device is exact. Otherwise keys whose
keysym does not depend on the layout (Escape, CapsLock, function keys,
arrows, modifiers, media keys) map to fixed codes, and printable keys map to
their US position, which is an assumption unless `WHYKEY_XKB_LAYOUT` or
`XKB_DEFAULT_LAYOUT` is `us` with no variant; under that assumption a match is
uncertain. Input captured by the compositor or from a remapper's virtual
device is already remapped and stays uncertain. When several configurations
or keys apply and disagree, when a configuration is found with no running
process, or when several remappers act on the key, the result is uncertain. A
configuration that cannot be read is unavailable, never a pass.

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

- `extensions/whykey-nvim`: stdlib Python 3 script querying running Neovim via `--remote-expr`. Declares `query_mode` and `identify_action` capabilities. Connects via `NVIM_LISTEN_ADDRESS`, active `nvim --listen` / `--server` instances, or default `$XDG_RUNTIME_DIR/nvim.<pid>.0` sockets; when several servers are found and none is named in the environment, the answer is indeterminate because the queried server may not be the focused one. Returns `status: "unavailable"` when no running Neovim server is found. Converts `KeyCombo` to Vim notation (`<C-x>`, `<C-Left>`, `<D-x>` for Super, `<lt>`, `<Bar>`). One `luaeval` query reads the mode from `nvim_get_mode` and the mapping from `maparg`. A Lua-callback mapping, a mapping (after following `<Plug>` chains up to 5 levels), or a key that only starts longer mappings (reported with the `timeoutlen` wait) is `handled`/`stops`. An unmapped `<C-i>`, `<C-m>`, `<C-[>`, or `<C-j>` falls back to the `<Tab>`, `<CR>`, `<Esc>`, or `<NL>` mapping, as Neovim does. No mapping is `indeterminate`, because built-in commands and `vim.on_key` handlers are not inspected; in terminal mode it is `indeterminate`/`continues`. Prompt modes, timeouts, AltGr, and keycode queries are `indeterminate`.
- `extensions/whykey-vscode`: stdlib Python 3 script querying user `keybindings.json` across Code, Code - Insiders, Code - OSS, VSCodium, and their Flatpak paths, searching `$XDG_CONFIG_HOME` before `~/.config`. Declares `identify_action` capability. Follows VS Code's resolver: user entries come after the defaults, and the last matching entry without a `when` clause decides. A `when` clause is named but not evaluated, so its binding is `handled`/`indeterminate`. `"command": "-id"` removes only default bindings. A chord prefix is `handled`/`stops`, an empty command is `not_handled`/`continues`, and `^command` is `handled`/`continues`. Keys follow VS Code's parser (`+`/`-` separators, `cmd`/`win`/`meta` as Super, `[KeyA]` scan codes, multi-chord keys). A key missing from the user file is `indeterminate`, because VS Code's default keybindings are not bundled; a User directory without `keybindings.json` counts as no user bindings. Profile keybindings or several VS Code builds make the answer `indeterminate`, as do keycode and AltGr queries.
- `extensions/whykey-emacs`: stdlib Python 3 script querying running Emacs via `emacsclient --eval`. Declares `query_mode` and `identify_action` capabilities. Connects via `EMACS_SERVER_FILE`, `EMACS_SOCKET_NAME`, or default daemon socket. Returns `status: "unavailable"` when emacsclient is missing or no daemon is reachable, with remediation to run `emacs --daemon` or `(server-start)`. Runs `key-binding` at point in the selected window's buffer, so text-property and overlay maps count, and reports the winning map from `current-active-maps` (`global`, `major`, `minor (<mode>)`, or an overriding, emulation, or text-property map). A prefix key is `handled`/`stops` because Emacs waits for the next key. In graphical frames `<return>`/`<tab>`/`<escape>`/`<backspace>` are looked up before `RET`/`TAB`/`ESC`/`DEL`, and an unbound shifted key falls back to its unshifted form. No binding is `indeterminate`, because `key-translation-map`, `input-decode-map`, and pre-command hooks are not inspected. A timeout or evaluation error is `indeterminate`, not "no daemon".

Parsing is case-insensitive. JSON keeps the numeric modifier mask. Text output orders modifiers `Ctrl`, `Alt`, `Shift`, `Super`, `Caps`, `Mod2`, `Mod3`, `AltGr` (Mod5), then the key. Kitty's Hyper and Num modifier bits are represented by the existing `MOD3`/`MOD2` masks. Mod5 means AltGr: terminal prediction never encodes it as a Kitty modifier and instead predicts the text the layout's AltGr level produces (see Ghostty inspection).

## Hyprland inspection

1. Run `hyprctl binds -j` to read the effective binding set. When an entry has key `""`, keycode 0, and is not a catch-all, also run the text `hyprctl binds` listing.
2. Run `hyprctl submap -j` to read the active submap.
3. Match the normalized key name and exact modifier mask. Keysym names in bindings (`comma`, `slash`, `minus`, `Prior`) are normalized the same way as the query. Hyprland reports some runtime-built Lua binds with key `""` and keycode 0. Such a bind first takes its key from the text listing, joined on dispatcher, argument, modmask, submap, and description. The listing's `key:` string must have its modifiers (Hyprland's `modFromSv` names) first and equal to the modmask, followed by exactly one key segment; text keys that belong to binds the JSON already resolves are removed first. A multi-key bind (`SUPER + code:10 + code:11`) or an ambiguous join stays unresolved. An unresolved bind then takes its key from literal Lua hints with the same modmask and description, but only when all of those hints agree (for example `SUPER + code:20`). Otherwise the key is unresolved. An unresolved bind in the active submap with the query's modifiers is reported as a possible match (`UnresolvedBindingKey`), never as an exact one.
4. Consider a binding active when it belongs to the active submap or has the universal-submap flag. Keep same-key bindings from inactive submaps in the evidence so mode-dependent shortcuts are visible without treating them as active.
5. Determine handling and propagation separately. Use Hyprland's `non_consuming`, `auto_consuming`, `release`, and `longPress` flags, the `pass` and `submap` dispatchers, and any dispatcher result visible through the runtime API. A `pass` binding is reported as redirected because its target window is not necessarily the next layer in the inspected chain. `__lua` and plugin dispatchers follow the same rule as every other dispatcher. Hyprland consumes the key unless the bind is `non_consuming`; an `auto_consuming` bind stays uncertain. A Lua callback can forward the key only by returning `pass_event`, which whykey cannot observe, so the evidence says so.
6. Include active same-key bindings with different modifiers as possible matches unless their `ignore_mods` value is known. Read `ignore_mods` from the runtime JSON field when present. Otherwise, use a Lua hint and then a declarative `.conf` hint for the exact modifier mask and key. A hint for another binding on the same key never applies, and a binding with no hint stays a possible match. For declarative config, follow literal `source`/`include` files and variables. `source` paths expand `*` and `?` per path component like glob(3): a leading dot matches only explicitly, and matches are read in byte-sorted order. Bracket classes `[abc]`, `[a-z]`, `[!x]`, and `[^x]` are matched too (a `]` right after `[` or the negation is literal, an unterminated `[` is literal); POSIX named classes and backslash escapes are not supported, and ranges compare code points. Configuration, `source`, and Lua files are read only when they are regular files of at most 1 MiB. The longest variable name wins; unknown names stay literal, and the bind is skipped. Read bind flags from the keyword suffix (`bindi`, `bindln`; letters `lremnatisodpcgukx`), skip multi-key `s` binds, and apply `unbind = MODS, KEY` in order. In discovered Lua files, scan balanced single- and multi-line literal `o.bind`/`hl.bind` calls in file order, without executing Lua. They supply source and description evidence and a literal `ignore_mods` value, including the helper default of `false`. `hl.unbind("COMBO")` removes earlier hints for that combo, a later bind adds it back, `hl.unbind("all")` clears every hint, and a non-literal unbind changes nothing. Globs and dynamic key or options expressions remain possible matches. Surface `locked`, `dont_inhibit`, and `allow_input_capture` when present, because they qualify inhibitor and capture behavior.
7. Report every active exact match that Hyprland reaches, including its dispatcher, argument, and description when present. Binds are walked in order, following Hyprland 0.56.2 `handleKeybinds`: the walk stops at the first certain match whose dispatcher is `submap` and that runs on press (not a release or long-press bind), and a catchall is skipped once an earlier consuming release bind matched. Binds after an uncertain match stay possible. Skipped binds are listed as `not reached: <action> (<scope>); <reason>` and do not affect the outcome. Release and long-press binds do not run on press, and a non-consuming bind does not pass a key another bind consumed. Further matches that fire together with the first are shown in the row as "also fires: …"; release and long-press binds are not among them, and the reported binding is the first certain bind that runs on press when there is one. Never execute `__lua` or plugin dispatchers; the evidence states that their runtime action is not executed by whykey.
8. Query `hyprctl devices -j` when available and include active keyboard names, main-device status, and keymap in the evidence. Accept both legacy string and structured per-device fields in `hyprctl binds -j`. Kernel keycodes enter as evdev and convert once to the XKB namespace (`+8`); Hyprland socket events and numeric binding fields are treated as already XKB. The native capture hook's keycode is XKB: Hyprland 0.56.2's `LuaEventHandler.cpp` pushes the libinput keycode plus 8. Missing RMLVO or layout data yields explicit uncertainty, never a guessed US layout.

## Sway and i3 inspection

Sway reads its running main configuration with `swaymsg -t get_config -r`. i3 reads `i3-msg -r -t get_config`, which returns JSON with the config and `included_configs`; i3-msg older than 4.20 rejects `-r`, so whykey retries without it and accepts the bare config text that i3-msg then prints. When no path comes with the config, the main file is `loaded_config_file_name` from `get_version`, then the usual search paths. Sway does not report its includes, so `include` lines are resolved from disk, relative to the including file, with `~`, `$ENV`, and last-component globs. Those files may differ from what Sway loaded. Both adapters parse `set` variables (longest name wins), `set_from_resource` fallbacks (X resources are not read), modes, `{}` blocks, line continuations, and comments. They also parse `bindsym`/`bindcode`/`unbindsym`/`unbindcode` with `--release`, `--locked`, `--inhibited`, and `--input-device`, and skip mouse buttons. They match symbolic names and explicit `code:<n>` queries with exact modifiers against the mode reported by `get_binding_state`. When that query fails, they use the default mode and mark the result uncertain. A `nop` binding still grabs the key. A Sway `--release` binding runs its command on release, and the focused client receives both the press and the release; the layer is handled and passes the key on. An i3 `--release` binding consumes the key: i3 installs the same passive asynchronous key grab for every binding, including `--release` ones (`src/bindings.c`), and never replays the key (`src/key_press.c`), so the focused client receives neither the press nor the release. The lab confirms both models with Sway 1.11 and i3 4.25.1 (`sway-release-bind`, `i3-release-bind`). On a native X11 session the i3 result is refined by the X11 grab probe. The focused PID comes from `get_tree`. Xfce reads `xfconf-query -c xfce4-keyboard-shortcuts -l -v`, covering literal custom/default command properties and their current channel values. Cinnamon reads `gsettings list-recursively org.cinnamon.desktop.keybindings` and reports the schemas and accelerator values returned by that live query. MATE reads its Marco and SettingsDaemon global-keybinding schemas through GSettings. Niri reads literal `binds` blocks from `NIRI_CONFIG` or `~/.config/niri/config.kdl`; runtime reload, active mode, and inhibitor state remain conditional. dwl reads literal `static const Key keys[]` rows from `DWL_CONFIG` or `~/.config/dwl/config.h`; dwl has no stable global-shortcut IPC, so function/action matches remain conditional. River reads literal `riverctl map` commands from `RIVER_INIT` or `~/.config/river/init`, preserving mode and reload uncertainty. Wayfire reads literal `binding_*` accelerator values from `WAYFIRE_CONFIG` or `~/.config/wayfire.ini`, preserving plugin activation and reload uncertainty. labwc reuses the Openbox-compatible `<keybind>` format from `LABWC_CONFIG` or `~/.config/labwc/rc.xml`; runtime reload remains conditional. bspwm/sxhkd reads literal key/command pairs from the files described under sxhkd inspection, preserving conditional reload state. Openbox reads `<keybind>` XML entries from `OPENBOX_CONFIG` or `~/.config/openbox/rc.xml`, preserving conditional runtime reload state. On X11, `xbindkeys` reads literal command/key pairs from `XBINDKEYSRC` or `~/.xbindkeysrc`. AwesomeWM parses literal `awful.key` declarations from `AWESOME_CONFIG` or `~/.config/awesome/rc.lua`, Qtile parses literal `Key([...], key, ...)` entries from `QTILE_CONFIG` or `~/.config/qtile/config.py`, and XMonad parses literal `xK_*` entries from `XMONAD_CONFIG` or `~/.xmonad/xmonad.hs`. These executable configurations are never evaluated; helpers, dynamic variables, modes, and reload state remain conditional. On a native X11 session, the X11 grab probe decides whether an sxhkd, Openbox, xbindkeys, AwesomeWM, Qtile, or XMonad binding is active: a held grab makes it consume the key, and a free key makes the configured binding inactive. The adapters remain read-only and preserve conditional status when the compositor does not expose enough runtime state. A desktop without one of these adapters is represented by a generic conditional compositor layer rather than a false Hyprland error.

## KDE Plasma inspection

KDE asks the running kglobalaccel. The queried key is converted to a Qt key code (key and modifier flags from Qt's `qnamespace.h`, `qkeysequence.cpp`, and `qxkbcommon.cpp`) and sent with `busctl --user --auto-start=no --json=short call org.kde.kglobalaccel /kglobalaccel org.kde.KGlobalAccel globalShortcutsByKey '(ai)(i)' 1 <code> 0`. For each match, whykey reads the owning component with `getComponent` and calls `org.kde.kglobalaccel.Component.isActive`. A shortcut of an active component consumes the key. A shortcut of an inactive component passes it on ("registered but inactive"), because `globalShortcutsByKey` also returns inactive shortcuts. Activity is reported per component, so an action that its application released while keeping others registered still looks active, and a detail line says so. The binding inventory uses `allComponents`, `allShortcutInfos`, and `isActive`. The daemon is never auto-started. When it is not running, whykey reads `kglobalshortcutsrc` (or `KGLOBALSHORTCUTS_CONFIG`) with KConfig escapes. A match there is handled with unknown forwarding, never consumed, because the file does not say what is loaded. `Key_Launch0`–`Key_Launch9` and `Key_MediaTogglePlayPause` are not mapped yet (D-093). The portal `org.freedesktop.portal.GlobalShortcuts` reader checks only reachability, because the portal exposes registration and session APIs, not a read-only list of other clients' shortcuts.

## sxhkd inspection

sxhkd has no include directive; a configuration file holds only comments, hotkey lines, and indented command lines, and extra files come from its command line. Whykey reads `SXHKD_CONFIG` when it names a file. Otherwise it reads the `-c CONFIG_FILE` and extra configuration arguments of the running sxhkd from `/proc/<pid>/cmdline`, split the way sxhkd's `getopt` string does and resolved against `/proc/<pid>/cwd`. Otherwise it reads the default `sxhkdrc` under whykey's own `XDG_CONFIG_HOME`. Each file is parsed on its own, in sxhkd's load order, so an unfinished hotkey at the end of one file never joins the next file. `include` or `source` lines are never followed.

The parser ports sxhkd 0.6.1's `load_config`, `process_hotkey`, and
`parse_chain`. A hotkey starts in column 0 and its command on the next
indented line; `#` starts a comment only in column 0, a trailing `\`
continues a line, and the latest hotkey pairs with the next command.
`{a,b}` sequences and `x-y` ranges expand in lockstep with the command (the
first sequence varies fastest), `_` is an empty element, and `\` escapes.
Each key token may start with `~` (replay: sxhkd also passes the key to the
focused client, reported as handled-and-passed) and then `@` (the binding
runs on release; the press is still withheld). Plain keys without modifiers
are bindings. `meta` is Mod1, `mode_switch` is Mod5, and an uppercase letter
means Shift plus the letter. Chord chains (`;` and `:`), pointer buttons,
and `any` are not bound to one key and are skipped.

Continuations follow sxhkd's buffers exactly: sxhkd keeps one byte offset,
shared by the hotkey and command buffers, so a continued command moves the
offset at which the next continued hotkey is written (it may land inside a
multi-byte character, or past the hotkey's terminating NUL, where the text
is invisible and nothing binds). Leading and trailing bytes for which C-locale
`isgraph` is false are trimmed, so non-ASCII characters at a line's edges
are dropped, as sxhkd does. A hotkey or command that would expand to more
than 4096 renderings is skipped with a detail; sxhkd renders lazily, whykey
would materialize every rendering.

## GNOME inspection

GNOME reads the live settings with `gsettings list-recursively` over `org.gnome.desktop.wm.keybindings`, `org.gnome.mutter.keybindings`, `org.gnome.mutter.wayland.keybindings`, `org.gnome.shell.keybindings`, and `org.gnome.settings-daemon.plugins.media-keys`, plus each `custom-keybinding:<path>` listed in `custom-keybindings`. Only string and string-array values are accelerators; other types, such as the integer `volume-step`, are ignored. Accelerators are parsed by mutter's rules (`meta-accel-parse.c`): tokens are case-insensitive, including `Primary`, `Ctl`, `Shft`, and `ModN`. Unknown tokens such as `<Release>` are skipped. `disabled` or an empty value means no binding, and `0xNN` is a keycode. `Above_Tab` matches keycode 49 and the grave key. `<Meta>` is not compared and the evidence says so. A missing schema is a note, not a failure. When `gsettings` falls back to its memory backend (defaults only, no running settings store), the layer is unavailable. Matching shortcuts are reported as consuming; custom commands are shown as evidence and never executed. The lab verifies this against the GNOME settings store, not against a running GNOME Shell.

Malformed individual binding entries do not discard the valid remainder: they are counted in the report and force the Hyprland result to remain conditional because an omitted entry could still capture the key.

Bindings that use numeric keycodes are inspectable with an explicit `code:<n>` query. When `xkbcli`, the main keyboard's RMLVO data, and its active layout index are available, the effective XKB symbol group is compiled and compared against numeric bindings. The keymap parser reads `symbols[N]` groups (and the `symbols[GroupN]` form written by libxkbcommon 1.7 and older), `xkb_types`, and each key's type (or the type libxkbcommon would pick automatically), so CapsLock changes the level only for types that map Lock and Shift+AltGr reaches level 4 on four-level types. Compose sequences, physical keys, and layout-specific edge cases remain conditional when the full state cannot be determined.

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

For the named terminal adapters and the generic fallback alike, an `altgr` (Mod5) query resolves the key's level-3 keysym in the active layout (level 4 with Shift) from the Hyprland devices keymap, or from `WHYKEY_XKB_*`/`XKB_DEFAULT_*`, using a captured physical keycode when one is present. Its text is sent as UTF-8 in both legacy and Kitty encodings; AltGr is never encoded as a Kitty modifier. A dead keysym makes the terminal layer consume the key ("starts dead-key composition; no bytes are sent until the next key"). The encoding is reported as unknown, with the reason, when the key's type has no level 3, no keymap is available, the keysym has no known text, Kitty report-all-keys mode (flag 8) is active, or AltGr is combined with Ctrl, Alt, or Super. CapsLock and NumLock for this text come from the query, not the live LEDs. Keysym text comes from a table derived from X.Org's `keysymdef.h` (see `DEPENDENCY_PROVENANCE.md`).

## TTY and shell inspection

1. Read the active terminal settings with `tcgetattr` against `/dev/tty`.
2. Inspect the normal bytes produced by the selected terminal adapter, not the Kitty probe bytes used by `listen`. When the terminal has no explicit binding for the key, the dedicated Kitty, Alacritty, Foot, WezTerm, and Konsole adapters use the shared default encoding. Ctrl with a letter, `@ [ \ ] ^ _`, or Space produces the legacy control byte, and Ctrl+Alt adds an ESC prefix. Alt with a letter sends ESC and the lowercase letter; the letter is uppercase only with Shift. Kitty encoding uses the lowercase codepoint (`\e[102;5u` for Ctrl+F).
3. Report `ISIG` signal characters (`VINTR`, `VQUIT`, `VSUSP`) only while `ISIG` is set, and `IXON` flow-control characters (`VSTOP`, `VSTART`) only while `IXON` is set, each under its own label. For `ctrl+z` with `ISIG` and `VSUSP` set to `^Z`, explain the resulting `SIGTSTP`. When a byte is shared with another key, say so: 0x1b is also Escape, 0x09 Tab, 0x0d Return, 0x00 both Ctrl+@ and Ctrl+Space, and 0x08 Ctrl+H.
4. The shell is the nearest interactive shell above whykey (or above the `--pid` target) in the process tree. The walk passes only through shell processes, skips shells running a `-c` command or a script, and recognizes login shells started as `-zsh`. `$SHELL` is used only when no shell is found, and the tmux and Screen rows label it as the login shell.
   - Bash: when `shell-init` supplied a live snapshot (`bind -p` for each keymap, `bind -S`, `bind -X`, and `bind -V`), that snapshot is the only source. Function, macro, and `bind -x` entries are parsed, and `bind -x` entries are labelled `shell command (bind -x)`. Without a snapshot, apply the `bind`, `bind -x`, `bind -m`, `bind -f`, and `set -o vi` commands in `/etc/bash.bashrc` and `~/.bashrc`; they run after INPUTRC and override it. Then check the selected `INPUTRC`, `~/.inputrc`, or `/etc/inputrc`. Follow literal `$include` directives and `set keymap` sections, apply later matching definitions as overrides, and fall back to the built-in defaults of a pristine bash 5.3.9 with Readline 8.3 (`bind -p` without startup files): 176 emacs entries, or, in vi mode, the 39-entry vi-insert keymap (the detail says "vi-insert keymap assumed"). Such answers carry the `UnverifiedDefault` reason, because sourced files and plugins may override them, and the report suggests `eval "$(whykey shell-init bash)"`. Prefix keys such as `\C-x` and `\e` are reported as waiting for the next key, with their continuations listed. Static and captured paths must not claim an Emacs default while the active keymap is vi. Bash vi-mode defaults are not modelled.
   - Zsh: the `shell-init` wrapper passes `bindkey -lL main; bindkey -L; bindkey -LM vicmd`. Without it, whykey runs the installed `zsh -f -c 'bindkey -lL main; bindkey -LM emacs; bindkey -LM viins; bindkey -LM vicmd'` for its defaults and applies the `bindkey` commands in `/etc/zshenv`, `/etc/zsh/zshenv`, `~/.zshenv`, `/etc/zshrc`, `/etc/zsh/zshrc`, and `~/.zshrc`, in that order. `bindkey` lines are parsed with `-R` ranges, `-a`, `-s`, `-r`, and `-A x main`. `bindkey -v`, `-e`, and `-A`, and bindings without `-M`, apply to the keymap `main` points to at that moment, so `bindkey '^T' w; bindkey -v` leaves `^T` in emacs. Lookup models how ZLE reads keys, so an unbound escape sequence leaves its unmatched tail on the line: unbound Ctrl+Left in emacs mode inserts `;5D`. In vi insert mode, ESC then `f` runs `vi-cmd-mode`. When an earlier Esc has switched to `vicmd` and no `vicmd` bindings were captured, the answer is unknown.
   - Fish: the wrapper passes `bind --all | string collect` and `$fish_bind_mode`. Without it, whykey runs `fish --no-config -c '<preset>; bind --all'` for the preset selected in `fish_variables`, then applies the `bind` commands in `conf.d`, `config.fish`, and `fish_user_key_bindings.fish`. Bindings are parsed with fish quoting and escapes (`\ct`, `\e\[1\;5D`, `\'`), fish 4 key names (`ctrl-x,ctrl-e`, `alt-left`), fish 3 `-k` names, `-M`/`-m`, and `-e`. A user binding beats a `--preset` binding regardless of order. A bind without `-M` belongs to mode `default`, which is vi normal mode when vi bindings are active, not insert mode. Each key press is matched as one unit, and a key that starts no longer binding runs on its own. Kitty-protocol (CSI `u`) and SS3 input are converted to legacy bytes before matching. Mode-specific bindings are selected only when the mode is known.
   - Answers built without a live snapshot are Handled/Indeterminate or Indeterminate/Continues with the `UnverifiedDefault` reason, and the report suggests the matching `shell-init` command. Plugins, sourced files, and conditionals (oh-my-zsh, fzf) are not visible to the static reader.
   - The shell line editor is an endpoint: it is the last layer that reads the key. A key it does not bind keeps the pass outcome, but the report never says the key continues downstream. The summary states the editor's effect: Readline rings the bell, discards the unmatched prefix, and inserts the trailing printable bytes (unbound Shift+Up inserts `A`); ZLE runs `undefined-key` and leaves the unmatched tail on the line; fish reads the key and drops it. The headline reads "<layer> reads the key last and has no binding for it; nothing downstream receives it", qualified with "applies if earlier layers forward the key" when an earlier layer is uncertain; that caveat is stated once, not repeated in the uncertainty section.

## Exit status

- `0`: inspection completed, whether or not a binding matched.
- `1`: a required layer or prerequisite is unavailable. A detected Hyprland IPC failure is represented as an indeterminate compositor result so downstream layers can still be inspected; the report remains conditional rather than claiming that the shortcut is unbound.
- `2`: invalid CLI input. This includes a non-UTF-8 argument other than a file path, an unknown key name, more than one non-modifier key, two different JSON schema selections, `--suppress` combined with `--terminal`, `--window`, `--evdev`, `--device`, or `--no-suppress`, and `--window` combined with `--terminal`, `--evdev`, or `--device`.

`whykey snapshot` uses the inspect rule: it exits `1` only when every layer is unavailable or the diagnostic budget expired, and then explains on stderr that the inspection is incomplete; the snapshot is still written.

`SIGPIPE` stays ignored. When stdout is closed early, every command except `listen` exits quietly with status 141 (the status a shell shows for a process ended by `SIGPIPE`), with nothing on stderr. `listen` keeps its normal error handling so that its terminal, Kitty-flag, and hook guards always restore state. An extension that exits without reading its request is an extension error (status 1), not a crash.

`whykey listen` uses the same statuses; `--suppress` without native suppression exits `1`. `listen` restores the terminal settings before returning, including after normal errors, Escape, and Ctrl+C. A signal guard is installed before raw mode is entered and outlives the capture session, so `SIGTERM` and `SIGHUP` also restore the terminal. On `SIGTSTP`, `listen` restores the terminal, pops the Kitty flags, removes any leased native hook, and then stops; after `SIGCONT` it re-arms capture. `SIGTSTP` arrives from Ctrl+Z only on terminals using the legacy encoding (where the TTY turns `^Z` into the signal), or from an external `kill -TSTP`. With the Kitty keyboard protocol active (Kitty, Ghostty, foot, WezTerm), Ctrl+Z is a CSI `u` key report, so it is captured and explained like any other key. The Hyprland native backend restores the recorded submap before reporting and retries a failed restoration twice; a restoration that still fails stays pending with an explicit error, and a final best-effort attempt on exit warns instead of failing silently. A process killed with `SIGKILL` cannot run restoration code. In that case the hook stays installed until its 5-second lease expires; a suppressing hook then restores the submap.

## Interactive capture

0. Prefer native compositor capture when its IPC is reachable. The Hyprland backend needs a Lua configuration: on connect it sends a harmless `hyprctl eval`, which Hyprland rejects under a hyprlang `.conf` configuration ("eval is only supported with the lua config manager"). The backend then reports itself unavailable ("needs a Lua config (hyprland.lua)") and the chain moves on. Otherwise it installs a token-owned Lua hook. In pass-through mode, the hook only listens to keyboard events and reports them to whykey as socket2 events. The hook reports only a keycode, which whykey reads with the main keyboard's layout, so a virtual keyboard that uploads its own keymap is misread (D-089). With `--suppress`, the backend also records the active submap and activates the private `__whykey_capture` submap. The hook counts as armed only after dispatcher success, the observed submap postcondition, and the token-specific armed event. Every hook carries a 5-second lease that is renewed every 2 seconds. If the lease expires, the hook removes itself, and a suppressing hook also restores the submap. Remove the hook, or restore the submap, before reporting each event, and re-arm only for the next one. A stale token never restores another session or revives an expired hook. The Sway backend subscribes to binding events over IPC and is pass-through-only, so it observes only keys that ran a Sway binding. It decodes the modifier names in `event_state_mask` into the modifier mask. When native capture is unavailable, the default chain opens the probe window (see Probe window); steps 1 to 4 apply only to terminal capture.
1. Open `/dev/tty`, save its `termios`, and temporarily disable canonical input, echo, extended input processing, and software flow control. `ISIG` stays enabled so legacy-encoded Ctrl+C and Ctrl+Z still raise `SIGINT` and `SIGTSTP`.
2. Push all currently specified Kitty capture enhancements (`CSI > 31 u`: disambiguation, event types, alternate keys, all-keys reporting, and associated text) when the terminal supports them, and pop them with `CSI < u` on exit. Legacy terminals simply ignore these private control sequences and continue with their normal encoding. The original flags are retained separately for downstream byte prediction.
3. Read one event. Escape or Ctrl+C ends capture. CSI and Kitty protocol sequences are buffered for a short bounded interval, and consecutive literal Escape bytes stay separate events.
4. Decode control bytes, UTF-8 text, Alt-prefixed bytes, common cursor/navigation CSI sequences, and Kitty `u` sequences (including Unicode/function/keypad/media keys, alternate key fields, associated text, and press/repeat/release events) into `KeyCombo`. Preserve unrecognized input as a raw observed key with an explicit unknown-origin warning rather than aborting capture; mark UTF-8 text as potentially produced by Compose/dead-key/layout processing.
5. Remove or restore the native hook before rendering each report, whether it is pass-through or suppressing, and re-arm it only for the next captured event. Keep terminal capture mode active while rendering repeated reports, inspecting the TTY layer against the saved `termios` snapshot. This prevents capture mode from changing the explanation of `Ctrl+Z` and avoids a restore/reopen gap between reports. Restore the terminal and keyboard protocol on normal exit, cancellation, handled signals (`SIGINT`, `SIGTERM`, `SIGHUP`, `SIGQUIT`), and errors. On `SIGTSTP`, restore before stopping and re-arm after `SIGCONT`. Evdev capture keeps `--events all` across re-arming. A process killed with `SIGKILL` cannot run restoration code.
6. Run the route in `ROUTE_ORDER`: remapper, compositor, terminal, input method, TTY, session/multiplexer, interactive application, and shell. Inside tmux, GNU Screen, or Zellij the session/multiplexer step runs before the TTY step, because the multiplexer reads the key before the pane's TTY driver does; when the multiplexer step is unavailable, the TTY step is still recorded and then the route stops. A consuming layer stops downstream analysis. When a layer is uncertain, later results are retained but marked conditional by the report. What the observation proves depends on the backend. A terminal capture proves that the key passed the remapper, compositor, and terminal. A key delivered to the probe window proves that it passed the remapper, compositor, and any input method. A native capture proves only that the compositor received it. Those layers get the observed tier. A not-delivered probe observation proves only that something above the window consumed the key. Capture mode does not prove forwarding through the TTY, multiplexer, application, or shell. The terminal adapter resolves a separate normal terminal input for the downstream layers. If that input cannot be inferred, the report labels the downstream result as based on captured bytes or unknown.
7. When an ancestor is Neovim or Vim, query the current `mode()` and live `maparg()` state through the editor's remote-expression interface when a server address is available; otherwise inspect common mappings in the relevant init/vimrc files. For Emacs, query `key-binding` through `emacsclient` when a server is available, then inspect common `global-set-key` and `define-key` declarations in standard init files. For Helix, Micro, and Kakoune, inspect their conventional static keymap files. Detect common full-screen TUIs such as fzf, less, lazygit, btop, ranger, and yazi as intermediate applications even when their mappings are not parsed. For VS Code/VSCodium, parse literal single-key entries from user `keybindings.json`; for recognized JetBrains IDEs, parse literal `keyboard-shortcut` entries from the selected XML keymap. Chords, `when` clauses, plugins, modes, and runtime overrides remain indeterminate. Plugin precedence and mappings that are not visible through the available RPC endpoint or static parser remain indeterminate.

The listener queries the terminal's existing Kitty keyboard flags before enabling its capture flags. A known previous state controls the normal encoding used by Ghostty. A terminal that does not answer the query uses a legacy fallback with inferred confidence. The evdev backend keeps pressed modifier keys per device, reads the initial pressed-key bitmap and lock LEDs when the kernel permits it, resynchronizes after `SYN_DROPPED`, and removes descriptors reported as disconnected. Evdev observations expose pressed left/right modifier names and locked LED names separately from the compatibility bitmask; XKB-style latched state remains explicitly unavailable. The chain also reports detected tmux, GNU Screen, Zellij, and SSH context. tmux root bindings, the active client key table when available, both configured prefixes, and the `prefix`/`prefix2` tables are read. A bare key is looked up only in the active table, or in root. A prefix-table binding applies only in an `inspect` sequence whose earlier step is the prefix. A bare prefix is reported as `tmux prefix key`, and summaries name the table that matched. Screen reads its system file (`SYSTEM_SCREENRC` or common system paths) followed by the user file (`SCREENRC`, or `~/.screenrc`), preserving the effective override order. It inspects the command prefix and literal `bind`/`bindkey` directives and compares direct sequences with common terminal encodings. In an `inspect` sequence, a key that follows the command prefix is resolved against `bind`. Zellij merges dumped installed defaults with `ZELLIJ_CONFIG_FILE`/`config.kdl`, applying user overrides last; `keybinds clear-defaults=true` disables that merge. If defaults cannot be dumped or the KDL shape is unsupported, an unmatched key remains conditional. `WHYKEY_ZELLIJ_MODE` can provide a known runtime mode; without it, the active Zellij mode remains an explicit assumption. If multiple multiplexer markers are present, each adapter is inspected independently and the nesting order is marked conditional. If capture temporarily bypasses a normal TTY consumer, the report stops at that TTY result instead of claiming that the shell received the key.

## Architecture

`KeyCombo` and `KeySequence` own parsing and normalization. `TerminalInput` carries the bytes sent to the child PTY, along with their source and confidence. Each concrete layer reports one typed `LayerResult` whose internal state is a `LayerId` identity plus a single `Outcome` control value; the legacy `status`/`propagation` JSON pair is derived during serialization so schema v1 and v2 keep their exact meaning. One static route order (`ROUTE_ORDER`) sequences the remapper, compositor, terminal, IME (before the TTY stage), TTY, multiplexer/session, application, and shell stages; inside a multiplexer the multiplexer stage runs before the pane TTY stage. A remapper result that consumes the key stops the route. If a redirect's target parses as a key, the later layers re-run on that key. If it does not parse, their outcomes are made conditional. keyd, kanata, and xremap return redirect and consume results from real configurations; a redirect produces the `Remapped` conclusion, whose headline names the remapper and target ("keyd remaps Caps Lock to Esc.") and whose remaining text comes from the downstream layers' conclusion for the target key. The IME layer is included whenever an IME is detected, right after the terminal. An inactive IME, or one whose engine is a plain keyboard layout (fcitx5 `keyboard-*`, IBus `xkb:*`), passes keys through. The selected terminal adapter (Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, or generic fallback) produces `TerminalInput` before TTY, session, application, and shell layers inspect it. JSON reports retain the numeric modifier mask, expose a human-readable `key_display`, and summarize overall confidence as `configured`, `confirmed`, or `conditional`. Captured observations keep associated text separate from the logical key and preserve alternate-key metadata. If propagation is uncertain, later layers are still inspected and the report preserves that uncertainty as a conditional result.

Desktop adapters are registered once in `src/registry.rs`. Each entry supplies its detection and IPC probes plus the operations that vary by desktop: key inspection, binding enumeration when supported, focused-window PID lookup when supported, and capability/doctor metadata. Route dispatch, `bindings`, focused-window lookup, `doctor`, `capabilities`, and the support-matrix drift test all read this registry; each command takes one `Environment` snapshot per run, so desktop detection runs once and is shared instead of being repeated per adapter.

Future versions can add more terminal protocols and shell layers. A layer may stop the explanation when it captures a key or pass the key to the next layer.

External integration commands use a bounded executor with a two-second per-query timeout and a one-megabyte stdout/stderr limit. A timeout, read failure, or oversized response remains an adapter error and does not block the rest of the report. Static inspections share a ten-second wall-clock budget across all layers and sequence steps. The executor caps every command's timeout at the remaining budget and does not start a command once the budget is spent. When the budget expires, the report retains completed layers and adds an unavailable `Diagnostic budget` result. `WHYKEY_COMMAND_TIMEOUT_MS`, `WHYKEY_COMMAND_MAX_OUTPUT`, and `WHYKEY_DIAGNOSTIC_TIMEOUT_MS` override these defaults for a session.
