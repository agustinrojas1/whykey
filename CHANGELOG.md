# Changelog

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
