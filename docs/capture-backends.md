# Capture backends

Native capture is the part of `whykey listen` that observes a shortcut before
the terminal sees it. The public contract lives in `src/capture.rs`:

```rust
trait CaptureBackend {
    fn id(&self) -> NativeBackendId;
    fn display(&self) -> &'static str;
    fn arm(&mut self, policy: CapturePolicy) -> Result<(), String>;
    fn next_observed_event(&mut self, events_all: bool)
        -> Result<Option<ObservedKey>, String>;
    fn wait_for_chord_release(&mut self, key: XkbKeycode, timeout: Duration)
        -> Result<ChordReleaseStatus, String>;
    fn is_suppressing(&self) -> bool;
    fn close(&mut self) -> io::Result<()>;
}
```

The listen loop owns terminal setup, polling, inspection, reporting, and
export. A backend owns its transport and capture-specific lifecycle. Add a
backend by implementing `CaptureBackend` and `NativeCaptureIo`, then adding one
arm to `select_native_backend` in `listen.rs`. The registry entry should expose
the same stable display name to capabilities and doctor.

Backends must preserve the D-016 lifecycle:

```text
Connected -> Installing -> Armed -> RestorePending -> Closed
```

Cleanup gets two retries after its first attempt. A stale owner token must never
restore another session's state. Suppression must be confirmed before reporting,
and restoration must finish before desktop inspection starts.

Every backend needs fake-transport tests. Test arm postconditions, event
filtering, chord-release handling, disconnects, retries, stale tokens, and
pass-through behavior without requiring a live compositor. Live tests belong in
the ignored integration suite and must never run in CI.

The current registry has one native backend, Hyprland. Sway IPC, portals, and
extension capture can be added without changing report routing or the terminal
fallback.
