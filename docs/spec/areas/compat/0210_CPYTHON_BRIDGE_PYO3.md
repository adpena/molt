# CPython Bridge via PyO3 (Draft)
**Spec ID:** 0210
**Status:** Draft
**Audience:** runtime, compiler, tooling, packaging
**Goal:** Define a safe, capability-gated CPython bridge using PyO3 for compatibility
fallbacks without compromising Molt’s determinism or performance goals.

---

## 1) Scope
PyO3 enables CPython interoperability, not native performance. This spec defines
how Molt can *optionally* use embedded CPython for Tier C dependencies and legacy
extensions, while preserving correctness and security.

**Non-goals**
- Using PyO3 to speed up Molt-native execution.
- Allowing unrestricted CPython execution in deterministic builds.

---

## 2) Bridge Modes
**A) Worker process (default, preferred)**
- CPython runs out-of-process; IPC via Arrow IPC or MsgPack/CBOR.
- Strong isolation; deterministic mode can hard-deny nondeterministic APIs.

**B) Embedded CPython (PyO3, optional)**
- Opt-in feature flag only (`molt --enable-cpython-bridge`).
- Capability-scoped and disabled in strict deterministic builds.
- Intended for dev tools, migration, or controlled production rollout.

---

## 3) Capability & Determinism Rules
- Bridge access requires explicit capability grants:
  - `python:bridge`
  - `python:bridge:modules=<allowlist>`
  - `python:bridge:extensions=<allowlist>`
- Deterministic builds must reject the bridge unless explicitly allowed by
  a deterministic exception manifest (owned by the user).
- All bridge calls are treated as `io + nondet + mutates(all)` unless a stricter
  effect contract is declared for the target function.

---

## 4) ABI & Data Exchange
**No pointer sharing.** Only canonical values cross the boundary:
- scalar: int, float, bool, None
- text/binary: str (UTF-8), bytes, bytearray
- containers: list, tuple, dict (string keys preferred)

**Encoding**
- Default: MsgPack/CBOR for structured data
- Arrow IPC for columnar/tabular data

**Ownership**
- Molt owns Molt objects; CPython owns PyObjects.
- All values are copied or serialized across the boundary.
- No borrowed references or zero-copy buffers unless explicitly whitelisted.

---

## 5) Performance Guardrails
The bridge is expensive and should be treated as a coarse-grained boundary.

**Rules**
- No per-element calls inside loops; batch requests required.
- Hot loops must be Molt-native or Rust/WASM packages.
- Bridge calls must be visible in profiling and benchmark output.

**Targets (initial)**
- In-process call overhead target: <50 us (no serialization)
- Worker IPC target: <1 ms per request (batch payloads only)

If targets are exceeded, the bridge call must emit a warning in `--profile`
mode and flag the call site for migration.

---

## 6) API Surface (Proposed)
`molt.cpython_bridge` (Python-facing) and `molt-runtime-bridge` (Rust)

Required host hooks:
- `bridge_call(module: str, func: str, args: bytes) -> bytes`
- `bridge_eval(expr: str) -> bytes` (dev-only; disabled in prod)
- `bridge_import(module: str) -> bool`

The Rust runtime should expose a typed wrapper to encode/decode canonical
values and enforce capability checks before invocation.

---

## 7) Safety & Concurrency
- The bridge must hold the GIL only while calling CPython.
- No callbacks into Molt while holding the GIL.
- No shared mutation of Molt objects from the CPython side.
- Embedded mode must be single-threaded unless the sub-interpreter + per-GIL
  story is stable and audited (future work).

---

## 8) Tooling & UX
- `molt deps` must label dependencies as Tier C with a `bridge` reason.
- `molt run` should print a warning when bridge calls execute in production
  without a compatibility waiver.
- `molt verify` must fail if bridge dependencies appear in deterministic builds.

---

## 9) Testing Requirements
- Differential tests comparing CPython vs Molt bridge outputs.
- Fuzz tests for serialization boundaries (MsgPack/CBOR/Arrow).
- Stress tests for repeated bridge calls (memory leaks, GIL safety).

---

## 10) Rollout Phases
1. **Dev-only bridge** (no production use)
2. **Capability-gated embedded bridge** (feature flag)
3. **Worker-process bridge** (production default)
4. **Migration to native packages** (Tier B) for hot paths

TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge phase 1 (dev-only embedded CPython; no production).
TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge phase 2 (capability-gated embedded bridge + effect contracts).
TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P2, status:planned): CPython bridge phase 3 (worker-process default + Arrow/MsgPack/CBOR batching).
