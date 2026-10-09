# whykey

[![CI](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml/badge.svg)](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml)

`whykey` explains what handles a Linux keyboard shortcut and whether the key
continues to the next layer.

It inspects the active compositor, terminal, TTY, multiplexer, application,
and shell. It never edits configuration files and never runs the shortcut it
inspects. Two things do touch your session:

- `whykey listen` on Hyprland installs a temporary Lua hook while it runs.
  The default pass-through hook only observes keys. `--suppress` adds a private
  submap so the shortcut does not fire. Both are held by a 5-second lease that
  whykey renews while listening. They are removed when `listen` exits, and
  Hyprland drops them on its own if the lease expires (for example, after
  `kill -9`).
- Compositor extension manifests and `whykey extension` run the programs you
  configured or named.

Native compositor capture is available on Hyprland (pass-through or
`--suppress`) and Sway (pass-through only).

```console
$ whykey ctrl+z
Ctrl+Z · Inspect

TTY is configured to suspend the foreground job.
This applies if earlier layers forward the key.

Effect
  Terminal signal handling sends a stop signal to the foreground job when this
  control byte arrives.

Relevant layers (configuration)
  Hyprland — Unavailable: compositor binding could not be inspected
  TTY driver — Configured to stop: terminal signal handling

Next
  Run `whykey listen` to observe whether the shortcut reaches the configured
  consumer.

Technical detail: use --verbose for the complete route and raw evidence.
```

## Install

### Using mise

```sh
mise use github:agustinrojas1/whykey
```

### Build from source

Whykey requires Linux and Rust 1.85 or newer:

```sh
cargo install --git https://github.com/agustinrojas1/whykey --locked
```

Whykey is intentionally a Linux-only binary. Its diagnostic contract includes
Linux evdev, PTY/termios, XDG session discovery, and Linux desktop IPC; the
project does not promise Windows or macOS support. Non-Linux builds fail with
an explicit platform message rather than producing a partially functional
binary.

Arch, Debian, RPM, and Nix packaging recipes are available in
[`packaging/`](packaging/).

### Distribution packages

Arch, Debian, RPM, and Nix recipes are maintained in-tree. Their public
publication follows the first release; see [`packaging/README.md`](packaging/README.md).
Only the Arch recipe has a package smoke test. The Debian, RPM, and Nix
recipes are not built in CI (see L-008 in the [defect register](DEFECT_REGISTER.md)).
The bundled editor extensions need `python3`.

## Usage

```sh
whykey ctrl+left                    # inspect one combination
whykey inspect ctrl+x ctrl+s        # inspect a sequence
whykey inspect --focused ctrl+z     # inspect the focused application
whykey listen                       # explain the next shortcut (pass-through by default)
whykey listen --suppress            # opt into temporary compositor suppression
whykey listen --repeat              # inspect one deliberate shortcut at a time
whykey listen --terminal            # force terminal-only capture
whykey listen --evdev               # observe a physical Linux input device
whykey doctor                       # check available integrations
whykey bindings --key ctrl+x        # list matching desktop bindings
whykey bindings --submap gaming    # list bindings in one submap
whykey conflicts --source hyprland  # find matching conflicts
whykey snapshot ctrl+super+return --output whykey-snapshot.json
whykey diff before.json after.json
whykey extension extensions/whykey-nvim ctrl+x # query live Neovim runtime via RPC
whykey extension extensions/whykey-vscode ctrl+x # query user VS Code keybindings.json
whykey extension extensions/whykey-emacs ctrl+x # query live Emacs keymap stack via emacsclient
```

Join modifiers and the key with `+`. Modifiers are `ctrl`, `alt`, `shift`,
`super`, `hyper`, `altgr` (Mod5), `caps`, and `num`. A key is a single
character, a named key such as `return`, `left`, `comma`, or `f1`, an XF86
media key, or `code:N`. Unknown names exit with status 2 and suggest the
closest valid name. `inspect` with several steps keeps prefix state between
them, so `ctrl+x ctrl+e` resolves a Readline two-key binding, and a tmux or
Screen prefix applies to the key that follows it.

`whykey doctor --json` includes a cause taxonomy (`not-installed`,
`permission-missing`, `ipc-unreachable`, or `api-does-not-expose`) and a safe
next check for unavailable integrations. It never repairs permissions or
changes compositor configuration.

Normal reports lead with a compact shortcut/operation header, a plain-English
diagnosis, its practical effect, relevant layers, and a specific uncertainty
statement when one matters. Use `--verbose` for the full route with raw events,
probe bytes, encoding internals, full modifier state, XKB candidates, keyboard
inventory, inactive submaps, and alternate keys. Use `--json` for stable schema-v1 output
or `--json-v2` for structured context and evidence; JSON always carries the
full evidence including verbose-only lines. `snapshot` saves a
versioned static report for later offline replay; it never captures or injects
input. New snapshots redact shell identity, private paths, and secret-bearing
assignments in captured evidence. They preserve the observed conclusion for
comparison, not raw IPC or configuration inputs required to re-run every
adapter offline. Inspect a snapshot before sharing it.

Run `whykey --help` for every command and option.

Human output uses `--color=auto` by default. Auto disables color when
`NO_COLOR` is set, stdout is not a terminal, or `TERM=dumb`. `--color=always`
and `--color=never` override those checks. JSON, NDJSON, shell-init, and
completion output never contain terminal styling.

## How it works

Whykey follows a key through the layers that can handle it:

```text
remapper → compositor → terminal → input method → TTY
         → multiplexer → application → shell
```

Each adapter uses read-only IPC or parses a small, known part of the relevant
configuration. Missing tools and dynamic configuration are reported as
uncertain evidence instead of guessed answers.

`whykey listen` captures the next deliberate shortcut to explain its path.
When native compositor capture is available (Hyprland, or Sway IPC), it
observes ordinary compositor bindings without suppressing them; this is the
default. `--no-suppress` states that default explicitly, and `--pass-through`
is an older alias for it. Pass `--suppress` to stop the shortcut from firing
while it is captured. Only Hyprland supports this. `--suppress` fails rather
than falling back to terminal capture when native suppression is unavailable,
and it is rejected together with `--terminal`, `--evdev`, or `--device`.
Because Hyprland's key event does not identify the originating keyboard,
device identity is reported as unavailable.

If native compositor capture is unavailable, Whykey falls back to terminal capture,
observing keys that reach the terminal. Pass `--terminal` to explicitly force
terminal-only capture. Pass `--evdev` to read Linux input events before the
compositor (may require permission to access `/dev/input/event*`). Whykey
never grabs an evdev input device and cleans up temporary native capture hooks
on exit. Press Escape or Ctrl+C to exit. With the Kitty keyboard protocol
active (Kitty, Ghostty, foot, WezTerm), Ctrl+Z is captured and explained like
any other key. On legacy-encoding terminals Ctrl+Z, like an external
`kill -TSTP`, suspends the job: Whykey first restores the terminal and removes
any Hyprland hook, then re-arms when it resumes.
Configuration files are never edited.
When multiple compositor adapters are applicable, Whykey reports their scores
in verbose output and schema-v2 context before selecting the highest-scoring one.

Replay a snapshot without querying the current desktop:

```sh
whykey replay whykey-snapshot.json
```

Compare two saved snapshots without querying the current desktop:

```sh
whykey diff before.json after.json
whykey diff --json before.json after.json
```

## Support

Whykey includes adapters for:

- Compositors read through live IPC or settings queries: Hyprland
  (`hyprctl`), Sway and i3 (the running configuration from `get_config`),
  herbstluftwm (`herbstclient`), GNOME, Cinnamon, and MATE (GSettings), and
  Xfce (`xfconf-query`)
- Compositors read from configuration files only, without runtime activation
  state: KDE Plasma (`kglobalshortcutsrc`), Niri, dwl, River, Wayfire, labwc,
  bspwm with sxhkd, Openbox, X11 xbindkeys, AwesomeWM, Qtile, and XMonad
- Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, and generic terminals
- Bash with Readline, Zsh with ZLE, Fish, tmux, GNU Screen, and Zellij
- Neovim, Vim, Emacs, Helix, Kakoune, Micro, VS Code, and JetBrains IDEs
- Fcitx5, IBus, keyd, Kanata, KMonad, input-remapper, and xremap

Support is best effort where a program does not expose its live keymap.
Generated configuration, plugins, firmware mappings, browser shortcuts, and
focus changes during inspection can remain uncertain.

On Wayland sessions, `whykey doctor` also probes read-only runtime surfaces for
KDE KGlobalAccel, xdg-desktop-portal GlobalShortcuts, and GNOME Shell. These
probes report service reachability and API limits; they never register a
shortcut or claim a binding inventory when the API does not expose one.

### Compositor extension adapters

User-owned manifests in `~/.config/whykey/adapters/*.toml` add static
inspection and binding inventory for a compositor without compiling a new
adapter. `WHYKEY_ADAPTER_DIR` selects another directory. Whykey only runs a
command named by a validated manifest, with the normal two-second and one-MiB
limits. It never scans `PATH` for adapters or evaluates a configuration file.

| Tier | Source | What it can prove |
| --- | --- | --- |
| T1 | live IPC | runtime state exposed by a compiled-in adapter |
| T2 | live query | a bounded read-only query with a known response |
| T3 | literal config | static mappings without runtime activation |
| T4 | manifest extension | user-selected scripts with conditional evidence |

T4 adapters do not capture input. `listen` still uses native Hyprland or Sway,
terminal, or evdev capture. See [the compositor manifest reference](docs/compositor-extensions.md).

See the [support matrix](SUPPORT_MATRIX.md) and
[adapter inventory](ADAPTER_INVENTORY.md) for exact coverage.

## Shell integration

The optional shell wrapper supplies the active Readline, ZLE, or Fish bindings:

```sh
eval "$(whykey shell-init bash)"
# eval "$(whykey shell-init zsh)"
# whykey shell-init fish | source
```

The wrapper is required for exact shell answers. Without it, Bash answers come
from inputrc files and Readline's built-in defaults. Zsh and Fish answers come
from literal bindings in `.zshrc`/`.zshenv` or `config.fish` and common
defaults. A default-based answer is reported as unverified, because startup
files, plugins, and tools like fzf can rebind keys at runtime. The report
then tells you which `shell-init` command to load.

Whykey does not edit shell startup files. Completion scripts are available
through `whykey completions bash|zsh|fish`.

## Development

These are the commands CI runs:

```sh
cargo test --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
python3 extensions/tests/run_all.py
python3 tools/generate_support_matrix.py --check
tools/check_package_versions.sh
```

Live Hyprland tests mutate the compositor and never run in the ordinary
suite. They live in `tests/hyprland_live.rs`, require `--ignored`,
`WHYKEY_RUN_LIVE_TESTS=1`, and an explicit test instance signature
(`WHYKEY_LIVE_INSTANCE_SIGNATURE` equal to `HYPRLAND_INSTANCE_SIGNATURE`),
and fail with a prerequisite message otherwise. Run them only in a dedicated
Hyprland session; each records the original submap and verifies its exact
restoration:
```sh
WHYKEY_RUN_LIVE_TESTS=1 WHYKEY_LIVE_INSTANCE_SIGNATURE="$HYPRLAND_INSTANCE_SIGNATURE" \
cargo test --test hyprland_live -- --ignored
```

The [specification](SPEC.md) documents output and inspection behavior.
Reproducible gaps belong in the [defect register](DEFECT_REGISTER.md).

## License

[MIT](LICENSE)
