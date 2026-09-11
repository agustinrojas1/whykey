# E2 candidate-note gate

Verbose reports only need compositor-candidate details when the environment
contains a compositor identity or desktop hint. The report renderer now checks
those five environment markers before collecting a second `Environment`
snapshot. Console-only verbose reports therefore avoid all compositor probes;
candidate-bearing sessions retain the existing snapshot and output.

## Post-split rerun (issue #73)

Using the same wrapper-PATH fixture and `--iterations 20` after the CLI split:

| Run | Median | P95 | Child CPU | External launches | Binary |
| --- | ---: | ---: | ---: | ---: | ---: |
| E1 baseline | 31.50 ms | 31.54 ms | 236.65 ms | 40 (40 IPC-like) | 28,217,256 bytes |
| Post-split | 31.54 ms | 31.57 ms | 249.14 ms | 40 (40 IPC-like) | 29,023,440 bytes |

The latency delta is within process-start noise and the launch count is
unchanged. The remaining E2 candidates were discarded rather than adding
complexity without a demonstrated gain; see the E2 close-out in
`REFACTORING_PLAN.md`.

Reproduce the focused measurement with:

```text
WHYKEY_PERF_LOG=/tmp/whykey-e2.log python3 tools/measure_perf_e1.py --iterations 20
```

Compare this with `inspect --verbose` and inspect the wrapper log. The change
is intentionally a guard, not a parallel-query rewrite: no candidate marker
means zero additional compositor probes, while a marker keeps the exact prior
selection and report text.
