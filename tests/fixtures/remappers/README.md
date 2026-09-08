# Remapper and Compose reproduction fixtures

These small, sanitized inputs exercise the boundary between transformations
that can be read literally and transformations that require runtime timing or
composition state:

- `keyd-caps-to-ctrl.conf` is a literal CapsLock → leftcontrol mapping.
- `keyd-overload-tap-hold.conf` uses keyd's overload form; the mapping is
  visible, but whether it is a tap or hold depends on timing.
- `kanata-tap-hold.kbd` contains a tap-hold alias and a layered definition.
- `input-remapper-caps-to-ctrl.json` and `xremap-caps-to-ctrl.yml` cover the
  other literal parser shapes.
- `compose-e-acute.txt` records a Compose sequence and its committed UTF-8
  result. A terminal-side observer can verify the committed character, but not
  reconstruct the original Compose/dead-key sequence.

The fixtures are intentionally not claims about a live remapper or IME. Tests
must keep runtime activation, virtual-device routing, timing, and Compose
history conditional unless those facts are separately observed.
