# Adapter testing checklist

Every compositor adapter must have a deterministic fixture under
`tests/fixtures/sessions/` and an entry in `matrix.json`. The fixture must
cover a positive result, a negative or empty result, and an IPC failure. The
adapter's inventory row and `support-matrix.json` entry are part of the same
gate.

`tools/check_adapter_gate.sh` validates those boundaries and deliberately
checks that an incomplete entry is rejected. The harness binaries under
`tests/fixtures/adapter_harness/bin/` provide stock-CI stand-ins for
`hyprctl`, `swaymsg`, and `i3-msg`; `socket2_server.py` provides a local Unix
socket for protocol tests. They never contact a user's compositor.

Capture lifecycle tests must cover:

- Connected → Installing → Armed → RestorePending → Closed;
- event delivery followed by restoration;
- stale-token refusal and bounded two-retry cleanup;
- pass-through ownership (listener removal only, no user-submap restore);
- chord-release timeout without a normal report; and
- IPC disconnect/failure without hanging the test process.

New adapters are not complete until the fixture, positive/negative/IPC-failure
tests, inventory boundary, support-matrix entry, and the gate all land in the
same change. Live compositor tests remain ignored and require their explicit
triple opt-in; they do not replace the hermetic gate.
