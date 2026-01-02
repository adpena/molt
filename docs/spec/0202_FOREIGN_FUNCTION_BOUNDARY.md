# Molt Foreign Function Boundary (FFB) v0.1
**Status:** Draft  
**Goal:** Define safe, performant interop with non-Molt code while preserving correctness.

Molt’s long-term stance:
- Tier 0: no CPython C-extension loading.
- Tier 1: optional bridge mode with strict constraints.

## Interop modes
A) Molt Packages (preferred): Rust/WASM components with declared effects + ABI  
B) WASM modules: portable sandbox + capability boundary  
C) Legacy native extensions: bridge-only; opaque calls with conservative assumptions

## Core contract: Effects + Ownership
Effects (conservative categories):
- `pure`, `alloc`, `io`, `mutates(args=[...])`, `throws(types=[...])`, `nondet`
Unknown → assume worst-case: `io + nondet + mutates(all) + throws(any)`.

Ownership:
- who allocates/frees
- whether foreign code retains references (if yes → values are pinned/escaped)

## Compiler rules at the boundary
- No reordering/elimination across unknown effects
- No alias assumptions unless declared
- Guarded specialization only with explicit contracts

## WASM ABI sketch (recommended baseline)
- strings/bytes: ptr+len in linear memory (UTF-8)
- results: ok(ptr,len) or err(code,ptr,len)
- capability table for FS/network; deterministic mode optional

## Pandas and the “super useful” question
Reality: pandas depends heavily on native vectorized kernels (numpy/Cython). Molt cannot reliably “introspect compiled C” and safely optimize through it.

Molt strategies that *do* make it useful for pandas-like workloads:
1) Molt-native columnar stack (Arrow-like) + Rust kernels + pandas-subset API  
2) WASM analytics kernels (portable) called from Molt-compiled orchestration  
3) Bridge via process boundary: call CPython/pandas worker over Arrow IPC (migration path)

Recommendation: implement (3) early for practicality; (1) as the long-term win.

## Web development usefulness
Molt can deliver immediate wins by compiling:
- routing, middleware, business logic
- serialization
- DB row mapping fast paths
- async I/O without a CPython-style GIL

DB drivers: prefer Molt-native Rust clients; allow legacy behind FFB in Tier 1 bridge mode.

## Validation requirements
- Declared effect signatures (or worst-case default)
- ABI fuzz tests
- differential tests vs reference implementations
- stress tests for concurrency + memory safety
