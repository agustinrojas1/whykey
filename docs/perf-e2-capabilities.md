# E2 capability JSON snapshot reuse

The capabilities JSON command used to collect the environment once while
building capabilities and a second time while building schema-v2 context.
Those snapshots repeat process, compositor, and manifest discovery in the same
command and can observe different session state.

| Path | Environment snapshots per command |
| --- | ---: |
| Before | 2 |
| After | 1 |

`capabilities::current_with_environment` and
`capabilities::render_json_with_environment` now share the command-owned
snapshot. The legacy public helpers still collect their own snapshot for
callers that use them independently. This is a measured call-count reduction,
not a parallel-query rewrite; output and schema fields are unchanged.
