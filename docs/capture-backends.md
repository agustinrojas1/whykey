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

trait NativeCaptureIo: CaptureBackend {
    fn socket_fd(&self) -> RawFd;
    fn read_incoming(&mut self) -> io::Result<usize>;
    fn renew_lease(&self) -> io::Result<()>;
}
```

The listen loop owns terminal setup, polling, inspection, reporting, and
export. A backend owns its transport and capture-specific lifecycle. Add a
backend by implementing `CaptureBackend` and `NativeCaptureIo`, then adding one
arm to `select_native_backend` in `listen.rs`. That selector is currently
Native capture selection now walks the scored compositor candidates and the
registry's optional capture factory. It returns a boxed transport, so adding
a second native backend does not change the listen loop. The registry entry
should expose the same stable display name to capabilities and doctor.

Source wire shapes have one canonical mapping:

| Context | Native source shape |
| --- | --- |
| In memory | `CompositorNative { backend }` |
| Schema v2 | `{"kind":"compositor-native","backend":"..."}` plus `source_label` |
| Schema v1 | `"Hyprland"` for the current backend, read and normalized as native; future backends use the canonical object until a compatibility form is defined |

The old externally tagged `{"CompositorNative":{"backend":"..."}}` object
is accepted as a reader-only compatibility form. New writers do not emit it.

`NativeCaptureIo` is the current Unix polling seam. Its file descriptor and
lease-renewal methods reflect Hyprland today. A Sway, portal, or extension
backend without a lease or Unix socket should move those operations into a
separate transport abstraction before it is registered.

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
