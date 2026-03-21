# Attribute Cache 56% Miss Rate Analysis

## How the Cache Works

### Architecture

The attribute site-name cache is a **string interning cache**, not an attribute
lookup result cache. It lives in:

- `runtime/molt-runtime/src/builtins/attributes.rs` (lines 25, 38-91)
- Global static: `ATTR_SITE_NAME_CACHE: OnceLock<Mutex<HashMap<u64, u64>>>`

### Cache Key Strategy

- **Key**: `site_id` -- a 46-bit FNV-1a hash of `(func_name, op_idx, lane)`
  computed at compile time by `stable_ic_site_id()` in the backend.
- **Value**: interned string object bits (`u64`) for the attribute name bytes.

Each IR call site (`get_attr_generic_obj`) gets a unique, deterministic site ID
baked into the compiled code as a constant operand.

### Lookup Flow (per attribute access)

1. `molt_get_attr_object_ic(obj_bits, attr_name_ptr, attr_name_len, site_bits)`
2. Extract `site_id` from `site_bits` via `ic_site_from_bits`.
3. **Cache check**: lock the global `Mutex<HashMap>`, look up `site_id`.
   - **Hit**: verify the cached bits are still a valid string and that the bytes
     match the attribute name. If so, increment the hit counter and return the
     interned bits.
   - **Miss**: allocate/intern a new string via `attr_name_bits_from_bytes`,
     insert into cache, increment the miss counter.
4. Call `molt_get_attr_name(obj_bits, name_bits)` -- the **actual** attribute
   resolution. This always runs regardless of cache hit/miss.

### What the Cache Actually Saves

The cache avoids re-interning the attribute name string on every call. It does
**not** cache the attribute lookup result. Every attribute access still performs
the full `attr_lookup_ptr` / MRO walk.

### Cache Size and Eviction

- **Unbounded** `HashMap` -- no size limit, no eviction policy.
- Entries are only removed when the cached string bytes no longer match the
  site's attribute name (rare), or at shutdown via `clear_attr_site_name_cache`.
- No intermediate invalidation from dict mutation, type creation, or GC.

## Why the Miss Rate is 56%

### Root Cause: First-Access Cold Misses Dominate

With 85 hits and 107 misses (44.7% hit rate), the pattern is:

1. **Every unique (function, op_idx) site misses on its first access.** If
   there are N distinct attribute-access sites across all compiled functions
   (including module loading, stdlib init, and the benchmark itself), the first
   call to each site is always a miss.

2. **Module loading and stdlib initialization** create a large number of
   one-shot attribute accesses. These sites are visited exactly once during
   startup, contributing a miss each time with no subsequent hit to amortize.

3. **Short-running benchmarks** (like fibonacci) have very few hot-loop
   attribute accesses relative to the startup overhead. If the benchmark itself
   has ~5 attribute sites visited ~17 times each, that produces ~5 misses +
   ~80 hits from the benchmark, plus ~102 cold misses from startup -- matching
   the observed ratio closely.

4. The cache is **global and mutex-protected**, meaning it is shared and never
   partitioned. There is no per-thread or per-function fast path.

### Contributing Factor: Cache Only Interns Strings

Even with a 100% cache hit rate, the performance benefit is limited because:
- The cache only saves one string allocation per site (amortized).
- The full attribute resolution (`molt_get_attr_name` -> `attr_lookup_ptr` ->
  MRO walk) executes unconditionally on every access.

## Whether Compiled Code Uses the Cache

### Yes, but only for `get_attr_generic_obj` ops

The codegen in both `wasm.rs` and `native_backend/function_compiler.rs` routes
`get_attr_generic_obj` IR ops through `molt_get_attr_object_ic`, which uses the
site-name cache.

However, **two other attribute access paths bypass the cache entirely**:

| IR Op                  | Runtime Function         | Uses IC Cache? |
|------------------------|--------------------------|----------------|
| `get_attr_generic_obj` | `molt_get_attr_object_ic`| Yes            |
| `get_attr_generic_ptr` | `molt_get_attr_ptr`      | No             |
| `get_attr_special_obj` | `molt_get_attr_special`  | No             |
| `get_attr_name`        | `molt_get_attr_name`     | No (name already resolved) |

Attribute accesses on known-pointer objects (`get_attr_generic_ptr`) and special
attributes (`get_attr_special_obj`) skip the IC path, so their lookups are
invisible to the hit/miss counters.

## Proposed Fix

### Phase 1: True Inline Cache for Attribute Results (Medium Effort)

An existing patch (`attr_patch.patch` in the repo root) proposes a TLS-based
inline cache that caches the **attribute lookup result**, not just the name
interning:

- 256-entry direct-mapped TLS cache (`ATTR_IC_TLS`).
- Keyed by `(site_id, class_bits, class_version)`.
- VM epoch counter for bulk invalidation.
- Fast path: if object type + version match, do a dict lookup with the cached
  name and return immediately, skipping the full `molt_get_attr_name` path.
- Populate on miss: after a successful lookup, if the value came from the
  instance dict, record it for the next access.

This would convert repeated `.append`, `.split`, `.find`, `.join` in hot loops
from O(MRO-walk) to O(dict-probe) on the fast path.

**Effort estimate**: ~2-3 days. The patch exists but needs:
- Correctness validation (epoch invalidation on `__setattr__`, metaclass changes)
- Integration of the `get_attr_generic_ptr` and `get_attr_special_obj` paths
- Benchmark validation to confirm >85% hot-path hit rate

### Phase 2: Reduce Startup Cold Misses (Low Effort)

Pre-warm the cache for common builtin attribute names (`append`, `join`,
`split`, `find`, `__init__`, `__call__`, etc.) during runtime initialization.
This would eliminate ~50-70 cold misses from stdlib/module loading.

**Effort estimate**: ~0.5 day.

### Phase 3: Lock-Free Fast Path (Low-Medium Effort)

Replace the `Mutex<HashMap>` with a lock-free direct-mapped array (similar to
the TLS approach in the patch). The current mutex acquisition on every attribute
access adds unnecessary contention overhead even in single-threaded workloads.

**Effort estimate**: ~1 day, partially addressed by Phase 1's TLS approach.
