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

Registry storage is process-global only to provide one address-stable membership
authority and to release learned capacity at embedded teardown. It carries an
atomic owner identity bound to the concrete heap `RuntimeState` before any
runtime initialization can allocate. A second simultaneous runtime identity is
rejected fail-closed; teardown can clear membership and release the owner only as
one all-shard transaction. Sequential re-initialization may then bind the empty
registry to its new state. Isolates share their parent runtime and therefore do
not create a competing heap owner.

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
versus full collection. The structural bound changes from `O(all tracked)` per
young sweep to `O(generation 0)` while full collection remains `O(all tracked)`.
