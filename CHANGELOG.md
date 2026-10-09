# Changelog

## [Unreleased]

### Fixes

- Hyprland: `__lua` and plugin dispatchers are now treated like any other consuming bind; the report says the key is consumed unless the Lua callback explicitly returns `pass_event`. Previously these binds were reported as opaque and their forwarding as unknown.
- Hyprland: bind keys that use keysym names (`comma`, `slash`, `minus`, `Prior`, …) now match the normalized query key.
- Hyprland: Lua binds that Hyprland reports with an empty key and keycode 0 now get their key from a matching literal Lua `code:` hint. When no hint exists, binds with the query's modifiers are reported as a possible match ("Hyprland may handle this: …") instead of being ignored. JSON v2 marks them with the `UnresolvedBindingKey` reason, and `bindings --key` lists them under `possible_matches`.
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
- Inspection now respects a remapper result that consumes or redirects a key: a consume stops the route, and a redirect re-targets later layers or makes them conditional. Note that the current remapper adapters only report pass or uncertain results, so this path does not yet trigger in practice.
- When several compositor extension manifests apply, only the highest-scored one runs its bindings command.
- The CLI test suite no longer inherits host terminal and compositor environment variables.

### Packaging and documentation

- Declare `python3` for the installed `whykey-nvim`, `whykey-vscode`, and `whykey-emacs` extensions: Debian and RPM `Recommends`, Arch `optdepends`. The Nix recipe adds `python3` so the extension shebangs are patched to the store path.
- README, SPEC, and the man page now say exactly what `listen` changes at runtime, which compositors are read through live IPC and which from parsed configuration, and that Bash/Zsh/Fish answers need `shell-init` to be exact.
- Correct the L-010 size figures and state how they were measured.

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
