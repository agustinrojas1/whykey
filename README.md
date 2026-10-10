# whykey

[![CI](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml/badge.svg)](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml)

`whykey` tells you what happens to a key combination on Linux: which program
takes it, and whether the key continues to the next program in line
(remapper, compositor or window manager, terminal, input method, TTY,
multiplexer, application, shell).

It gets its answer in three steps. Each answer is labelled with the step that
produced it.

1. **Observe.** `whykey listen` watches the key itself.
2. **Ask the running program.** Compositor IPC, GNOME's settings, KDE's
   kglobalaccel, the X server's key grabs, tmux, and your shell's live
   bindings.
3. **Read configuration files.** This is the fallback when a program cannot
   be asked. Such answers are marked `config file (static)` or
   `default (unverified)`, because the running program may disagree.

whykey never edits configuration and never runs the shortcut it inspects.

```console
$ whykey super+t
Super+T · Inspect

Sway is configured to consume the shortcut.

Effect
  The configured consumer stops this shortcut before later layers can handle it.

Relevant layers (configuration)
  Sway — Configured to stop: exec touch /tmp/fired [live query]
      action exec touch /tmp/fired; source /root/.config/sway/config:1

Technical detail: use --verbose for the complete route and raw evidence.
```

(Output from the lab's headless Sway, where `Mod4+t` runs `touch /tmp/fired`.)

## 1. Observe: `whykey listen`

`listen` waits for one key and uses the first capture method that works:

| Order | Backend | What it proves |
| --- | --- | --- |
| 1 | Native compositor capture: Hyprland (Lua hook; needs a `hyprland.lua` config) or Sway (binding events over IPC) | The compositor received the key. Sway reports only keys that ran a Sway binding. |
| 2 | Probe window: a small focused Wayland or X11 window titled "whykey: press a shortcut here (Esc exits)". Skipped in SSH sessions. | Whether the desktop delivered the key to a normal window. If only modifier changes arrive, something above the window (compositor, window manager, hotkey daemon, or input method) probably consumed the key. |
| 3 | Terminal capture | The key reached this terminal, and the bytes it arrived as. Keys consumed earlier never show up here. |

`--window`, `--terminal`, and `--evdev [--device PATH]` force one backend.
`whykey listen --dry-run` prints the chain and the reason for each choice
without capturing anything.

The probe window can only infer "not delivered" from what it does not see.
Pressing and releasing Super alone looks exactly like Super+T swallowed by
the desktop, so a "not delivered" verdict is reported as conditional, not
confirmed. The window also cannot tell which program took the key; the
inspection that follows names the likely owner. Over SSH, the display
belongs to the remote side, so the default chain skips the probe window;
`--window` still forces it.

On Hyprland, native capture installs a temporary Lua hook. By default it
only observes. `--suppress` adds a private submap so the shortcut does not
fire. Both are held by a 5-second lease that whykey renews while listening.
Hyprland drops them by itself if the lease expires, for example after
`kill -9`. With a `hyprland.conf` config, Hyprland rejects the hook
(`hyprctl eval` needs the Lua config manager), and `listen` falls back to the
probe window and says why. `--suppress` never falls back, and only Hyprland
supports it.

`whykey listen --evdev` reads the kernel's view of the keyboard, before any
remapper or compositor. It is the only backend that needs extra permission.
Press Escape or Ctrl+C to stop listening.

## 2. Ask the running program

Running `whykey KEY` (or `whykey inspect KEY...`) queries what is running
now:

- Hyprland: `hyprctl binds -j`, submap and devices.
- Sway and i3: the running config from `get_config` and the mode from
  `get_binding_state`.
- X11 (any window manager or hotkey daemon): whykey asks the X server whether
  another client holds a passive grab for the key on the root window. The X
  server refuses the grab (`BadAccess`) when another client holds it. The
  probe takes the grab for a moment and releases it at once. A free key
  means only that no client holds a root-window grab for it. The probe
  cannot name the client that holds a grab.
- GNOME: `gsettings` over the mutter, GNOME Shell, window-manager, media-key,
  and custom keybinding schemas.
- KDE Plasma: the running kglobalaccel over D-Bus (`globalShortcutsByKey`
  and the component's `isActive`). whykey never starts the daemon.
- Bash, zsh, and fish: live bindings, when you load the wrapper (see
  [Shell integration](#shell-integration)). The shell is found by walking up
  the process tree, not from `$SHELL`.
- tmux (`list-keys`), herbstluftwm (`herbstclient`), Xfce (`xfconf-query`),
  Cinnamon and MATE (`gsettings`), and WezTerm (`wezterm show-keys`).

## 3. Fall back to configuration files

Niri, River, Wayfire, labwc, dwl, Openbox, sxhkd, xbindkeys, AwesomeWM, Qtile,
XMonad, the remappers (keyd, kanata, kmonad, input-remapper, xremap), most
terminals, editors, and GNU Screen are read from their configuration files.
On X11 the grab probe above still checks whether something holds the key.
A file says what is configured, not what is loaded. Plugins, generated files,
and executable configs (Lua, Python, Haskell) are never evaluated, so
these answers stay conditional.

## Evidence tiers

Every layer row in the report carries a tier, and JSON v2/NDJSON path
entries carry it as `tier`:

| Tier | JSON | Meaning |
| --- | --- | --- |
| observed | `observed` | A capture saw the key pass (or stop at) this layer. |
| live query | `live_query` | The running program answered. |
| config file (static) | `config_file` | Parsed from configuration files; the running program may differ. |
| default (unverified) | `default` | Built-in defaults; your configuration may override them. |

A row with no tier means nothing answered (an unavailable integration).
Schema v1 JSON is unchanged.

## Verified against reality

Fixture tests only show that whykey agrees with its own authors. Support
claims here come from the [lab](lab/README.md): real programs in containers
receive real synthetic input, and their reaction is the ground truth (a bind
ran a command, a key arrived at an independent test client). Each scenario
also needs a positive control: the probe must have seen a key, or the bind
must be shown able to fire, so a dead setup cannot pass by default. whykey's
verdict must match the ground truth. The current run has 125 scenarios: 111
match exactly, 14 pass only as honest uncertainty (see below), none fail,
and none are lab errors. The per-capability list is in the
[support matrix](SUPPORT_MATRIX.md).

**Lab-verified** (Fedora 44 image):

| Environment | Version | Scenarios |
| --- | --- | --- |
| Hyprland, `hyprland.conf` and `hyprland.lua`, in a KVM guest | 0.56.2 | 22, plus 2 probe-window |
| Sway (headless) | 1.11 | 14, plus 2 probe-window |
| i3 on Xvfb | 4.25.1 | 12 (5 with `DISPLAY` unset, to check the adapter without the X11 grab probe), plus 3 probe-window (one is a lone Super tap) |
| KDE kglobalacceld on Xvfb (daemon only, no Plasma session) | 6.7.5 | 6 (3 with `DISPLAY` unset) |
| GNOME: headless mutter (mutter-owned bindings only; GNOME Shell cannot start in the container, so Shell and gnome-settings-daemon bindings are not verified) | mutter 50.5 | 4 |
| sxhkd on Xvfb (replay `~`, release `@`, braces, ranges, plain keys, continuations) | 0.6.1 | 11 |
| Openbox, AwesomeWM, xbindkeys on Xvfb | 3.6.1, 4.3, 1.8.7 | 2 each |
| bash, zsh, fish in a pty | 5.3.9, 5.9, 4.6.0 | 27 (14 with `shell-init`, 13 without) |
| tmux and GNU Screen, with bash, zsh, or fish inside | 3.7c, 5.0.1 | 16 |

Without `shell-init`, the 13 shell scenarios pass only as honest
uncertainty: whykey says the shell probably handles the key, unverified.
`tmux-root-alt-escape` also passes only as uncertainty: the generic terminal
fallback has no Alt+Escape encoding, so whykey cannot tell which bytes tmux
receives. These 14 do not count as verification.

**Live-verified by hand on the maintainer's desktop:** static `whykey KEY`
inspection on Hyprland 0.56.2, bash with `shell-init`, and the Neovim
extension.

**Not verified against the real program:** Niri, River, Wayfire, labwc, dwl,
Qtile, XMonad, herbstluftwm, Xfce, Cinnamon, MATE, a full GNOME Shell or
Plasma session, the remappers (keyd, kanata, kmonad, input-remapper,
xremap), IBus and Fcitx5, the terminal emulator adapters (Ghostty, Kitty,
Alacritty, foot, WezTerm, Konsole; the lab's shells run in a bare pty),
Zellij, Emacs, Vim, Helix, Kakoune, Micro, JetBrains IDEs, and VS Code
(whose default keybindings are not inspected). `listen --evdev`, Sway native
capture, Hyprland `--suppress`, and capture of a physical keyboard on a real
desktop have no lab scenario. These
adapters work from documentation, upstream source, and fixtures. Treat
their answers as informed guesses.

## Permissions

The probe window, terminal capture, the X11 grab probe, and every query
above need no extra permission. `listen --evdev` needs read access to
`/dev/input/event*`. `whykey doctor` names the keyboards it cannot open and
prints a temporary grant for exactly those devices:

```sh
sudo setfacl -m u:"$USER":r '/dev/input/eventN'   # gone after replug or reboot
```

Joining the `input` group also works, but it grants permanent access to
every input device.

## Install

```sh
mise use github:agustinrojas1/whykey@2.1.0
# or, with Rust 1.85 or newer:
cargo install --git https://github.com/agustinrojas1/whykey --locked
```

Pin a version with mise: the exact `@2.1.0`, or the major `@2`. By default
mise hides releases younger than 24 hours (`minimum_release_age`), so an
unpinned `github:agustinrojas1/whykey` can silently resolve to an older major
release right after a new one is published. During that first day `@2` fails
instead; an exact version such as `@2.1.0` installs at once.

Each release publishes these binaries, each archive with a `.sha256` file:

| Asset | Runs on |
| --- | --- |
| `whykey-<version>-x86_64-unknown-linux-gnu.tar.gz` | x86_64 with glibc 2.28 or newer (Debian 10, Ubuntu 18.10, RHEL 8 and later) |
| `whykey-<version>-x86_64-unknown-linux-musl.tar.gz` | any x86_64 Linux: fully static, no libc needed (Alpine, older glibc) |
| `whykey-<version>-aarch64-unknown-linux-musl.tar.gz` | any aarch64 Linux: fully static |

On glibc systems mise picks the `gnu` archive for x86_64; aarch64 only has
the static `musl` archive.

whykey builds only on Linux; other targets fail with an explicit message.
Arch, Debian, RPM, and Nix recipes are in [`packaging/`](packaging/README.md).
Only the Arch recipe has a package smoke test (L-008 in the
[defect register](DEFECT_REGISTER.md)).

The editor extensions (`whykey-nvim`, `whykey-vscode`, `whykey-emacs`) are
`python3` scripts. The distribution packages install them next to `whykey`.
mise and `cargo install` do not put them on `PATH`. With mise they are in
`"$(mise where github:agustinrojas1/whykey)/extensions"`; with Cargo, take
them from the repository's `extensions/` directory. Add that directory to
`PATH` or link the scripts into a directory that is already on it.

## Usage

```sh
whykey ctrl+left                    # inspect one combination
whykey inspect ctrl+x ctrl+e        # a sequence; prefix state carries over
whykey inspect --focused ctrl+z     # use the focused window's process
whykey listen                       # observe the next shortcut
whykey listen --window --repeat     # probe window, one shortcut at a time
whykey listen --dry-run             # show the capture chain only
whykey doctor                       # what whykey can observe and ask here
whykey bindings --key ctrl+x        # list matching desktop bindings
whykey conflicts --source hyprland  # find duplicate bindings
whykey snapshot ctrl+super+return --output snap.json
whykey replay snap.json
whykey diff before.json after.json
whykey extension extensions/whykey-nvim ctrl+x   # ask a running Neovim
```

Join modifiers and the key with `+`. Modifiers: `ctrl`, `alt`, `shift`,
`super`, `hyper`, `altgr` (Mod5), `caps`, `num`. A key is a character, a
named key (`return`, `left`, `comma`, `f1`), an XF86 media key, or `code:N`.
Keypad keys are their own keys: `kp_enter` is not `return`, and `kp_1`,
`kp_home`, `kp_add` and the other `kp_` names are not the main keys.
Unknown names exit with status 2 and suggest the closest valid name.

Reports open with a diagnosis, its effect, the relevant layers with their
tiers, what remains uncertain, and a "Next" step (for example, run
`whykey listen` or load `shell-init`). `--verbose` shows the full route and
raw evidence. `--json` emits schema v1; `--json-v2` adds context, evidence,
and tiers. `whykey doctor` lists the observation taps available now and,
for each layer, whether its answer will come from a live query, a config
file, or nothing.

`whykey --help` lists every command and option. The
[specification](SPEC.md) and the [man page](whykey.1) cover the details, and
[ADAPTER_INVENTORY.md](ADAPTER_INVENTORY.md) gives the sources and limits of
each adapter.

## Shell integration

```sh
eval "$(whykey shell-init bash)"
# eval "$(whykey shell-init zsh)"
# whykey shell-init fish | source
```

The wrapper passes the shell's live bindings to whykey (`bind -p/-S/-X/-V`,
`bindkey -lL main; bindkey -L; bindkey -LM vicmd`, or
`bind --all | string collect` plus `$fish_bind_mode`). Without it, whykey
asks the installed shell for its defaults (`zsh -f`, `fish --no-config`) and
applies the `bind`/`bindkey` lines it finds in your startup files. Those
answers are marked unverified, because plugins and tools such as fzf rebind
keys at runtime. Completion scripts: `whykey completions bash|zsh|fish`.

## Development

CI runs:

```sh
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
python3 extensions/tests/run_all.py
python3 tools/generate_support_matrix.py --check
python3 tools/generate_dependency_provenance.py --check
tools/check_package_versions.sh
```

The lab runs on pull requests (`.github/workflows/lab.yml`). To run it locally:

```sh
docker build -t whykey-lab lab/
cargo build --release --locked
python3 lab/run.py -j 12
```

See [lab/README.md](lab/README.md) for the scenario format and the KVM
requirement of the Hyprland scenarios.

Live Hyprland tests change the compositor's state, so they never run in the
normal suite. They need `--ignored`, `WHYKEY_RUN_LIVE_TESTS=1`, and
`WHYKEY_LIVE_INSTANCE_SIGNATURE` equal to `HYPRLAND_INSTANCE_SIGNATURE`. Run
them only in a dedicated session:

```sh
WHYKEY_RUN_LIVE_TESTS=1 WHYKEY_LIVE_INSTANCE_SIGNATURE="$HYPRLAND_INSTANCE_SIGNATURE" \
cargo test --test hyprland_live -- --ignored
```

Report reproducible gaps in the [defect register](DEFECT_REGISTER.md).

## License

[MIT](LICENSE)
