# Molt Guard + Deoptimization Contract (GDC) v0.1
**Status:** Draft
**Goal:** Define explicit guard primitives and deopt wiring so Molt can use profile-driven speculation safely.

## Current runtime type-guard contract

The existing SimpleIR `guard_tag(value, expected)` and `guard_type(value, expected)`
both call `molt_guard_type` and optionally return an identity alias of `value`.
The runtime converts `expected` through `to_i64`; rejected values raise
`TypeError("guard type tag must be int")`. Accepted carriers include bool,
integral direct floats in the supported range and bounded integers. A tag mismatch
only records `guard_tag_type_mismatch` when profiling is enabled, then returns
the original value. It neither raises nor establishes a type refinement. `ANY`
returns the source directly.

The compiler can bypass the call for a dominating literal-i64 tag when the runtime
profile flag is off. With profiling enabled it preserves every executed mismatch
event. The flag is captured once per function activation from the runtime epoch.
Full-width integer boxing remains fallible on the profiling path and stays inside
the backend operand transaction. Complete guard removal requires a separate
counter-free proof that the source already matches the tag.

The structured speculative transfer below describes the draft deopt interface;
it must not be inferred from today's runtime type-guard operation.

## Concepts
- Guard: side-effect-free predicate; on failure transfer to fallback.
- Deopt: controlled escape from optimized code to less specialized code.

Tier policy:
- Tier 0: no speculative deopt (guards only for provable checks or contract-violations).
- Tier 1: guards + mandatory deopt for any speculative assumption.

## Guard primitives (normative)
Type/layout:
- `guard_type(x, TypeId)`
- `guard_tag(x, Tag)`
- `guard_layout(x, LayoutId)`

Shapes and targets:
- `guard_dict_shape(d, ShapeId)` where the current builtin `dict` shape comes
  from the runtime `dict` type object's layout version; exact builtin
  dictionaries must pass that guard without falling through the mismatch/deopt
  counters.
- `guard_dict_has_keys(d, [k1,k2,...])`
- `guard_callee(site_id, symbol_id)`

Bounds:
- `guard_len_ge(a, n)`
- `guard_index_in_bounds(a, i)`

Exception-preventing checks (prefer these over “assume no exception”):
- e.g. `guard_ne(y, 0)` before division

## Structured form
```
block fast_path(args):
  guard_type(x, i64) else deopt slow_path
  guard_dict_shape(d, S1) else deopt slow_path
  body...
```

## Diagnostics loop
Runtime may emit:
- guard hit rates
- deopt frequency
- newly observed types/shapes
as `molt_runtime_feedback.json` for iterative specialization.

## Testing requirements
- Unit tests per guard primitive
- Property tests: optimized vs unoptimized equivalence under random inputs
- Differential tests vs CPython for supported semantics

TODO(compiler, owner:compiler, milestone:LF2, priority:P1, status:planned): method-binding safety pass (guard/deopt on method lookup + cache invalidation rules for call binding).
