# Virtual desktop-session fixtures

These payloads are consumed by `tests/cli.rs` to exercise every supported
desktop/compositor adapter without requiring a live session. They model the
documented API or configuration shape, not a user's actual configuration.
Version-specific behavior and runtime registration remain conditional unless
the fixture explicitly provides that evidence.

`matrix.json` records the adapter-to-test and fixture mapping. The CLI test
suite validates that every listed fixture exists and that every supported
desktop adapter has a corresponding virtual-session test.
