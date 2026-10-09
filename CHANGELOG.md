# Changelog

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
