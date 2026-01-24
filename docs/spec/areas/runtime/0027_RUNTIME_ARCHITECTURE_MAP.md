Title: Runtime Architecture Map
Status: Draft
Owner: runtime
Last Updated: 2026-01-23

## Summary
This document maps the Molt runtime's major subsystems, ownership boundaries,
lock invariants, and unsafe surface area. It provides a navigation guide for
refactors and is the canonical place to understand how runtime pieces fit
before splitting `molt-runtime` into focused modules.

## Current Top-Level Shape
- `runtime/molt-runtime`: runtime execution engine, builtins, scheduler,
  capability checks, and host-facing entrypoints. Most logic lives in
  `runtime/molt-runtime/src/lib.rs` today.
- `runtime/molt-obj-model`: NaN-boxed `MoltObject`, pointer registry, and
  object-representation helpers.
- `runtime/molt-backend`: codegen backend (lowering to runtime ABI).

## Core Subsystems (Current Locations)
### Runtime State
- Owner: runtime
- Current location: `runtime/molt-runtime/src/lib.rs` (RuntimeState).
- Contract: `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`.
- Notes: owns caches, pools, async registries, and the GIL-like lock.

### Concurrency + GIL
- Owner: runtime
- Current location: `runtime/molt-runtime/src/lib.rs` (GilGuard + TLS depth).
- Contract: `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`.
- Notes: GIL guards runtime mutation; re-entrant per thread via TLS depth.

### Provenance + Pointer Registry
- Owner: runtime
- Current location: `runtime/molt-obj-model/src/lib.rs`.
- Notes: sharded pointer registry (read/write locks) for NaN-boxed pointers.

### Async Runtime
- Owner: runtime
- Current location: `runtime/molt-runtime/src/lib.rs`.
- Notes: custom scheduler + sleep queue; native I/O poller lives in runtime;
  WASM runs a single-threaded loop.

### Builtins + Dispatch
- Owner: runtime
- Current location: `runtime/molt-runtime/src/lib.rs`.
- Notes: attribute access, container ops, string/number kernels, and call
  dispatch are centralized here today.

### WASM Host Calls
- Owner: runtime
- Current location: `runtime/molt-runtime/src/lib.rs` + `wit/molt-runtime.wit`.
- Notes: runtime ABI for wasm targets and host bindings.

## Locking Contract Summary
- The GIL is the outermost lock for runtime mutation.
- Pointer registry uses sharded RwLocks; resolve uses read locks, register/release
  use write locks.
- See `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md` for ordering rules.

## Unsafe Surface Area
Unsafe usage currently appears in several runtime hot paths (task polling,
FFI entrypoints, and pointer manipulation). The refactor target is to isolate
unsafe code in provenance/object modules with narrow, documented interfaces.

## Target Module Layout (Incremental, Planned)
This is the intended internal structure for `molt-runtime` during the refactor
(no behavioral change implied):

- `state/` (RuntimeState, init/shutdown, TLS cleanup, metrics)
- `concurrency/` (GIL guard, assertions, lock helpers)
- `provenance/` (handle resolution + pointer registry adapters)
- `object/` (headers, alloc, scanning)
- `builtins/` (attr, containers, strings, numbers, exceptions)
- `call/` (dispatch + frame logic)
- `async_rt/` (scheduler, sleep queue, cancellation)
- `wasm/` (host calls and wasm-specific adapters)

## Ownership Guide (Planned Modules)
- `runtime/molt-runtime/src/state/*`: runtime
- `runtime/molt-runtime/src/concurrency/*`: runtime
- `runtime/molt-runtime/src/provenance/*`: runtime (perf focus)
- `runtime/molt-runtime/src/object/*`: runtime
- `runtime/molt-runtime/src/async_rt/*`: runtime (async-runtime focus)
- `runtime/molt-runtime/src/builtins/*`: runtime
- `runtime/molt-runtime/src/call/*`: runtime
- `runtime/molt-runtime/src/wasm/*`: runtime

## Related Specs
- `docs/spec/areas/runtime/0003-runtime.md`
- `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`
- `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`
- `docs/spec/areas/runtime/0502_EXECUTION_ENGINE.md`
