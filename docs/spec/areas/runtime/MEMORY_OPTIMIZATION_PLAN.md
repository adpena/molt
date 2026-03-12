# Memory Layout and Allocation Optimization Plan

**Status:** Draft
**Author:** runtime team
**Date:** 2026-03-12
**Audience:** runtime engineers, compiler engineers, performance team

---

## 1. Object Layout — Current State and Optimization Opportunities

### 1.1 MoltObject (NaN-Boxed Value Representation)

**File:** `runtime/molt-obj-model/src/lib.rs`

The `MoltObject` is a `#[repr(transparent)]` wrapper around `u64` (8 bytes). This is the universal value type — every variable, collection element, and function argument is a `MoltObject`.

**Encoding scheme (bits 63..0):**

| Type    | Layout | Payload |
|---------|--------|---------|
| Float   | Raw IEEE 754 `f64` bits | 64-bit double |
| Int     | `QNAN \| TAG_INT \| 47-bit signed value` | Range: `[-(2^46), 2^46 - 1]` |
| Bool    | `QNAN \| TAG_BOOL \| (0 or 1)` | 1 bit |
| None    | `QNAN \| TAG_NONE \| 0` | Singleton |
| Pending | `QNAN \| TAG_PENDING \| 0` | Async sentinel |
| Ptr     | `QNAN \| TAG_PTR \| 48-bit masked address` | Heap pointer |

Tag constants occupy bits 48..50 (`TAG_MASK = 0x0007_0000_0000_0000`), leaving 3 bits for tags and 48 bits for payload.

**Assessment:** The NaN-boxing is well-implemented. Floats pass through at zero cost. Integers inline up to 47-bit signed range. The tag space has 2 unused tag values (6 and 7) that can be exploited for future optimizations.

**Optimization opportunities:**

- **O1.1 — Use unused tag slots for small strings.** Tag value 6 (`0x0006_0000_0000_0000`) could encode inline strings up to 6 bytes (48 bits of payload). This would eliminate heap allocation for the vast majority of attribute names, single-character strings, and short identifiers. Python programs are dominated by short strings (variable names, dict keys, method names).

- **O1.2 — Inline small tuples.** Tag value 7 could encode a 2-element tuple of two inline integers (each 24 bits, range `[-8M, +8M)`). This covers the common case of `(index, value)` pairs and coordinate tuples returned from `enumerate()`.

- **O1.3 — Widen integer inline range.** The current 47-bit range covers `[-70T, +70T)`. This is already generous. No change needed.

### 1.2 MoltHeader (Heap Object Header)

**File:** `runtime/molt-runtime/src/object/mod.rs` (line 197)

```rust
#[repr(C)]
pub struct MoltHeader {
    type_id: u32,         // 4 bytes — object type discriminator
    ref_count: AtomicU32, // 4 bytes — reference count
    poll_fn: u64,         // 8 bytes — async poll function pointer
    state: i64,           // 8 bytes — state machine state / class bits
    size: usize,          // 8 bytes — total allocation size
    flags: u64,           // 8 bytes — header flags (17 flags defined)
}
// Total: 40 bytes per heap object
```

**Assessment:** 40 bytes of header overhead per heap object is substantial. The `poll_fn` and `state` fields are only used by generators, async tasks, and class instances (via `state` storing class bits). Every string, tuple, list, dict, range, and slice pays for these 16 bytes of async/class machinery.

**Optimization opportunities:**

- **O1.4 — Split hot/cold header fields.** The first 8 bytes (`type_id` + `ref_count`) are accessed on every refcount operation and type check. The remaining 32 bytes (`poll_fn`, `state`, `size`, `flags`) are cold for most operations. Restructure to:

  ```
  HotHeader (8 bytes):
      type_id: u32
      ref_count: AtomicU32

  ColdHeader (32 bytes, only for types that need it):
      poll_fn: u64
      state: i64
      size: usize
      flags: u64
  ```

  Simple types (string, bytes, int-overflow/BigInt, tuple, range, slice) need only the 8-byte hot header. This saves 32 bytes per string and tuple allocation. For a program with 100K live strings, this is 3.2 MB saved.

- **O1.5 — Compress flags field.** Only 17 flag bits are defined (bits 0..16). The `flags` field is `u64` (8 bytes) but could be `u32` (4 bytes) or even `u16` (2 bytes). This saves 4-6 bytes per object when combined with header restructuring.

- **O1.6 — Eliminate `size` field for fixed-size types.** The `size` field stores the total allocation size for `std::alloc::dealloc`. For fixed-size types (bound method = header + 16 bytes, range = header + 24 bytes, etc.), the size is statically known from `type_id`. Only variable-size types (string, bytes, dataclass, generator) need a stored size. This saves 8 bytes per fixed-size object.

- **O1.7 — Compress `type_id` to `u16`.** There are currently 48 type IDs (100..247). A `u16` (65K values) is sufficient. Combined with a `u16` flags field, the hot header becomes: `type_id: u16` + `flags: u16` + `ref_count: AtomicU32` = 8 bytes, with no padding.

### 1.3 Per-Type Payload Sizes

All sizes below are **payload only** (after the 40-byte `MoltHeader`). The total allocation is `MoltHeader + payload`.

| Type | Payload | Layout | Notes |
|------|---------|--------|-------|
| **Int (inline)** | 0 | Encoded in `MoltObject` u64 | No heap allocation for `[-2^46, 2^46)` |
| **Float** | 0 | Encoded in `MoltObject` u64 | No heap allocation |
| **Bool** | 0 | Encoded in `MoltObject` u64 | No heap allocation |
| **None** | 0 | Encoded in `MoltObject` u64 | Singleton |
| **BigInt** | varies | `Box<BigInt>` via handle registry | Heap-allocated via `num_bigint` |
| **String** | `sizeof(usize) + len` | `[len: usize][data: [u8; len]]` | Inline byte data, no separate heap Vec |
| **Bytes** | `sizeof(usize) + len` | Same as String | |
| **Bytearray** | `ptr + u64` = 16 | `[vec_ptr: *mut Vec<u8>][hash: u64]` | Mutable; Vec on separate heap |
| **List** | `ptr + ptr + u64` = 24 | `[desc_ptr: *mut DataclassDesc][vec_ptr: *mut Vec<u64>][hash: u64]` | Vec of NaN-boxed elements |
| **Tuple** | `ptr + u64` = 16 | `[vec_ptr: *mut Vec<u64>][hash: u64]` | Immutable; same Vec storage |
| **Dict** | `ptr + ptr` = 16 | `[order_ptr: *mut Vec<u64>][table_ptr: *mut Vec<usize>]` | Insertion-ordered; open addressing |
| **Set/Frozenset** | `ptr + ptr` = 16 | Same as Dict | |
| **Range** | `3 * u64` = 24 | `[start: u64][stop: u64][step: u64]` | Three NaN-boxed values |
| **Slice** | `3 * u64` = 24 | `[start: u64][stop: u64][step: u64]` | |
| **Iter** | `u64 + usize` = 16 | `[target: u64][index: usize]` | |
| **BoundMethod** | `2 * u64` = 16 | `[func: u64][self: u64]` | |
| **Function** | `8 * u64` = 64 | `[fn_ptr][arity][dict][closure][code][trampoline][annotations][annotate]` | |
| **Class (Type)** | `8 * u64` = 64 | `[name][dict][bases][mro][layout_ver][annotations][annotate][qualname]` | |
| **Code** | `8 * u64` = 64 | `[filename][name][firstlineno][linetable][varnames][argcount][posonly][kwonly]` | |
| **Exception** | ~72+ | 9 NaN-boxed fields | |
| **Generator** | `48 + variable` | `[send][throw][closed][exc_depth][yield_from][...]` | + state machine payload |

**Optimization opportunities:**

- **O1.8 — Inline small tuples.** Tuples of 0-3 elements could store elements directly in the payload rather than allocating a separate `Vec<u64>` on the heap. A 2-element tuple currently requires: 40 (header) + 16 (payload with Vec ptr) + 24 (Vec heap: ptr + len + cap) + 16 (2 elements) = **96 bytes across 2 allocations**. Inlined: 40 (header) + 16 (2 elements inline) = **56 bytes, 1 allocation**. This is a 42% reduction for the most common tuple size.

- **O1.9 — Inline small lists.** `MAX_SMALL_LIST` is already 16. Lists of 0-4 elements could store elements inline (same approach as tuples). Lists are mutable so the inline-to-heap transition must be handled on growth.

- **O1.10 — Small dict optimization.** Dicts with 0-4 entries could use a flat `[(key, value); N]` array instead of the hash table. Linear scan of 4 entries is faster than hash table lookup due to cache effects.

- **O1.11 — String deduplication for short strings.** Strings <= 16 bytes could be interned automatically. The existing `sys.intern()` path (`runtime/molt-runtime/src/builtins/sys_ext.rs`, line 34) uses a global `Mutex<HashMap<String, u64>>`. This should be extended to automatic interning for attribute names and short literals.

---

## 2. Allocator Strategy — Current State and Optimization Opportunities

### 2.1 Current Allocator

**Files:** `runtime/molt-runtime/src/object/mod.rs` (lines 460-583), `runtime/molt-runtime/src/arena.rs`

The current allocator is **system `std::alloc::alloc` / `std::alloc::alloc_zeroed`** (which maps to the platform's `malloc`/`calloc` — typically Apple's `libmalloc` on macOS, glibc `malloc` on Linux). There is no jemalloc or mimalloc dependency.

**Allocation path:**
1. Check if the type is pool-eligible (`TYPE_ID_OBJECT`, `TYPE_ID_BOUND_METHOD`, `TYPE_ID_ITER`).
2. If pool-eligible, try `object_pool_take()` — first from thread-local storage (TLS), then from global pool.
3. If pool miss, call `std::alloc::alloc` with alignment 8.
4. Initialize the `MoltHeader` at the start of the allocation.
5. Return a pointer offset by `sizeof(MoltHeader)` (the payload pointer).

**Deallocation path:**
1. Decrement refcount. If it reaches 0, run type-specific cleanup (recursively dec_ref children).
2. Try to return to object pool via `object_pool_put()`.
3. If pool is full, call `std::alloc::dealloc`.

**Object Pool:**
- Size-segregated free lists, bucketed by `total_size / 8` (line 404).
- Maximum poolable size: 1024 bytes (`OBJECT_POOL_MAX_BYTES`).
- Per-bucket limit: 4096 global (`OBJECT_POOL_BUCKET_LIMIT`), 1024 TLS (`OBJECT_POOL_TLS_BUCKET_LIMIT`).
- TLS pool is checked first (no locking), global pool second (Mutex).
- Only `TYPE_ID_OBJECT`, `TYPE_ID_BOUND_METHOD`, and `TYPE_ID_ITER` are pool-eligible.

**TempArena:**
- `runtime/molt-runtime/src/arena.rs` — a simple bump allocator with chunk-based growth.
- Used for temporary allocations during specific operations (not general-purpose).
- Chunk size minimum 1024 bytes, grows by allocating new chunks.
- `reset()` keeps the first chunk, `clear()` drops all chunks.

### 2.2 Optimization Opportunities

- **O2.1 — Bump allocator nursery for short-lived objects.** Most Python objects are short-lived (function-local temporaries, intermediate results). A per-thread bump allocator (nursery) would:
  - Eliminate per-object `malloc`/`free` overhead for ~80% of allocations.
  - Provide excellent cache locality (sequential allocation = sequential memory).
  - Enable batch deallocation (reset the bump pointer on function return).

  **Design:** Each compiled function gets a nursery reset point. Objects allocated during function execution use bump allocation. On function return, if no references escaped, the nursery resets. Escaped objects are promoted to the heap allocator.

  **Implementation sketch:**
  ```rust
  thread_local! {
      static NURSERY: RefCell<BumpAllocator> = RefCell::new(BumpAllocator::new(256 * 1024));
  }
  ```

  **Compiler integration:** The TIR pass can track whether objects escape the current scope (escape analysis). Non-escaping objects use nursery allocation. The backend emits `nursery_alloc` instead of `alloc_object` for scoped temporaries.

- **O2.2 — Extend object pool to more types.** Currently only 3 types are pool-eligible. High-churn types that should also be pooled:
  - `TYPE_ID_TUPLE` — created heavily by multiple return values, `enumerate()`, `zip()`.
  - `TYPE_ID_DICT` — created for keyword arguments, class instances.
  - `TYPE_ID_STRING` — created for string operations (but variable-size, so needs size-class pooling).
  - `TYPE_ID_EXCEPTION` — created on every `try/except` path.
  - `TYPE_ID_CALLARGS` — created on every dynamic call.

- **O2.3 — Size-class segregated free lists.** Replace the current `total_size / 8` bucketing with power-of-2 size classes:
  - 16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024 bytes.
  - This reduces internal fragmentation (current scheme has 128 buckets of 8-byte granularity, most of which are empty).
  - Each size class gets its own slab allocator with page-aligned blocks.

- **O2.4 — Thread-local allocation buffers (TLABs).** The current TLS pool (`OBJECT_POOL_TLS`) already provides thread-local free lists but with a 1024-entry limit per bucket. Extend this to a proper TLAB:
  - Each thread gets a 64 KB contiguous region for bump allocation.
  - When the TLAB is exhausted, request a new one from the global allocator.
  - No locking on the fast path (thread-local bump pointer).

- **O2.5 — Arena allocation for function-scoped temporaries.** The existing `TempArena` is a good foundation but is not integrated into the compilation pipeline. Extend it:
  - Compiler emits arena scope entry/exit around function bodies.
  - All non-escaping allocations use the arena.
  - Arena reset on scope exit reclaims all memory in one operation.

- **O2.6 — Consider mimalloc.** [mimalloc](https://github.com/microsoft/mimalloc) provides size-segregated thread-local free lists, excellent cache behavior, and low fragmentation. Adding `mimalloc = "0.1"` as a Cargo dependency and setting it as the global allocator would provide immediate benefits without custom allocator work:
  ```rust
  #[global_allocator]
  static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
  ```

---

## 3. Reference Counting Optimizations

### 3.1 Current Implementation

**File:** `runtime/molt-runtime/src/object/mod.rs` (lines 851-1040)

The current RC implementation:
- `ref_count` is `AtomicU32` in every `MoltHeader`.
- `inc_ref_ptr` uses `fetch_add(1, Relaxed)` — always atomic, even for single-threaded access.
- `dec_ref_ptr` uses `fetch_sub(1, AcqRel)` — full acquire-release barrier on every decrement.
- Immortal objects (flag `HEADER_FLAG_IMMORTAL`, bit 15) skip RC operations entirely.
- `NotImplemented` type also skips RC.
- NULL pointer checks on every inc/dec.
- GIL assertion on every inc/dec (`crate::gil_assert()`).

**Hot path cost:** Every refcount operation involves:
1. NULL check
2. Pointer arithmetic (subtract `sizeof(MoltHeader)` to find header)
3. Immortal flag check (read `flags` field)
4. Atomic operation

For a simple function call `f(x)`, the argument `x` gets: inc_ref (push to callargs) + dec_ref (callargs cleanup) + possible inc_ref (return value alias protection). That is 2-3 atomic operations per argument per call.

### 3.2 Optimization Opportunities

- **O3.1 — Biased reference counting.** Since Molt holds the GIL for all object mutations, the vast majority of refcount operations are single-threaded. Replace `AtomicU32` with a biased scheme:
  - **Local count** (non-atomic `u32`): incremented/decremented without atomics when the GIL is held.
  - **Shared count** (atomic `u32`): used only when an object is published to another thread (e.g., via `ThreadPoolExecutor`).
  - Objects start biased to the allocating thread. Publishing triggers bias transfer.

  **Expected impact:** Eliminates atomic operations from ~99% of refcount updates in single-threaded code. Even with the GIL, atomic operations have measurable overhead due to cache-line bouncing and store-buffer flushing.

  **Implementation:** The `flags` field already has 47 unused bits. Use one bit (`HEADER_FLAG_SHARED`) to indicate whether the object has been published. Non-shared objects use non-atomic operations.

- **O3.2 — Compile-time refcount elision.** The compiler (TIR/LIR passes) can eliminate redundant inc/dec pairs:
  - **Move semantics:** When a value is moved (last use of a variable), skip inc_ref at the destination and dec_ref at the source.
  - **Borrowed returns:** When a function returns a value that was passed as an argument, the callsite already holds a reference. The callee's inc_ref and the caller's subsequent dec_ref cancel.
  - **Loop-invariant hoisting:** If a variable's refcount is incremented/decremented on every loop iteration but the variable itself does not change, hoist the inc_ref before the loop and dec_ref after.

  **Implementation:** Add a `RefCountElision` pass to the LIR optimizer. Track reference ownership through SSA values. Emit `move` operations instead of `copy + inc_ref` when the source is dead after the operation.

  **Expected impact:** 30-50% reduction in refcount operations for typical Python code. This is the single highest-impact optimization in this plan.

- **O3.3 — Deferred reference counting (Deutsch-Bobrow).** Defer refcount updates for stack-local references:
  - Stack slots do not contribute to the refcount. Only heap-to-heap references are counted.
  - On scope exit, scan the stack frame for heap pointers and dec_ref any that are not reachable from the heap.
  - Trade: slightly more complex scope exit for zero refcount overhead on local variable assignment.

  **Risk:** Complicates debugging and makes the refcount "inaccurate" for debugging tools. Should be gated behind `--profile release`.

- **O3.4 — Coalescing adjacent refcount operations.** When the compiler generates:
  ```
  inc_ref(x)
  inc_ref(x)
  dec_ref(x)
  ```
  Coalesce to a single `inc_ref(x)`. This is a peephole optimization on the LIR.

- **O3.5 — Remove GIL assertion from release builds.** The `crate::gil_assert()` call in every `inc_ref_ptr` and `dec_ref_ptr` is a debug safety check. In release builds, this should be compiled out entirely (it may already be via `cfg(debug_assertions)`, but should be verified and enforced).

- **O3.6 — Prefetch header on type check.** When the compiler knows a refcount operation will follow a type check (common pattern: check type, then operate on object), prefetch the header cache line during the type check to hide memory latency.

---

## 4. Cache Optimization

### 4.1 Current Cache Behavior

**Assessment:** The current layout has poor cache utilization for several reasons:
- The 40-byte `MoltHeader` means that accessing the payload (e.g., string data) requires touching a second cache line for objects whose header straddles a 64-byte boundary.
- `Vec<u64>` indirection for lists, tuples, dicts means that iterating over a list requires chasing two pointer hops: `MoltObject` -> payload -> `Vec` heap allocation -> elements.
- Dict lookup requires hashing (touching the key string bytes), then probing the hash table (a separate `Vec<usize>`), then reading the order vector (another `Vec<u64>`).

### 4.2 Optimization Opportunities

- **O4.1 — Hot/cold header splitting (see O1.4).** Move `poll_fn`, `state`, `size`, `flags` to a cold header. The hot header (`type_id` + `ref_count`) fits in 8 bytes and shares a cache line with the first 56 bytes of payload. This means that for strings up to 48 bytes (after the 8-byte length field), the entire object fits in one cache line.

- **O4.2 — Pointer compression.** On 64-bit platforms with the GIL, all Molt heap objects live in a single address space region. If we allocate from a contiguous arena (or mmap region), we can represent heap pointers as 32-bit offsets from a base address. This:
  - Halves pointer storage in collections (list elements, dict entries).
  - Doubles the effective cache capacity for pointer-heavy data structures.
  - Is compatible with NaN-boxing (the `MoltObject` remains 64-bit, but collection storage uses compressed pointers).

  **Implementation:** Reserve a 4 GB mmap region at startup. All `alloc_object` calls allocate within this region. Store `u32` offsets in `Vec<u32>` instead of `Vec<u64>` for list/tuple/dict storage.

  **Constraint:** Limits total heap to 4 GB. Acceptable for the vast majority of programs. Provide a fallback to 64-bit pointers via a feature flag.

- **O4.3 — String interning with FxHash.** The current interning in `sys_ext.rs` uses `std::collections::HashMap` (SipHash). Replace with `FxHashMap` (or `ahash`) for faster hashing of short strings. Attribute name strings (`__init__`, `__str__`, `self`, etc.) are looked up on every method call — fast interning is critical.

  The `intern_static_name` function in `runtime/molt-runtime/src/state/cache.rs` (line 801) already provides per-name `AtomicU64` slots for commonly used attribute names. Extend this pattern to all attribute names resolved at compile time.

- **O4.4 — Small string optimization (SSO).** Strings <= 22 bytes (the payload size if we use an 8-byte hot header + 2-byte length) could store data inline in the object allocation, eliminating the pointer indirection. Currently strings already store data inline (`[len: usize][data: [u8; len]]` per `alloc_string` in `builders.rs` line 945), which is good. The optimization here is to reduce the header overhead to make more string data fit in a single cache line.

- **O4.5 — Dict hash cache.** Store the hash of each key alongside the key in the order vector. Currently, dict lookup must recompute the hash of the probe key on every lookup. Caching hashes in the order vector (layout: `[key_bits, value_bits, hash, key_bits, value_bits, hash, ...]`) avoids rehashing on resize and speeds up equality comparisons (compare hash first, then full equality).

- **O4.6 — Prefetch for sequential collection access.** When iterating over a list or tuple, prefetch the next N elements' headers. The compiler can emit `prefetchnta` instructions in the loop body for the element at index `i + 4`.

---

## 5. GC Integration — Cycle Collector Design

### 5.1 Current State

**File:** `docs/spec/areas/runtime/0009_GC_DESIGN.md`

The GC design doc (`0009_GC_DESIGN.md`) describes an aspirational hybrid RC + generational tracing GC. The current implementation is **pure reference counting** with:
- No cycle detection.
- No nursery/generational collection.
- Object pool recycling for 3 types.
- Immortal flag for singleton objects.
- Weakref support via a global registry (`runtime/molt-runtime/src/object/weakref.rs`).

Reference cycles (e.g., `a.x = b; b.x = a`) currently leak memory.

### 5.2 Optimization Plan

- **O5.1 — Trial deletion cycle collector.** Implement a cycle collector based on CPython's approach (trial deletion / "GC protocol"):
  1. Track all objects that *could* participate in cycles (containers: list, dict, set, tuple, class instances, generators).
  2. Periodically, run trial deletion: tentatively decrement refcounts for all internal references. Objects whose trial refcount reaches 0 are unreachable (part of a cycle). Collect them.
  3. Three generations: gen0 (newly allocated containers), gen1 (survived one collection), gen2 (survived two).

  **Why trial deletion over tracing:** Trial deletion integrates cleanly with the existing RC system. It only needs to track container objects (not all objects). It does not require a full root scan. It is the approach CPython uses and is well-understood.

  **Implementation:**
  - Add a `gc_tracked` linked list (intrusive, using 2 pointer fields in the cold header).
  - On container allocation, link the object into gen0.
  - Trigger gen0 collection every N container allocations (N = 700, matching CPython's default).
  - Trigger gen1 collection every 10 gen0 collections.
  - Trigger gen2 collection every 10 gen1 collections.

- **O5.2 — Generational hypothesis exploitation.** Most Python objects die young. The nursery bump allocator (O2.1) naturally implements the generational hypothesis: objects that survive nursery reset are promoted to the main heap. The cycle collector only needs to track promoted objects.

- **O5.3 — Incremental cycle detection.** For long-running programs, gen2 collections can be expensive. Implement incremental tri-color marking:
  - Budget: process N objects per allocation epoch (e.g., 100 objects per 1000 allocations).
  - Use a write barrier to catch mutations during incremental marking.
  - The existing `HEADER_FLAG_HAS_PTRS` (bit 0) can be repurposed as a GC mark bit during collection.

- **O5.4 — Weak reference integration.** The weakref registry (`runtime/molt-runtime/src/object/weakref.rs`) already clears weak references when objects are freed (`weakref_clear_for_ptr`). The cycle collector must also clear weak references to cycle members before reclaiming them. The existing `weakref_clear_for_ptr` function should be called during cycle collection.

---

## 6. WASM Memory Model

### 6.1 Current State

**Files:** `runtime/molt-runtime/src/constants.rs` (WASM table base), `runtime/molt-runtime/Cargo.toml` (wasm32 deps)

WASM-specific adaptations:
- `target_arch = "wasm32"` conditional compilation for features like `getrandom`, `mio`, TLS, etc.
- `WASM_TABLE_BASE_FALLBACK = 256` — function table base offset.
- `profile_alloc_type` is `inline(always)` on wasm32 to enable dead-code elimination.
- No `xz2`, `mio`, `socket2`, `rustls`, `tungstenite` on WASM (native-only deps).
- Thread/process task drop is disabled on WASM (`#[cfg(not(target_arch = "wasm32"))]`).

### 6.2 WASM Memory Constraints

- **Linear memory:** WASM uses a single contiguous linear memory that grows in 64 KB pages. The current maximum is ~4 GB (32-bit addressing). The `memory.grow` operation is expensive and non-reversible.
- **No virtual memory:** No `mmap`, no page-level protection, no lazy allocation. Every byte of linear memory is backed by physical memory.
- **Single-threaded (wasm32):** No threads, no atomics (unless SharedArrayBuffer + threads proposal). The GIL is trivially held. `AtomicU32` operations compile to plain loads/stores.
- **No `std::alloc::dealloc` granularity:** The WASM allocator (dlmalloc by default in Rust's wasm target) manages linear memory internally. Fragmentation is a major concern because memory cannot be returned to the OS.

### 6.3 Optimization Opportunities

- **O6.1 — Linear memory layout strategy.** Pre-allocate a large memory region at startup (`memory.grow` to target size) rather than growing incrementally. Incremental growth causes fragmentation in the dlmalloc free lists. Target: allocate 64 MB at startup, grow in 16 MB increments.

- **O6.2 — Bump allocator for WASM.** The bump allocator (O2.1) is even more valuable on WASM because:
  - No virtual memory means every allocation has real cost.
  - Linear memory cannot be returned, so minimizing peak usage is critical.
  - Bump allocation + batch reset minimizes peak memory by reusing the same region.

- **O6.3 — Pointer compression (mandatory on WASM).** WASM already uses 32-bit pointers. The NaN-boxing scheme stores pointers in the lower 48 bits, but on WASM only 32 bits are meaningful. The upper 16 bits of the pointer field are always zero. This is already efficient — no further compression needed at the `MoltObject` level.

  However, the `POINTER_MASK` (48 bits) and `canonical_addr_from_masked` (sign extension for x86-64 canonical addresses) are unnecessary on WASM. A WASM-specific fast path could skip the sign extension:
  ```rust
  #[cfg(target_arch = "wasm32")]
  fn ptr_from_nanbox(bits: u64) -> *mut u8 {
      (bits & 0xFFFF_FFFF) as *mut u8  // Lower 32 bits only
  }
  ```

- **O6.4 — Bulk memory operations.** WASM bulk memory operations (`memory.copy`, `memory.fill`) are significantly faster than byte-by-byte copies. Use these for:
  - Collection resize (list grow, dict rehash).
  - String concatenation.
  - Tuple construction from slices.

  Rust's `std::ptr::copy_nonoverlapping` should already compile to `memory.copy` on WASM, but verify this in the generated WASM output.

- **O6.5 — Non-atomic refcounting on WASM.** Since wasm32 (without threads) is inherently single-threaded, replace `AtomicU32` refcounts with plain `u32` on the WASM target. This eliminates atomic instruction overhead:
  ```rust
  #[cfg(target_arch = "wasm32")]
  type RefCount = Cell<u32>;
  #[cfg(not(target_arch = "wasm32"))]
  type RefCount = AtomicU32;
  ```

- **O6.6 — WASM memory usage tracking.** Expose `memory.size` to the runtime so the GC/cycle collector can use memory pressure as a trigger. When linear memory usage exceeds 75% of the current allocation, trigger a collection cycle.

---

## 7. Implementation Priority

Ordered by expected impact (performance gain) / effort (implementation cost):

| Priority | ID | Optimization | Impact | Effort | Risk |
|----------|----|-------------|--------|--------|------|
| **P0** | O3.2 | Compile-time refcount elision | Very High | High | Medium |
| **P0** | O6.5 | Non-atomic refcounting on WASM | High | Low | Low |
| **P1** | O3.1 | Biased reference counting | High | Medium | Medium |
| **P1** | O2.2 | Extend object pool to more types | Medium | Low | Low |
| **P1** | O1.8 | Inline small tuples | Medium | Medium | Low |
| **P1** | O2.6 | Switch to mimalloc | Medium | Low | Low |
| **P2** | O1.4/O4.1 | Hot/cold header splitting | Medium | High | Medium |
| **P2** | O2.1/O2.4 | Bump allocator nursery / TLABs | High | High | Medium |
| **P2** | O5.1 | Trial deletion cycle collector | Critical (correctness) | High | Medium |
| **P2** | O1.1 | Inline small strings in NaN-box | Medium | Medium | Medium |
| **P3** | O4.2 | Pointer compression | Medium | Very High | High |
| **P3** | O4.5 | Dict hash cache | Low-Medium | Medium | Low |
| **P3** | O3.3 | Deferred reference counting | Medium | High | High |
| **P3** | O1.10 | Small dict optimization | Low-Medium | Medium | Low |

**Note:** O5.1 (cycle collector) is marked P2 for performance priority but is **P0 for correctness** — reference cycles currently leak memory. It should be implemented as soon as the foundational allocator work (O2.1, O2.2) is stable.

---

## 8. Measurement and Validation

All optimizations must be validated against the differential test suite and benchmarks:

1. **Correctness:** `tests/molt_diff.py` full sweep with `MOLT_DIFF_MEASURE_RSS=1`.
2. **Performance:** `tools/bench.py` and `tools/bench_wasm.py` before/after comparison.
3. **Memory:** RSS measurement via `MOLT_DIFF_MEASURE_RSS=1`. Track peak RSS and allocation counts via the existing `ALLOC_COUNT` / `ALLOC_*_COUNT` counters in `constants.rs`.
4. **Fragmentation:** Add a `tools/memory_fragmentation.py` tool that measures: (allocated bytes) / (RSS bytes) ratio. Target: > 0.85.

---

## 9. Key Source Files Reference

| File | What it contains |
|------|-----------------|
| `runtime/molt-obj-model/src/lib.rs` | NaN-boxing scheme, `MoltObject`, pointer registry |
| `runtime/molt-runtime/src/object/mod.rs` | `MoltHeader`, `alloc_object`, `inc_ref_ptr`, `dec_ref_ptr`, object pool, deallocation |
| `runtime/molt-runtime/src/object/layout.rs` | Per-type field accessors (function, class, iter, etc.) |
| `runtime/molt-runtime/src/object/builders.rs` | Allocation helpers for all types (list, tuple, dict, string, etc.) |
| `runtime/molt-runtime/src/object/type_ids.rs` | Type ID constants (48 types, range 100-247) |
| `runtime/molt-runtime/src/object/utf8_cache.rs` | UTF-8 index/count caching for string operations |
| `runtime/molt-runtime/src/object/weakref.rs` | Weak reference registry and callback system |
| `runtime/molt-runtime/src/arena.rs` | `TempArena` bump allocator (chunk-based) |
| `runtime/molt-runtime/src/constants.rs` | `MAX_SMALL_LIST`, inline int range, WASM table base, perf counters |
| `runtime/molt-runtime/src/provenance/` | Pointer registry (provenance tracking, debug-only in release) |
| `runtime/molt-runtime/src/state/cache.rs` | `intern_static_name`, attribute name caching |
| `runtime/molt-runtime/src/builtins/sys_ext.rs` | `sys.intern()` string interning table |
| `docs/spec/areas/runtime/0009_GC_DESIGN.md` | Aspirational GC design (hybrid RC + generational tracing) |
