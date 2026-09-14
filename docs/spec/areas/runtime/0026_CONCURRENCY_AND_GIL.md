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
- `threading.Thread` defaults to shared-runtime semantics for CPython parity.
  `MOLT_THREAD_ISOLATED` explicitly selects isolated execution with the
  `ThreadSpawnIsolated` operation gate; missing shared-thread permission must
  never implicitly select isolated execution. Thread targets/args remain
  serialized across this boundary.
- WASM is single-threaded and runs a single scheduler loop; host calls must be
  non-blocking when holding the GIL.

### 3.1 Native Application Bootstrap Ownership

After initializing a fresh native isolate, the runtime invokes the final image's
`molt_isolate_bootstrap` while holding its GIL. Compiled applications own this
symbol in their compiler-generated object. Direct-link hosts, libtests,
integration tests, and runtime fuzz executables instead declare exactly one
`declare_app_bootstrap!` provider from `molt-runtime-core` (re-exported by
`molt-runtime`): a real `AppBootstrapProvider::Initializer`, or explicitly
`Unavailable` when the image contains no application to initialize. Unavailable
bootstrap emits `MOLT_APP_BOOTSTRAP_UNAVAILABLE` with its owning image label and
aborts; it cannot return a success-shaped `None` or zero. Dependencies must not
export a bootstrap via shared Cargo features, weak C symbols, or fuzzing cfgs.

The isolate bootstrap ABI is `() -> owned Molt object` on every target. Its
generated success and failure exits return explicit `None`; a `ret_void`-only
body has a void native signature and cannot implement this ABI. Ordinary
`molt_main` and `molt_host_init` wrappers retain their void native signatures.
Normal hosts invoke only `molt_main`, which owns setup. Host-export-only browser
entry requires `molt_host_init` and never substitutes raw isolate bootstrap.
Completed main initialization also satisfies later host-export initialization.

Initialization success is a prerequisite to payload execution or host-export
calls. Generated entry, host-init, isolate-bootstrap, and import wrappers
preserve pending setup exceptions and branch to their failure return before
subsequent initialization work; they never clear a newly raised setup exception.
Native isolate lifecycle captures success before reporting clears diagnostic
state, skips the payload on failure, and still tears down and completes its
thread handle. WASM hosts inspect the owning runtime instance's canonical
pending-exception export after startup and before snapshots or later host calls;
a missing status export is an ABI error, not evidence of success. Existing
formatted-error hosts report the original error. Minimal hosts retain pending
runtime state and emit `MOLT_APP_BOOTSTRAP_FAILED`.

This native link contract does not replace WASM's per-Store app-export
registration in `molt-wasm-host/src/isolate_host.rs`. WASM bootstrap and import
callbacks invoke the actual registered app exports; absent exports are errors.
Native imports use the installed module registry, not an app-owned
`molt_isolate_import` fallback.

### C-API runtime lifetime

The immutable runtime hook table, static C shell addresses, and loaded extension
code are process-owned. Shell-to-class bindings, type dictionaries/MROs/caches,
and executable-record lookup are runtime-owned. A runtime restart must retire
every exact old binding and rebuild type readiness; a process-once hook install
does not imply that the new runtime's classes have been published.

Static type retirement closes readiness before detaching the whole owned root
cohort. Reference destruction runs outside registry locks while the retiring
runtime remains available. Only a new runtime bootstrap reopens readiness.
Native isolates sharing a process cannot rebind the primary runtime's C shells;
extension admission must fail before loading or publishing in those isolates.
Separate WASM instances retain their independent static/runtime state.

A C-callable registration borrows its receiver. The resulting function owns it
through the ordinary traced closure graph; executable registries contain no
object references. ABI producers balance only conversion temporaries they own.
Thread-state shutdown drains runtime TLS and C error/context/dictionary edges
in one quiescence loop under destruction custody, before class bindings and
singleton storage are retired. Lifecycle locks never enclose finalizer calls.

Process exit, explicit embedding shutdown, native isolates and WASM instances
use the same ordered retirement and current-thread C/Molt-TLS fixed point;
their process-global ownership and registry-reset policies differ. An absent C
record is not fabricated just to drain runtime TLS. Native isolates do not own
the primary runtime's static C state or process-wide retained-thread count.

Canonical exception identities are independent of mutable Python `builtins`
bindings, with aliases sharing one owning cache entry. Unknown dynamically
resolved exception classes retain ordinary RC/GC semantics: cache removal does
not grant permission to dismantle a user class.

One typed class-reference slot family governs construction, replacement, GC
visitation and terminal detachment. Shutdown pins the canonical class cohort,
closes new weakref/cookie admission, and releases existing weakref callbacks and
namespace/annotation contents while name, metaclass, MRO and physical layout
remain readable. Reentrant callbacks can repopulate caches/TLS or materialize
another canonical class; observed ownership removal forces another drain pass.
Only quiescence plus validation of the remaining exact metadata/cohort graph
permits identity detachment. Exception/type/C-shell/builtin anchors and pins are
released before immortal singleton storage becomes mortal. No public ABI call
belongs in that callback-free tail. These are implementation invariants; target
and version support still requires the corresponding execution receipts.

Generic managed C type views also have interpreter lifetime. At finalization
their forward/reverse identities and stable runtime holds retire under the class
cohort pins, including views with outstanding direct C references. Those C
pointers cannot be dereferenced or decrefed after finalization. Process-static
C shells are distinct: their addresses survive, while their exact runtime
bindings are retired and rebuilt. Destruction custody remains active through
view and singleton release; cleanup must not create a new thread-state record
after the fixed point. Late callbacks may still import modules and publish code,
so module/extension state and code-cache owners participate in the same drain.

Root draining does not reset handle identity sequences. Executor, future,
event-loop, pipe-transport and cancellation handles remain distinct from any
registrations created by late callbacks in the same runtime. Removing an owner
is activity; a noninitial monotonic counter by itself is not. Pending executor
work retains its callable and arguments; futures own results and callbacks,
and non-waiting shutdown leaves worker join custody with the runtime. All
callback-bearing registry cohorts detach before release, outside their locks.
Late atexit registrations are released without starting a second exit phase.

Late callbacks may refill either ownership domain; a separate early C-state
cleanup is not sufficient. Only embedding teardown asserts the process-wide
retained-thread count is zero. WASM shutdown custody authorizes C bridge
callbacks without fabricating an ordinary application execution frame or
lifecycle lease. Process-static pending-call admission and its queue retire
only when the lifecycle authority identifies the owning runtime; isolate
teardown may neither consume/discard those callbacks nor close primary
admission. Isolates likewise cannot retire process-static CPython bindings.

Lifecycle tests use the shared cold-lifecycle transaction rather than an
ordinary exception/pending-call snapshot fixture: their body owns initialization
and permanent shutdown, so the fixture must not introduce a retained C record or
restore snapshots through a dead runtime. Cold lifecycle and trusted fresh-runtime
fixtures share production retirement/reset custody; only trusted bootstrap
temporarily changes capability policy and excludes competing initializers.
Ordinary test transactions retain their snapshot semantics. Fresh-runtime
extension/import tests run in the normal shard; obsolete permanent-shutdown
ignores must not hide them. Tests racing whole-runtime lifecycle transitions
still run in separate processes to avoid unrelated live owners.

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
- The Rust async runtime event loop and I/O poller now provide the default
  `asyncio` core: timer cancellation propagates into the runtime scheduler,
  readiness waiter cancellation is removed from poll queues, and equal-deadline
  timers plus surviving I/O waiters resume in deterministic FIFO order within a
  single loop turn.

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
- TODO(runtime, owner:runtime, milestone:RT2, priority:P1, status:planned): define the per-runtime GIL strategy, runtime instance ownership model, and the allowed cross-thread object sharing rules.
- TODO(perf, owner:runtime, milestone:RT2, priority:P1, status:planned): reduce handle-resolution overhead beyond the sharded registry and measure lock-sensitive benchmark deltas (attr access, container ops).
