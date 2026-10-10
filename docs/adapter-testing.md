# Adapter testing checklist

Every compositor adapter must have a deterministic fixture under
`tests/fixtures/sessions/` and an entry in `matrix.json`. The fixture must
cover a positive result, a negative or empty result, and an IPC failure. The
adapter's inventory row and `support-matrix.json` entry are part of the same
gate.

`tools/check_adapter_gate.sh` validates those boundaries and deliberately
checks that an incomplete entry is rejected. The harness binaries under
`tests/fixtures/adapter_harness/bin/` provide stock-CI stand-ins for
`hyprctl`, `swaymsg`, and `i3-msg`. The Sway and i3 stand-ins answer
`get_config`, `get_binding_state`, `get_tree`, and `get_version` with real
reply shapes; `socket2_server.py` provides a local Unix socket for protocol
tests. They never contact a user's compositor.

Capture lifecycle tests must cover:

- Connected → Installing → Armed → RestorePending → Closed;
- event delivery followed by restoration;
- stale-token refusal and bounded two-retry cleanup;
- pass-through ownership (listener removal only, no user-submap restore);
- chord-release timeout without a normal report;
- IPC disconnect/failure without hanging the test process; and
- terminal job control through a real PTY: `SIGTSTP` restores the terminal
  before the job stops, `SIGCONT` re-arms capture, and `SIGTERM` restores the
  terminal (`tests/listen_pty.rs`).

New adapters are not complete until the fixture, positive/negative/IPC-failure
tests, inventory boundary, support-matrix entry, and the gate all land in the
same change. Live compositor tests remain ignored and require their explicit
triple opt-in; they do not replace the hermetic gate.

## Reality lab

Fixtures show only that an adapter agrees with its authors. An adapter is
*verified* when a [lab](../lab/README.md) scenario runs the real program,
sends it real synthetic input, and records ground truth from the program's
reaction. Each scenario needs a positive control, and whykey's verdict must
match the ground truth. When the program can run in the Fedora lab image,
add at least one bound and one unbound scenario under
`lab/scenarios/<group>/`. Then run `python3 lab/run.py --only '<id>'` and
`python3 tools/generate_support_matrix.py` so that the support matrix
records the scenario. Without a scenario, the adapter stays *unverified*
in the support matrix, however many fixtures pass.
