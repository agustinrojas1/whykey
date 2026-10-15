# Changelog

## [Unreleased]

### Added

- `whykey super --alone` (and `ctrl`, `alt`, `shift`, `hyper`, `altgr`; also
  `whykey inspect super --alone`) asks what happens when a modifier is
  pressed and released on its own. It reads keyd `overload`/`timeout`,
  kanata and kmonad `tap-hold`, xremap `modmap` `held`/`alone`, GNOME
  `overlay-key`, KDE `kwinrc [ModifierOnlyShortcuts]`, Hyprland
  `bindr`/`bindrt` and Sway/i3 `bindsym --release`, and renders the usual
  route table. Without `--alone`, a bare modifier is still an error, now
  naming the flag. JSON schema v2 reports `input.alone`; v1 is unchanged.
  Tap-hold findings now say what a tap emits and what holding does.

## [2.3.0] - 2026-10-15

The lab grows to 249 scenarios: 208 match exactly, 41 pass only as honest
uncertainty, and none fail. It adds Vim, Helix, Kakoune, Micro, and
Zellij; Fcitx5 and IBus; keyd, kanata, xremap, kmonad, and input-remapper
in the Hyprland VM; GNOME Shell with gsd-media-keys; KWin; Cinnamon; Sway
native capture; and Hyprland `listen --suppress`. Those programs exposed
24 defects, fixed below. The lab now runs in a base image plus five group
images published to GHCR, and CI runs only the scenarios a change can
affect, in parallel shards (a full run takes about 9 minutes instead of
35). The README is rewritten for first-time readers.

### Fixes

- With no input context focused, Fcitx5 answers `CurrentInputMethod`
  with an empty string. The input-method layer used to stop there and
  report the runtime query as unavailable; it now reads `State` and
  reports Fcitx5 as closed (no focused input context), so keys pass
  through. Found on a real Hyprland desktop. Test
  `fcitx5_without_a_focused_input_context_reads_the_closed_state`.
- IBus with no engine selected forwards keys; whykey used to report its
  runtime API as unreachable (D-140).
- Hyprland `listen --suppress` produces a report again. It always timed
  out after the modifier release (D-183). A key it holds without a
  binding keeps its normal route, and the report says the application did
  not receive it this time (D-184).
- Sway native capture: `bindcode` bindings no longer abort `listen`,
  keycodes are converted from XKB to evdev, `--release` bindings are
  reported as releases, and the waiting text says which keys Sway cannot
  report (D-180 to D-182, D-185).
- The X11 grab probe checks the path to the focused window as well as the
  root window, so Cinnamon's per-window shortcuts are no longer reported
  as unbound (D-162). Cinnamon's window bindings and media keys are read
  from their own schemas (D-161).
- GNOME shortcuts that only gnome-shell or gsd-media-keys grab are not
  consumed while that daemon is down (D-160).
- KDE on Wayland: a kglobalacceld that owns `org.kde.kglobalaccel` keeps
  KWin from handling shortcuts, and whykey now says pass there (D-163).
- Niri reads included files, `Mod` follows the backend (Alt when nested)
  and `mod-key`/`mod-key-nested`, and unreadable binds make a no-match
  unknown (D-170, D-171, D-177).
- Wayfire answers from the running compositor over `ipc-rules` when that
  plugin is loaded, and reads metadata default bindings, activator
  alternatives, activators outside `binding_*`, and evdev key names
  (D-172 to D-176).
- Vim and Neovim mappings with lowercase special keys or plain keys are
  found; editors and Zellij without a matching binding answer
  uncertain-continues instead of indeterminate (D-130 to D-132).
- Zellij assumes its `default_mode` and reports a binding there as
  handled-uncertain, since its current mode is not readable (D-133).
- With a remapper running on Hyprland, printable keys use the main
  keyboard's layout from `hyprctl devices` instead of being uncertain
  (D-150).

### Lab

- New environments: GNOME Shell with gsd-media-keys (`gnome_shell`),
  KWin (`kwin`), Cinnamon (`cinnamon`), Fcitx5 and IBus (`ime`), and
  keyd, kanata, xremap, kmonad, and input-remapper in the Hyprland VM
  (`remap`). New groups cover Vim, Helix, Kakoune, Micro, and Zellij
  (`termapps`), Sway native capture and Hyprland `--suppress`
  (`capture`), and more Niri and Wayfire cases.
- `lab/run.py --record DIR` stores every input whykey read in a scenario
  (commands, files, environment, process and TTY state), and
  `cargo test --test lab_replay` replays the 81 recorded scenarios in under
  a second without Docker. A replay that needs an input the recording lacks
  fails, so a changed query sends the change back to the real lab
  (`docs/lab-replay.md`).
- Scenarios can require their binding to fire (`expect`), check state
  after whykey exits (`wait_whykey`, `restored`, `mark_probe`), and check
  upstream redirects (`whykey.via`, `whykey.not_via`).

## [2.2.0] - 2026-10-11

The lab grows to 172 scenarios: 156 match exactly, 16 pass only as honest
uncertainty (13 shells without `shell-init`, `emacs-unbound-f7`,
`niri-bind-consumed`, and `wayfire-bind-consumed`), and none fail. The
lab adds Niri (nested in Sway), Wayfire, labwc, dwl, Qtile, XMonad, Xfce,
MATE, the kitty, alacritty, foot, WezTerm, Konsole, and Ghostty terminals,
Emacs 30.2 (daemon and `emacsclient`), and Neovim 0.12.5. The Hyprland VM
gets a kernel keyboard (QEMU virtio-keyboard, driven through QMP), so
`listen --evdev` and Hyprland's native listen and `--window` capture run
against real kernel input.

### Fixes

- Alt+Escape is predicted as ESC ESC in terminal prediction, not as a
  lone Escape. Test `encodes_alt_escape_as_escape_prefixed_escape`; lab
  `tmux-root-alt-escape` (exact).
- A `code:N` query is headlined by its layout symbol. The verbose report
  keeps the keycode. Test `a_code_query_is_headlined_by_its_layout_symbol`.
- A remap whose target is a keypad key reports the downstream key as the
  keypad key (`KP_*`). Digits stay conditional on NumLock. Test
  `emitted_keypad_codes_reach_downstream_as_keypad_keys`.
- KDE maps the `Launch` (0–F) keys, keypad shortcuts, and shifted digit
  symbols. Test `maps_launch_media_keypad_and_shifted_digit_keys`; lab
  `kde-launch5-key`, `kde-shift-digit-symbol`.
- A Hyprland virtual keyboard that brings its own keymap is reported as
  `code:N`, with only the keycode certain, and never ends `listen`. Test
  `virtual_keyboard_keymaps_leave_only_the_keycode_certain`.
- VS Code's default keybindings are bundled: VS Code 1.140.0's default
  table (US layout), the keybindings of its bundled extensions, and their
  removals. An installed VS Code of another version adds a note that the
  defaults may have changed. `tools/generate_vscode_defaults.py`
  regenerates the table; `WHYKEY_VSCODE_DEFAULTS`, `VSCODE_PRODUCT_JSON`,
  and `VSCODE_EXTENSIONS` override its inputs. Test
  `whykey_vscode_extension_matches_fixture_keybindings`.
- Extension responses may carry an optional `tier` (`live_query`,
  `config_file`, or `default`). `observed` is reserved for whykey's own
  captures and is rejected. Test `accepts_only_tiers_an_extension_can_establish`.
- Emacs answers are live-verified: a bound key fires (`emacs-global-bind`,
  `emacs-minor-mode-bind`, `emacs-prefix-c-x`), and an unbound key reports
  honest uncertainty (`emacs-unbound-f7`). Test
  `whykey_emacs_extension_matches_fixture_runtime_queries`.
- `whykey-nvim` identifies built-in commands from `$VIMRUNTIME/doc/index.txt`.
  Neovim scenarios: `nvim-builtin-ctrl-a`, `nvim-normal-bind`,
  `nvim-unbound-f7`.
- Terminal adapters: WezTerm reads a capital letter as Shift and its
  `EmitEvent` consumes the key; Alacritty `command` bindings and foot's
  `action = keys` parser and pipe actions consume the key. Tests
  `wezterm_capital_letter_is_shift_and_emit_event_consumes`,
  `alacritty_command_binding_is_consuming`, `foot_pipe_binding_is_consuming`.
  Konsole's keytab is encoding, so it passes. Lab `wezterm-*`, `alacritty-*`,
  `foot-*`, `konsole-*`.
- labwc and dwl consume bound keys while the compositor runs. dwl reads
  `MODKEY` from its `#define`, and falls back to Alt without one. Tests
  `defaults_modkey_to_alt_without_define`; lab `labwc-bind-consumed`,
  `dwl-bind-consumed`.
- Wayfire reads evdev key names. Test `parses_wayfire_evdev_key_names`.
  Its bound keys are `handled` with uncertainty (`wayfire-bind-consumed`).
- River 0.4 and later reports `unavailable` with the reason; there is no
  `riverctl` to query. Test `parse_river_version`.
- MATE reads `run-command-N` and `keybinding-commands` for run-command
  bindings. Test `run_command_binding_takes_command_from_keybinding_commands`;
  lab `mate-bind`, `mate-unbound`.
- Qtile reads its variables and modifier names from its config. Tests
  `qtile_variables`, `resolves_qtile_modifier_variables`; lab `qtile-bind`,
  `qtile-unbound`. XMonad and Xfce are lab-verified too (`xmonad-*`,
  `xfce-*`).

### Lab

- Environment plugins: `lab/envs/*.py` (one module per environment), with
  package families in `lab/packages/`. See `lab/README.md`.
- Hyprland scenarios run on the kernel keyboard (`hyprland-qkbd-*`).

## [2.1.0] - 2026-10-11

A verification sweep of 2.0.0 (host, configuration, packaging, and lab
stress runs) found the bugs fixed here. The lab now runs 125 scenarios: 111
match exactly, and 14 pass only as honest uncertainty (13 shells without
`shell-init`, plus `tmux-root-alt-escape`). None fail.

### Fixes

- A non-UTF-8 argument no longer panics. File arguments (`replay`, `diff`,
  `snapshot --output`, the `extension` program) accept any path. Any other
  non-UTF-8 argument is a usage error: "argument X is not valid UTF-8",
  status 2.
- Closing stdout early (`whykey --help | true`) no longer panics with
  "failed printing to stdout". Every command except `listen` exits quietly
  with status 141, the status a shell shows for SIGPIPE. SIGPIPE itself stays
  ignored, so `listen`'s terminal and hook guards always restore state, and
  an extension that exits without reading its request is reported as an
  extension error (status 1) instead of killing whykey.
- Error messages echo at most 64 characters of the input. A combination
  with many `+` segments is summarized ("and N more segments"); 120 000
  `+` characters used to print the whole input back.
- `whykey snapshot` uses the same exit rule as `whykey KEY`: status 1 only
  when every layer is unavailable or the diagnostic budget ran out. stderr
  then says why; the snapshot is still written.
- `--verbose` no longer prints Rust debug text for the Hyprland scope. It
  reads `scope: submap default`, `every submap (universal bind)`, or
  `unknown`.
- Key names keep their libxkbcommon spelling (`XF86AudioMute`,
  `XF86TouchpadOff`), `code:N` is lowercase, and switches read
  `switch:off:Lid Switch`.
- Hyprland binds that fire together with the reported one are shown:
  "Configured to stop: Focus on next window; also fires: Reveal active
  window on top". Release and long-press binds are not listed as firing on
  press, and the reported binding prefers a bind that runs on press. Lab
  `hyprland-release-and-press-bind`.
- Hyprland inspect follows `handleKeybinds` (0.56.2): a matching submap bind
  that runs on press stops the binds after it, catchall included, and a
  catchall is skipped after a consuming release bind. Skipped binds are
  listed as `not reached: …; <reason>`. Release and long-press binds no
  longer count as running on press, and a non-consuming bind no longer lets
  through a key another bind consumed. Lab `hyprland-submap-break`,
  `hyprland-submap-break-catchall`.
- Keypad keys are their own keys end to end: `kp_enter` (alias `kp_return`)
  is not `return`, and `kp_0`–`kp_9`, `kp_home`, `kp_end`, `kp_page_up`,
  `kp_page_down`, `kp_add`, `kp_subtract`, `kp_multiply`, `kp_divide`,
  `kp_equal`, and `kp_decimal` are not the main keys. Hyprland resolves
  them to the keypad keycodes (KP_Enter is 104, Return is 36), KDE encodes
  `Keypad|Key_Enter`, and terminal prediction encodes keypad keys (Kitty
  codes 57399–57426; legacy mode notes the application-keypad alternative).
  Remapper rules on keypad keys (keyd `kpenter`, `kp1`) match only the
  keypad keys, and main keys no longer match keypad codes. The X11 grab
  probe resolves keypad digits on the NumLock level without adding Shift.
  i3 and Sway follow their opposite NumLock rules: i3 matches a keypad digit
  only with `Mod2` in the `bindsym`, Sway only without it. On a real
  Hyprland desktop `super+kp_1` used to report workspace 1. Lab
  `hyprland-keypad-enter`, `hyprland-keypad-enter-not-return`,
  `hyprland-return-not-keypad-enter`, the three `sway-` equivalents,
  `i3-keypad-digit-numlock`, `i3-keypad-digit-without-numlock-modifier`,
  `sway-keypad-digit`, `sway-keypad-digit-mod2`.
- `bindings --key` and `conflicts --key` parse a key query like inspect: a
  bare key matches under any modifiers, a combination needs exactly its
  modifiers, and `code:N` matches Hyprland records listed by layout symbol.
  Other text stays a case-insensitive substring filter. `super+b` used to
  return ten substring hits.
- `conflicts` keeps the order the source reports binds in. A Hyprland submap
  bind that is not last reads "the submap bind stops the binds listed after
  it"; otherwise "multiple binds fire together".
- tmux: `list-keys` keys are read case-sensitively (`C` is Shift+c, `BTab`
  is Shift+Tab), escaped (`\%`, `\;`) and quoted (`"M-{"`) keys and the `-`
  key are understood, and `Escape`, `BSpace`, `Tab`, `Enter`, `PgUp`,
  `PgDn`, and the `KP` names are mapped. Lab `tmux-prefix-lowercase`,
  `tmux-prefix-uppercase`, `tmux-prefix-escaped-percent`,
  `tmux-prefix-escaped-semicolon`, `tmux-prefix-dash`,
  `tmux-root-quoted-brace`, `tmux-root-btab`, `tmux-root-alt-escape`.
- Inside tmux, Screen, or Zellij the multiplexer comes before the pane's TTY
  driver in the route, so a tmux bind on `C-c` is no longer reported as
  SIGINT. If the multiplexer cannot be inspected, the pane TTY row is still
  reported before the route stops. Lab `tmux-prefix-before-pane-tty`.
- Readline defaults are regenerated from a pristine bash 5.3.9 / Readline
  8.3 (176 emacs entries, including `\e0`–`\e9` digit-argument and
  `do-lowercase-version`). Vi mode uses the 39-entry vi-insert keymap
  instead of "vi defaults not modelled".
- A key the shell line editor does not bind is no longer said to continue
  downstream: the editor is the last layer that reads it. The report states
  the effect instead: Readline rings the bell and inserts the leftover bytes
  (`shift+up` inserts `A`), ZLE runs `undefined-key`, fish drops the key.
  After an uncertain earlier layer the caveat is stated once.
  Lab `bash-unbound-shift-up-*`, `zsh-unbound-ctrl-left-*`.
- Row summaries are no longer cut at the first `;`, so fish and `bind -x`
  commands are shown whole (long summaries end in `…`). Lab
  `fish-preset-alt-left-init`.
- The sxhkd parser is a port of sxhkd 0.6.1: `~` (replay: the key also
  reaches the focused window), `@` (release), `{a,b}` and `{1-9}` expansion
  in lockstep with the command, `_` empty elements, plain keys without
  modifiers, `meta`/`mode_switch`, uppercase letter as Shift, line
  continuations, and column-0 comments. A continued line shares sxhkd's
  single byte offset between the hotkey and command buffers, as sxhkd does,
  so odd continuations bind what sxhkd binds (this used to panic inside a
  multi-byte character). Non-ASCII bytes at a line's edges are trimmed as
  sxhkd does in the C locale. A line that expands to more than 4096
  renderings is skipped with a detail saying so. Chord chains, pointer
  buttons and `any` are skipped. Lab `sxhkd-replay`, `sxhkd-replay-plain`,
  `sxhkd-brace`, `sxhkd-brace-range`, `sxhkd-plain-key`, `sxhkd-release`,
  `sxhkd-continuation-shared-offset`, `sxhkd-continuation-past-text`,
  `sxhkd-non-ascii-command`.
- The X11 grab probe no longer overrides an adapter that knows the grabbing
  program replays the key (sxhkd `~`): the result stays handled-and-passed
  with the grab evidence attached.
- `whykey doctor` and `capabilities` report native Hyprland capture as
  unavailable on a `hyprland.conf` session ("needs a Lua config
  (hyprland.lua)"), using the same no-op `hyprctl eval` as `listen`.
- `inspect --focused` on a terminal emulator follows the foreground process
  of its one pty, so the application and shell rows describe the program
  reading the key. With several ptys the focused terminal is reported as
  unknown. stderr notes that the terminal and TTY rows describe whykey's own
  terminal.
- An empty `TERM_PROGRAM` or `LC_TERMINAL` counts as unset.
- `whykey-emacs`: when `emacsclient` is not on `PATH` it says Emacs is not
  installed and suggests installing it, instead of `emacs --daemon`.
- Support matrix attribution: verdicts decided by the X11 grab probe are
  credited to the new `compositor.x11-grab` entry (also in `capabilities`).
  Adapters keep only scenarios their own logic decided, plus a
  `lab_binding` list where they only named the binding. New `-no-display`
  scenarios check the i3 and KDE adapters without the grab probe.
- Removed three ignored `listen` throughput tests that asserted bulk
  streaming, which `listen` 2.0 does not do.

### Packaging

- Release binaries: `x86_64-unknown-linux-gnu` now needs only glibc 2.28
  (2.0.0 needed 2.34 and warned about GLIBC_2.39), and new fully static
  `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` archives are
  published, each with a `.sha256`. Built with cargo-zigbuild; the workflow
  checks the glibc floor and static linking (no interpreter and no `NEEDED`
  entry in `readelf`) and runs each binary on Ubuntu
  20.04, Debian oldstable, and Alpine (amd64 and arm64). CI gains a musl
  test job.
- Distribution test suites pass when PID 1 is a shell (containers, the Nix
  sandbox): the PID-1 test now targets its own process.
- Nix: `packaging/default.nix` allows `nix-build packaging`; `python3` is a
  check input, the extension and fixture shebangs are patched, and `target/`
  and `result` are excluded from the source.
- Debian: `python3 <!nocheck>` build dependency, tests honor `nocheck`, and
  the upstream `CHANGELOG.md` is shipped under its own name instead of
  overwriting the Debian changelog.
- RPM: `python3` is only Recommended; the automatic `/usr/bin/python3`
  requirement is excluded.
- Arch: the PKGBUILD has a Maintainer line (namcap is clean).
- The source archive contains only Git-tracked files.
- README: pin `@2` with mise (its `minimum_release_age` can otherwise give
  an older major), a per-target asset table, and where the extensions are
  for mise and Cargo installs.

### Lab

- 125 scenarios (from 81): keypad and NumLock, submap-break, release versus
  press binds, tmux key syntax and ordering, unbound shell keys, sxhkd
  syntax and continuations, and `-no-display` adapter checks.
- `vkbd` primes each new virtual keyboard with a no-op modifiers event, and
  `rdkbd` spends Mutter's dropped first press on a Shift tap. The focus gate
  requires the Shift press, not a lone release. `sway-mode-inactive` passed
  30 of 30 repeated runs.
- `lab/mutate.py` reports which scenarios catch a deliberate whykey bug; all
  10 X11 adapter mutations are caught. Lab errors are reported separately
  and never count as catches, and a run that leaves no results stops the
  tool.
- `run.py --repeat N` runs scenarios repeatedly to hunt flakes; `--vms`
  defaults to a third of the CPUs with KVM. A new `binding` check requires
  a substring of the reported binding's action. `vkbd.py --numlock` types
  with NumLock locked.

## [2.0.0] - 2026-10-10

whykey now checks its answers against the running programs. A reality lab
runs them, and whykey's verdict must match what they actually do. The first
lab run against 1.5.0 matched in 16 of 40 scenarios. This release runs 81
scenarios: 70 match exactly, and the other 11 (shells without `shell-init`)
pass only as honest uncertainty, which does not count as verification.

### Changed (may break scripts)

- `whykey listen` uses a new default chain: native compositor capture, then
  a focused probe window when a Wayland or X11 display is usable, then the
  terminal. Before this, the fallback went straight to the terminal. Use
  `--terminal` to get the old fallback. `--dry-run` and `--explain-capture`
  print the numbered chain with a reason for each stage.
- Doctor and capabilities: the `gnome-shell-runtime` reader is replaced by
  `gnome-gsettings`. GNOME and KDE no longer report `api-does-not-expose`.
- `whykey doctor` no longer suggests joining the `input` group for evdev.
  It prints a temporary `sudo setfacl -m u:"$USER":r …` command for exactly
  the keyboards it cannot open.

### Features

- Reality lab (`lab/`): 81 scenarios run the real Sway 1.11, i3 4.25,
  Hyprland 0.56.2 (both `hyprland.conf` and `hyprland.lua`, in a KVM guest),
  Openbox, AwesomeWM, sxhkd, xbindkeys, kglobalacceld 6.7.5, headless
  mutter 50 (mutter-owned GNOME bindings only), bash, zsh, fish, tmux, and GNU Screen. Each scenario sends
  synthetic input (a virtual keyboard with the US keymap, wtype, xdotool, or
  pty keystrokes; mutter's RemoteDesktop D-Bus API for GNOME). The ground truth is the program's reaction: a bind
  touched a file, or a key reached an independent test client. Each
  scenario carries a positive control, so a setup in which nothing could
  fire cannot pass, and a scenario fails when whykey disagrees. Matches
  accepted only as honest uncertainty do not count as verification.
  `python3 lab/run.py` runs it locally, and
  `.github/workflows/lab.yml` runs it on pull requests. See `lab/README.md`.
- Probe window: `whykey listen --window` (`-w`) opens a small focused
  Wayland or X11 window (app_id/WM_CLASS `whykey-probe`). A key that reaches
  it was delivered by the desktop. If only modifier changes arrive, the
  report says, for example, "Super was pressed and released and no other key
  reached this window: either the key you pressed with Super was consumed by
  the compositor/desktop or an input method, or Super was pressed alone (or
  with the mouse)." That observation's confidence is `conditional`. The
  default chain skips the probe window in SSH sessions; `--window` still
  forces it.
  New JSON values: source `{"kind":"probe-window"}`, disposition
  `not_delivered`, and capability `capture.probe-window`.
- X11 grab probe: on a native X11 session, whykey asks the X server whether
  another client holds a passive grab on the queried key (core `GrabKey` and
  XInput2, with Lock and NumLock variants). A grab confirms the window
  manager's or hotkey daemon's binding as consumed. A free key (no client
  holds a grab for it on the root window) makes a configured binding
  inactive. A grab that no adapter claims shows as an
  "X11 key grab" layer. Under XWayland a grab only adds a note, and over SSH
  the probe is skipped.
- KDE Plasma: whykey asks the running kglobalaccel over D-Bus
  (`globalShortcutsByKey`, then the component's `isActive`) and never
  starts the daemon. An inactive component passes the key on.
  `kglobalshortcutsrc` is now only a fallback, and it never gives Consumed.
- GNOME: live `gsettings` over the mutter, mutter-wayland, window-manager,
  GNOME Shell, and media-key schemas plus custom keybindings. Accelerators
  are parsed by mutter's rules (`Primary`, `Above_Tab`, keycodes,
  `disabled`).
- Evidence tiers: each layer row is tagged `observed`, `live query`,
  `config file (static)`, or `default (unverified)`. JSON v2 and NDJSON path
  entries gain `tier`; schema v1 is unchanged. The "Next" section names the
  step that would raise the tier, such as `whykey listen` or loading
  `shell-init`.
- Doctor lists "Observation taps (what whykey can watch now)": native
  capture, probe window, X11 grab probe, terminal, and evdev with how to
  enable it. It also lists "Layer sources": for each layer, whether its
  answer will come from a live query, a config file, or nothing. Doctor JSON
  v2 gains `observation_taps` and `layer_sources`; v1 is unchanged.
- Support matrix: each entry records the lab scenarios that verify it
  (`lab`) and any hand checks on the maintainer's machine (`live`).
  `tools/generate_support_matrix.py --lab-results FILE` adds the results of
  a lab run.
- zsh and fish adapters were rewritten and checked against the real shells
  (178 behaviour checks plus the committed lab scenarios). zsh parses
  `bindkey -L` output and models how ZLE reads keys and keymaps. For
  example, an unbound Ctrl+Left leaves `;5D` on the line. fish models fish 4
  key names and fish 3 `-k` names, `--preset`/user precedence, and modes; a
  bind without `-M` belongs to the default (vi normal) mode. Without
  `shell-init`, whykey asks the installed shell for its defaults
  (`zsh -f`, `fish --no-config`) and applies the `bindkey`/`bind` lines in
  the startup files.
- bash without `shell-init` now applies `bind`, `bind -x`, `bind -m`, and
  `set -o vi` from `/etc/bash.bashrc` and `~/.bashrc`.
- The shell is now taken from the process tree (the nearest interactive
  shell above whykey), not from `$SHELL`. `$SHELL` is only the fallback. The
  tmux and Screen rows label `$SHELL` as the login shell.

### Fixes

- i3 was always reported as unavailable ("could not read the running i3
  config"). `i3-msg -t get_config` prints plain text unless given `-r`.
  whykey now passes `-r` and also accepts plain text from i3-msg older than
  4.20.
- GNOME: shortcuts in the window-manager and mutter schemas (for example
  the default `<Super>Tab`) were missed, because only media keys were read.
  The integer `volume-step 6` was read as a binding for the key `6`.
- Openbox, AwesomeWM, sxhkd, xbindkeys, and KDE bindings that the lab saw
  fire were reported as conditional. Unbound keys were reported as
  indeterminate. The X11 grab probe and the kglobalaccel query now decide.
- zsh: lookups never matched, because escaped bytes (`\x14`) were compared
  with decoded keys. `bindkey -v`/`-e`/`-A` and bindings without `-M` now
  follow the keymap `main` points to at that moment.
- fish `shell-init` sent its bindings as a single line, so nothing matched.
  It now uses `bind --all | string collect`.
- zsh `shell-init` now also dumps the `main` keymap alias and the `vicmd`
  keymap (`bindkey -lL main; bindkey -L; bindkey -LM vicmd`).
- bash: a user `bind -x` on Ctrl+T was reported as `transpose-chars` when
  `shell-init` was not loaded.
- Terminal byte prediction: Alt+F was encoded as `ESC F` (read by bash as
  `do-lowercase-version`). It is now `ESC f`; uppercase needs Shift. Kitty
  encoding uses the lowercase codepoint (`\e[102;5u` for Ctrl+F).
- Sway `--release`: the lab confirms that the focused client receives the
  press and the release while the command runs on release. The headline
  now says "Sway handles the shortcut and passes it on; other layers are
  uncertain." instead of calling Sway's own forwarding unknown. i3
  `--release` consumes the key, as the lab confirms.
- Hyprland: with a `hyprland.conf` config, `whykey listen` failed with
  "failed to arm" and a cleanup warning. Native capture now reports itself
  unavailable ("needs a Lua config (hyprland.lua)"), and listen falls back
  to the probe window.
- Hyprland: the `description` of a Lua `hl.bind(…, { description = … })`
  is now read. Lua source hints show it, and `code:` binds can recover
  their key by description.
- Hyprland (D-073): `wtype` input did fire the Lua key hook, but every key
  arrived as keycode 9, because wtype uploads its own keymap. With the real
  layout, keycode 9 is Escape, which ends `listen`; that most likely ended
  the original host test. The lab's virtual keyboard sends real keycodes,
  and native Lua capture works with it. Virtual keyboards with their own
  keymap are still misread (D-089).
- The portal reachability probe checks `NameHasOwner` first. Before this,
  `gdbus introspect` started `xdg-desktop-portal`.

### Internal

- The probe windows use `wayland-client` 0.31 (pure-Rust backend, no
  libwayland at runtime) with `wayland-protocols` 0.32, and `x11rb` 0.14 with
  XInput. xremap YAML is parsed with `yaml-rust2` 0.13. This replaces the
  hand-written subset parser: block scalars, tags, and quoted keys are now
  accepted, while aliases, several documents, and complex keys are still
  rejected. The new crates and their licences are in
  `DEPENDENCY_PROVENANCE.md` and `debian/copyright`. The vendor directory
  of the offline source archive is now about 65 MB. About 18 MB of that is
  `windows-sys`, which is vendored even though Linux builds do not compile
  it. The binary still links only libc and libgcc_s; no libwayland or libxcb
  is linked.
- `lab/` is excluded from the published crate.

## [1.5.0] - 2026-10-10

### Features

- Remappers: kmonad and input-remapper are now evaluated for the queried key. kmonad reads the configuration file from the running process's command line (and its `--fallthrough`, `--input`, and `--implicit-around` options), or `KMONAD_CONFIG` and `~/.config/kmonad/*.kbd` when no kmonad runs. It evaluates `defcfg` (`device-file`, `fallthrough`, `implicit-around`), named and unnamed `defsrc`, the first `deflayer`, aliases, kmonad's key and shifted names (a bare `@` is Shift+2), modifier prefixes, and `around` forms. Remaps redirect; layer, macro, command, compose, and dead-key buttons, `XX`, and keys dropped without fallthrough consume; tap, multi-tap, sticky, stepped, and timeout buttons stay conditional. input-remapper reads `INPUT_REMAPPER_CONFIG` (a directory or its `config.json`), otherwise `$XDG_CONFIG_HOME/input-remapper-2` or `~/.config/input-remapper-2`, and evaluates every autoloaded preset in both preset formats, resolving symbols through `xmodmap.json` and `KEY_*` names. Autoload keys name input-remapper device groups (a duplicate `<name> N` uses the preset directory `<name>` and leaves the device unknown), and a preset applies to every evdev node of its group, such as the keyboard's "Consumer Control" node. Macro names with non-ASCII characters are read without a crash. Key and combination outputs redirect; `disable` and macros consume, except conditional macros such as `if_tap` and `mod_tap`. Presets applied from the GUI without autoload are unknown. For both remappers the answer stays conditional unless the captured key came from the configured device.
- Dead keys: a key that starts dead-key composition now gets its own headline ("AltGr+' is a dead key (dead_acute) in the active layout; …") instead of being described as configured to consume the key. JSON v2 and NDJSON path entries gain an optional `dead_key` field with the keysym; schema v1 is unchanged.
- Hyprland: `source =` paths with bracket classes (`[abc]`, `[a-z]`, `[!x]`, `[^x]`) are now expanded like glob(3), in linear time. POSIX named classes and backslash escapes are not supported.
- `whykey-nvim`: an unmapped `<C-i>`, `<C-m>`, `<C-[>`, or `<C-j>` now resolves to its `<Tab>`, `<CR>`, `<Esc>`, or `<NL>` mapping, as Neovim does (`:help CTRL-I`), and the summary says so ("maps <C-i> (as <Tab>) to …"). The extension was checked against a live Neovim 0.12.5.

### Fixes

- Hyprland: while whykey's capture submap is active, the query for the previous submap no longer mistakes a failed or unexpected reply for a submap name. A real submap called `none` is kept, and when the previous submap is unknown, bindings are evaluated as in the default submap with a detail line that says so.
- `meta-key` is no longer accepted as an alias for AltGr (Mod5); a combination containing it is a usage error (exit status 2).

### Internal

- kanata and kmonad share one s-expression reader (`src/remapper/sexp.rs`) with the 64-level nesting cap.
- The Hyprland and `listen` source files are now real modules instead of `include!`d files.

## [1.4.0] - 2026-10-09

### Features

- Remappers: keyd, kanata, and xremap are now evaluated for the queried key, so a remap or a consuming action changes the route. keyd reads `/etc/keyd/*.conf` (or `WHYKEY_KEYD_CONFIG`) including `include`, `[aliases]`, `[ids]`, the default layout, and the modifier layers for the held modifiers. kanata reads the `-c`/`--cfg` file of the running process (or `KANATA_CONFIG`, then `~/.config/kanata/kanata.kbd`) and evaluates `defsrc` with the first layer, aliases, modifier prefixes, `multi`, and `defoverrides`. xremap reads the running process's configuration paths and `--device` (or `XREMAP_CONFIG`, then `~/.config/xremap/*.yml`) and evaluates `modmap` feeding `keymap`. Tap-hold, one-shot, application or window conditions, and other timing- or state-dependent actions stay conditional; layer switches, macros, and commands consume the key. A redirect re-runs the later layers on the target key, and the headline reads "keyd remaps Caps Lock to Esc. …". A configuration that cannot be read is reported as unavailable, never as a pass, and disagreeing configurations or several active remappers stay uncertain. Printable keys are mapped by their US position and stay uncertain unless `WHYKEY_XKB_LAYOUT` or `XKB_DEFAULT_LAYOUT` is `us` with no variant. kmonad and input-remapper are still detected but not evaluated, and the report says so.
- AltGr: `altgr+…` queries now predict the level-3 text of the active layout (level 4 with Shift) and send it as UTF-8 in both legacy and Kitty encodings, instead of encoding AltGr as the Kitty Meta modifier. This applies to the named terminal adapters and to the generic fallback for unknown terminals. A dead key (for example `dead_acute` on `us altgr-intl`) is reported as consumed by the terminal layer because it starts dead-key composition and sends no bytes. When the key has no level 3, no keymap is available, the keysym has no known text, Kitty report-all-keys mode is active, or AltGr is combined with Ctrl, Alt, or Super, the encoding is reported as unknown with the reason. The keymap comes from Hyprland's devices JSON or the `WHYKEY_XKB_*`/`XKB_DEFAULT_*` environment.
- Hyprland: when `hyprctl binds -j` reports a bind with an empty key and keycode 0, whykey now recovers its key from the text `hyprctl binds` listing, joining on dispatcher, argument, modifiers, submap, and description. `whykey super+1` on an Omarchy Lua config now resolves to "Switch to workspace 1" instead of a possible match. Multi-key and ambiguous entries stay unresolved.
- Hyprland: `bindings` and `conflicts` show a `code:N` bind as the key it produces in the active XKB layout, with the certainty note "code:N shown as its key in the active XKB layout".
- Hyprland: `source =` paths with `*` and `?` globs are followed in sorted order, as glob(3) does; a leading dot matches only explicitly. Bracket classes (`[…]`) are not expanded.
- Shell completions for Bash, Zsh, and Fish are generated from the same command and option table as `--help` and the usage errors. `--ndjson` is completed only for `listen`, and `replay` and `diff` complete file names.

### Fixes

- XKB: the keymap parser now reads `xkb_types` and each key's type. CapsLock affects only key types that map Lock, and Shift+AltGr selects level 4 on four-level types. Before this, CapsLock was applied to every key and Shift+AltGr never reached level 4. The listener's shifted-keysym evidence follows the same rules.
- Hyprland: the keycode reported by the native Lua capture hook is confirmed to be XKB (evdev + 8) from Hyprland 0.56.2's source, so whykey's single evdev conversion is correct.
- XKB: `symbols[GroupN]=` lines, which libxkbcommon 1.7 and older write, are now read into the right layout group. Before this, every group of a multi-layout keymap on those versions was read as group 1, so the last layout won.
- kanata and xremap configurations nested more than 64 levels deep are reported as not evaluated. Before this, such a file aborted every whykey command with a stack overflow, because remappers are detected at startup.
- Hyprland: `source` glob matching no longer backtracks exponentially; its time is bounded by the pattern length times the file-name length, so a pattern with many `*` can no longer stall whykey. Hyprland configuration, `source`, and Lua files are read only if they are regular files of at most 1 MiB.
- Configuration files are now read only when they are regular files, and reads stop at the size limit. A FIFO or device named as a configuration file, for example as the running sxhkd's `-c` argument, can no longer hang whykey; sxhkd reports such paths as unreadable.
- The ten-second diagnostic budget is now enforced inside every external command: a command's timeout is cut to the remaining budget, and once the budget is spent no further command starts.
- `--json-v2` with `--schema-version 1`, or two different `--schema-version` values, is now a usage error (exit status 2) instead of one silently winning.
- `whykey doctor` separates four evdev causes, each with its own detail and next check: `/dev/input` missing (`not-installed`), `/dev/input` not listable (`permission-missing`), no keyboard devices (`api-does-not-expose`), and keyboards that cannot be opened (`permission-missing`). Doctor JSON gains `evdev.detail`.
- evdev: keyboards whose sysfs key bitmap spans several words were classified as non-keyboards, because the bitmap was read least-significant word first. They are now found by `listen --evdev` and `doctor`.
- sxhkd: whykey no longer follows `include`/`source` lines, which sxhkd does not have. It reads `SXHKD_CONFIG`, otherwise the `-c` file and extra configuration files from the running sxhkd's command line (relative to its working directory), otherwise the default `sxhkdrc`. Each file is parsed on its own.
- i3: a `bindsym --release` binding is now reported as consuming the key, because i3 grabs the key for every binding and never replays the press (checked against i3's `bindings.c` and `key_press.c`). It was reported as uncertain.
- Bash/Readline: without a `shell-init` snapshot, a default prefix key and the "unbound key after a prefix" answer now carry the `UnverifiedDefault` reason.
- `whykey-vscode`: the resolver follows VS Code's own rules. User entries override defaults, the last entry without a `when` clause wins, and a `when` clause makes the answer conditional and names the clause. A key missing from the user file is uncertain, because VS Code's default keybindings are not inspected. `-command` removes only default bindings, a chord prefix consumes the key, an empty command passes it on, and `^command` runs and passes it on. Key parsing follows VS Code's parser (`cmd`/`win`/`meta` are Super, `[KeyA]` scan codes, multi-chord keys). `CODE:N` and AltGr queries are uncertain. JSONC comments are stripped only outside strings. `$XDG_CONFIG_HOME` is searched first, plus Code - Insiders and Flatpak paths. A User directory without `keybindings.json` is uncertain rather than unavailable. Profile keybindings or several VS Code builds make the answer uncertain.
- `whykey-nvim`: Lua-callback mappings (empty rhs, including which-key triggers) were reported as not handled; they now consume the key. `<Plug>` chains are followed, a key that only starts longer mappings is reported as a prefix that waits `timeoutlen`, and the mode comes from `nvim_get_mode`. No mapping is uncertain, because built-in commands and `vim.on_key` handlers are not inspected. Terminal mode, prompts, and timeouts are uncertain. Default `nvim.<pid>.0` sockets are discovered, and several servers make the answer uncertain.
- `whykey-emacs`: the query runs in the selected window's buffer, so text-property and overlay maps count, and the winning map names global, major, minor (mode), or overriding maps. A prefix key reports that Emacs waits for the next key. Shift translation and GUI function-key events (`<return>` before `RET`) are followed. No binding is uncertain, because translation maps and hooks are not inspected. A timeout or evaluation error is uncertain instead of "No running Emacs daemon".
- Hyprland: `__lua` and plugin dispatchers are now treated like any other consuming bind; the report says the key is consumed unless the Lua callback explicitly returns `pass_event`. Previously these binds were reported as opaque and their forwarding as unknown.
- Hyprland: bind keys that use keysym names (`comma`, `slash`, `minus`, `Prior`, …) now match the normalized query key.
- Hyprland: Lua binds that Hyprland reports with an empty key and keycode 0, and that the text `hyprctl binds` listing cannot resolve, get their key from a matching literal Lua `code:` hint. When no hint exists, binds with the query's modifiers are reported as a possible match ("Hyprland may handle this: …") instead of being ignored. JSON v2 marks them with the `UnresolvedBindingKey` reason, and `bindings --key` lists them under `possible_matches`.
- Hyprland: `ignore_mods` comes only from the matching bind (runtime field, then the Lua or `.conf` hint for the exact modifiers and key). A different bind on the same key no longer applies. `.conf` flags are read from the `bind` keyword suffix (`bindi`, `bindln`, …). `.conf` `unbind` and Lua `hl.unbind` are applied in file order, and variables resolve using the longest matching name.
- `whykey conflicts`: binds whose key the source does not report are no longer grouped together; the result counts them as a limitation. Duplicate Hyprland binds on the same key are classified as firing together, because Hyprland runs every matching bind unless a `submap` bind stops the later ones.
- Bash/Readline: the built-in default table now matches GNU Readline's emacs map. `C-x` and `ESC` are prefix keys, `C-l` is `clear-screen`, `C-]` is `character-search`, and `complete-into-braces` is on `M-{`. A live `shell-init` snapshot is now the only source when present.
- Shells: Bash, Zsh, and Fish answers built from defaults now carry the `UnverifiedDefault` reason. The headline says the default binds the key but your shell configuration may override it, and the report suggests the matching `eval "$(whykey shell-init …)"` command.
- Bash/Readline: `bind -x` entries are labelled `shell command (bind -x): NAME` instead of as macros.
- Terminal byte prediction: Ctrl with `@ [ \ ] ^ _` or Space now yields the matching control bytes (0x00, 0x1b to 0x1f). Ctrl+Alt adds an ESC prefix. The Kitty, Alacritty, Foot, WezTerm and Konsole adapters now predict the normal bytes for unbound keys, and reports point out bytes that are shared with another key (for example, Ctrl+[ and Escape). `TERM=xterm-kitty` is recognised as Kitty.
- TTY: Ctrl+S and Ctrl+Q are labelled `terminal flow control (IXON)` and are reported only while IXON is set. Signal keys are labelled `terminal signal handling`.
- Sequences: `whykey inspect` now follows prefix state across steps. This covers multi-key Readline bindings (`ctrl+x ctrl+e`), tmux prefix and `switch-client -T` tables, and the GNU Screen command escape. A bare key is looked up only in tmux's root or active table. A key with no binding after the tmux prefix is reported as discarded.
- Key input: unknown key names are rejected with exit status 2, and a close match is suggested (`ctrl+retrun` → `return`). `altgr` and `iso_level3_shift` are accepted as aliases for Mod5, and human output shows Mod5 as `AltGr`.
- Reports: a layer is described as "has a matching key" only when it reported binding evidence. When nothing matches, the headline says so and names the layers whose handling is unknown. Pass-through rows without a binding are shown only with `--verbose`.
- Input methods: an inactive IME, or one whose engine is a plain keyboard layout, is reported as passing keys through (`not_handled`/`continues`) rather than indeterminate. The Input method row now comes right after the Terminal row.
- Sway and i3: the adapters requested a `get_bindings` IPC message that does not exist, so they were always unavailable. They now parse the running configuration returned by `get_config`, including modes, variables, `bindsym`/`bindcode`/`unbind*` flags and includes, and they match against the mode reported by `get_binding_state`. A `nop` binding is reported as grabbing the key.
- `listen` on Sway now keeps the modifiers from binding events.
- `listen --suppress` exits with status 2 when combined with `--terminal`, `--evdev` or `--device`. It fails, rather than falling back to terminal capture, when native suppression is unavailable. Error hints now name `--no-suppress`; `--pass-through` is still accepted as an alias.
- `listen` now restores the terminal (and removes any leased Hyprland hook) before stopping on Ctrl+Z/SIGTSTP, and re-arms on resume. The signal guard covers the whole session, so SIGTERM/SIGHUP can no longer leave the terminal in raw mode. Evdev capture keeps `--events all` after re-arming.
- Hyprland pass-through `listen` hooks now carry the same 5-second self-removing lease as suppression. The hook is removed before each report and re-armed afterwards.
- Inspection now respects a remapper result that consumes or redirects a key: a consume stops the route, and a redirect re-targets later layers or makes them conditional.
- When several compositor extension manifests apply, only the highest-scored one runs its bindings command.
- The CLI test suite no longer inherits host terminal and compositor environment variables.

### Packaging and documentation

- Declare `python3` for the installed `whykey-nvim`, `whykey-vscode`, and `whykey-emacs` extensions: Debian and RPM `Recommends`, Arch `optdepends`. The Nix recipe adds `python3` so the extension shebangs are patched to the store path.
- README, SPEC, and the man page now say exactly what `listen` changes at runtime, which compositors are read through live IPC and which from parsed configuration, and that Bash/Zsh/Fish answers need `shell-init` to be exact.
- Correct the L-010 size figures and state how they were measured.
- Add end-to-end PTY tests for `listen` job control: `SIGTSTP` stops the job with the terminal restored, `SIGCONT` re-arms capture, and `SIGTERM` restores the terminal. The reference terminal settings are captured right after the PTY is opened, before whykey can enter raw mode.
- `DEPENDENCY_PROVENANCE.md` records the vendored keysym-to-Unicode table (`src/xkb_keysyms.txt`, derived from X.Org's `keysymdef.h`) and its license.

## [1.3.0] - 2026-09-11

### Changed

- Redesign human-readable CLI output around a clear diagnosis, practical effect, relevant layers, and explicit uncertainty.
- Add shared terminal styling, `--color=auto|always|never`, narrow-width wrapping, and a focused default view with complete `--verbose` evidence.
- Polish listen-mode status messages and repeat separators while keeping machine-readable output plain and schema-compatible.

## [1.2.2] - 2026-09-11

### Changed

- Complete the post-1.0 refactoring batch, including CLI dispatch separation, adapter boundary documentation, IME and XKB evidence limits, herbstluftwm support, bounded include resolution, and measured performance follow-up.
- Keep local planning notes out of the packaged source tree.

## [1.2.1] - 2026-09-11

### Packaging and testing

- Install first-party extensions (`whykey-nvim`, `whykey-vscode`, `whykey-emacs`) across Arch PKGBUILD, RPM spec, Debian rules, and Nix recipes, verified by version scripts and package smoke gates.
- Add hermetic extension test fixtures and wire `extensions/tests/run_all.py` into CI.
- Compact CLI test boilerplate and table-drive extension scenarios with explicit assertion labels.

## [1.2.0] - 2026-09-11

### Added

- Ship `extensions/whykey-vscode`, a stdlib-only first-party extension querying user `keybindings.json` across Code, Code - OSS, and VSCodium. Supports comment stripping, chord-prefix matching, `when`-clause triage, and unbind removals (`"command": "-id"`). Product defaults are explicitly not evaluated.
- Ship `extensions/whykey-emacs`, a stdlib-only first-party extension querying the running Emacs daemon in a single `--eval` to return the active keybinding stack and winning map (`global`, `major`, or `minor`). Reports remediation guidance when no daemon is reachable.

## [1.1.0] - 2026-09-11

### Added

- Ship `extensions/whykey-nvim`, a stdlib-only first-party extension answering `whykey extension` from live Neovim runtime state (`mode()` + `maparg()`) over extension protocol v1. Reports `unavailable` without a `--listen` server and is never auto-invoked.
- Add a read-only `doctor` check reporting `whykey-nvim` availability and Neovim server reachability without executing anything.
- Document the first-party extension contract (install, one-command usage, limits) in README, SPEC, and the man page.
- Record simplification epic closure at ~21k SLOC (L-010): residual gap accepted; capture lifecycle, platform stubs, and strict typing deliberately retained. (Corrected: ~21k counts non-blank, non-comment lines in `src/**/*.rs` excluding `#[cfg(test)]` items, which was 20,693 at this release. The same tree is 30,242 physical lines including inline unit tests. See L-010.)

## [1.0.2] - 2026-09-11

### Changed

- Separate inspection requests, route outcomes, continuation policy, and endpoint selection into typed internal contracts.
- Build layer results through shared constructors and keep report conclusions independent of display wording.
- Preserve the existing JSON contracts for unadapted applications while adding typed modifier ambiguity to schema v2 binding evidence.

### Validation

- Add sanitized differential fixtures and compare 49 deterministic CLI, replay, diff, and inspection scenarios against the pre-refactor baseline.
- Record and verify the baseline identity, reject accidental self-comparison, and fail closed on invalid baseline manifests.

## [1.0.1] - 2026-09-09

### Fixes

- Isolate compositor-mutating Hyprland tests behind `--ignored`, `WHYKEY_RUN_LIVE_TESTS=1`, and an explicit test instance signature; the ordinary suite never touches the compositor.
- Separate evdev and XKB keycodes with typed conversions; a captured evdev code converts once and never matches a raw XKB number.
- Model capture as an explicit `CaptureState`: arming requires dispatcher success, the observed submap postcondition, and the token-specific armed event; cleanup retries twice and only reaches `Closed` after verifying the restored submap.
- Decide universal scope, action wording, and dispatcher uncertainty from typed binding evidence instead of parsing detail text; opaque dispatchers are never reported as executed.
- Keep normal reports to one screen behind `--verbose` for raw events, modifier state, XKB internals, inventory, inactive submaps, and alternate keys; JSON keeps the full evidence.
- Prevent a stale Hyprland capture cleanup from restoring another listener's submap.
- Treat an unconfirmed chord release as a timeout and suppress the normal report.
- Make replay v1-to-v2 conversion independent of the machine that renders it.
- Compare snapshot/report sequences and repeated route layers without collapsing entries.
- Redact shell identity, private paths, and secret-bearing assignments from newly created snapshots.

### Features

- Add `--device` and `--submap` filters to `whykey bindings` and `whykey conflicts`, backed by typed adapter metadata.

### Compatibility and packaging

- `whykey diff --json --schema-version 2` now emits a schema-v2 envelope.
- Validate the declared Rust 1.85 MSRV with Clippy and make CI derive source archive names from Cargo metadata.

## [1.0.0] - 2026-09-07

First general-availability release of Whykey following release-candidate verification.

### User-visible behavior

- `whykey <combination>`: inspects a single shortcut combination across all layers.
- `whykey inspect <combination-or-sequence>`: analyzes single keys or sequences up to 64 steps, with optional `--pid <pid>`, `--focused` window target resolution, and `--instance <id>` for Hyprland multi-instance selection.
- `whykey listen`: captures real-time terminal key events (including Kitty keyboard protocol disambiguation) or pre-compositor physical Linux events (`--evdev`), supporting `--repeat`, `--timeout`, `--count`, `--events all`, `--output`, and line-delimited schema-v2 records (`--ndjson`).
- `whykey doctor`: verifies read-only availability of controlling TTY, compositor sessions, terminal integrations, XKB tools, shell snapshots, SSH, multiplexers, and input-method daemons.
- `whykey capabilities`: reports the runtime availability and support matrix of all system integrations.
- `whykey bindings`: dumps effective desktop bindings in a unified JSON inventory.
- `whykey conflicts`: identifies duplicate shortcut collisions within the same session context.
- `whykey replay`: re-renders stored JSON reports without re-injecting input.
- `whykey shell-init`: provides shell wrappers for Bash, Zsh, and Fish to capture runtime editor modes and keymaps.
- `whykey completions`: generates shell completion scripts for Bash, Zsh, and Fish.

### Compatibility

- Default output uses schema version 1 for backwards compatibility. Schema version 2 is available via `--schema-version 2` or `--json-v2` with structured evidence, session context, and typed operation envelopes.
- Extension interface remains on protocol v1 (`whykey extension <program> <combination>`).
- Minimum Supported Rust Version (MSRV): Rust 1.85.0 (2024 edition).

### Supported adapters

- **Compositors & Window Managers**: Hyprland, Sway, i3, KDE Plasma, XFCE, Cinnamon, MATE, Niri, River, Wayfire, Labwc, bspwm (sxhkd), Openbox, X11 (`xbindkeys`), AwesomeWM, Qtile, XMonad, and generic Wayland/X11 compositors.
- **Terminals**: Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, and generic terminal emulators.
- **Multiplexers & Sessions**: tmux, GNU Screen, Zellij, and SSH remote sessions.
- **Shells & Line Editors**: Readline (Bash), ZLE (Zsh), Fish.
- **Input Methods & Remappers**: Fcitx5 (read-only D-Bus), IBus; keyd, kanata, input-remapper, xremap.
- **Interactive Applications**: Neovim, Vim, Emacs, Helix, Kakoune, Micro, VS Code / VSCodium, JetBrains IDEs.

### Known limitations

- Multi-instance Hyprland ambiguity resolution relies on active Wayland socket identity when available.
- Hyprland Lua binding scanning is literal-only and does not evaluate dynamic Lua expressions.
- Layout mapping relies on active XKB group and symbols; layout transformations remain conditional when xkbcli or keymap state is inaccessible.
- Terminal Kitty protocol negotiation handles known flags; unsupported responses fall back to inferred legacy byte streams.
- Focused-window resolution relies on compositor-exported tree/activewindow APIs (Hyprland, Sway, i3); desktop environments without read-only window PID APIs report focus discovery as unsupported.
- Generated configuration files and dynamic includes across complex desktop/editor setups are evaluated best-effort from standard XDG paths.
