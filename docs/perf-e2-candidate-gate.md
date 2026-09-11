# E2 candidate-note gate

Verbose reports only need compositor-candidate details when the environment
contains a compositor identity or desktop hint. The report renderer now checks
those five environment markers before collecting a second `Environment`
snapshot. Console-only verbose reports therefore avoid all compositor probes;
candidate-bearing sessions retain the existing snapshot and output.

Reproduce the focused measurement with:

```text
WHYKEY_PERF_LOG=/tmp/whykey-e2.log python3 tools/measure_perf_e1.py --iterations 20
```

Compare this with `inspect --verbose` and inspect the wrapper log. The change
is intentionally a guard, not a parallel-query rewrite: no candidate marker
means zero additional compositor probes, while a marker keeps the exact prior
selection and report text.
