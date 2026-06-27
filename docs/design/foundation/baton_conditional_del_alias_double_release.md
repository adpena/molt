# Baton: conditional-`del` alias-group double-release (silent wrong value)

**Status:** FALSIFIED (2026-06-26). The original drop-insertion double-free
diagnosis below was WRONG. The real root cause was a native-backend
**division-family carrier-store mismatch** (`i % 7` stored boxed under a
raw-i64-carrier output name). Fixed in `arith_division.rs` by routing the
boxed division-family result through `def_var_from_numeric_result` (the
carrier-aware store `add`/`sub`/`mul` already use). The `del`/alias in the
original repro were red herrings: `y = x` lowers to a `binding_alias` (own +1)
and the aliasing is already refcount-balanced — there is no double-free.

**Severity (when open):** P0 — silent WRONG ANSWER.

## What actually happened (corrected root cause)

`tests/differential/memory/alias_reassign_conditional_del.py` printed `219`
instead of CPython's `123`. The `219` was NOT reused-freed memory — it was the
**low byte(s) of a NaN-boxed integer's tag bits**, surfaced because `str(i % 7)`
stringified the box bits as if they were a raw i64.

The minimal repro that isolates it has NO `del`, NO aliasing, NO slicing:

```python
def f(n):
    i = 0
    while i < n:
        print(i % 7)          # molt printed 9221401712017801216, 9221401712017801217, ...
        i = i + 1             # CPython: 0, 1, 2
f(3)
```

`9221401712017801216` is `0x7FFC_0000_0000_0000` — the quiet-NaN integer tag
with payload 0. The `print`/`str` consumer read the NaN-box bits as a raw i64.

### The carrier-store mismatch (file:line)

The value-range pass (`runtime/molt-tir/src/tir/passes/value_range.rs`,
`ValueRangeTransferRule::Mod` -> `IntRange::mod_const`) proves `i % 7 ∈ [0, 7)`
and the representation plan marks the result SSA name a raw-i64 carrier
(`RawI64Safe`). Producer and consumer BOTH key the carrier off
`representation_plan.is_raw_int_carrier_name(name)`
(`runtime/molt-tir/src/representation_plan.rs:518`), so that predicate was
consistent — they agreed the name is a raw carrier.

The actual divergence was in the native `mod`/`inplace_mod` lane
(`runtime/molt-backend/src/native_backend/function_compiler/fc/arith_division.rs`):

- The raw-primary fast path (which stores a genuine raw i64 and returns early)
  requires `out_is_int_primary` AND **both operands** to be raw-i64 Variables.
- For `i % 7` the constant divisor `7` is NOT a raw-i64 *Variable*
  (`int_raw_value` returns `None`), so the lane fell to the boxed fallback,
  producing a NaN-**boxed** `res`.
- That boxed `res` was then written with the **raw** store
  `def_var_named(out, res)`. Because `is_raw_int_carrier_name(out)` is `true`,
  every consumer (`int_raw_value`, the `print` call-arg path via
  `ensure_boxed_overflow_safe`) treated the variable as a raw i64 and read the
  box bits directly. Storage and consumption disagreed.

`add`/`sub`/`mul` never had this bug because they finalize through
`def_var_from_numeric_result` — the carrier-aware store that, for an `I64`
(boxed) value flowing into a raw-carrier name, *unboxes* to raw via
`def_var_from_boxed_transport`. The whole division family (`div`, `floordiv`,
`floor_div`, `binop_floor_div`, `mod`, `inplace_mod`, `pow`, `pow_mod`, `round`,
`trunc`) was on the raw `def_var_named` store; all of those boxed-result stores
were migrated to `def_var_from_numeric_result`.

### Why it did NOT reproduce at module level / for add/mul/floordiv-without-const
At module scope the loop IV is typically not promoted to a raw-i64 carrier the
same way (so the boxed store landed under a boxed name — consistent). `add`/`mul`
already used the carrier-aware finalizer. The bug needed: a value-range-provable
division-family result (raw-carrier output name) whose native lane took the
boxed fallback (e.g. const divisor) and stored through the raw `def_var_named`.

## Scope / backends
- Native (Cranelift) only. The shared value-range `RawI64Safe` marking in
  `runtime/molt-tir/` is UNCHANGED, so LLVM/WASM/Luau carrier classification is
  unaffected. Those backends lower division through the value-keyed
  `repr_by_value` lattice and `def_var_from_boxed_transport`-equivalent LIR
  stores, not the native name-keyed `def_var_named` path, so they did not share
  this store-site bug.

## Regression coverage
- `tests/differential/int_loop_modulo/mod_const_loop_iv.py` (the minimal `i % 7`)
- `tests/differential/int_loop_modulo/mod_var_divisor_loop_iv.py` (`k = 7; i % k`)
- `tests/differential/int_loop_modulo/floordiv_const_loop_iv.py` (`i // 3`)
- `tests/differential/int_loop_modulo/mod_accumulate_str_loop_iv.py`
  (the `str(i % 7)` accumulator minimized from the original alias test)
- `tests/differential/memory/alias_reassign_conditional_del.py` now prints `123`.

## Note: cycle-collector OOM (still open, separate work)
`tests/differential/memory/cycle_leak_clean_control.py` OOMs — the cycle
collector does not reclaim reference cycles yet (the GC gap). That is unrelated
to this carrier-store bug and remains open.
