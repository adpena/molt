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
  handle resolve is centralized in `provenance/handles.rs`. Rust-owned opaque
  handles are minted only with `opaque_handle_bits`, which stores the raw Rust
  pointer in a dedicated sharded generational slab and returns a bounded
  synthetic immediate-int handle ID independent of the host address. Resolve
  is indexed O(1), while pointer-based release uses the shard-local reverse
  index; generations reject stale IDs after slot reuse. Pointer-tagged
  `MoltObject` bits are reserved for real Molt heap objects so refcount and
  finalizer paths never cast opaque host allocations as Molt headers.

### Async Runtime
- Owner: runtime
- Current location: `runtime/molt-runtime/src/async_rt/` (scheduler, poll helpers,
  task pointer resolution, channels, sockets, and cancellation).
- Notes: Rust owns each asyncio loop's ready stream and timer custody. Both
  `run_forever` and `run_until_complete` consume one captured turn; work scheduled
  by that turn waits for the next turn. Future callbacks and fd readiness enter
  the same Handle/context/exception-reporting path. Future waiters and ordinary
  callbacks share registration order; a waiter allocates a runtime promise only
  when suspension is needed. Callback timers and loop-owned sleeps share one
  clock and ordered index, with FIFO ties and O(log n) insertion, firing, and
  cancellation. Cancellation releases the owned callback/task edge immediately
  and removes both index entries, without deferred tombstones. Unbound `block_on`
  tasks retain their separate blocking owner; its worker acquires the GIL before
  claiming due task pointers. All wait discovery follows the same await graph
  without a semantic depth limit and detects cycles without allocation.
  An idle loop decides to park under its registry lock and blocks on its own
  parker until ready work, its earliest deadline, `stop()`, close, or signal/C
  pending-call work for the active process-main owner. Ready and earlier-deadline
  publications claim a parked loop under that lock and wake it after releasing
  it. Signals and successful C pending-call enqueues share one admitted park
  route; its arm/recheck fences preserve the pending-call ring's publication.
  Native parks release the GIL (Unix self-pipe `poll`, Windows auto-reset event).
  Linked WASM waits until the next deadline; registered
  host socket/WebSocket waiters bound a host wait to the shared 5 ms progress
  interval because the host ABI has no multiplexed readiness wait. Pending
  process waits and DB/process stream retries use that same interval through
  their scheduled awaiter's existing timer queue. Process retries pump host
  completion before checking exit. Loops without such work have no polling
  cap; `block_on` keeps its own capped waits. Native fd support and WASM capability gates
  remain target-specific. These implementation rules do not establish
  verified-subset or release performance acceptance.

### Builtins + Dispatch
- Owner: runtime
- Current location: `runtime/molt-runtime/src/builtins/` (builtins) and
  `runtime/molt-runtime/src/call/` (dispatch + call bindings). Type helpers live
  in `runtime/molt-runtime/src/builtins/type_ops.rs`.
- Notes: attribute access, container ops, string/number kernels, and call
  dispatch are centralized here today.
  Ordinary dictionary index lookup has one admission boundary in
  `object/ops/dict_set_tables.rs::dict_find_entry`: value and key/value
  reads, membership, and the lookup phases of pop, deletion, and
  setdefault share it. Exact `str` keys reuse the string's
  canonical cached hash and the existing `dict_exact_string_lookup`
  probe. A same-hash non-exact key makes that probe undecided, so ordinary
  equality runs in table order and restarts after dictionary mutation.
  String subclasses retain their hash/equality callbacks; reads preserve
  pending exceptions and borrow the stored key/value without extra owners.
  Reads, ordinary writes and setdefault share key-hash admission. Both
  setdefault forms use one known-hash probe followed by the shared
  reservation and append transaction, without replaying hash/equality
  callbacks. Returned defaults own one reference; the dictionary owns
  its separate edge. Prehashed writes and mapping merges keep their
  existing known-hash contract. Physical metadata byte reads remain
  callback-free storage queries, distinct from ordinary key equality.
  Class-policy lookups and instance-dictionary reads use this same runtime
  path on native and WASM; no class fact or lookup result is cached here.

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
