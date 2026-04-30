Title: Runtime State Lifecycle and Shutdown
Status: Draft
Owner: runtime
Last Updated: 2026-04-30

## Summary
Molt's runtime uses process-global caches (builtins, interned names, module and
exception caches, capability state, and async registries). These
live for the life of the process and cannot be reclaimed, which blocks Miri
from passing leak checks and makes long-running processes accumulate memory.
This document defines a production-grade lifecycle with explicit init/shutdown,
full teardown of global caches, and a path to auditability.

## Goals
- Provide explicit `molt_runtime_init()` and `molt_runtime_shutdown()`.
- Allow full teardown of all runtime-global caches.
- Distinguish executable process exit from embedding teardown: native
  executables must run Python-level exit hooks and then hard-exit without
  C/Rust allocator or TLS destructor teardown.
- Preserve current fast paths (minimal overhead for steady-state execution).
- Enable Miri leak checks to pass without suppressing leaks.
- Prepare for optional allocation tracking and future GC/cycle collection.

## Non-Goals (Phase 1)
- Replace ref counting with a tracing GC.
- Introduce a full cycle collector.
- Require pervasive API changes in generated code or wasm ABI (unless unavoidable).

## Performance + Concurrency Constraints
- The steady-state runtime must not add new locks or dynamic dispatch in hot paths.
- Initialization/shutdown must be explicit and rare; no hidden work on every call.
- Async/coroutine and channel paths must remain zero-cost at runtime when the
  lifecycle is already initialized.

## Current Leak Sources (Non-Exhaustive)
- Builtin classes (`BuiltinClasses`) and their `__bases__`/`__mro__` tuples.
- Interned names (`INTERN_*`) and method tables (OnceLock values).
- Module cache, exception cache, last-exception tracking.
- Parse arena and TLS caches retain allocations until shutdown/thread exit.
- Capability cache and hash secret storage.
- Async registries (task exception stacks, cancel tokens, per-task maps).

## Proposed Architecture
### RuntimeState
Introduce a `RuntimeState` struct that owns all runtime-global state:
- Builtin classes and method table caches.
- Interned names and attribute name caches.
- Module/exception caches and last-exception tracking.
- Hash secret and capability cache.
- Async registries and task metadata maps.

Expose a single global pointer (fast path) to the active RuntimeState:
- `molt_runtime_init()` allocates and initializes the state, then publishes
  the pointer.
- `molt_runtime_shutdown()` revokes the pointer and tears down all state.

### Initialization
- Idempotent initialization (multiple calls return success).
- Strict ordering: intern base names first, then builtin classes, then caches.
- Fail fast if initialization fails; do not leave partial global state.

### Shutdown
- Requires runtime quiescence (no running tasks/threads).
- Drains caches (module/exception, intern tables, method caches).
- Flushes TLS caches.
- Decrefs builtin classes, tuples, and method objects.
- Clears async registries and task metadata.

### Executable Process Exit
- Native executable stubs and backend-generated `molt_main` success exits use
  `molt_runtime_exit(code)` rather than `molt_runtime_shutdown()`.
- `molt_runtime_exit` runs the safe Python-level process-exit subset once:
  worker quiescence, task/exception cleanup, `atexit` callback execution, and
  stdio flushing.
- `molt_runtime_exit` intentionally does not free `RuntimeState` or depend on
  Rust/C TLS destructors; it calls `_exit(code)` after
  Python-level finalization. Full state reclamation remains the explicit
  embedding/C-API `molt_runtime_shutdown()` contract.

## Implementation Status (2026-04-30)
- `molt_runtime_init()` is wired into generated entrypoints; executable exits
  route through `molt_runtime_exit()` for Python-level finalization plus
  hard-exit, while `molt_runtime_shutdown()` remains the explicit embedding
  teardown API.
- `RuntimeState` now owns builtin classes, interned/method caches, module/exception caches,
  hash/capability state, async registries, and argv storage (no lazy_static globals).
- TLS guard drains per-thread caches on thread exit; scheduler/sleep worker threads
  still participate in shutdown cleanup and are joined before teardown completes.
- Pointer registry is reset on shutdown so NaN-boxed addresses cannot outlive
  runtime teardown; object pointer resolution consults the registry to satisfy
  strict provenance tooling.
- Immediate object-address recycling pools were removed: NaN-boxed pointer
  identity can outlive refcount-zero in generated cleanup edges, so all
  allocator-backed objects now return directly to the allocator on decref.
- The implicit thread-local object nursery was removed from the default
  allocation path: without a global write barrier and function-exit reset
  contract, nursery objects could escape and later drop heap-backed payloads
  while their object headers remained addressable. Scope arenas remain the
  only bulk-reclaimed object storage and are marked explicitly with
  `HEADER_FLAG_ARENA`.
- Remaining: optional allocation registry + pointer registry lock overhead optimization (OPT-0003).

## Allocation Tracking (Phase 2)
Add an optional allocation registry for full teardown validation:
- Debug builds can enable full tracking by default.
- Release builds can opt-in for diagnostics.
- Registry supports leak detection and per-type summaries.

## GC/Cycle Collection Guidance
Ref counting remains the primary strategy in Phase 1. A cycle collector or
tracing GC is a separate milestone, because it would touch object layouts,
write barriers, and reachability semantics. This plan explicitly prepares the
groundwork (allocation registry + lifecycle control) to make that evolution
safe and measurable.

## Safety and Concurrency
- `molt_runtime_shutdown()` must acquire a global runtime lock (GIL or
  equivalent) to block concurrent access while tearing down.
- TLS caches must be drained on all threads or tracked and reclaimed at
  shutdown (scheduler/sleep worker threads now participate in shutdown cleanup).
- WASM host environments must wire lifecycle entrypoints where applicable.

## Implementation Plan
1. Create `RuntimeState` and move high-risk globals first (builtin classes,
   interned names, module cache, exception cache).
2. Provide init/shutdown entrypoints and wire in CLI/tests.
3. Migrate TLS caches into runtime-managed registries.
4. Add optional allocation registry and leak reports.
5. Gate all runtime entrypoints on a valid RuntimeState pointer.

## Test Plan
- Unit tests invoke init/shutdown and assert that caches are cleared.
- Miri runs with leak checks must pass (no `alloc` leaks).
- Stress tests exercise init/shutdown in loops (no growth).

## Open Questions
- Should shutdown be required or optional in production binaries?
- Do we need a per-runtime allocator arena for fast teardown?
