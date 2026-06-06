<!-- Foundation audit 32. Architect: read-only research-granted agent, 2026-06-06.
Saved verbatim. 23 findings (C-01..C-12 complexity, L-01..L-11 laziness); most cross-
referenced to owned arcs (28 asyncio, 26 generators, 30 core-language, RuntimeSurfacePlan)
without duplication. HEADLINE NEW FINDING: glob.iglob is EAGER (glob_mod.rs:108) — a
behavioral-correctness violation (OOM-class on large trees, same class as the os.walk
history), scheduled for the next build slot. Plus: difflib missing b2j index, await-
waiter index rebuild inconsistency, two one-liner eagerness fixes. Includes the
COMPLEXITY RATCHET: tests/scaling/ harness (n/2n/4n wall+RSS ratio assertions) + the
doc-31 RSS-growth oracle extension. -->

# molt Algorithmic-Complexity + Eager-vs-Lazy Audit

**Scope:** Classes A (algorithmic complexity hazards) and B (eager-that-should-be-lazy). Findings not listed in owned arcs 20/26/28/29/30 are new. Findings already owned are cross-referenced without duplication.

---

## Part 1: Complexity Findings

### Finding C-01: `token_is_cancelled` O(d) parent-chain walk under lock on every task poll

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/cancellation.rs:341-358`

**Code pattern:**
```rust
let mut current = id;
let mut depth = 0;
while current != 0 && depth < 64 {
    let Some(entry) = map.get(&current) else { return false; };
    if entry.cancelled { return true; }
    current = entry.parent;
    depth += 1;
}
```

The `cancel_tokens` `Mutex<HashMap>` is held for the entire loop. Every task poll that reaches the cancellation check in `execute_task` (scheduler.rs:3038 area) calls this. For a flat cancel-token tree (all tasks using token depth 1) this is O(1). For a structured-concurrency tree with depth d, it is O(d) under lock.

**Worst-case:** O(d) per task poll where d is the cancel-token tree depth. With 64-cap and typical structured concurrency trees of depth 4-8, this is bounded but still 4-8 HashMap lookups under a single Mutex per poll. Every task poll acquires this lock regardless of whether the task has a non-default token.

**Realistic trigger:** `asyncio.TaskGroup` nesting, `TaskGroup` within `TaskGroup` creates depth-3 trees. Any application using structured concurrency.

**CPython bound:** CPython uses a simple flag on the `Task` object; `CancelledError` is raised by directly calling `task.cancel()`. No parent-chain walk per poll. O(1).

**Severity:** MEDIUM. The 64-depth cap prevents unbounded cost. The Mutex acquire itself is the larger overhead than the walk depth in practice. Doc 28 §1.3 item 8 also identifies this.

**Fix sketch:** Cache the "effective cancelled" state as a single bit per token, invalidated upward (dirty propagation) on `cancel()`. On poll, check the local cached bit (O(1) no-lock) before acquiring the mutex. Alternatively, inline the token's cancelled state into `MoltHeader.flags` as `HEADER_FLAG_CANCEL_PENDING` already does for the per-task pending bit — there is no need to re-walk the tree if the header flag is authoritative.

**Owned arc:** Doc 28 (asyncio frontier). Do not duplicate the fix design here; point to doc 28 Phase 3 (cancel-token redesign).

---

### Finding C-02: `wake_tasks_for_cancelled_tokens` O(T×d) scan: iterates all tasks × parent-walk per token

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/cancellation.rs:316-339`

```rust
for (token_id, tasks) in map.iter() {
    if token_is_cancelled(_py, *token_id) {  // O(d) under nested lock
        wake_list.extend(tasks.iter().copied());
    }
}
```

`cancel_tokens` map is already locked in `token_is_cancelled`, but `token_is_cancelled` acquires `cancel_tokens` again (re-entrant Mutex risk? No — it takes `cancel_tokens(_py).lock()` freshly, meaning the outer `map` borrow and the inner `token_is_cancelled` lock are the SAME Mutex. This is a **deadlock if `cancel_tokens` is a regular `Mutex`**. Verification: `cancel_tokens` returns `&runtime_state(_py).cancel_tokens` which is a `Mutex`. The outer `wake_tasks_for_cancelled_tokens` calls `let map = task_tokens_by_id(_py).lock()` (line 319 — it is `task_tokens_by_id`, not `cancel_tokens`), then inside the loop calls `token_is_cancelled` which locks `cancel_tokens`. These are different mutexes, so no deadlock. The complexity issue remains: T tokens × d depth per token.

**Worst-case:** O(T×d) where T = number of distinct token IDs. In a scenario with 10,000 tasks each with a unique token, cancelling the root token walks all 10,000 entries plus d parent hops per entry.

**Realistic trigger:** Large `asyncio.gather(...)` with custom cancel scopes.

**CPython bound:** CPython's `Task.cancel()` directly sets a flag on specific tasks, O(k) where k is the number of directly watched tasks. No T-scan.

**Severity:** HIGH for large task counts. The entire scan runs under GIL with multiple Mutex operations per token.

**Fix sketch:** Maintain a `cancelled_ids: HashSet<u64>` in RuntimeState, updated atomically when any token is cancelled. `wake_tasks_for_cancelled_tokens` then does one `HashSet` lookup per token rather than the full parent-walk. Owned by doc 28 Phase 3.

---

### Finding C-03: Five separate `HashMap<PtrSlot, _>` lookups per task poll — `task_exception_stacks`, `task_exception_handler_stacks`, `task_exception_depths`, `task_last_exceptions`, `task_results`

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:3034-3042`, `3085-3091` (context save/restore on each task poll enter/exit)

Each poll acquires five separate `Mutex<HashMap<PtrSlot, Vec<u64>>>` (or similar). That is 5 lock/unlock cycles, 5 HashMap key probes (PtrSlot = raw pointer, hash by pointer value), and 5 allocation-or-move operations (Vec take/store) per task iteration.

**Worst-case:** O(1) per poll for each map lookup (HashMap), but the constant factor is ~10 Mutex acquire/release cycles + 5 pointer hashes per task context switch. With 100k tasks doing `sleep(0)`, this is 1M Mutex operations/second from exception-state alone.

**CPython bound:** CPython tasks do not have this overhead; Python frame state is stack-based, not HashMap-based.

**Severity:** HIGH. Doc 28 §1.3 item 1 and item 6 call this the dominant overhead. The structural fix (inline all five fields into a per-task `TaskState` struct embedded in `MoltHeader` or an arena-allocated side table) is the doc 28 Phase 2 work.

**Owned arc:** Doc 28 Phase 2 (task-local state inlining). Point; do not duplicate.

---

### Finding C-04: `run_once` drain-and-collect Vec allocation per event loop iteration

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs:529`

```rust
let ready_batch: Vec<u64> = {
    let mut map = registry.loops.lock().unwrap();
    let Some(state) = map.get_mut(&loop_key) else { ... };
    state.ready.drain(..).collect()   // allocates Vec<u64> every run_once call
};
```

Every `run_once` iteration allocates a `Vec<u64>` to hold the drained callbacks, even when zero callbacks are pending.

**Worst-case:** O(n) allocation of n callback pointers per `run_once` call. At `asyncio.run()` calling `run_once` in a tight loop (10k–100k calls/sec), this is 10k–100k allocations/sec.

**CPython bound:** CPython's event loop uses a deque (`_ready`) and drains it in place without an intermediate allocation, swapping to a new deque atomically (Python object reuse).

**Severity:** MEDIUM. Allocation cost is proportional to the ready queue length. For typical single-task work, this is 1 element/alloc.

**Fix sketch:** Pre-allocate a reuse buffer in `EventLoopState`; swap the ready `VecDeque` with an empty one under lock, iterate the swapped-out one without re-acquiring. Doc 28 §2.3 (Ready Queue: Intrusive Linked List) is the structural fix.

**Owned arc:** Doc 28 Phase 1/2. Point; do not duplicate.

---

### Finding C-05: O(n²) string concatenation in loops — `s += x` emits `molt_add` per iteration, no builder

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops_arith.rs:61-113` (the `molt_add` / `molt_inplace_add` string path, no special treatment)

Frontend: `_augassign_op_kind` emits `INPLACE_ADD` for `s += x` where `x` is a string. The native backend emits `molt_inplace_add`. `molt_inplace_add` for strings falls through to `concat_bytes_like` which allocates a new buffer of `l_len + r_len`, copies both inputs, and frees the old buffer. Each iteration is O(current_length). Over n concatenations: O(1) + O(2) + ... + O(n) = O(n²) time and O(n²) total allocation traffic (assuming no immediate reclaim).

**Worst-case:** `s = ""; for x in large_list: s += str(x)` is Θ(n²) where n = len(large_list).

**Realistic trigger:** Any log/report builder, CSV or JSON serialization loop, path-building loop.

**CPython bound:** CPython implements the same O(n²) pattern for `str +=` in a loop — this is a known Python anti-pattern. The CPython docs recommend `"".join(parts)`. CPython does have a micro-optimization for single-ref string realloc in-place (CPython's unicode.c `unicode_concatenate` checks refcount == 1), but this only works if the string object has no other references. Molt does not implement this realloc shortcut. **The gap vs CPython is: CPython's realloc shortcut makes `s += x` amortized O(n) when the reference count allows it.** PyPy's JIT eliminates the allocation entirely via escape analysis.

**Severity:** HIGH. O(n²) time + O(n²) allocation for a common user pattern. doc 30 §5a (Family 5: String Builder) identifies this. The microbench `bench_str_concat_loop.py` is missing (doc 30 §microbench lane 2).

**Fix sketch (doc 30 verdict):** Either (a) extend `deforestation.rs` to recognize the `inplace_add(str, str)` loop pattern and convert to `[parts].join("")`, or (b) add a `StringBuilder` runtime type, or (c) implement the realloc shortcut in `molt_inplace_add` for the refcount-1 case. Option (c) is the smallest delta and narrows the CPython parity gap. Option (a) is the zero-allocation solution.

**Parity risk:** The realloc shortcut changes the observable address of the string object mid-loop but Python strings are immutable so no semantic divergence.

**Owned arc:** Doc 30 →  commissioning slot #38 (String Builder/Loop Concat). Not yet owned by any active implementation doc. **This is a new, unowned finding in need of a frontier doc.**

---

### Finding C-06: `asyncio.gather` / `await_waiter_register` — O(N) Mutex acquires at gather setup

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:1889-1951`

`await_waiter_register` acquires `task_waiting_on`, `await_waiters`, and `await_waiter_index_map` (three separate `lock()` calls) per edge registration. `asyncio.gather(*tasks)` registers N edges, costing 3N Mutex acquires.

**Worst-case:** `asyncio.gather(*[create_task(f()) for _ in range(N)])` = 3N Mutex acquires at gather time. For N=1000, that is 3000 Mutex acquire/release cycles.

**CPython bound:** CPython's `asyncio.gather` appends to a plain Python list; the "done callback" registration is 1 list.append per child, O(N) with no lock overhead (GIL already held).

**Severity:** MEDIUM–HIGH for large gather calls.

**Fix sketch:** Merge the three await-waiter maps into a single `Mutex<AwaitWaiterGraph>` struct. One lock, batch all N insertions. Doc 28 §1.3 item 7 already identifies this. Owned by doc 28.

---

### Finding C-07: Multi-comparison chaining via heap `LIST_NEW` cells — optimizer-opaque O(k) allocs per comparison chain

**File:line:** Frontend, `visit_Compare` — doc 30 §1b at line 58: "Multi-comparison chaining (e.g., 1 < x < 10): implemented via `LIST_NEW` cell + `STORE_INDEX` pairs — NOT SSA-phi-based."

Each intermediate result of a multi-way comparison chain (`a < b < c < d`) is materialized into a heap list cell and reloaded. For a k-way comparison chain: k-1 `alloc_list` calls, k-1 `STORE_INDEX` calls, k-2 `INDEX` loads.

**Worst-case:** k comparisons = k-1 heap allocations per evaluation. In a loop with a 3-way range check (`lo <= x <= hi`), this is 1 heap allocation per loop iteration.

**CPython bound:** CPython evaluates chained comparisons by short-circuit in the bytecode directly; intermediate `bool` results are never heap-allocated (they are CPython's singleton `True`/`False` objects).

**Severity:** MEDIUM. Common pattern in numeric bounds checks, range guards. Optimizer (GVN/SCCP/LICM) cannot see through the list cell to the scalar values.

**Fix sketch:** Replace the `LIST_NEW` cell pattern with PHI/if-else in `visit_Compare`. The intermediate `bool` is a scalar that belongs in an SSA phi, not a heap list cell. Doc 30 Batch A item 2 calls this out as a fix-only task.

**Owned arc:** Doc 30 Batch A. Not yet a commissioned implementation arc. New finding requiring a fix slot.

---

### Finding C-08: match/case lowers to heap-cell boolean flags — O(nesting_depth) allocs per match statement

**File:line:** `visitors/pattern_match.py:33-38` (the `_emit_match_cell` pattern); doc 30 §Family 8.

Every pattern-match check emits a `LIST_NEW` 1-element cell to hold the boolean match result. For a `match` statement with n cases each with depth-d sub-patterns, this is O(n×d) heap allocations per `match` evaluation. The cells are not reused across cases.

**Worst-case:** A `match` statement with 10 cases, each a 3-level nested `MatchClass`, allocates 30 boolean cells per call site invocation. Inside a hot loop, this produces 30 allocations × RC overhead per iteration.

**CPython bound:** CPython (3.10+) match/case compiles to `MATCH_MAPPING`, `MATCH_SEQUENCE`, `MATCH_CLASS`, `MATCH_KEYS` bytecode ops that push directly onto the value stack — no heap allocation for the pattern flags.

**Severity:** MEDIUM. Common in user code using structured pattern dispatch. All optimizer passes (SCCP, GVN, LICM, type_guard_hoist) are blind to match-condition semantics.

**Fix sketch:** Doc 30 §Family 8 Verdict: lower `match/case` to CFG diamonds with SSA-phi booleans and typed `isinstance` type guards. This enables `type_guard_hoist` and `block_versioning` to fire on post-match branches.

**Owned arc:** Doc 30 frontier-doc commission slot #40 (renumbered from #33). Not yet a commissioned implementation doc. **New, unowned, requiring a frontier doc.**

---

### Finding C-09: `difflib.get_close_matches` — O(M×W²) per call

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/difflib.rs:511-535`

`get_close_matches_impl` calls `get_matching_blocks_impl` (which calls `find_longest_match` recursively) for each of M possibilities against the word of length W. Each `find_longest_match` is O(|a|×|b|). The recursive LCS decomposition via `matching_blocks_recursive` has O(|a|×|b|) per level and recurses O(min(|a|,|b|)) times in the worst case, giving O(|a|²×|b|) or O(|a|×|b|²) worst case. For `get_close_matches` with M entries of average length W: O(M×W²) overall.

**Worst-case trigger:** `difflib.get_close_matches(word, large_dict.keys())` with a large vocabulary (e.g., 10,000 words of length 20) = O(10000 × 400) = O(4,000,000) per call.

**CPython bound:** CPython uses the same Ratcliff/Obershelp algorithm with the same asymptotic complexity. However CPython's `SequenceMatcher` maintains a `b2j` dict that indexes positions of each element in `b`, converting `find_longest_match` from O(|a|×|b|) to O(|a|×count_of_matches). For typical English words (low match density), this is much faster than the naive double-loop. Molt's `find_longest_match` at line 46-57 uses two rolling arrays (`j2len`/`new_j2len`) which is the same space-optimized DP but without CPython's `b2j` index. The missing `b2j` pre-index makes Molt's implementation slower by a constant factor on average but same worst-case.

**Severity:** LOW–MEDIUM. Not a hot path but called in error messages and interactive tools (Python's `SyntaxError: did you mean...`).

**Fix sketch:** Add a `b_to_positions: HashMap<char, Vec<usize>>` pre-index in `find_longest_match`, built once from `b[blo..bhi]`, mirroring CPython's `b2j`. This reduces average-case from O(|a|×|b|) to O(|a|×avg_match_count).

**Parity risk:** None; same output, faster path.

---

### Finding C-10: `timer drain` in `run_once` — O(k) Mutex re-acquires per expired timer

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs:551-591`

The timer-drain loop re-acquires `registry.loops.lock()` on every iteration to peek/pop one timer from the BinaryHeap, then releases and re-acquires again to check the cancelled set. For a batch of k expired timers, this is O(k) Mutex acquire/release pairs.

**Worst-case:** 1000 timers fire simultaneously (e.g., `asyncio.sleep(1.0)` for 1000 tasks) = 2000 Mutex acquire/release cycles to process one `run_once` batch.

**CPython bound:** CPython processes all expired timers in a single pass within the lock, no re-acquisition.

**Severity:** LOW–MEDIUM. The GIL already serializes execution, so Mutex contention is zero, but the lock/unlock overhead itself (cache line bouncing) remains. Doc 28 §1.3 item 2 identifies this as the second most critical timer bottleneck.

**Fix sketch:** Collect all expired timers into a local `Vec` under a single lock acquire, drop the lock, then process callbacks outside the lock. Owned by doc 28 Phase 1.

---

### Finding C-11: `glob.iglob` eager materialization — semantically wrong

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/glob_mod.rs:108-116`

```rust
/// `glob.iglob(...)` - same as glob but returns a list (in Molt, generators
/// compile to eager lists; this is semantically equivalent).
pub extern "C" fn molt_glob_iglob(...) -> u64 {
    // In AOT context, iglob == glob (no lazy iteration).
    molt_glob_glob(...)
}
```

`glob.iglob` is documented as returning a lazy iterator; the comment justifies making it eager via "generators compile to eager lists." This is **incorrect**: `glob.iglob` is specifically designed for large file trees where materializing the full list OOMs. The semantic contract is that `iglob` yields paths one at a time. Calling `molt_glob_glob` (which materializes into a list) before returning means a directory tree with 500,000 entries materializes all paths into memory before the caller can process the first one.

**Worst-case:** `for f in iglob("/large/tree/**/*")` on a 500k-file tree allocates 500k path strings before yielding any. This is an OOM source similar to the `os.walk` eager-listdir history.

**CPython bound:** `glob.iglob` is a generator; paths are produced on demand from `os.scandir`. O(1) memory with respect to directory size.

**Severity:** HIGH. Behavioral correctness violation — it is not "semantically equivalent," it changes the memory profile and breaks parity for large trees.

**Fix sketch:** Implement `molt_glob_iglob` as a proper lazy iterator (TYPE_ID_GENERATOR or a new TYPE_ID_GLOB_ITER) backed by Rust's `glob::glob` which already returns a lazy `Paths` iterator. The lazy wrapper holds the `Paths` iterator and produces one element per `IterNext` call.

**Parity risk:** None; the fix matches CPython behavior exactly. However the generator-fusion arc (D1) needs to be complete before `for f in iglob(...)` compiles to an optimized loop; before D1, this will still produce pair-per-yield allocations, just with correct memory semantics.

---

### Finding C-12: `await_waiter_register` calls `rebuild_unique_index` on index inconsistency — O(N) rebuild under lock

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:1931-1934`

```rust
if waiter_index.positions.len() != waiters.len() {
    waiter_index.positions = rebuild_unique_index(waiters.as_slice());
    ...
}
```

`rebuild_unique_index` iterates the entire `waiters` Vec and rebuilds the `HashMap<T, usize>` from scratch. This is O(N) where N is the number of existing waiters. This fires whenever the index falls out of sync with the Vec (which happens after `swap_remove` operations due to the index not always tracking perfectly).

**Worst-case:** `asyncio.gather(*N_tasks)` where tasks complete in non-FIFO order causes repeated full rebuilds during unregistration. Each rebuild is O(N) under the await-waiters Mutex.

**CPython bound:** CPython does not maintain such an index; plain list operations.

**Severity:** LOW–MEDIUM. The index is designed to prevent O(N) linear scans in `swap_remove`, but the fallback rebuild negates this benefit.

**Fix sketch:** Fix `indexed_unique_vec_swap_remove` to not leave the index in an inconsistent state, eliminating the need for the rebuild trigger. The inconsistency arises because `swap_remove` updates the swapped element's position but not the removed element — the index entry for the last element is updated but `index.len()` can diverge from `values.len()` if insertions happen without corresponding index updates. Audit all insertion paths for completeness.

---

## Part 2: Eager-vs-Lazy Findings

### Finding L-01: `glob.iglob` — already documented as C-11 above. Cross-reference.

---

### Finding L-02: `asyncio_task_registry_values_impl` — eager O(N) Vec materializes all tasks for each `all_tasks()` call

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:871-880`

```rust
fn asyncio_task_registry_values_impl(_py: &PyToken<'_>) -> u64 {
    let guard = asyncio_task_map(_py).lock().unwrap();
    let values = guard.values().copied().collect::<Vec<_>>();  // eager
    drop(guard);
    let ptr = alloc_list(_py, values.as_slice());
    ...
}
```

`asyncio.all_tasks()` always materializes a full list of all registered tasks. If only 1 task is needed (e.g., checking if any tasks are alive), the caller still pays O(N) for all N tasks.

**CPython bound:** CPython 3.12 `asyncio.all_tasks()` uses a `WeakSet` and returns a `set` — also O(N) to materialize, but the set is not constructed unless called. The underlying `_all_tasks` is a `WeakSet` which is iterated lazily in C.

**Severity:** LOW. Typical task counts are small (< 1000). Called rarely (debug/shutdown paths).

**Fix sketch:** Add a lazy iterator variant for internal callers that only need to check "is any task alive" without full materialization. For the Python-level `all_tasks()` semantic, O(N) is correct.

**Parity risk:** None; this is an internal optimization only.

---

### Finding L-03: `asyncio_task_registry_live_values_impl` — calls `done()` method on every task eagerly before returning

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:1144-1231`

The `molt_asyncio_task_registry_live` function iterates all registered tasks, calls `molt_getattr_builtin` to get the `done` method, calls the method, and filters. This is O(N) `getattr` + method calls at list-construction time, even if the caller only needs to check the first alive task.

**CPython bound:** CPython's internal `_all_tasks` WeakSet iteration does not call `done()` per element; done state is checked only at the Python asyncio layer.

**Severity:** LOW.

---

### Finding L-04: `asyncio_event_waiters_cleanup_token_impl` — `.filter(|bits| *bits != 0).collect()` eager allocation

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:979`

```rust
let waiters: Vec<u64> = raw_waiters.into_iter().filter(|bits| *bits != 0).collect();
```

Allocates a new filtered `Vec<u64>` from the drained waiter list. For cleanup on an event with many tombstoned (zeroed) entries, this reallocates memory that could be avoided by iterating in-place.

**CPython bound:** Python list.remove() modifies in-place. The Rust analog here would be `retain(|bits| *bits != 0)` on the Vec, which is in-place with no reallocation.

**Severity:** TRIVIAL. One-line fix.

**Fix sketch:** Replace `raw_waiters.into_iter().filter(...).collect()` with `raw_waiters.retain(|bits| *bits != 0); raw_waiters`. This is in-place with the same semantics.

---

### Finding L-05: Startup eager module-init — 574KB `sys.py` initializes unconditionally

**File:line:** MEMORY.md `project_startup_baseline_20260603.md`; runtime init phase trace.

The 574KB `sys.py` body always runs at startup (+2.15ms full vs micro). This is the `MODULE_IMPORT` eager-init noted in the startup baton. Content includes large string tables, platform detection code, and attribute initialization that is irrelevant for programs not using `sys`.

**CPython bound:** CPython uses lazy module execution — `sys` is always imported, but its initialization is minimal and the stdlib is compiled bytecode. Molt runs the full 574KB script.

**Severity:** MEDIUM for startup-sensitive CLI tools. The `molt.gpu` and networking subsystems add further overhead when linked.

**Fix sketch:** Defer `sys` module attribute construction behind a `MODULE_IMPORT` boundary so attributes are computed on first access, not at startup. Already identified in `tmp/design_startup_deferred_init.md` and the memory notes. Owned by the RuntimeSurfacePlan sprint.

**Parity risk:** MEDIUM. `sys` module attributes must be available immediately when CPython code accesses them; lazy init requires careful first-access detection that must not cause observable ordering divergence.

---

### Finding L-06: `dict_iter` path for dict keys — calls `molt_dict_keys` (view alloc) then wraps in an `ITER` object

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops_iter.rs:659-676`

When `molt_iter(dict_bits)` is called:
1. `molt_dict_keys(dict_bits)` allocates a `dict_keys_view` object (line 659-660)
2. An `ITER` wrapper object is allocated (line 668)

That is two allocations (view + iter) for iterating a dict's keys. CPython allocates one `dictkey_iterator` object backed directly by the dict.

**CPython bound:** `iter(d)` in CPython allocates a single `dictkey_iterator` object. O(1), one allocation.

**Severity:** LOW. The view allocation is necessary for dict.keys() semantics. But when the view is only used as an iterator and immediately discarded, it is an unnecessary intermediate object.

**Fix sketch:** When the type_id is `TYPE_ID_DICT` and the call path is `molt_iter(dict_bits)` (not an explicit `dict.keys()`), allocate a `TYPE_ID_ITER` directly backed by the dict without the intermediate view. This requires a new iterator type or a flag on the existing ITER to distinguish "dict-backed iter" from "view-backed iter."

---

### Finding L-07: `stealers.clone()` — Vec clone on worker thread spawn

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs:2818`

```rust
let stealers_clone = stealers.clone();
```

`stealers` is a `Vec<Stealer<MoltTask>>`. This clones the entire Vec (one `Arc`-increment per stealer) per spawned worker thread. This happens once at scheduler initialization so it is not a hot-path issue. Noted for completeness.

**Severity:** TRIVIAL. Startup-only, bounded by thread count.

---

### Finding L-08: Generator pair-per-yield allocation (40 bytes per `next()`)

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs` — per doc 26 §1.4 item 7.

Already fully documented in doc 26 §1.4. Every `yield v` allocates a 40-byte `(value, done_flag)` tuple. The structural fix is generator fusion (D1). **Point to doc 26; do not duplicate.**

---

### Finding L-09: Per-resume exception-stack Vec clone — context swap on every generator send, even for non-exception generators

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs` (line 384 area, per doc 26 §1.3 and the seed claim)

Already documented in doc 26 §1.3 and in the MEMORY note for doc 28. The context swap unconditionally saves/restores `ACTIVE_EXCEPTION_STACK` and `EXCEPTION_STACK` (two `Vec<u64>` via `std::mem::take`) plus the exception depth counter. For generators with no try/except blocks, this overhead is pure waste. The C2 fix (`430e09793`) ensures correctness; the performance optimization (gate the swap on `has_exception_handlers()`) is the doc 26 residual. **Point to doc 26; do not duplicate.**

---

### Finding L-10: `logging` module — format string is evaluated before the level check when using `%`-style args

**File:line:** The Python `logging` module (CPython stdlib, running interpreted through molt).

When user code writes `logging.debug("Result: %s", expensive_computation())`, the `expensive_computation()` call is evaluated before `logging.debug` can check whether the DEBUG level is enabled. This is the standard Python logging anti-pattern; the idiomatic fix is `if logger.isEnabledFor(logging.DEBUG): logger.debug(...)` or using lazy string formatting. This is not a molt-specific bug — it is the standard Python behavior — but it is worth noting as a parity-neutral eager-eval hazard.

**CPython bound:** Identical behavior. CPython's `logging` module evaluates args before the level check for the same reason.

**Parity risk:** None — this matches CPython. **No fix needed**; note for the user that logging lazy eval is the user's responsibility, same as CPython.

---

### Finding L-11: dict.update() from a dict — calls `molt_dict_items` (allocates view) then iterates

**File:line:** `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops_dict.rs:115-123`

```rust
if object_type_id(ptr) == TYPE_ID_DICT {
    let iter_bits = molt_dict_items(other_bits);  // allocates items-view
    ...
}
```

`dict.update(other)` when `other` is a dict allocates a `dict_items_view` object just to iterate it. An optimized implementation would iterate the dict's internal storage directly without creating the intermediate view object.

**CPython bound:** CPython's `dict_merge` (Objects/dictobject.c) copies directly from the internal hash table without creating a view iterator.

**Severity:** LOW. Called on dict.update() hot paths in code that merges dicts in loops.

**Fix sketch:** Add a `dict_iter_items_direct(dict_ptr) -> impl Iterator<(u64, u64)>` internal function and use it in dict.update() to avoid the view allocation. Alternatively, the `dict_iter_take_pairs` pattern already in the codebase may serve.

---

## Part 3: Complexity Ratchet Design

### Scaling-Test Harness Sketch

The problem: O(n) code can silently regress to O(n²) and existing tests will not catch it unless inputs are scaled.

**Proposed design: `tests/scaling/` harness with scaling assertions**

Each scaling test runs the subject operation at sizes n, 2n, 4n (three data points), measures wall time and peak RSS, and asserts the ratio of ratios is within a bound that proves subquadratic scaling.

For an O(n) operation: `T(4n)/T(2n) ≈ 2.0 ± ε` (linear).
For an O(n²) operation: `T(4n)/T(2n) ≈ 4.0`.

A test asserts `ratio ≤ 3.0` (allows for log factors, cache effects).

**Implementation sketch (tests/scaling/harness.py):**

```python
import time, os, resource

def scaling_assert(fn, sizes, max_ratio=3.0, label=""):
    """fn(n) must run in O(n) or better."""
    times = []
    for n in sizes:
        t0 = time.perf_counter()
        fn(n)
        times.append(time.perf_counter() - t0)
    ratio = times[-1] / times[-2]  # T(4n) / T(2n) for sizes=[n,2n,4n]
    assert ratio <= max_ratio, f"{label}: scaling ratio {ratio:.2f} > {max_ratio} (expected ≤ 2x)"

def rss_scaling_assert(fn, sizes, max_rss_ratio=2.5, label=""):
    """RSS growth must be subquadratic."""
    rss_vals = []
    for n in sizes:
        fn(n)
        rss_vals.append(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    ratio = rss_vals[-1] / rss_vals[-2]
    assert ratio <= max_rss_ratio, f"{label}: RSS ratio {ratio:.2f} > {max_rss_ratio}"
```

**Specific scaling test definitions:**

| Test | fn(n) | assert |
|------|--------|--------|
| C-05: string concat | `s=""; [s:=s+str(i) for i in range(n)]` | wall ≤ 3.0× at 2n→4n |
| C-07: multi-compare | `[1<x<n for x in range(n)]` | wall ≤ 2.5× |
| C-08: match/case depth | match on n-wide class, n times | wall ≤ 2.5× |
| C-01: cancel depth | cancel-token depth-16 tree, n tasks | wall ≤ 2.5× |
| C-06: gather N | `asyncio.gather(*[noop() for _ in range(n)])` | wall ≤ 2.5× |
| C-11: iglob large tree | `list(glob.iglob(pattern))` on n-file tree | RSS ≤ 2.5× |

**Integration with doc 31 (fuzzing lane):** Doc 31's oracle framework should add an RSS-growth oracle that, for scaled fuzz inputs (n and 2n element seeds), asserts `RSS(2n_input) ≤ 4 × RSS(n_input)`. Any fuzz corpus entry that violates this ratio is a potential O(n²) regression and should be filed as a complexity bug. The existing `fuzz_ir_passes.rs` target at `/Users/adpena/Projects/molt/fuzz/fuzz_targets/fuzz_ir_passes.rs` is the right hook for backend-side complexity checks.

---

## Per-Finding Table

| ID | Class | File:line anchor | Bound | Trigger | CPython bound | Severity | Fix sketch | Parity risk | Owned arc |
|-----|-------|------------------|-------|---------|---------------|----------|------------|-------------|-----------|
| C-01 | Complexity | cancellation.rs:341 | O(d) per poll, d=depth | structured concurrency nesting | O(1) flag | MEDIUM | cache cancelled bit in header | none | Doc 28 Phase 3 |
| C-02 | Complexity | cancellation.rs:316 | O(T×d) on cancel | large gather + cancel | O(k) direct wakeup | HIGH | `cancelled_ids` HashSet | none | Doc 28 Phase 3 |
| C-03 | Complexity | scheduler.rs:3034 | 5 Mutex/poll (O(1) each) | any task poll | stack-based, 0 locks | HIGH | inline fields into task header | none | Doc 28 Phase 2 |
| C-04 | Complexity | event_loop.rs:529 | O(n) alloc per run_once | asyncio.run() tight loop | deque swap, no alloc | MEDIUM | intrusive list (doc 28 §2.1) | none | Doc 28 Phase 1 |
| C-05 | Complexity | ops_arith.rs:61 | O(n²) time + alloc | `s += x` in loop | O(n) amortized (refcount shortcut) | HIGH | realloc shortcut or deforestation | none | NEW — doc slot #38 |
| C-06 | Complexity | scheduler.rs:1889 | O(3N) Mutex at gather | asyncio.gather(N_tasks) | O(N) no-lock | MEDIUM-HIGH | merge await-waiter Mutexes | none | Doc 28 §1.3 item 7 |
| C-07 | Complexity | frontend/visit_Compare | O(k-1) allocs, chain of k | 1<x<10 in loop | O(1), stack booleans | MEDIUM | PHI/if-else in visit_Compare | none | Doc 30 Batch A |
| C-08 | Complexity | pattern_match.py:33 | O(n×d) allocs per match eval | match/case in loop | O(1), bytecode stack | MEDIUM | SSA-phi lowering | none | Doc 30 slot #40 |
| C-09 | Complexity | difflib.rs:511 | O(M×W²) | get_close_matches, large vocab | O(M×W×avg_matches) with b2j | LOW-MED | add b2j pre-index | none | NEW |
| C-10 | Complexity | event_loop.rs:551 | O(k) Mutex per timer | k timers expire simultaneously | O(k), single lock | LOW-MED | batch under one lock | none | Doc 28 Phase 1 |
| C-11 | Eagerness | glob_mod.rs:108 | O(N) alloc before first yield | iglob on large tree | O(1) per element | HIGH | lazy iterator backed by Paths | none | NEW |
| C-12 | Complexity | scheduler.rs:1931 | O(N) rebuild on inconsistency | high-churn await graph | N/A | LOW-MED | fix inconsistency root cause | none | Doc 28 impl |
| L-01 | Eagerness | glob_mod.rs:108 | — | same as C-11 | — | — | see C-11 | — | — |
| L-02 | Eagerness | scheduler.rs:871 | O(N) task alloc | all_tasks() | WeakSet lazy | LOW | lazy variant for internal callers | none | — |
| L-03 | Eagerness | scheduler.rs:1144 | O(N) done() calls | all_tasks() filtering | N/A | LOW | lazy filter | none | — |
| L-04 | Eagerness | scheduler.rs:979 | O(N) realloc | event cleanup | retain in-place | TRIVIAL | replace collect with retain | none | trivial fix |
| L-05 | Eagerness | runtime startup | +2.15ms startup | every program | lazy module init | MEDIUM | defer MODULE_IMPORT | HIGH | RuntimeSurfacePlan |
| L-06 | Eagerness | ops_iter.rs:659 | 2 allocs per dict iter | for k in dict | 1 alloc | LOW | direct dict iterator | none | — |
| L-07 | Eagerness | scheduler.rs:2818 | Vec clone at init | startup | N/A | TRIVIAL | — | none | — |
| L-08 | Eagerness | generators.rs:384 | 40B alloc per yield | generators in loops | N/A | HIGH | D1 generator fusion | none | Doc 26 D1 |
| L-09 | Eagerness | generators.rs:384 | Vec swap per send | any generator send | N/A | HIGH | gate on has_exception_handlers | none | Doc 26 residual |
| L-10 | Eagerness | logging module | args eval before level check | logging.debug in loop | same | NONE | user responsibility (same as CPython) | N/A | — |
| L-11 | Eagerness | ops_dict.rs:115 | 1 extra alloc per dict.update | dict.update(other_dict) | direct internal copy | LOW | dict_iter_items_direct | none | — |

---

## Top-10 Fix Ledger (severity × effort ordering)

1. **C-11 / L-01: glob.iglob eager materialization.** SEVERITY=HIGH, EFFORT=LOW. Single Rust function. Create a lazy `TYPE_ID_GLOB_ITER` backed by `glob::Paths`. Parity-critical: currently returns wrong semantics (entire tree in memory vs streaming). Anchor: `glob_mod.rs:108`. No owned arc — **schedule as a standalone fix in the next available build slot.**

2. **C-05: O(n²) string concat in loops.** SEVERITY=HIGH, EFFORT=MEDIUM. The realloc shortcut (refcount-1 path in `molt_inplace_add`) is the smallest delta. The full fix (doc 30 slot #38 string builder) requires a frontier doc. Anchor: `ops_arith.rs:61`. Parity gap vs CPython's realloc shortcut.

3. **C-02 + C-03: asyncio cancellation O(T×d) + 5 HashMap lookups per poll.** SEVERITY=HIGH, EFFORT=HIGH. Structural: doc 28 Phases 2 and 3. Cannot be fixed with point patches. Anchor: `cancellation.rs:316`, `scheduler.rs:3034`.

4. **C-04 + C-10: run_once drain alloc + timer re-lock per expiry.** SEVERITY=MEDIUM, EFFORT=MEDIUM. Doc 28 Phase 1. Anchor: `event_loop.rs:529`, `551`.

5. **C-07: multi-comparison chaining via LIST_NEW.** SEVERITY=MEDIUM, EFFORT=LOW. Frontend-only change in `visit_Compare`. Replace LIST_NEW cell with PHI/if-else. Anchor: frontend `visit_Compare`, doc 30 Batch A. Schedule in next available build slot.

6. **C-08: match/case heap-cell flags.** SEVERITY=MEDIUM, EFFORT=HIGH. Requires new SSA-phi lowering and TypeGuard integration. Anchor: `pattern_match.py:33`, doc 30 slot #40. Requires a frontier doc (dependent on doc 04b L4).

7. **L-04: eager filter-collect in event_waiters_cleanup.** SEVERITY=TRIVIAL, EFFORT=TRIVIAL. One-line fix: `raw_waiters.retain(|bits| *bits != 0)`. Anchor: `scheduler.rs:979`. Schedule inline with any doc 28 work.

8. **L-11: dict.update dict-to-dict allocates items view.** SEVERITY=LOW, EFFORT=LOW. Add `dict_iter_items_direct` for internal use. Anchor: `ops_dict.rs:115`.

9. **C-09: difflib get_close_matches missing b2j index.** SEVERITY=LOW-MED, EFFORT=LOW. Add a `HashMap<char, Vec<usize>>` pre-index in `find_longest_match`. Anchor: `difflib.rs:29`.

10. **C-12: await-waiter index rebuild inconsistency.** SEVERITY=LOW-MED, EFFORT=MEDIUM. Audit all `indexed_unique_vec_insert` call paths for completeness. Anchor: `scheduler.rs:1931`.

---

## Findings Belonging to Existing Owned Arcs (not new)

- **C-03 (5 HashMap lookups per poll):** Doc 28 §1.3 item 1 and 6. The architectural fix is inline task-local state into `MoltHeader` — do not start a separate fix outside doc 28.
- **C-01 (token_is_cancelled walk):** Doc 28 §1.3 item 8.
- **C-02 (wake_tasks_for_cancelled scan):** Doc 28 §1.3 item 8.
- **C-04 (drain-collect alloc per run_once):** Doc 28 §1.3 item 3, §2.3.
- **C-06 (3 Mutex per await register):** Doc 28 §1.3 item 7.
- **C-10 (timer re-lock):** Doc 28 §1.3 item 2.
- **L-08 (pair-per-yield):** Doc 26 §1.4 item 7, D1 generator fusion.
- **L-09 (exception swap per send):** Doc 26 §1.3, D1 residual.
- **L-05 (startup eager sys.py):** RuntimeSurfacePlan sprint + `tmp/design_startup_deferred_init.md`.
- **C-05 (string concat O(n²)):** Doc 30 §5a, slot #38 frontier doc. Partially owned; the realloc shortcut fix can proceed independently.
- **C-07 (LIST_NEW multi-compare):** Doc 30 Batch A. Fix slot available.
- **C-08 (match/case heap cells):** Doc 30 §Family 8, slot #40. Requires doc 04b L4 prerequisite.

## New Findings Requiring Scheduling (not owned by any existing arc)

1. **C-11/L-01: glob.iglob eager — behavioral correctness violation.** `glob_mod.rs:108`. Fix immediately; it is currently semantically wrong vs CPython (OOM risk on large trees), not just slower.
2. **C-09: difflib get_close_matches missing b2j index.** `difflib.rs:29`. Low effort, quality-of-implementation fix.
3. **C-12: await-waiter index inconsistency.** `scheduler.rs:1931`. Should be fixed as part of any doc 28 await-waiter graph rework.
4. **L-04: drain → retain one-liner.** `scheduler.rs:979`. Fix inline with doc 28 work.
5. **L-11: dict.update view allocation.** `ops_dict.rs:115`. Fix inline with any dict-ops pass.