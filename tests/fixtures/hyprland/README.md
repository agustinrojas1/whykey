# Sanitized Hyprland fixtures

These files are deterministic, hand-written examples of the JSON and Lua
shapes consumed by the Hyprland adapter. They contain no machine-specific
paths, signatures, window titles, or user configuration. Unit tests include
them directly so parser behavior does not depend on a live compositor.

- `instances.json` covers Wayland-socket instance selection.
- `binds-representative.json` covers default, inactive-submap, universal, and
  non-consuming bindings.
- `devices-keyboards.json` covers a main keyboard plus an additional device.
- `devices-layout-groups.json` covers active layout-group selection.
- `omarchy-bindings.lua` covers consecutive literal Lua declarations.
- `instances-ambiguous.json` preserves ambiguity when no Wayland socket matches.
- `instances-version-matrix.json` keeps valid instances when one inventory
  entry has an incompatible field shape.
- `binds-malformed-entry.json` keeps valid entries beside one malformed entry.
- `binds-structured-device.json` covers newer structured device scopes.
- `binds-invalid-object.json` covers a valid JSON response with the wrong shape.
- `version-matrix.json` ties the supported legacy/structured/malformed wire
  shapes together without claiming a package version for either shape.
- `hyprctl-stale.stderr` is the sanitized stderr response used for stale-IPC
  regression coverage.

Additional package-version and runtime-session coverage remains tracked in the
implementation plan and defect register.
