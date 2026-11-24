# Molt Guard + Deoptimization Contract (GDC) v0.1
**Status:** Draft
**Goal:** Define explicit guard primitives and deopt wiring so Molt can use profile-driven speculation safely.

## Source annotations and runtime guards

Source annotations in the default hint policy remain visible to Python code and
can guide lowering. Compilation requires an independent proof before specializing
an operation. The annotation-only profiling guard was retired with its IR and
runtime ABI. The guard primitives below describe a draft speculative transfer.

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
- `guard_layout(x, ClassDefinition, LayoutVersion)`: `x`'s class was created by
  one compiled class statement (its program-slot identity, stamped when the
  statement runs) and still has that statement's layout version (HF-220).
  Rebinding the class name cannot redirect it.

Shapes and targets:
- `guard_dict_shape(d, ShapeId)` (not implemented: the wire kind and its
  lowerings were retired with HF-220 because no frontend produced them).
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
