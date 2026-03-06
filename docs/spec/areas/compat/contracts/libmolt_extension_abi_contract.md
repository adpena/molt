# libmolt Extension ABI Contract
**Spec ID:** 0217
**Status:** Draft
**Owner:** runtime + tooling
**Goal:** Define the stable ABI boundary and bounded source-compat header model
for C/C++ extensions recompiled against Molt.

---

## 1. Principles
- `libmolt` is a recompile target, not a `libpython` drop-in.
- Stable ABI and source-compat are different promises and must stay separated.
- `MOLT_C_API_VERSION` versions the stable ABI tier only.
- Compatibility overlays may grow to unblock real extension builds without
  implying CPython ABI compatibility.
- Private/generated upstream headers are never part of the `libmolt` contract.

---

## 2. Contract Tiers

### 2.1 Tier A: Stable ABI
- Canonical header: `include/molt/molt.h`
- Contract:
  - opaque `MoltHandle`-based object model
  - exported `molt_*` runtime symbols
  - `MOLT_C_API_VERSION`
- Stability promise:
  - major-versioned
  - intended to remain small, explicit, and toolable
  - the only header tier that downstream code should treat as ABI-stable

### 2.2 Tier B: CPython Source-Compat Facade
- Canonical entrypoints:
  - `include/Python.h`
  - `include/molt/Python.h`
  - small legacy forwarding headers such as `datetime.h`, `frameobject.h`,
    `pymem.h`, and `structmember.h`
- Contract:
  - source-level compatibility shims for high-value extension code
  - maps `Py*` names onto `molt_*` runtime primitives, helper macros, and
    fail-fast stubs where semantics are still missing
- Stability promise:
  - bounded and documented
  - not a frozen ABI surface
  - may expand between releases without changing `MOLT_C_API_VERSION`

### 2.3 Tier C: Ecosystem Overlays
- Current focus:
  - `include/numpy/*`
  - top-level NumPy forwarding/config bridge headers such as
    `arrayobject.h`, `_numpyconfig.h`, `config.h`, and
    `npy_cpu_dispatch_config.h`
- Contract:
  - unblock real-world ecosystem builds such as NumPy and pandas source trees
  - provide source-shape compatibility for selected public include lanes
- Stability promise:
  - compatibility-targeted, not ABI-stable
  - explicitly allowed to be partial
  - must stay bounded to shipped, documented headers

---

## 3. Explicit Exclusions
- No binary compatibility with CPython extension wheels.
- No promise that extensions using CPython private structs or direct object
  layout access will compile or run.
- No promise that private/generated third-party headers are shipped. This
  includes examples such as:
  - `numpy/arraytypes.h`
  - upstream `numpy/_core/src/**`
  - dispatch/generated header graphs created by upstream build systems
- No silent fallback to CPython or host Python at runtime.

---

## 4. Tooling Contract
- `molt extension build` must record the targeted header contract in
  `extension_manifest.json`.
- `molt extension scan` must evaluate support against an explicit, curated list
  of contract headers rather than an unbounded recursive header crawl.
- Tooling must report the distinction between:
  - stable ABI headers
  - source-compat headers
  - excluded private/generated headers

---

## 5. Practical Scope
- The goal is not “compile all Python extensions” in the CPython sense.
- The goal is:
  - compile extensions that can be recompiled against `libmolt`
  - preserve a narrow stable ABI core
  - add bounded source-compat overlays for high-value ecosystems
  - reject private/generated upstream build dependencies unless Molt chooses to
    ship an explicit compatibility overlay for them

This means:
- simple or Limited-API-style extensions should converge on `molt/molt.h` plus
  a small facade set
- high-value ecosystems such as NumPy may require additional source-compat
  overlays
- extensions that fundamentally require CPython internals remain out of scope
  for `libmolt` and belong, if anywhere, in the explicit bridge policy lane

---

## 6. Relationship To Other Specs
- C-API v0 surface: `docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md`
- C-API symbol coverage: `docs/spec/areas/compat/surfaces/c_api/c_api_symbol_matrix.md`
- CPython bridge policy: `docs/spec/areas/compat/contracts/cpython_bridge_policy.md`
