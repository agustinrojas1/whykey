# Whykey simplification and optimization plan

## Purpose and scope

Simplify Whykey's diagnostic core, reduce genuine duplication, isolate responsibilities, and optimize measured costs while preserving existing behavior.

Start with the diagnostic engine, not an adapter rewrite. Keep these objectives separate:

- **Refactoring:** fewer duplicated rules, fewer ambiguous states, and clearer ownership of responsibilities.
- **Optimization:** fewer external queries, less repeated work, and lower latency demonstrated by measurement.

Do not promise a percentage reduction in code size or a performance improvement before establishing a baseline. Fewer lines achieved through opaque macros or callback machinery do not count as simplification.

This document is a plan, not a record of completed implementation. Creating it does not authorize executing every phase.

## Repository observations

During the planning inspection, the repository was on `main` with a clean working tree. Re-check Git status and HEAD before execution; this is a historical observation, not an assumption about future state.

The manifest and relevant orchestration, reporting, discovery, and capture code were inspected. Source-file lengths below include embedded tests and are not direct measures of production complexity. References identify the planning baseline and may move.

| Area | Observed structure | Recommended intervention |
| --- | --- | --- |
| `src/layers/mod.rs`, 912 lines | Repeatedly reconstructs result prefixes at early exits and diagnostic deadlines | Unify result accumulation and continuation policy |
| `src/report.rs`, 1,719 lines | Selects conclusions while constructing presentation text | Separate conclusion evaluation from rendering |
| `src/listen.rs`, 3,223 lines | Combines protocols, terminal handling, evdev, signals, capture, and export | Separate components without changing lifecycle behavior |
| `src/layers/hyprland.rs`, 3,297 lines | Combines discovery, IPC, Lua/config parsing, XKB, and matching | Separate data acquisition from pure analysis |
| Session context | `Environment` detects multiplexers while `session.rs` reads the same environment variables again | Reuse context with an explicit lifetime |

Two concrete signs of technical debt were also observed:

- `src/report.rs:681` selects a conclusion through `summary.contains("no exact binding")`.
- `src/layers/mod.rs:631` names `input.is_some()` as `bytes_verified`, although `TerminalInput` distinguishes configured, predicted, and observed input.

Do not silently change behavior while addressing these structures. Characterize the current behavior first. Record newly discovered defects and address behavioral corrections separately from mechanical refactoring.

## Non-negotiable constraints

1. Make one transformation per reviewable change. Do not combine file moves, algorithm changes, and output changes.
2. Compare against the baseline binary using identical controlled inputs, including errors and external interactions.
3. Preserve CLI behavior, JSON contracts, evidence-array ordering, replay, exit codes, and capture policies.
4. Avoid universal abstractions. Do not turn all adapters into a generic framework.
5. Preserve existing work. Do not commit, tag, publish, or rewrite history without explicit authorization.

Read repository instructions and re-check the working tree before execution. Never expose credentials or private runtime evidence. Keep live tests that mutate desktop state opt-in and isolated.

## Phase A: Establish a behavioral safety net

**Objective:** make preservation of behavior demonstrable before changing production structure.

- [x] **A1. Record a reproducible baseline**
- [x] **A2. Turn evaluation scenarios into executable contracts**
- [x] **A3. Build a differential comparison workflow**

### A1. Record a reproducible baseline

Record:

- Commit, working-tree state, and toolchains.
- Current test and verification results.
- A baseline binary built from that exact revision.
- Production-code metrics separately from test metrics.
- Commands and fixtures used for comparisons.

If the working tree is not clean, record its relationship to the baseline without discarding changes or creating an unauthorized commit.
**Status:** Complete. Rebuild workflow implemented in `tools/rebuild_baseline.sh`, creating isolated worktrees and recording provenance metadata (commit, toolchain, Cargo.lock hash, binary hash, build command).

**Completion criterion:** the original binary remains executable independently of subsequent source changes.
### A2. Turn evaluation scenarios into executable contracts

Use existing evaluation artifacts as references and convert reproducible scenarios into isolated automated tests.

Prioritize:

- Application targets versus parent shells.
- Missing, predicted, configured, and observed bytes.
- Upstream matches with unavailable downstream layers.
- Submaps, universal bindings, and inventory filters.
- Capture arming, cancellation, timeout, cleanup, and ownership loss.

Do not make local `eval/` artifacts a required dependency of distributed tests. Extract only minimal, sanitized fixtures into the appropriate test locations.

**Completion criterion:** each sensitive behavior has a test that fails if its contract changes.

### A3. Build a differential comparison workflow

Run baseline and candidate binaries against the same controlled environments.

| Contract | Comparison |
| --- | --- |
| Deterministic text | Exact output |
| JSON | Fields, values, and array ordering |
| CLI behavior | Exit status, stdout, and stderr |
| External interactions | Query sequence and permitted mutations |
| Replay, snapshot, and diff | Offline results without desktop queries |

Normalize only genuinely variable values, such as temporary paths. Do not remove uncertainty, evidence, or fields to make comparisons pass. Do not sort evidence arrays whose order is part of the observed behavior.
**Status:** Complete. Implemented in `tools/differential_compare.sh` with 5 automated self-tests, sanitized fixtures under `tests/fixtures/differential/`, and controlled inspect scenarios for the refactored route.

**Completion criterion:** every difference produces a readable report and requires an explicit explanation.
## Phase B: Simplify the diagnostic core

**Priority:** this is the highest-value phase. Complete it before changing capture internals or large parsers.

- [x] **B1. Separate shared types from orchestration**
- [x] **B2. Accumulate results once**
- [x] **B3. Make route policy explicit**
- [x] **B4. Separate conclusion evaluation from rendering**

### B1. Separate shared types from orchestration

`layers/mod.rs` currently contains module declarations, shared types, serialization, and route execution.

A tentative split:

```text
layers/
  mod.rs          public facade and compatible re-exports
  model.rs        results, evidence, and shared types
  inspection.rs   route execution
```

Initially move code only. Preserve existing APIs through re-exports where necessary.

**Completion criterion:** file movement introduces neither new logic nor behavioral differences.

### B2. Accumulate results once

Replace repeated reconstruction of prefixes such as compositor, terminal, TTY, session, and application with explicit incremental accumulation.

Extract small operations for:

- Appending a layer result.
- Recording diagnostic budget exhaustion.
- Deciding whether inspection continues.
- Selecting application or shell as the intended endpoint.

Do not replace the route with an indiscriminate loop over adapters. Dependencies between terminal encoding, TTY handling, multiplexers, and target selection are real.

**Completion criterion:** repeated prefix construction disappears while result ordering and query ordering remain unchanged.

### B3. Make route policy explicit

Centralize ownership of these decisions:

- When a result stops inspection.
- What `force_continue` means.
- Which target should be inspected.
- Which stages require input bytes.
- When unavailable evidence constitutes an operational failure.

Do not collapse these distinct decisions into a generic boolean.

Review `InspectRequest.key: Option<_>`, which later requires `expect`. If every inspection requires a key, encode that requirement at construction time while preserving necessary compatibility.

**Completion criterion:** each routing decision has one clear owner and table-driven tests.

### B4. Separate conclusion evaluation from rendering

Introduce a pure evaluation step that converts layer findings and capture context into a typed conclusion.

```text
Layer results + capture context
              |
              v
     Conclusion evaluation
              |
              v
 Presentation and compatible serialization
```

The renderer should format the conclusion rather than reconstruct diagnostic policy. Migrate existing branches individually, preserving exact text initially.

Replace `summary.contains(...)` decisions with equivalent typed evidence. Do not silently reinterpret legacy reports or invent evidence during replay.

**Completion criteria:**

- Changing detail wording cannot change the diagnostic conclusion.
- Conclusion precedence is testable without comparing paragraphs.
- Text and structured output do not implement contradictory policy.

**Status:** Complete. Conclusion evaluation (`evaluate_layer_conclusion`) is pure and typed, using `UncertaintyReason::ModifierAmbiguity` and `UncertaintyReason::UnknownEndpoint` without interpreting summary wording or display labels. Legacy compatibility is isolated at `normalize_legacy_layer`.
## Phase C: Remove genuine redundancy

- [x] **C1. Reuse existing environment context**
- [ ] **C2. Extract only semantically equivalent duplication (Partial/Deferred)**
- [ ] **C3. Reduce repetitive result construction (Partial)**
### C1. Reuse existing environment context

`Environment` already centralizes some discovery. Extend its use before introducing a second discovery mechanism.

Start with confirmed duplication in `session.rs`: tmux, Screen, Zellij, and SSH detection.

Distinguish context by lifetime:

| Information | Treatment |
| --- | --- |
| Identity of the inspection session | Collect and reuse within its defined lifetime |
| Focus, submap, active mode, or mutable configuration | Query or refresh when the operation requires it |

Do not add a global cache. Reusing dynamic state across `listen --repeat` without invalidation can make reports incorrect.
**Status:** Complete. Environment context is reused across session discovery and inspections.

**Completion criterion:** redundant discovery is removed without freezing state that must remain fresh.
### C2. Extract only semantically equivalent duplication

Inventory repeated parsing, normalization, and result-construction helpers.

For each candidate:

1. Compare semantics and edge cases.
2. Extract the smallest useful helper.
3. Migrate one consumer.
4. Migrate the next consumer and compare behavior.

Do not unify parsers merely because they split strings. Shell escaping, Lua literals, and terminal protocol syntax have different contracts.
**Status:** Partial / Deferred. Limited cleanup applied to Cinnamon and GNOME result helpers; broader deduplication across remaining desktop adapters deferred to maintain bounded scope.

**Completion criterion:** demonstrated duplication disappears without adding adapter-specific switches to a supposedly shared abstraction.
### C3. Reduce repetitive result construction

Identify frequent, valid `LayerResult` and evidence initializations. Introduce small constructors where they reduce repetition and make the assertion clearer.

Avoid builders that permit ambiguous combinations or hide the diagnostic claim. Keep schema compatibility at serialization boundaries instead of distributing it through adapters.
**Status:** Partial. Retained explicit constructors (`pass`, `unavailable`) where they improve readability; removed dead unused abstraction (`LayerResult::simple`). Broad builder abstractions avoided.

**Completion criterion:** common results require less boilerplate without concealing their semantics.
## Phase D: Split large modules by responsibility

This phase improves navigation and isolation. Moving code does not itself reduce total complexity or improve performance.

- [ ] **D1. Separate Hyprland acquisition and analysis**
- [ ] **D2. Separate listening components**
- [ ] **D3. Reduce `main.rs`**

### D1. Separate Hyprland acquisition and analysis

Tentative structure:

```text
hyprland/
  mod.rs
  ipc.rs
  config.rs
  matching.rs
  keyboard.rs
```

Responsibilities:

- `ipc`: queries and instance selection.
- `config`: declarative and Lua hints.
- `matching`: pure binding evaluation.
- `keyboard`: device and XKB integration.

Move code first; evaluate internal deduplication afterward.

**Completion criterion:** matching can be tested without compositor access or filesystem reads.

### D2. Separate listening components

Tentative structure:

```text
listen/
  mod.rs
  terminal.rs
  evdev.rs
  protocol.rs
  output.rs
```

Initially keep guards beside the resources they control. Do not change `Drop` order, signal handling, descriptor closure, retries, or restoration while moving code.

Preserve the distinction between suppression cleanup, which owns restoration, and pass-through cleanup, which does not own the user's active submap.

**Completion criterion:** each backend is understandable independently and cleanup interaction traces remain equivalent.

### D3. Reduce `main.rs`

Separate argument parsing, command dispatch, and exit-status policy. Leave `main` as a small entry point.

Do not add a CLI dependency merely to reduce line count. First evaluate changes to help text, argument errors, dependencies, and compatibility.

**Completion criterion:** command and exit-status behavior can be tested without duplicating policy.

## Phase E: Measure, optimize, and close

- [ ] **E1. Measure before optimizing**
- [x] **E2. Optimize the dominant measured cost**
- [x] **E3. Run complete final verification**

### E1. Measure before optimizing

Measure separately:

- Inspection latency.
- External process launches and IPC calls.
- Repeated parsing per inspection.
- Memory usage and binary size.
- Build and test execution time.

Separate fixture measurements from live-system measurements. Do not attribute external command latency to Rust computation.

### E2. Optimize the dominant measured cost

Recommended order:

1. Eliminate repeated external queries.
2. Avoid loading and parsing the same configuration repeatedly within one inspection.
3. Remove expensive clones identified by measurements.
4. Improve matching algorithms if their cost is material.
5. Optimize formatting only if it contributes meaningful cost.

Do not parallelize queries by default. Parallel execution can change observation timing, diagnostic budget use, and consistency.

Each optimization must include reproducible before/after measurements and behavioral comparisons. Discard optimizations that add complexity without a demonstrated benefit.

#### E2 close-out measurement (issue #73)

The post-CLI-split rerun used the same wrapper-PATH method and 20 fixture
iterations as E1. The fresh binary measured 31.54 ms median / 31.57 ms p95,
249.14 ms child CPU, 40 external launches (40 compositor IPC-like), and a
29,023,440-byte binary. E1 recorded 31.50 ms median / 31.54 ms p95,
236.65 ms child CPU, 40 launches, and a 28,217,256-byte binary.

The small latency difference is process-start noise, while the binary grew as
the requested CLI and evidence work landed. No remaining candidate has a
measured material gain that justifies extra caching, clones, matching changes,
or formatting complexity. E2 is therefore closed under the discard rule;
future optimization needs a new measurement and a separate before/after PR.

### E3. Run complete final verification

After the final change, run:

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo +1.85.0 test --all-targets --locked
cargo +1.85.0 clippy --all-targets --locked -- -D warnings
python3 tools/generate_support_matrix.py --check
python3 tools/generate_dependency_provenance.py --check
bash tools/check_package_versions.sh
cargo package --locked --allow-dirty
git diff --check
```

Also run differential comparisons and the executable evaluation scenarios. Inspect the package contents to ensure local evaluation artifacts remain excluded.

Use a fresh independent reviewer for final verification. Report blocked checks explicitly rather than claiming success.

Live tests that mutate Hyprland require an authorized dedicated environment. Fake transports cannot prove every live lifecycle property; state that limitation honestly.

## Work sequencing and delegation

| Block | Scope | Stop point |
| --- | --- | --- |
| First | Phase A + Phase B | Differential comparisons pass and the diagnostic core is simpler |
| Next | Phase C | Demonstrated duplication is removed |
| Optional | Phase D | Large modules are separated by responsibility |
| Evidence-driven | Phase E | Measured optimizations and final verification are complete |

Do not execute all phases in parallel. Shared contracts in the diagnostic core require a stable sequence.

Suitable independent workstreams include:

- Duplication inventory.
- Differential comparison tooling.
- Independent module extraction after shared types stabilize.
- Fresh final review.

Assign clear file ownership to workers. Do not have several agents edit shared model or orchestration files concurrently.

**Initial planning estimate:** allocate a 60–90 minute work session to establish the baseline and select behavioral contracts. This is an estimate, not a completion guarantee for the safety net or the overall refactor. Estimate subsequent implementation work after assessing differential coverage.

## Success criteria

| Criterion | Required result |
| --- | --- |
| Route rules | One identifiable owner for each decision |
| Conclusions | Pure evaluation without parsing human-readable text |
| Context | No redundant rediscovery in migrated paths |
| Safety | Preserved lifecycle and external-effect restrictions |
| Compatibility | No unapproved differences in contracts or evaluation scenarios |

Measure simplification through reduced duplicated decisions, clearer dependencies, and smaller units of responsibility. Do not use total line count as the primary target, and do not delete tests to improve metrics.

## Initial execution recommendation

Authorize **Phase A and Phase B only** for the first implementation block. Review their results before extending the scope.

That block targets the highest-value simplification without turning a working diagnostic tool into an open-ended rewrite.
