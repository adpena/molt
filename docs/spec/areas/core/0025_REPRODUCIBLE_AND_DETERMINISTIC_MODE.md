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
