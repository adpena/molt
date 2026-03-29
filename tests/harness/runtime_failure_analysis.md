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
