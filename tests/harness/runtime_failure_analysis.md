# Molt Runtime Failure Analysis

## Data Source

Full-suite conformance run against 461 Monty test files (2026-03-29).

## Compilation Summary

| Category | Count | Rate |
|----------|-------|------|
| Compile success | 416 | 90% |
| MOLT_COMPAT_ERROR | 18 | 4% (correct rejections of invalid code) |
| Traceback (async) | 18 | 4% (Monty async format not supported) |
| Other compile error | 2 | <1% |
| Timeout | 7 | 2% |
| **Total** | **461** | |

## Runtime Results (Raise-Expected Tests)

Of tests expecting a Python exception:

| Result | Count |
|--------|-------|
| Correct exception raised | 33 |
| Wrong exception type | 3 |
| No exception raised | 2 |
| Compile error (skipped) | 215 |
| **Total tested** | **253** |

### Specific Failures

1. **`comprehension__unbound_local.py`** — expects `UnboundLocalError`, Molt exits 0
   - Root cause: comprehension scope variable resolution differs from CPython
   - Fix difficulty: Medium (frontend scope analysis)

2. **`fstring__error_eq_align_on_str.py`** — expects `ValueError`, Molt exits 0
   - Root cause: f-string format spec validation missing for `=` alignment on strings
   - Fix difficulty: Easy (add validation in format spec parser)

3. **`fstring__error_float_f_on_str.py`** — expects `ValueError`, Molt raises `TypeError`
   - Root cause: Molt raises `TypeError: format requires float` instead of `ValueError`
   - CPython raises `ValueError: Unknown format code 'f' for object of type 'str'`
   - Fix difficulty: Easy (change exception type in format dispatch)

4. **`fstring__error_int_d_on_float.py`** — expects `ValueError`, Molt raises `TypeError`
   - Same pattern: wrong exception type in format spec validation
   - Fix difficulty: Easy (same fix as #3)

5. **`fstring__error_int_d_on_str.py`** — expects `ValueError`, Molt raises `TypeError`
   - Same pattern
   - Fix difficulty: Easy (same fix as #3)

## Top 3 Fixes by Impact

### Fix 1: f-string format spec exception types (fixes 3 tests)

Molt raises `TypeError` when a format spec like `f"{value:d}"` is applied to the
wrong type. CPython raises `ValueError`. The fix is in the format spec dispatch
in `runtime/molt-runtime/src/object/ops_format.rs` — change the exception type
from `TypeError` to `ValueError` for format code mismatches.

### Fix 2: Comprehension scope resolution (fixes 1 test)

`comprehension__unbound_local.py` tests that a variable used in a comprehension
but assigned after it raises `UnboundLocalError`. Molt's frontend may not correctly
track the scope of comprehension variables per PEP 709.

### Fix 3: f-string `=` alignment validation (fixes 1 test)

`fstring__error_eq_align_on_str.py` tests that `f"{s:=10}"` raises `ValueError`
for string types (the `=` alignment is only valid for numeric types).

## Success-Expected Tests (Assert-Only)

Of 116 compiled success-expected tests, runtime results show most assertions pass.
The walrus operator and while-loop edge cases identified in earlier samples remain
the primary assertion failures.

## Recommendations

1. Fix f-string format exception types (3 tests, easy, 1 file change)
2. Fix comprehension unbound-local detection (1 test, medium)
3. Fix walrus operator assignment in conditionals (from earlier analysis)
4. Run full success-expected suite with warm cache to get accurate pass count

## Deep Investigation: `global` Assignment Bug

### Symptom
`global x; x = value` inside a function does not update the module dict.
Both regular assignment and walrus operator `:=` are affected.

### Root Cause (Partial)
The frontend code at `frontend/__init__.py:20947` correctly checks for
`global_decls` and now emits `MODULE_SET_ATTR`. However, the compiled
binary still shows the old behavior — the module dict is not updated.

### Hypothesis
The `MODULE_CACHE_GET` inside a function may return a stale or different
module object than the one at module level. Or the module may not be
registered in the cache when the function runs (timing issue during
module initialization).

### Next Steps
1. Add IR dump logging to verify `module_set_attr` ops ARE in the function IR
2. Check if `module_cache_get` returns the correct module object at runtime
3. Test whether the issue is in all backends (native, WASM, LLVM) or just one
4. Compare with how `module_get_global` (which DOES work for reads) resolves modules

## TIR Pass Interaction Bug: `while False` corrupts variables

### Symptom
`while False: count += 1` at module level corrupts `count` — it becomes
the module object instead of retaining its assigned value.

### Findings
- `MOLT_TIR_OPT=0` (all passes disabled): PASSES
- Skipping any single pass: PASSES
- Only SCCP + DCE: PASSES
- All passes together: FAILS

This is a pass-ordering/interaction bug where the combination of all 8
TIR passes produces incorrect code. No single pass is the culprit.

### Impact
Affects: `while__all.py`, `execute_ok__all.py`, `edge__all.py`
and any test with `while False` or unreachable loop bodies.

### Next Steps
1. Dump IR before and after each pass to find where the corruption starts
2. The issue is likely in how dead loop bodies interact with module-level
   variable storage (the loop body references `count` which forces it into
   a mutable binding, and a later pass incorrectly rewrites it)

## Module-Level Loop Variable Mutation (Root Cause Analysis)

### Symptom
```python
total = 0
for x in [1, 2, 3]:
    total += x
print(total)  # prints module object, not 6
```

### Root Cause
`_prepare_mutable_control_flow_bindings` (line 10016) moves loop-mutated
variables from `self.locals` to the module dict. Inside the loop body,
`_store_local_value` re-adds the variable to `self.locals` (line 7513)
AND writes to the module dict (line 7464). After the loop, `visit_Name`
finds the stale local in `self.locals` (line 6205-6207) and uses it
instead of reading from the module dict.

Post-loop re-eviction from `self.locals` doesn't fix this because the
post-loop `module_get_attr` reads back the value from the module dict,
but the test shows `total=<module>` — suggesting `module_get_attr` is
returning the MODULE OBJECT instead of the attribute value.

### Hypothesis
The `_emit_module_attr_set_on` at line 7464 may be writing to the wrong
module object, or the module dict lookup for `total` finds the module
object itself (name collision between the variable name and the module).

### Impact
Affects ALL for-loops and while-loops at module level that mutate variables.
This is the single largest conformance blocker — fixing it would likely
pass 10+ additional tests.

### Fix Approach
Need to dump the IR to verify that `module_get_attr` post-loop reads
the correct variable, and that `module_set_attr` inside the loop writes
to the correct attribute name on the correct module object.
