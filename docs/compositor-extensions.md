# Compositor extension manifests

Whykey can read a compositor that has no compiled-in adapter through a small
user-owned manifest. This is Tier 4 support. The manifest names the command
that supplies binding data, so nothing runs unless the user installs the
manifest.

## Security rules

Manifests live in `~/.config/whykey/adapters/` by default. Set
`WHYKEY_ADAPTER_DIR` to use another directory. Whykey reads at most 64 TOML
files, each at most 64 KiB. It does not search `PATH`, load files from the
working directory, or evaluate TOML as code. Commands use argv arrays and the
standard two-second, one-megabyte bounded runner. An explicit `sh -c` is
allowed because the manifest names the shell; an untrusted shell fragment is
still the manifest author's responsibility.

## Manifest fields

```toml
id = "herbstluftwm"
display = "herbstluftwm"
tier = "extension"
applicable_env = ["HERBSTLUFTWM_SOCKET"]
applicable_desktop = ["herbstluftwm"]
bindings_cmd = ["herbstclient", "list_keybinds"]
focused_cmd = ["sh", "-c", "herbstclient attr clients.focus.winid"]
```

`id` uses lowercase letters, digits, and hyphens. At least one applicability
hint is required. A non-empty environment variable or matching
`XDG_CURRENT_DESKTOP` value makes the adapter applicable. A manifest that
uses a built-in adapter ID is ignored and appears in `doctor` warnings.

`bindings_cmd` prints a JSON array of objects with `key`, `action`, and optional
`context`, `device`, and `submap` strings. Literal `key action` lines are also
accepted. Invalid keys are skipped and counted. Every record is labeled
`script-reported; runtime activation conditional`. Actions are never run.

`focused_cmd` prints a decimal PID or `{"pid":1234}`. A missing command or a
failed query is conditional evidence, not a fatal diagnostic error.

## Tiers and limits

T1 compiled-in live IPC and T2 bounded live queries can report runtime state.
T3 literal configuration and T4 manifests describe what is configured, not
what the compositor currently loaded. T4 never participates in `listen`
capture. Native Hyprland capture, terminal capture, and evdev remain the only
capture transports.

The environment snapshot discovers manifests once. An applicable adapter's
binding command runs at most once per snapshot and its result is shared by
`inspect` and `bindings`. `doctor` and `capabilities` report applicability and
command reachability without executing the command.

First-party examples for herbstluftwm, bspwm/sxhkd, and dwl are under
`extensions/compositor/`. They are examples, not auto-installed adapters.
