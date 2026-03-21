# String Allocation Pressure Analysis

## Profile Data (2026-03-20)

| Benchmark | alloc_string | alloc_tuple | alloc_dict | alloc_count | Loop iters |
|-----------|-------------|-------------|------------|-------------|------------|
| bench_fib | 15,085 | 2,849 | 2,693,923 | 8,101,402 | ~2.7M calls |
| bench_sum | 15,063 | 2,844 | 1,382 | 23,755 | 10M range |
| bench_sum_list | 2,015,066 | 1,002,845 | 1,382 | 3,023,765 | 1M list |
| bench_str_find | 515,103 | 2,850 | 1,387 | 523,827 | 500K append |
| bench_str_split | 65,104 | 2,850 | 1,387 | 73,827 | 50K append |

## Where Strings Are Allocated

### 1. The alloc_string call chain

Every string object is allocated through:

    alloc_string()
      -> alloc_bytes_like_with_len()
        -> alloc_object(TYPE_ID_STRING)
          -> profile_alloc_type()
            -> ALLOC_STRING_COUNT++

File: `runtime/molt-runtime/src/object/builders.rs:945`

### 2. Call sites that trigger string allocation

| Category | Function | File:Line | Called when |
|----------|----------|-----------|------------|
| IR const_str | `molt_string_from_bytes` | `builtins/strings.rs:1895` | Every `const_str` IR op |
| str() conversion | `molt_str_from_obj` | `object/ops.rs:5208` | `print()`, `str()`, f-strings |
| repr() conversion | `molt_repr_from_obj` | `object/ops.rs:5224` | `repr()`, containers |
| Attr name creation | `attr_name_bits_from_bytes` | `builtins/attr.rs:278` | Attribute lookup (1-entry TLS cache) |
| Intern static | `intern_static_name` | `state/cache.rs:803` | Dunder name lookups (cached after first call) |
| C API strings | `alloc_string` in c_api.rs | `c_api.rs` (many sites) | Module init, error messages |
| String iteration | `alloc_string` | `object/ops.rs:38586` | `for ch in string:` |
| String concat | `concat_bytes_like` | via `molt_add` | `str + str` |

### 3. Baseline overhead

All benchmarks share ~15K string allocations from runtime/module initialization:
- Module creation (`__file__`, `__package__`, `__spec__`, `__dict__`, etc.)
- `importlib.machinery` loading
- Function object setup (`__name__`, `__qualname__`, `__module__`, etc.)
- Builtin type registration

## Root Cause Analysis

### bench_sum_list: 2M strings for integer summation

Delta from baseline: +2,000,003 strings, +1,000,001 tuples

The lowered IR for `bench_sum_list` places `const_str` ops for frame-local
variable names inside the loop body:

    125: loop_start
    ...
    148: const_str "x"        <-- allocates a new string EVERY iteration
    149: check_exception
    150: dict_set ['locals', '"x"', value]
    ...
    167: const_str "total"    <-- allocates a new string EVERY iteration
    168: check_exception
    169: dict_set ['locals', '"total"', value]
    ...
    182: loop_continue
    183: loop_end

Each `const_str` compiles to `molt_string_from_bytes()`, which calls
`alloc_string()` unconditionally. With 1M iterations and 2 variable names
per iteration, this produces exactly 2M string allocations.

File: `runtime/molt-backend/src/native_backend/function_compiler.rs:534-576`
-- `const_str` always emits a call to `molt_string_from_bytes`.

The +1M tuples come from the (value, done) iteration pairs in the fallback
loop path (used when `vec_sum_int` fast-path is unavailable or when the IR
lowerer does not hoist the fast-path branch).

Note: An alternate lowering (found in cache `58b9663a`) correctly hoists
the `const_str` ops before `loop_start`. The inconsistency suggests the
lowerer's hoisting pass is not always applied.

### bench_str_find: 515K strings for string building

Delta from baseline: +500,000 strings = exactly the `repeat_text` loop count

The `repeat_text` function has a `while i < count` loop that runs 500K times.
The loop body updates frame locals via `dict_set` with a `const_str "i"` key.
The per-iteration string allocation is produced by the same `const_str`-in-loop
pattern as bench_sum_list when the lowerer does not hoist `const_str` ops.

### bench_sum: 15K strings (baseline only)

`bench_sum` uses `for i in range(10_000_000)` which compiles to an optimized
`vec_sum_int` fast-path. The loop body is never entered, so no per-iteration
`const_str` allocations occur. The 15K strings are purely from initialization.

### bench_fib: 15K strings (baseline only)

`fib(30)` performs ~2.7M recursive calls but allocates only 15K strings
(baseline). Function calls do not allocate strings per-call because the
dunder name lookups (`__call__`) use `intern_static_name` which caches after
the first allocation.

## The frame_locals dict: core design issue

The compiler maintains a `frame_locals` dict (Python's `locals()`) by emitting
`dict_set` after every variable assignment. This requires string keys for
variable names. When `const_str` ops for these keys are not hoisted out of
loops, each iteration allocates fresh string objects for the same variable
names.

Even when `const_str` is hoisted, the `frame_locals` dict update is O(1) per
assignment but still requires the dict machinery (hashing, probing). For tight
numeric loops, this is unnecessary overhead.

## Top 3 Optimization Recommendations

### 1. Intern const_str values at function entry (highest impact)

Problem: `const_str` compiles to `molt_string_from_bytes` which always
allocates a new string.

Solution: Add a `const_str_intern` op (or modify the existing codegen) that:
- Allocates the string once at function entry
- Stores it in a function-local slot
- Reuses the slot for all subsequent references

For bench_sum_list this eliminates 2M allocations (2 per iteration x 1M).
For bench_str_find this eliminates 500K allocations.

Expected impact: -99% of string allocations in non-string benchmarks.

Files to modify:
- `runtime/molt-backend/src/native_backend/function_compiler.rs` (codegen)
- `runtime/molt-runtime/src/builtins/strings.rs` (add interning API)

### 2. Eliminate frame_locals dict in optimized mode (high impact)

Problem: Every variable assignment emits a `dict_set` to `frame_locals`,
even when `locals()` is never called.

Solution: In functions that do not call `locals()`, `dir()`, `vars()`, or
`exec()`/`eval()` with local access, skip the `dict_set` entirely. This is a
compiler-level optimization in the IR lowering pass.

Expected impact: Eliminates all per-iteration `dict_set` calls and their
associated string key lookups. For bench_sum_list, this removes 2M dict_set
calls and (once const_str is interned) the remaining hash/probe overhead.

Files to modify:
- IR lowering pass (wherever `dict_set` for frame_locals is emitted)

### 3. Small String Optimization (SSO) for short strings (medium impact)

Problem: Even 1-byte strings like `"i"`, `"x"`, `"a"` allocate a heap
object with a 40+ byte header.

Solution: Store strings <= 23 bytes inline in the NaN-boxed representation
(using a tagged pointer to a stack buffer or an inline encoding). This avoids
the `alloc_object` -> `malloc` path entirely for common short strings.

Alternatively, implement a global string intern table for short strings. Python
interns single-character strings and identifier-like strings; Molt should do
the same.

Expected impact: For benchmarks where const_str interning (Rec 1) is
applied, SSO has less impact. But for dynamic string creation (f-strings,
slicing, single-char iteration), SSO would eliminate heap pressure.

Files to modify:
- `runtime/molt-obj-model/src/lib.rs` (NaN-boxing)
- `runtime/molt-runtime/src/object/builders.rs` (alloc_string)

## Summary

| Optimization | bench_sum_list | bench_str_find | bench_str_split |
|-------------|---------------|---------------|-----------------|
| Intern const_str | -2,000,000 | -500,000 | -50,000 |
| Elide frame_locals | -2,000,000 dict ops | -500,000 dict ops | -50,000 dict ops |
| SSO (< 23 bytes) | minor | minor | minor |

The single most impactful change is ensuring `const_str` values used as
`dict_set` keys inside loops are allocated exactly once per function call,
not once per loop iteration.
