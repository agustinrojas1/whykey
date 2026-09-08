# Evdev capability fixtures

These files are sanitized snapshots of the hexadecimal words exposed through
`/sys/class/input/eventN/device/capabilities/key`. They exercise capability
classification without requiring access to a real device or assuming that a
particular event number identifies a keyboard.

- `keyboard-letter.hex` advertises a representative alphanumeric key.
- `keyboard-space.hex` advertises Space through the keyboard capability bit.
- `non-keyboard.hex` advertises no alphanumeric, Enter, or Space key.
