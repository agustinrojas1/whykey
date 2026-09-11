# Differential Comparison Fixtures

These fixtures provide sanitized, minimal, and reproducible diagnostic reports used by `tools/differential_compare.sh`.

## Fixture Inventory

- `replay-compositor-consumed.json`: Schema v2 diagnostic report where a Wayland compositor (`Hyprland`) matches and consumes `SUPER + RETURN`.
- `replay-terminal-consumed.json`: Schema v2 diagnostic report where a terminal emulator (`Ghostty`) consumes `CTRL + SHIFT + C` for clipboard copying.
- `replay-tty-signal.json`: Schema v2 diagnostic report where the TTY line discipline handles `CTRL + Z` as `VSUSP`, generating `SIGTSTP`.
- `replay-shell-readline.json`: Schema v2 diagnostic report where a shell readline binding (`Bash`) intercepts `CTRL + R` for reverse history search.
- `replay-v1-unhandled.json`: Legacy Schema v1 diagnostic report where a character (`A`) passes unhandled through all layers.

## Privacy and Sanitization Guarantees

These fixtures contain only generic, synthetic configuration paths (such as `/etc/hyprland/hyprland.conf` and `/etc/ghostty/config`). They contain no personal user paths, usernames, credentials, machine identifiers, or private runtime state.
