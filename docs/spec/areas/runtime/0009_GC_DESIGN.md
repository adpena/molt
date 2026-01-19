# Molt GC Design: Hybrid RC + Generational Tracing
**Spec ID:** 0009
**Status:** Draft (implementation-targeting)
**Audience:** runtime engineers, compiler engineers
**Goal:** Combine deterministic reference counting with a generational tracing collector to reduce overhead on short-lived objects while preserving predictable resource release for FFI.

---

## 1. Summary
Molt uses a **hybrid memory manager**:
- **RC (Reference Counting)** for immediate reclamation and deterministic FFI resource release.
- **Generational tracing GC** to eliminate RC overhead on short-lived objects and to reclaim cycles.
- **Deterministic triggers** based on allocation bytes and epoch counters (no time-based triggers).

The design prioritizes predictable behavior, low latency, and multicore scalability.

---

## 2. Object Model and Header
All heap objects carry a header with RC and GC metadata:
```
struct MoltHeader {
    type_id: u32,
    ref_count: u32,
    poll_fn: u64,
    state: i64,
    size: usize,
    gc_flags: u32,
    gen_age: u16,
    pad: u16,
}
```
Notes:
- `gc_flags` includes mark bits and "in young" tracking.
- `gen_age` tracks promotions from nursery to old generation.

---

## 3. Allocation Strategy
### 3.1 Nursery (Young Generation)
- Default allocation target for managed objects.
- Uses bump-pointer allocation per thread.
- Objects in nursery start with **deferred RC** (local RC not yet materialized).

### 3.2 Old Generation
- Promoted after surviving `PROMOTION_AGE` young collections.
- Uses free-list allocation or segregated size classes.
- Full RC is materialized for objects that escape the nursery.

---

## 4. Reference Counting (RC)
### 4.1 Biased RC
- Objects begin with **biased RC** for the allocating thread.
- Non-atomic increments/decrements for thread-local references.
- Transition to shared RC (atomic) when published across threads.

### 4.2 RC and FFI
- FFI buffers and external resources use RC for deterministic release.
- No user-visible finalizers; release is internal and deterministic.

---

## 5. Generational Tracing GC
### 5.1 Young Collection
- Triggered by **nursery allocation bytes** (not time).
- Uses a stop-the-world but short scan of:
  - thread roots
  - task stacks
  - remembered sets (old -> young references)

### 5.2 Old Collection
- Triggered when old-gen allocated bytes exceed threshold or when cycle backlog grows.
- Performed incrementally using a tri-color marking strategy.
- Work budgeted in fixed steps per allocation epoch.

### 5.3 Write Barrier
- When storing a young reference into an old object, record the old object in the remembered set.
- Barrier is required for correctness and tuned for low overhead.

---

## 6. Cycle Detection
- Cycles that RC cannot reclaim are resolved by tracing GC.
- Objects are added to a **cycle candidate queue** when their RC drops to a low watermark but are not freed.
- The old-gen collector prioritizes these candidates.

---

## 7. Deterministic Trigger Policy
All GC activity is driven by deterministic, input-only counters:
- `nursery_bytes_since_last_gc`
- `old_bytes_since_last_gc`
- `cycle_candidate_bytes`
- `gc_epoch`

No wall-clock or system time triggers are allowed.

---

## 8. Target Throughput and Latency
These are engineering targets for the initial implementation:
- **Young GC pause target:** <= 2 ms per 16 MB nursery
- **Old GC work budget:** <= 5% CPU under steady allocation
- **Total GC overhead:** <= 10% on allocation-heavy benchmarks
- **Promotion rate:** <= 15% of nursery objects in typical workloads

---

## 9. Integration with Compiler and Runtime
- Compiler inserts write barriers on stores to heap objects.
- Runtime tracks allocation counters per thread and synchronizes GC epochs.
- Task scheduler must be able to yield to GC checkpoints at safe points.

## 9.1 Region/Arena Allocation for Temps
- Short-lived compiler/runtime temps use a bump arena to reduce allocator pressure.
- Arena allocations are reset at deterministic boundaries (parse/compile phases).
- Arena objects must not escape into the heap object graph.

---

## 10. Acceptance Criteria
- Deterministic output across runs with identical inputs and profiles.
- No leaks in cycle-heavy tests (graphs, lists of lists, closures).
- Measurable reduction in RC traffic for short-lived objects.
