# Call Bind IC Miss Rate Analysis

## Observed Data

- `call_bind_ic_hit`: 25 total (5 per benchmark)
- `call_bind_ic_miss`: 432 total (~86 per benchmark)
- Hit rate: **5.5%** (target: >90% after warmup)

## How the IC Works

The call bind inline cache lives in `runtime/molt-runtime/src/call/bind.rs`.

### Architecture

- **Thread-local direct-mapped cache**: 256-slot array indexed by `site_id & 0xFF`
- **Global Mutex<HashMap>** for cross-thread visibility (also updated on miss)
- **Site IDs** are compile-time FNV-1a hashes of `(func_name, op_idx, lane)`, NaN-boxed as inline ints

### Dispatch Flow (`call_bind_ic_dispatch`, line 1490)

1. Parse `site_bits` into `site_id` via `ic_site_from_bits`
2. If `builder_ptr` is non-null, attempt TLS lookup by `site_id`
3. If TLS hit, call `try_call_bind_ic_fast` to validate the entry
4. On fast-path success: increment `CALL_BIND_IC_HIT_COUNT`, return
5. Otherwise: increment `CALL_BIND_IC_MISS_COUNT`, call `molt_call_bind` (slow path)
6. After slow-path call, try `call_bind_ic_entry_for_call` to populate the IC

### IC Entry Creation (`call_bind_ic_entry_for_call`, line 1387)

The IC can only be populated for **two** callable kinds:

| Callable Type | Condition | IC Kind |
|---|---|---|
| `TYPE_ID_FUNCTION` | arity <= 4 | `CALL_BIND_IC_KIND_DIRECT_FUNC` |
| `TYPE_ID_BOUND_METHOD` | fn_ptr == `molt_list_append` **only** | `CALL_BIND_IC_KIND_LIST_APPEND` |

Everything else returns `None` -- the IC is **never populated**.

## Root Cause: The IC Population Filter is Too Narrow

### Problem 1: Bound methods are almost entirely excluded

The IC only caches `list.append` as a bound method. All other method calls --
`str.split`, `str.join`, `dict.get`, `dict.update`, `list.sort`, etc. -- pass through
`call_bind_ic_dispatch` but `call_bind_ic_entry_for_call` returns `None` for them.
Every invocation is a guaranteed miss that can never warm up.

This is the **dominant cause** of the 94.5% miss rate. The `call_method` IR op (used
for all `obj.method(args)` patterns) is lowered to `molt_call_bind_ic` in the native
backend (line 9432 of `function_compiler.rs`), so every method call increments the
miss counter without any hope of a future hit.

### Problem 2: TYPE_ID_TYPE calls are excluded

Constructor calls (`MyClass(...)`, `int(x)`, `dict(...)`) are `TYPE_ID_TYPE`. The IC
has no entry kind for type calls, so class instantiation always misses.

### Problem 3: Arity > 4 functions are excluded

Functions with more than 4 parameters cannot be cached. This is a minor contributor
but affects functions with default arguments that expand the arity.

### Problem 4: IC validation can reject valid entries

Even when the IC IS populated (for `DIRECT_FUNC` or `LIST_APPEND`), the
`try_call_bind_ic_fast` function re-validates the function pointer on every hit
(lines 1454, 1466). If the function object is re-allocated between calls (e.g.,
closures created in loops), the pointer check fails and falls through to miss.

## Why the 5 Hits per Benchmark Exist

The 5 hits likely come from `list.append` calls in benchmarks that build lists in
loops. After the first miss (which populates the IC), subsequent iterations hit.
But with ~86 total IC-routed calls per benchmark and only `list.append` being
cacheable, the hit rate stays at ~5%.

## What Most Hot Calls Actually Do

Most hot-path calls in benchmarks do NOT go through the IC at all:

- `range()` -> lowered to `RANGE_NEW` op (no IC involvement)
- `len()` -> lowered to `molt_len` via `CALL_FUNC` (direct call, no IC)
- `fib()` (recursive) -> lowered to `call` op (direct function call, no IC)
- `print()` -> lowered to `PRINT` op (no IC)

The IC is only consulted for `call_bind`, `call_indirect`, and `call_method` ops.
These are the "dynamic dispatch" cases: method calls, keyword-argument calls,
star-argument calls, and calls to unknown callables.

## Proposed Fix

### Phase 1: Extend `call_bind_ic_entry_for_call` (Medium effort, ~2-3 days)

Add IC entry kinds for the common bound method patterns:

```
CALL_BIND_IC_KIND_BOUND_METHOD_GENERIC = 3  // any bound method with arity <= 4
CALL_BIND_IC_KIND_TYPE_CALL = 4             // TYPE_ID_TYPE constructor calls
```

For `CALL_BIND_IC_KIND_BOUND_METHOD_GENERIC`:
- Cache the inner function's `fn_ptr` and arity
- On fast-path hit, extract `self` from the bound method, prepend to args, direct-call

For `CALL_BIND_IC_KIND_TYPE_CALL`:
- Cache the `__init__` function pointer for the class
- On fast-path hit, allocate instance + direct-call `__init__`

### Phase 2: Remove arity <= 4 restriction (Low effort, ~1 day)

Use `call_function_obj_vec` for all arities (already used on the fast path for
arity <= 4). The vec allocation is minor compared to the full `molt_call_bind`
slow path.

### Phase 3: Per-site polymorphic IC (Higher effort, ~1 week)

Replace the monomorphic direct-mapped cache with a 2-4 entry polymorphic IC per
site. This handles call sites that alternate between a small number of types
(e.g., `isinstance` checks dispatching to different methods).

### Expected Impact

Phase 1 alone should bring the hit rate from 5.5% to 70-80% for typical benchmarks,
since method calls are the dominant IC-routed operation. Phase 2 adds another 5-10%.
Phase 3 handles the long tail of polymorphic sites.

## Key Files

- `runtime/molt-runtime/src/call/bind.rs` -- IC implementation (lines 63-1524)
- `runtime/molt-runtime/src/call/dispatch.rs` -- Direct call dispatch (bypasses IC)
- `runtime/molt-runtime/src/constants.rs:97-98` -- Counter definitions
- `runtime/molt-backend/src/native_backend/function_compiler.rs` -- Call lowering
- `runtime/molt-backend/src/lib.rs:342-360` -- `stable_ic_site_id` hash function
- `src/molt/frontend/__init__.py:13799` -- `_emit_dynamic_call` (decides CALL_BIND vs CALL_FUNC)
