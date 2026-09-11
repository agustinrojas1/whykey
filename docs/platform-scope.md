# Platform scope

The shipped `whykey` binary is Linux-only. This is an explicit product
boundary, not an accidental omission: evdev capture, PTY/termios behavior,
XDG session discovery, Linux process/session inspection, and compositor IPC
are all part of the supported diagnostic contract.

The repository's CI and release jobs therefore target Linux only. Packaging
metadata declares Linux platforms, and non-Linux compilation fails with a
clear message from `src/lib.rs` rather than silently omitting capture or
desktop features.

A future cross-platform effort would extract a separately specified
`whykey-core` (key/model/report/replay/diff/schema) and add OS feature crates.
That is intentionally not part of the current binary or compatibility promise.
