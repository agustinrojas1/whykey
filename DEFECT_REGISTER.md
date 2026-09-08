# Whykey defect register

This register tracks concrete, reproducible behavior gaps and regressions
separately from the capability contract in [`support-matrix.json`](support-matrix.json)
and from accepted coverage limits. An item stays in the open table until its
behavior is fixed and a regression test covers it. A planned capability is
not automatically a defect; it belongs in the support matrix unless it
contradicts a documented guarantee.

## Open items

| ID | Priority | Area | Description | Evidence or reproduction |
| --- | --- | --- | --- | --- |
| D-011 | Release blocker | Terminal capture | A key release generated while launching `whykey listen` could be reported as the requested key. | Ghostty reported `RETURN` release (`CSI 13;1:3u`) immediately after launch. Regression: `listen_ignores_a_kitty_release_before_the_next_press`. |
| D-012 | Release blocker | Terminal cancellation | The documented `Esc Esc` exit only recognized literal bytes, while Kitty encodes Escape and Ctrl+C as keyboard-protocol events. | Real interactive capture could not be exited. Regressions: `listen_exits_through_a_pty_after_kitty_escape` and `listen_exits_through_a_pty_after_kitty_ctrl_c`. |
| D-013 | Release blocker | Repeat capture | Capture mode remained active during inspection and rendering, allowing release/repeat events and typed input to queue into a report loop. | Real `--repeat` session became unusable. Regression: `listen_repeat_restores_between_reports`. |
| D-014 | High | CLI safety | Invoking `whykey` with no arguments started raw terminal capture. | Regression: `no_arguments_prints_help_instead_of_entering_capture_mode`. |
| D-015 | High | JSON schema | `listen --json --schema-version 2` labelled its operation as `inspect`. | Fixed in the listener render path; release verification must parse a schema-v2 listener record. |

## Post-1.0 issues (future protocol coverage and adapters)

The following areas are deferred to post-1.0 issue tracking and do not block 1.0:

- **Protocol coverage**: Complete Kitty keyboard protocol negotiation matrix, compositor-independent libxkbcommon evdev state evaluation, and application-level IME preedit/dead-key history reconstruction (`input.ime`).
- **Additional adapters**: Additional Wayland/X11 compositors beyond the 17 built-in desktop adapters (`compositor.additional-desktops`), and dynamic configuration resolvers for bespoke desktop setups.

## Known limitations (accepted boundaries, not defects)

The following are accepted 1.0 boundaries, tracked as normal capability
limitations. None of them is a reproducible failure against documented
behavior, and none should block a release on its own.

| ID | Area | Boundary | Evidence of current coverage |
| --- | --- | --- | --- |
| L-001 | Hyprland | The complete multi-instance ambiguity and package-version matrix is not exercised live. | `tests/fixtures/hyprland/version-matrix.json`, `instances-version-matrix.json`, `hyprctl-stale.stderr`, `instances-ambiguous.json`, and `leaves_duplicate_socket_matches_unselected` cover legacy/structured/malformed wire shapes and ambiguous selection. |
| L-002 | Hyprland Lua | Literal-only scanning cannot prove that every executable Lua declaration was loaded or evaluate dynamic helpers. | `src/layers/hyprland.rs` skips quoted text and Lua long-bracket comments/strings; tests cover balanced literals and preserve conditional evidence. |
| L-003 | Keyboard layout | Evdev-to-XKB translation remains conditional without compositor-independent libxkbcommon state and complete XKB type, latch, and lock evaluation. | Shared `src/xkb.rs` parses real `symbols[N]` layout groups, preserves `NoSymbol` level positions, and keeps symbol levels ordered; physical capture tests cover code namespaces, offsets, explicit multi-group maps, and common level selection. |
| L-004 | Terminal protocol | Kitty negotiation and associated-text handling do not cover every capability and malformed response combination. | `src/listen.rs` preserves all valid alternate-key codes in wire order, keeps the compatibility first entry, and rejects invalid alternate codes. |
| L-005 | Continuous capture | Whykey is an interactive diagnostic, not a high-throughput terminal event recorder. | Repeat captures one deliberate key at a time and restores the terminal before reporting; bulk latency benchmarks are intentionally non-release experiments. |
| L-006 | Focus and targets | Process ancestry and focused-window APIs cannot prove that the selected process is the user's intended target across focus-change races. | Focus adapters and PID selection are covered by `tests/cli.rs`. |
| L-007 | Configuration | Generated files, conditional includes, and all syntax/version variants are not uniformly resolved across adapters. | Adapter unit tests cover common TOML, YAML, JSON, XML, and KDL forms. |
| L-008 | Distribution | Clean-environment installation is verified for the locked source path and the Arch recipe only. | `DEPENDENCY_PROVENANCE.md`, the Cargo clean-install smoke gate, and `tools/arch_package_smoke.sh`; Debian/RPM/Nix builders are unavailable. |

## Closed items

| ID | Resolution | Regression evidence |
| --- | --- | --- |
| D-009 | Support-matrix documentation and capability-registry drift now fail the Rust test suite. | `capabilities::tests::support_matrix_is_the_registry_contract_and_has_generated_docs`; `registry::tests::support_matrix_cannot_drift_from_registry_capability_entries`; `python3 tools/generate_support_matrix.py --check`. |
| D-010 | The declared Rust 1.85 MSRV now builds without unstable let-chain syntax. | `cargo +1.85.0 check --locked`; `cargo +1.85.0 test --all-targets --locked`; both stable and 1.85 Clippy with `-D warnings`. |
| D-001..D-008 | Reclassified as accepted limitations L-001..L-008 above; they were coverage boundaries, not open defects. | This register. |

When an open item is fixed, move it to the closed table with the specific
test or verification command that proves the guarantee.
