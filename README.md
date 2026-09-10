# whykey

[![CI](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml/badge.svg)](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml)

`whykey` explains what handles a Linux keyboard shortcut and whether the key
continues to the next layer.

It inspects the active compositor, terminal, TTY, multiplexer, application,
and shell. It never executes the shortcut or changes your configuration.
Native Hyprland listening temporarily changes the compositor session to capture
the event, then restores the previous submap before reporting it.

```console
$ whykey ctrl+left
Key: CTRL + LEFT

Result:
  ✓ Final handler: Bash / Readline
  Bash / Readline handles and consumes CTRL + LEFT.
```

## Install

### Build from source

Whykey requires Linux and Rust 1.85 or newer:

```sh
cargo install --git https://github.com/agustinrojas1/whykey --locked
```

Arch, Debian, RPM, and Nix packaging recipes are available in
[`packaging/`](packaging/).

### Distribution packages

Arch, Debian, RPM, and Nix recipes are maintained in-tree. Their public
publication follows the first release; see [`packaging/README.md`](packaging/README.md).

## Usage

```sh
whykey ctrl+left                    # inspect one combination
whykey inspect ctrl+x ctrl+s        # inspect a sequence
whykey inspect --focused ctrl+z     # inspect the focused application
whykey listen                       # explain the next shortcut (suppresses Hyprland bindings)
whykey listen --pass-through        # explain the shortcut without suppressing its action
whykey listen --repeat              # inspect one deliberate shortcut at a time
whykey listen --terminal            # force terminal-only capture
whykey listen --evdev               # observe a physical Linux input device
whykey doctor                       # check available integrations
whykey bindings --key ctrl+x        # list matching desktop bindings
whykey conflicts --source hyprland  # find matching conflicts
whykey snapshot ctrl+super+return --output whykey-snapshot.json
whykey diff before.json after.json
```

Use `--verbose` for the full route. Use `--json` for stable schema-v1 output
or `--json-v2` for structured context and evidence. `snapshot` saves a
versioned static report for later offline replay; it never captures or injects
input. New snapshots redact shell identity and private paths in captured
evidence. They preserve the observed conclusion for comparison, not raw IPC or
configuration inputs required to re-run every adapter offline.

Run `whykey --help` for every command and option.

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
When running inside a compatible Hyprland session, it defaults to capturing
and temporarily suppressing ordinary Hyprland bindings via a private submap
and runtime Lua event hook. Pass `--pass-through` to observe shortcuts while
allowing their normal actions to run. Because Hyprland's key event does not
identify the originating keyboard, device identity is reported as unavailable.

If native Hyprland capture is unavailable, Whykey falls back to terminal capture,
observing keys that reach the terminal. Pass `--terminal` to explicitly force
terminal-only capture. Pass `--evdev` to read Linux input events before the
compositor (may require permission to access `/dev/input/event*`). Whykey
never grabs an evdev input device and cleans up temporary Hyprland hooks
on exit. Press Escape or Ctrl+C to exit. If native Hyprland capture is used,
the compositor session is temporarily changed; configuration files are not.

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

- Hyprland, Sway, i3, GNOME, KDE Plasma, Xfce, Cinnamon, MATE, Niri, River,
  Wayfire, labwc, bspwm with sxhkd, Openbox, X11 xbindkeys, AwesomeWM, Qtile,
  and XMonad
- Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, and generic terminals
- Bash with Readline, Zsh with ZLE, Fish, tmux, GNU Screen, and Zellij
- Neovim, Vim, Emacs, Helix, Kakoune, Micro, VS Code, and JetBrains IDEs
- Fcitx5, IBus, keyd, Kanata, KMonad, input-remapper, and xremap

Support is best effort where a program does not expose its live keymap.
Generated configuration, plugins, firmware mappings, browser shortcuts, and
focus changes during inspection can remain uncertain.

See the [support matrix](SUPPORT_MATRIX.md) and
[adapter inventory](ADAPTER_INVENTORY.md) for exact coverage.

## Shell integration

The optional shell wrapper supplies the active Readline, ZLE, or Fish bindings:

```sh
eval "$(whykey shell-init bash)"
# eval "$(whykey shell-init zsh)"
# whykey shell-init fish | source
```

Whykey does not edit shell startup files. Completion scripts are available
through `whykey completions bash|zsh|fish`.

## Development

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
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
