# Molt WASM Targets And Constraints
**Spec ID:** 0401
**Status:** Draft
**Priority:** P1
**Audience:** runtime engineers, compiler engineers, WASM implementers
**Goal:** Define explicit WASM targets, host constraints, and portability rules.

---

## 1. Targets
Molt supports multiple WASM deployment targets with different constraints.

### 1.1 Browser Target
- Host: JS runtime (browser, web worker).
- Interop: explicit schema-first boundary; no arbitrary object proxies.
- I/O: capability-gated; no blocking I/O.
- Threads: disabled by default; gated by `wasm-threads` availability.

### 1.2 WASI Target
- Host: WASI runtime (server, edge, sandbox).
- Interop: explicit ABI with host functions; no implicit host object access.
- I/O: capability-gated; WASI permissions required.
- Threads: optional; enabled only when host supports threads safely.

### 1.3 Edge Worker Target
- Host: provider worker runtime (JS + WASI blend).
- Interop: schema-first; only explicit host imports.
- I/O: capability-gated; provider-specific APIs must be explicit in docs.

---

## 2. Constraints

### 2.1 Determinism
- WASM builds must be deterministic when `--deterministic` is enabled.
- Nondeterministic capabilities (time, randomness, network) require explicit
  capability grants.

### 2.2 Portability
- No reliance on host-specific undefined behavior.
- Stable ABI for host calls; versioned with explicit compatibility policy.

### 2.3 Module Size And Cold Start
- Size and cold-start targets are defined in
  `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`.
- Every WASM release must report size + cold-start metrics.

---

## 3. Interop Rules
- All host boundaries are schema-first and versioned.
- No implicit JS object wrapping or dynamic import behavior.
- All host imports must be enumerated in the ABI manifest.

---

## 4. Capability Policy
- `db.read`/`db.write` for database access.
- `net.*` for network access (gated by target).
- `fs.*` for filesystem access (WASI only, gated).
- `time.*` and `rand.*` for nondeterminism (explicit grants).

---

## 5. Testing And Validation
- WASM parity tests must cover strings, bytes, memoryview, control flow, and
  async protocols.
- Each target must run the same parity suite unless explicitly documented.

---

## 6. Open Questions
- Default thread policy per target.
- ABI compatibility window and deprecation policy.
