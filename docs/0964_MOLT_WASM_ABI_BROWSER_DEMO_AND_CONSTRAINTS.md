# Molt WASM Platform v0.1 — ABI, Browser Demo, and Hard Constraints
**Document ID:** 0964
**Status:** Canonical (Foundational)
**Audience:** Molt compiler/runtime engineers, AI coding agents, WASM implementers
**Purpose:** Consolidate three critical foundations into a single, authoritative document:
1. WASM ABI specification (v0.1)
2. Browser demo definition aligned with Molt metrics
3. Explicit constraints and non-goals for WASM support

This document is **binding** for early Molt WASM development.

---

## PART I — WASM ABI SPECIFICATION (v0.1)

### 1. Design principles
- **Schema-first**: all data crossing the boundary is schema-defined
- **Deterministic**: no reflection-based behavior
- **Portable**: identical semantics in browser and server
- **Minimal**: no CPython compatibility guarantees

> Rule: *If it cannot be described by a schema, it cannot cross the ABI.*

---

### 2. ABI surface
The ABI consists of **four primitives only**:

1. `init(runtime_config)`
2. `call(function_id, payload, schema_id)`
3. `poll(task_id)`
4. `cancel(task_id)`

No direct memory access, no object proxies, no dynamic symbol lookup.

---

### 3. Data representation
All payloads use one of:
- **MsgPack** (default)
- **Arrow IPC** (tabular / bulk)

Each payload includes:
- `schema_id`
- `schema_version`
- `payload_bytes`

Schema IDs must match between browser and server builds.

---

### 4. Error model
Errors are structured and schema-defined:
- validation errors
- runtime errors
- cancellation errors

No stack traces cross the boundary by default.

---

### 5. Versioning rules
- ABI version is explicit (`abi_version = 0.1`)
- Backward-incompatible changes require a new ABI version
- Schema evolution rules apply (see 0921)

---

## PART II — CANONICAL BROWSER DEMO (METRICS-ALIGNED)

### 6. Purpose of the browser demo
The browser demo exists to prove:
- Molt WASM portability
- server ↔ browser symmetry
- schema-first execution
- real (not toy) usefulness

It is **not** a performance shootout with native Rust.

---

### 7. Demo definition: “Typed Data Explorer”

**Scenario**
- Browser loads a Molt WASM module
- User selects a dataset (JSON/CSV)
- Data is validated against a schema
- Transformations run inside Molt WASM
- Results rendered in the browser

**Operations**
- filter
- map
- aggregate
- paginate

All operations are:
- async
- cancellable
- schema-validated

---

### 8. Metrics alignment
This demo must report:
- startup time (cold/warm)
- WASM module size
- memory usage
- operation latency (P95)

These metrics map directly to 0960.

---

### 9. Server ↔ browser symmetry
The *same compiled module* must be runnable:
- in the browser (WASM)
- on the server (native)

Only the host runtime differs.

---

## PART III — HARD CONSTRAINTS AND NON-GOALS

### 10. What Molt WILL support in WASM
- schema-defined functions
- async execution
- cancellation
- deterministic IO abstractions
- explicit module loading

---

### 11. What Molt WILL NOT support in WASM (by design)
- full Python stdlib
- dynamic imports
- monkeypatching
- reflection-heavy metaprogramming
- object-level JS ↔ Python interop
- implicit global state

If a feature requires CPython semantics, it is **out of scope**.

---

### 12. AI AGENT RULES (MANDATORY)

When implementing Molt WASM features:

1. Do not emulate CPython
2. Do not pass raw objects across the ABI
3. Do not add implicit behavior “for convenience”
4. Prefer explicit failure over silent fallback
5. Validate everything at boundaries

Any design violating these rules must be rejected.

---

## 13. Success criteria
Molt WASM is successful when:
- browser demos share code with server binaries
- schemas are the single source of truth
- WASM artifacts are small and deterministic
- constraints are clear and enforceable

---

## 14. North star
> **Molt WASM is not “Python in the browser.”
> It is “portable, compiled Python semantics with explicit contracts.”**
