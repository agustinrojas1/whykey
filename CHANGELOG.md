# Changelog

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
