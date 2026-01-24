Title: Runtime Architecture Map
Status: Draft
Owner: runtime
Last Updated: 2026-01-24

## Summary
This document maps the Molt runtime's major subsystems, ownership boundaries,
lock invariants, and unsafe surface area. It provides a navigation guide for
refactors and is the canonical place to understand how runtime pieces fit
before splitting `molt-runtime` into focused modules.

## Current Top-Level Shape
- `runtime/molt-runtime`: runtime execution engine, builtins, scheduler,
  capability checks, and host-facing entrypoints. `runtime/molt-runtime/src/lib.rs`
  is now a router + re-export surface; core logic lives in focused modules under
  `state/`, `concurrency/`, `object/`, `builtins/`, `call/`, and `async_rt/`.
- `runtime/molt-obj-model`: NaN-boxed `MoltObject`, pointer registry, and
  object-representation helpers.
- `runtime/molt-backend`: codegen backend (lowering to runtime ABI).

## Core Subsystems (Current Locations)
### Runtime State
- Owner: runtime
- Current location: `runtime/molt-runtime/src/state/runtime_state.rs` (RuntimeState),
  with caches in `state/cache.rs`, TLS helpers in `state/tls.rs`, metrics in
  `state/metrics.rs`, and recursion tracking in `state/recursion.rs`.
- Contract: `docs/spec/areas/runtime/0024_RUNTIME_STATE_LIFECYCLE.md`.
- Notes: owns caches, pools, async registries, and the GIL-like lock.

### Concurrency + GIL
- Owner: runtime
- Current location: `runtime/molt-runtime/src/concurrency/gil.rs` (GilGuard + TLS depth).
- Contract: `docs/spec/areas/runtime/0026_CONCURRENCY_AND_GIL.md`.
- Notes: GIL guards runtime mutation; re-entrant per thread via TLS depth.

### Provenance + Pointer Registry
- Owner: runtime
- Current location: `runtime/molt-obj-model/src/lib.rs` (pointer registry) with
  adapters and ABI entrypoints in `runtime/molt-runtime/src/provenance/`
  (`handles.rs`, `pointer_registry.rs`).
- Notes: sharded pointer registry (read/write locks) for NaN-boxed pointers;
  handle resolve is centralized in `provenance/handles.rs`.

### Async Runtime
- Owner: runtime
- Current location: `runtime/molt-runtime/src/async_rt/` (scheduler, poll helpers,
  task pointer resolution, channels, sockets, and cancellation).
- Notes: custom scheduler + sleep queue; native I/O poller lives in runtime;
  WASM runs a single-threaded loop.

### Builtins + Dispatch
- Owner: runtime
- Current location: `runtime/molt-runtime/src/builtins/` (builtins) and
  `runtime/molt-runtime/src/call/` (dispatch + call bindings). Type helpers live
  in `runtime/molt-runtime/src/builtins/type_ops.rs`.
- Notes: attribute access, container ops, string/number kernels, and call
  dispatch are centralized here today.

### WASM Host Calls
- Owner: runtime
- Current location: `runtime/molt-runtime/src/lib.rs` (entrypoints + wasm table
  indices) + `wit/molt-runtime.wit`.
- Notes: runtime ABI for wasm targets and host bindings.

### WASM Parity Checklist (In Progress)
- Backend lowering: `runtime/molt-backend/src/wasm.rs` (imports table + ABI gaps).
- Host imports: remaining runtime imports tracked in `docs/spec/STATUS.md`
  (string formatting, `__str__`, file APIs, and sys/os placeholders).
- Async I/O: wasm socket readiness + `io_poller` host wiring (RT2, P0).
- DB parity: wasm client shims for `db_query`/`db_exec` (DB2).
- Tests: keep wasm parity suites in `tests/test_wasm_*.py` aligned with STATUS.

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
- `constants.rs` (shared runtime constants and counters)
- `utils.rs` (shared helper utilities)

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
