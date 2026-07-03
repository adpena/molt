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

## 2. Feature Profiles

Feature profiles are semantic compatibility contracts, not size/performance
profiles and not marketing names. A build may select a richer feature profile
only when the target runner, post-link optimizer, and deployment host all pass
their checked-in support probes.

### 2.1 `wasm-mvp`
- Portable baseline for browser, WASI, and edge workers.
- Uses flattened function types and avoids recursive type groups.
- Keeps `--disable-gc` in Binaryen so post-link optimization cannot rewrap the
  type section into GC-only encodings.

### 2.2 `wasm-refs`
- Enables reference-types behavior when the target contract proves support.
- Allows `externref` for opaque host-managed capabilities.
- Must not proxy arbitrary JS objects or bypass the schema-first host boundary.

### 2.3 `wasm-gc`
- Enables WasmGC only as an explicit target contract.
- Requires runner/browser support probes, Binaryen `--enable-gc` validation,
  export-contract verification, and end-to-end Molt runner/browser tests.
- May lower only proven closed-layout Molt objects to WasmGC `struct`/`array`
  storage. Dynamic Python objects, extension ABI objects, and unknown package
  values remain in the ordinary handle/buffer/ABI lanes.
- Must publish size, cold-start, allocation-count, host-call-count, and
  throughput deltas against the matching `wasm-mvp` artifact before any support
  claim.

---

## 3. Constraints

### 3.1 Determinism
- WASM builds must be deterministic when `--deterministic` is enabled.
- Nondeterministic capabilities (time, randomness, network) require explicit
  capability grants.

### 3.2 Portability
- No reliance on host-specific undefined behavior.
- Stable ABI for host calls; versioned with explicit compatibility policy.
- Feature use is gated by the selected feature profile. No code path may infer
  support from browser family, CLI profile name, or optimizer availability.

### 3.3 Module Size And Cold Start
- Size and cold-start targets are defined in
  `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`.
- Every WASM release must report size + cold-start metrics.

---

## 4. Interop Rules
- All host boundaries are schema-first and versioned.
- No implicit JS object wrapping or dynamic import behavior.
- All Molt runtime imports must be enumerated in
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml` and regenerated with
  `tools/gen_wasm_abi.py`.
- `externref` and WasmGC references are not an escape hatch around the ABI. They
  are typed lowering/storage facts owned by the selected feature profile.

---

## 5. Capability Policy
- `db.read`/`db.write` for database access.
- `net.*` for network access (gated by target).
- `fs.*` for filesystem access (WASI only, gated).
- `time.*` and `rand.*` for nondeterminism (explicit grants).

---

## 6. Testing And Validation
- WASM parity tests must cover strings, bytes, memoryview, control flow, and
  async protocols.
- Each target must run the same parity suite unless explicitly documented.
- Feature-profile tests must validate the emitted artifact with the same runner
  and post-link optimizer configuration used by deployment. `wasm-gc` additionally
  requires browser E2E validation because browser VM GC behavior is part of the
  performance and lifetime contract.

---

## 7. Open Questions
- Default thread policy per target.
- ABI compatibility window and deprecation policy.
