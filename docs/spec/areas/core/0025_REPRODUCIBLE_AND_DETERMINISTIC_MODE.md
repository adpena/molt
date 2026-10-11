# Reproducible And Deterministic Mode
**Spec ID:** 0025
**Status:** Draft
**Priority:** P1
**Audience:** compiler engineers, runtime engineers, tooling engineers
**Goal:** Define deterministic build/runtime behavior and reproducibility rules.

---

## 1. Definitions
- **Reproducible build**: identical binary given identical inputs and toolchain.
- **Deterministic runtime**: identical observable behavior given identical inputs.

---

## 2. Build Determinism
Builds must be reproducible when the deterministic flag is enabled.

### 2.1 Inputs That Must Be Stable
- Source tree content and ordering.
- Lockfiles (`uv.lock`, Cargo.lock).
- Toolchain versions (Rust, Python, linker).

### 2.2 Build Rules
- No nondeterministic timestamps embedded in artifacts.
- Stable ordering for any generated tables or metadata.
- Hash seeds and randomized data structures must be fixed.

---

## 3. Runtime Determinism

### 3.1 Time
- In deterministic mode, `time.time()` and `time.monotonic()` return a
  deterministic clock anchored to process start.
- Wall-clock access requires explicit capability grants.

### 3.2 Randomness
- `random` and any internal RNG must use a fixed seed by default.
- Explicit seeds override the default but remain deterministic.

### 3.3 Hashing
- Hash randomization is disabled or fixed to a stable seed.
- Hash results must be stable across runs and targets.

### 3.4 Scheduling
- Task scheduling is deterministic for identical workloads.
- Any non-deterministic scheduling policy must be explicitly gated.

---

## 4. Interfaces

### 4.1 Build Flag
- CLI: `molt build --deterministic`
- Environment: `MOLT_DETERMINISTIC=1`

### 4.2 Capability Gates
- `time.wall`: allow wall-clock access.
- `rand.nondeterministic`: allow nondeterministic RNG.

### 4.3 WASM Runtime Determinism

When `MOLT_DETERMINISTIC=1` is set, the WASM host (`molt-wasm-host`) automatically applies:

- **NaN canonicalization**: All NaN payloads are normalized to a canonical form via
  `cranelift_nan_canonicalization(true)`. This prevents CPU-specific NaN payload
  differences from producing divergent WASM execution results.

- **Sequential compilation**: `parallel_compilation(false)` ensures the Cranelift
  JIT produces identical native code regardless of thread scheduling during
  compilation.

These flags are applied automatically — no manual Node.js/wasmtime flags are needed
when deterministic mode is on.

#### Limitations

- WASM execution under V8 (Node.js) may still require `--no-wasm-tier-up` and
  `--liftoff-only` flags for full determinism. These are NOT auto-applied by Molt
  and must be passed explicitly when using Node.js as the WASM runner.
- Wasmtime's own tier-up (if enabled) should be disabled separately via
  `config.strategy(Strategy::Cranelift)` (already the default).

---

## 5. Validation
- Deterministic builds must be bit-identical.
- Deterministic runtime tests must repeat with stable outputs.
- WASM and native targets must match in deterministic mode.


### 5.1 Phase-specific proof evidence

`tools/check_deterministic_runtime.py` emits
`molt.deterministic-runtime-proof.v3`. Each observation records separate `build`
and `runtime` launch evidence. The compiler receives `PYTHONHASHSEED=0`; the
runtime receives the observation index (starting at 1). A report must record
those distinct environments, not infer a runtime setting from the build mode.
The build's cache and artifact directories likewise do not describe the guest's
inherited environment.

`tools/check_reproducible_build.py` emits `molt.reproducibility-proof.v3`.
Repeated artifact comparisons retain each `build` launch, while IR audit
observations retain their `compiler` launch (hash seeds start at 0). Compare-only
mode reads existing artifacts and does not manufacture compiler launch evidence.

The two tools project the same finite public fields from the environment passed
to each guarded command: `PYTHONPATH`, `PYTHONHASHSEED`, `MOLT_DETERMINISTIC`,
`MOLT_CACHE`, `MOLT_EXT_ROOT`, `MOLT_TARGET_ROOT`, `CARGO_TARGET_DIR`,
`MOLT_BACKEND_DAEMON`, `MOLT_BACKEND_DAEMON_SOCKET_DIR`, `TMP`, and `TEMP`.
Unset values are JSON `null`. These fields describe launch inputs, not the full
ambient environment or the program's later environment. Other inherited values
and credentials are not copied into the report.

Runtime observations share the inherited compiler build roots
(`CARGO_TARGET_DIR`, `MOLT_EXT_ROOT`, `MOLT_TARGET_ROOT`). Each observation
gets its own working directory, `MOLT_CACHE`, output path and no backend
daemon, so no compile state passes between observations. Each build uses the
proof plan's nested build budget unless `--build-timeout` sets one.

Launch evidence binds those fields to argv, cwd, guard return status, and child
return status. `completed` means that the guard returned without a recorded
guard interruption; a nonzero child return still fails the proof. `timeout`,
`guard-error`, and `error` are not successful observations. Exception evidence
records an attempted command and has no claimed child return status. A failed
build leaves `runtime` null. Completed earlier observations remain available if
a later phase fails; selected/executed/pass/fail/error counters still count
complete comparison cells, not subprocess attempts. Each cell also reports
`completed_runs`: observations that completed acquisition and temporary-resource
cleanup. Isolated build, runtime and IR directories use the shared owned-directory
allocator, which returns canonical host paths and refuses cleanup of a replaced
allocation. A failure during artifact hashing, stat, read, or cleanup retains prior
observations and all known launch records, but the affected cell is an error.
A completed child remains `completed` when a later parent-side operation fails;
setup failure before launch leaves the phase null. IR success requires exactly
the requested number of completed observations, regardless of whether an error
has a nonempty diagnostic string.

Build output must be a JSON object with a nonempty string artifact path. Invalid
objects, envelopes, or path values are build errors with the completed launch
retained and no runtime launch. Compare-only rejects the same malformed inputs
as a counted error without inventing process evidence.

Source admission belongs to each selected build or IR cell. Batch scheduling
must not prefilter sources or repeat a filesystem sweep after collecting build
results. When IR auditing is requested, every selected source has an IR cell,
including unreadable or missing sources; those cells count as errors rather than
disappearing from the denominator. Without IR auditing, no IR source admission
runs. Compare-only handles JSON reads/decoding, artifact-path admission and
artifact reads within its single counted result boundary.

Both tools resolve corpus names through `config/reproducibility_corpus.toml`.
The existing `full` corpus contains exactly `examples/hello.py`,
`examples/simple_ret.py`, and `examples/sieve_bench.py`. Its scheduled native
`dev` proof cell is a bounded qualification of those programs, not the full
release support matrix, CPython conformance, or cross-backend acceptance. The
canonical proof plan and verified-subset/release authorities retain those
broader obligations.
