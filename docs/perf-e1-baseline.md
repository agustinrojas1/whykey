# E1 performance baseline

Measurement only; no production optimization is included in this report.
Run `cargo build --locked` followed by:

```text
python3 tools/measure_perf_e1.py --iterations 20
python3 tools/measure_perf_e1.py --iterations 20 --live
```

The fixture run uses an isolated HOME and deterministic failing wrappers for
`hyprctl`, `swaymsg`, `i3-msg`, `gsettings`, `xfconf-query`, `pgrep`, and `ps`.
It measures the whole CLI subprocess, so wrapper/process startup is not
misreported as Rust computation. The wrapper log is the external-launch and
compositor-IPC-like call count. Parser reads are reported separately because
the production binary does not expose a read counter; source inspection found
no repeated parser cache that can be claimed from this measurement alone.

Machine and toolchain for the committed run:

| Field | Value |
| --- | --- |
| OS/kernel | Linux omarchy 7.1.9-arch1-2, x86_64 |
| Rust | rustc 1.98.1 (48a229cea 2026-09-01) |
| Cargo | cargo 1.98.1 (797e8a9bc 2026-08-05) |
| Fixture iterations | 20 |
| Fixture command | `whykey inspect ctrl+c` with wrapper PATH |
| Live command | `whykey inspect ctrl+c` with current environment; opt-in |

Baseline numbers are generated from the current checkout and intentionally
remain machine-specific. E2 optimization tickets must rerun the same command
and include before/after values; they may not infer external-command latency
from the child CPU column.

## Recorded run

The following values were recorded on the machine/toolchain above:

| Scenario | Median | P95 | Child CPU | External calls/inspection | Binary |
| --- | ---: | ---: | ---: | ---: | ---: |
| Fixture | 31.50 ms | 31.54 ms | 236.65 ms / 20 | 2.0 compositor IPC-like | 28,217,256 bytes |
| Live system | 163.94 ms | 164.06 ms | 1,917.56 ms / 20 | not wrapper-counted | 28,217,256 bytes |

The fixture row used the second run (the first run was 31.55/31.63 ms and
244.34 ms child CPU); this small variation is expected process-start noise.
The cached test suite completed in 2.159 s with `cargo test --all-targets
--locked --quiet`. A clean-build duration is intentionally not recorded from a
warm developer target directory; CI's build step is the reproducible build
measurement.

Rerun output is authoritative for a different machine; the committed values
are the reproducible baseline for this checkout and toolchain. Build/test
timing is captured by the normal CI job and should be quoted from the job
summary rather than attributed to inspection code.
