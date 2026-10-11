# Generational cyclic-GC authority

Status: implemented runtime authority; CPython 3.12 generational semantics.

Molt uses precise reference counting for acyclic lifetime and a tracing cycle
collector for the remainder. The collector has one scheduling authority in
`RuntimeState.gc` and one membership authority in `object::gc`:

- New cycle-capable allocations enter generation 0 with a monotonic allocation
  ordinal. Exact dict/tuple dynamic untracking changes membership, not allocation
  accounting.
- A generation-N collection snapshots only generations `0..=N`, sorts once by
  allocation ordinal, and promotes reachable and resurrected survivors to
  `min(N + 1, 2)`. This preserves deterministic finalizer, weakref, and clear
  order while removing old objects from young-collection cost.
- Counts and defaults match CPython 3.12: thresholds `(700, 10, 10)`, count 0 is
  GC allocations minus deallocations, and collection N resets counts `0..=N`
  while incrementing N+1. Automatic selection chooses the oldest due generation
  and retains CPython's oldest-generation 25% long-lived heuristic.
- Allocation schedules work only. The generated call-return/backedge eval-breaker
  poll owns automatic collection, so constructors never synchronously traverse a
  partially initialized graph.
- Collection statistics are per runtime and exposed through `gc.get_stats()`.
  Embedded teardown resets thresholds, counts, pending work, statistics, registry
  ordinals, and reusable workspace together.
- `freeze()` moves ordinary registry entries to permanent generation 3 without
  changing allocation order; `unfreeze()` returns them to generation 2. Normal
  snapshots and `get_objects()` exclude permanent entries, while referrer scans
  include every generation.
- The exhaustive heap-lifecycle dispatcher enumerates full NaN-boxed owned values.
  Cycle collection is its heap-pointer projection; `get_referents()` and
  `get_referrers()` retain inline values. Referrer targets are deduplicated into
  the reusable GC workspace's hash set, so the scan is
  `O(tracked objects + owned edges + targets)` rather than multiplying every
  edge by the argument count. This prevents public introspection from growing a
  second per-type traversal or quadratic membership authority.
- `callbacks` and `garbage` are canonical per-runtime rooted lists, cleared with
  module state at teardown. Collections snapshot callbacks before each `start`
  and `stop` phase, pass the CPython info dictionary, and report callback failures
  as unraisable. Normal PEP-442 collection leaves garbage empty;
  `DEBUG_SAVEALL` retains the final unreachable partition there without clearing
  its edges.

## Concurrency and target contract

The default deterministic-GIL and wasm32 builds use `Cell` control words, while
all targets use the same mutex-sharded membership authority. In deterministic
builds the GIL makes shard contention zero; retaining the same safe storage type
avoids an alternate unsafe-cell implementation for a measured sub-percent
steady-state difference. Explicit native `free-threaded` builds use lock-free
atomic words for bookkeeping and the same mutex-protected shards, but tracing
fails before snapshot or mutation until the scheduler supplies a real
stop-the-world epoch. A GIL-shaped compatibility lock is not a substitute.

The registry stores opaque provenance-safe pointers behind 64 shards. Snapshot
and promotion freeze all shards as one metadata transaction; graph traversal runs
after releasing registry locks. wasm32 and i686 use the same u64 allocation
ordinal and i64 trial-ref lanes, so pointer width cannot alter ordering or
reachability.

Registry storage is process-global to provide one address-stable membership
authority and to release learned capacity at embedded teardown. Its process owner
is bound before initialization allocates. Shared threads use that runtime;
explicit native isolates have separate `RuntimeState` owners. Every tracked
managed/native membership records its owning runtime. Collection, introspection,
freeze/unfreeze and permanent-generation counts select only that owner's entries,
so callbacks, version policy and GC counters operate on the same heap. Supported
isolate transport copies bytes; it does not transfer heap ownership. Untracked
exact dictionaries keep their header-only allocation accounting fast path.

The existing C thread-state registry also records runtime ownership. Isolate
class retirement checks its own C owners; process retirement still requires the
aggregate C/native owner sets to drain. Process-static extensions remain refused
in isolates before native GC admission. A successful isolate retires all its
memberships and C records before releasing its registry lifetime and state.
Failed teardown preserves both and keeps the failing thread's TLS runtime binding
through native thread exit: no fallback to primary ownership, state address reuse,
foreign membership clear, or subsequent process registry reset is allowed.

Synchronous C thread-state drains propagate invariant failures to their existing
runtime-owner boundary in every teardown mode. Embedding lifecycle entrypoints
then enter terminal `Failed` state and preserve the failing thread's exact C
record, managed edges and registry membership before clearing its runtime TLS
binding. That record leaves TLS destructor custody without invoking callbacks
or decrementing retained-owner counts; the failed runtime allocation remains
alive until process exit. A mismatched owner refuses retention and aborts.
Process exit reports the original failure and exits nonzero. A panic during an
actual native-thread TLS destructor still aborts immediately. Successful drains
retain their normal callback and record-retirement behavior.

Native spawn reserves the primary runtime through its existing CLOSED/count
execution-admission atomic before publishing work to an OS thread. The reservation
is transferable and does not pretend to be the caller's execution depth or C
attachment. Shared workers also reserve their actual origin RuntimeState, which
may be an isolate. Each origin closes its borrower admission before teardown;
after its owned services join, remaining external borrowers cause refusal before
collection, atexit, or root retirement.
Cancellation and native spawn failure release unpublished reservations. Successful
workers release them only after runtime TLS and C thread-state callbacks reach
the existing two-domain cleanup fixed point. Failed cleanup retains the origin
and process reservation. Process exit may exempt its exact current execution
depth; transferable reservations and foreign retained C records remain independent
owners. Reset checks lifecycle/admission invariants before changing publication.

Process service workers belong to the existing ProcessRegistry shutdown owner,
not permanent execution reservations which would prevent shutdown from reaching
its stop/join boundary. Registry closure must precede the first collection or
callback; task and stream owners remain live until every retained OS worker has
finished. Scheduler, sleep, I/O, async thread-pool and concurrent executor workers
retain their handles in one shared join-owner implementation within their existing
service fields. Each service is published before startup; its owner serializes
readiness and preserves partial-start failure without a retry lane. Constructors
pass the exact RuntimeState origin to the native worker. Each ordinary dispatch
acquires process admission, then an origin borrow, before GIL/ABI attachment;
WorkerEnd releases TLS before the origin and process counts. One process admission
atomic encodes CLOSED plus bounded ordinary and joined-service counts; admission
checks overflow within each field; valid releases decrement their owned field
with one atomic subtraction, and invalid releases are fail-stop invariant errors.
The existing execution-depth TLS word also carries the inherited role, so no
second service-depth authority is needed. Nested entries inherit the admitted
service role, while transferable shared/isolate spawn reservations remain ordinary.
Process exit may exempt only its own ordinary depth; a service callback cannot
retire its own join owner. Idle services hold no permanent execution reservation.

Teardown closes origin admission and stops and joins all published services before
the first collection. Admitted callbacks can finish under their retained exact
origin during Finalizing, including first C-API entry while its GIL is suspended.
Inherited entry reacquires GIL/attachment under its existing lifetime capability
and preserves the requested cleanup policy without reopening ordinary admission.
A callback waiting for GIL rechecks origin admission
before dispatch. Process retirement refuses ordinary/unjoined owners at preflight,
but drains joined-service counts. When admitted services retain C thread state,
the foreign-C-owner check follows their join. A remaining foreign C owner then
causes terminal retention before collection/atexit/root retirement; without active
services the existing nonmutating foreign-owner preflight refusal applies. A queued owned value whose admission is refused remains in
the service queue for GIL-held teardown. Terminal worker TLS cleanup uses explicit
retained-origin shutdown custody, including during primary Finalizing, and reaches
the runtime/C thread-state fixed point before clearing origin TLS. Panic or failed
join remains in the published owner and prevents origin destruction. A source-owned
join is not evidence that an unjoined worker finished. Native-no-net and no-native-
worker WASM services retain their target-specific behavior.

Nonwaiting executor shutdown retains the existing pool registry owner while
native work or failed dispositions remain. A per-runtime stopped-ID worklist
schedules successful completion reclamation; it owns no pool/handle/heap roots.
The existing generated async-work safepoint services the stopped-ID FIFO with
an eight-unit budget. Each turn inspects up to eight worker records; unfinished
owners return to the FIFO tail so they cannot block completed owners. Inspection
charges actual records, with a minimum one-unit charge for stale/empty IDs.
Closed pools reserve the same turn quantum for queued-message release. Thus a
hot poll inspects at most eight records and releases at most eight messages;
finalizer execution is not preemptible. Pool creation uses eight units, each
submission two units, and shutdown `2 * actual_worker_count + 8` units, stopping
early when the FIFO empties. Retirement effort follows actual work instead of
configured capacity. Live or failed workers remain retained; queued work rejected
after origin closure retires in bounded later turns.

Executor construction starts no native workers and imposes no arbitrary worker
limit. The Python constructor retains the original `max_workers` object and owns
its observable `<= 0` validation. Exact builtin integer, boolean and float limits
use a native ceiling equivalent to integer-count comparison, including positive
fractions, infinity and NaN. Larger-than-addressable positive ceilings allocate
no capacity; NaN admits construction but never starts workers. Other admitted
limits retain one runtime owner and use the canonical reflected comparison
protocol only when growth is needed, preserving Fraction, Decimal and custom
objects without coercion.

Each pool serializes submission and stop through its own gate. Registry locks
only publish, find or remove owners; the shared service handle lock remains
callback-free. A contended pool gate releases the GIL while waiting, so a limit
comparison can block on another pool without preventing its own resumption.
Each queued job owns one unfinished obligation through execution, callbacks,
reference destruction and execution-guard cleanup. Submission appends at most
one worker when unfinished demand exceeds the existing handle count and capacity
remains; settled idle workers are reused. The shared service handle owner
reserves storage before native startup and retains every successful handle for
join. Fixed services retain their one-shot readiness rule.

The dynamic limit owner is detached exactly once after worker join and queued
work retirement, outside pool and registry locks. Runtime teardown closes thread
admission before taking its owner cohort, and constructors recheck admission
under registry publication. Manual drains retain the exact pool/future cohort
across joins and callback-bearing destruction; a later cohort created by those
callbacks remains registered for a subsequent drain.

Following CPython, submission publishes its future before queueing the work and
then attempts native growth. Ordinary native startup refusal leaves the pool
usable and its accepted work queued; only the unreturned public future owner is
removed, outside callback-hostile locks. A later submission can retry startup.
Preparing handle storage can raise `MemoryError`; OS thread-start refusal raises
`RuntimeError`. Origin-refused requeue retains its unfinished obligation; actual
worker or stopped-queue retirement settles it exactly once.

A single shared handle transaction implements blocking join and completed-worker
reaping, releasing both owner locks and GIL before actual native joining.
Nonblocking in-progress joins remain pending; blocking callers wait for the
existing transaction's published disposition on the owner's condition variable,
without holding the GIL or owner mutex. Extracted handles retain their target
thread identity so self-join always refuses, including during another joiner's
transaction. Blocking join first rejects membership anywhere in the owner set,
before joining any earlier sibling, preventing reciprocal worker joins. Failures stay in the original registry and leave automatic polling.
Successful pools leave the registry only after joins and queued-owner release.
Reentrant polls cannot consume the same worklist entry; a cleanup unwind keeps
the registry owner and restores its pending ID. Synchronous runtime
retirement still stops and joins all owners before its fixed-point root drain.

The no-work native safepoint path performs one per-runtime pending-flag load,
allocates nothing, and takes no reaper/error-custody mutex or error-preservation
transaction. Worklist allocation occurs only on first nonwaiting/explicit pool
closure. The cursor and flag are scheduling summaries of the existing owner,
not alternative success/failure authorities. Blocking contention adds one
condition variable per existing service owner; its wait path is absent from
uncontended joins and empty safepoints. WASM has no native reaping work.
Empty safepoint overhead, completed join latency, repeated executor-cycle memory,
and callback-bearing batch latency require measurements on claimed targets;
source bounds do not establish a latency or RSS result.


Membership attribution adds one owner word per tracked managed/native entry and
per retained C record/registration. Native isolate spawn also reserves the process;
shared spawn reserves process and origin, with one admission atomic per native
RuntimeState. GREEN allocations and initially untracked exact dictionaries gain
no membership allocation or lookup. Scoped snapshots scan process-wide registry
metadata before traversing the selected owner's graph. Scoped retirement checks
also scan the existing registries. These costs and failure-retained storage have
not yet been measured across native targets; no per-owner index or performance
improvement is implied. Atomic-refcount free-threaded builds still refuse tracing
without a real stop-the-world epoch; this ownership correction does not remove
that hold.

## Performance and allocation proof

After workspace warm-up, repeated collection performs zero Rust heap allocations.
The cycle-capable allocation/deallocation probe separately records wall time,
allocator calls/bytes/peak live bytes, and the exact registry-access count over
4,096-object rounds. Five warm runs measured a 9.55 ms median round on
`origin/main` and 9.50 ms with a GIL-only shard cell (0.5%, within noise), with
equivalent allocation telemetry. The safe shared mutex authority is therefore
retained; the runtime profiler's contention-count and wait-nanosecond counters
make any future material free-threaded cost visible.
The generational probe holds a long-lived population, creates a small young set,
and attests both exact scanned-candidate counts and median/p95 latency for young
versus full collection. Graph traversal for a young sweep covers its selected generation-0 cohort,
while a full collection traverses the selected runtime's ordinary generations.
Both first inspect the shared registry metadata; young-collection wall cost is
not an `O(generation 0)` guarantee for that process-wide scan.

### Shared native thread call identity

An admitted shared call is retained in its native handle, owned by both the
spawning parent and the worker Arc. Callable, args and kwargs are released under
the origin runtime before worker cleanup; native creation failure releases them
in the parent. No Python-issued call token or process-global payload table is
used. Display metadata is a lazy per-RuntimeState registry keyed by the live
handle address and holding weak handles. Forgetting display metadata cannot
cancel an admitted call. A weak handle retains its allocation, preventing
address reuse until the metadata entry is retired. Native and WASM declarations
share the three-argument spawn/register ABI; WASM retains its capability refusal.
