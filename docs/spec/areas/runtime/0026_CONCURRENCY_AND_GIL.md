# Concurrency And GIL Contract

## 1. Purpose
This document defines the runtime's thread-safety and locking contract: what is
serialized, what is permitted to run concurrently, and how locks must be
ordered to avoid deadlocks and performance regressions.

## 2. Definitions
- Runtime instance: a single `RuntimeState` with its owned caches, registries,
  scheduler state, and object model allocation pools.
- GIL: the global runtime execution lock that serializes mutation of runtime
  state and Python-visible objects.
- Runtime mutation: any operation that allocates, mutates, or frees runtime
  objects, or touches global caches/registries/scheduler state.
- Host thread: an OS thread that enters the runtime (e.g., worker threads,
  embedding API callers, or host callbacks).

## 3. Current Contract (RT1)
- Runtime execution is serialized within a process: a single GIL-like lock
  guards runtime mutation and Python-visible execution for the global
  `RuntimeState` singleton.
- The async scheduler runs with a single worker thread by default to preserve
  deterministic asyncio ordering; set `MOLT_ASYNC_THREADS` (>1) to opt in to
  parallel scheduling.
- The GIL is re-entrant per thread via a TLS depth counter; nested runtime calls
  must not deadlock.
- Runtime state and object headers are not thread-safe; `Value`/object headers
  are not `Send`/`Sync` unless explicitly stated in the object model spec.
- Cross-thread sharing of live Python objects is unsupported; data must be
  serialized or frozen before crossing threads.
- `threading.Thread` defaults to isolated runtime instances; thread targets/args are
  serialized and the thread object is not shared across runtimes.
- `threading.Thread` may opt into **shared-runtime** semantics when the
  `thread.shared` capability is enabled; threads then see shared module globals
  but still use serialized targets/args (no arbitrary object passing yet).
- WASM is single-threaded and runs a single scheduler loop; host calls must be
  non-blocking when holding the GIL.

## 4. Locking Model And Ordering
- The GIL is the outermost lock for runtime mutation.
- Provenance-sensitive subsystems (pointer registry / handle resolution) use
  internal sharded locks; resolve paths use read locks, registration uses write
  locks.
- Lock ordering is strictly: GIL -> handle table -> pointer registry. Locks must
  never be acquired in the reverse order.
- Runtime-internal mutexes (scheduler, async registries, object pools, caches)
  must only be acquired while holding the GIL unless a subsystem explicitly
  documents a GIL-free path.
- Host I/O, sleeps, or blocking calls must not occur while holding the GIL.

## 4.1 GIL-Exempt Operations (Explicit Exceptions)
- Runtime entrypoints that mutate state must acquire a `PyToken` via
  `with_gil`/`with_gil_entry`; any GIL-exempt entrypoints must be listed here.
- `molt_handle_resolve` is treated as GIL-exempt for long-term performance
  goals; it must remain read-only against runtime state and rely solely on the
  pointer registry's sharded read locks for safety.
- If `molt_handle_resolve` ever requires mutation or additional locks, the
  exception must be removed and this document updated before merging.

## 5. Planned Evolution
- Per-runtime GIL: move the GIL into `RuntimeState` so each worker thread owns a
  runtime instance, collapsing cross-thread contention.
- Lock scope reduction: lower handle-resolution overhead beyond the current
  sharded registry (lock-free read path or cheaper fast paths).
- Parallel in-process Python threads (long-term) require an explicit shared
  memory contract (freeze/share rules or per-object synchronization).

## 6. Tracking
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned):
  define the per-runtime GIL strategy, runtime instance ownership model, and
  the allowed cross-thread object sharing rules.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned):
  reduce handle-resolution overhead beyond the sharded registry and measure
  lock-sensitive benchmark deltas (attr access, container ops).
