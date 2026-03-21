# Tuple Boxing in List Iteration: Analysis

## Problem

`bench_sum_list.py` (summing 1M integers via `for x in nums`) allocates ~1,002,845
tuples. A plain `for x in list` loop should allocate zero tuples.

## Where Tuples Are Allocated

### Root cause: `molt_iter_next` returns `(value, done)` tuples

Every call to `molt_iter_next` in `runtime/molt-runtime/src/object/ops.rs` (line 38039)
wraps its result in a freshly heap-allocated 2-tuple `(value, is_exhausted)`.

For the **list** path specifically (lines 38625-38644):

```rust
if target_type == TYPE_ID_LIST {
    let elems = seq_vec_ref(target_ptr);
    if idx == ITER_EXHAUSTED || idx >= elems.len() {
        // done case: alloc_tuple(&[none, true])
        ...
    }
    let val_bits = elems[idx];
    iter_set_index(ptr, idx + 1);
    let done_bits = MoltObject::from_bool(false).bits();
    let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);  // <-- HERE
    ...
    return MoltObject::from_ptr(tuple_ptr).bits();
}
```

Every single element yielded by the iterator allocates a 2-tuple `(value, false)`.
When the iterator is exhausted, one final tuple `(none, true)` is allocated.
For 1M elements: **1,000,001 tuples** from the iterator itself.

The remaining ~2,844 tuples come from:
- `list(range(size))` construction: `range` iteration also uses the same
  `molt_iter_next` -> `alloc_tuple` path (lines 38646-38694)
- The `range` iterator allocates one `(value, false)` tuple per element during
  list construction, but these are consumed and freed incrementally

### How the frontend consumes the tuple

In `src/molt/frontend/__init__.py` (lines 8031-8043), the for-loop is lowered to:

```python
pair = self._emit_iter_next_checked(iter_obj)   # ITER_NEXT op -> returns tuple
done = INDEX(pair, 1)                            # extract done flag
LOOP_BREAK_IF_TRUE(done)
item = INDEX(pair, 0)                            # extract value
```

The backend (`runtime/molt-backend/src/native_backend/function_compiler.rs`,
line 3962) directly calls `molt_iter_next` which returns the tuple as a u64
(pointer to heap-allocated tuple object).

## Why This Happens (Architectural Reason)

Molt's iterator protocol is modeled after Python's generator protocol, using a
**"pair return"** convention: every `__next__` call returns a `(value, done)` tuple
rather than using exceptions (StopIteration) for termination signaling. This is a
deliberate design choice to avoid the overhead of exception handling on every
iterator exhaustion.

However, the current implementation **heap-allocates** this pair as a real Molt
tuple object (`TYPE_ID_TUPLE`) via `alloc_tuple`. This means:
1. Each iteration step allocates a tuple on the heap
2. The tuple header + 2 element slots are initialized
3. Reference counts are managed
4. The tuple is immediately destructured (INDEX ops) and then becomes garbage

The pair is effectively an **ABI return convention** that accidentally goes through
the full object allocation path.

## Proposed Fix

### Option A: Two-word struct return (zero allocation, best performance)

Change `molt_iter_next` to return two values via a struct return or out-parameter:

```rust
#[repr(C)]
pub struct IterResult {
    value: u64,   // the yielded value (or None if done)
    done: u64,    // boolean: true if exhausted
}

pub extern "C" fn molt_iter_next_fast(iter_bits: u64) -> IterResult {
    // ... same logic but return IterResult { value, done } instead of alloc_tuple
}
```

The frontend/backend would emit two extracts from the struct return instead of
INDEX ops on a tuple. On x86-64 and aarch64, a 2x u64 struct is returned in
registers (rax+rdx or x0+x1), so this is zero-allocation.

**Changes required:**
- `runtime/molt-runtime/src/object/ops.rs`: Add `molt_iter_next_fast` returning `IterResult`
- `runtime/molt-backend/src/native_backend/function_compiler.rs`: Emit call with 2-return sig
- `src/molt/frontend/__init__.py`: Emit `ITER_NEXT_FAST` that produces two SSA values
- All other backends (luau, wasm, rust) need updating too

### Option B: Tagged sentinel return (minimal changes)

Use a special sentinel value (e.g., a tagged NaN or a dedicated `ITER_DONE` constant)
to signal exhaustion, so `molt_iter_next` returns a single u64:
- Normal: returns the value bits directly
- Exhausted: returns `ITER_DONE_SENTINEL`

The for-loop checks `result == ITER_DONE_SENTINEL` instead of unpacking a tuple.

**Simpler but** requires a value that can never be a valid Python object.

### Option C: Tuple-free list specialization (incremental, list-only)

Add a specialized `molt_list_iter_next(iter_bits: u64, out_done: *mut bool) -> u64`
that writes the done flag to an out-parameter and returns the value directly.
Only specialize for `TYPE_ID_LIST` initially; other types keep the tuple path.

The frontend can emit this specialized op when it knows the iterable is a list
(type inference already tracks this via `type_hint`).

## Expected Impact

- **Tuple allocations**: Reduced from ~1,002,845 to ~2,845 (the non-iteration tuples)
  for bench_sum_list. That is a **99.7% reduction** in tuple allocations.
- **Total allocations** (`alloc_count`): Reduced by ~1,000,001
- **Performance**: The per-element cost drops from ~{alloc + init + refcount + index + dealloc}
  to a simple function call + branch. Expected **20-40% speedup** on iteration-heavy
  benchmarks, depending on how much time is spent in allocation vs. arithmetic.
- **Memory pressure**: Significantly reduced, improving cache behavior for tight loops.

## Recommendation

Option A (struct return) is the cleanest long-term solution and completely eliminates
the allocation overhead for all iterator types. Option C is a good incremental step
if a full refactor is too risky.
