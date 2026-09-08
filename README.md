# whykey

[![CI](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml/badge.svg)](https://github.com/agustinrojas1/whykey/actions/workflows/ci.yml)

`whykey` explains what handles a Linux keyboard shortcut and whether the key
continues to the next layer.

It inspects the active compositor, terminal, TTY, multiplexer, application,
and shell. It never executes the shortcut or changes your configuration.

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
whykey listen                       # explain the next terminal key
whykey listen --repeat              # inspect one deliberate key at a time
whykey listen --evdev               # observe a physical Linux input device
whykey doctor                       # check available integrations
whykey bindings                     # list detected desktop bindings
whykey conflicts                    # find conflicts in that inventory
```

Use `--verbose` for the full route. Use `--json` for stable schema-v1 output
or `--json-v2` for structured context and evidence.

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

`whykey listen` observes the next deliberate press that reaches the terminal.
It temporarily enables terminal key capture only while waiting, then restores
the terminal before explaining the result. Press Escape or Ctrl+C to exit.
Its optional `--evdev` mode reads Linux input events before the compositor and
may require permission to access `/dev/input/event*`. Whykey never grabs an
input device.

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

The [specification](SPEC.md) documents output and inspection behavior.
Reproducible gaps belong in the [defect register](DEFECT_REGISTER.md).

## License

[MIT](LICENSE)
