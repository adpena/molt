# Axiom Inventory

**Generated:** 2026-03-16
**Total axioms:** 68 across 6 files

This document lists every `axiom` declaration in the Lean codebase, categorized by
type, with justification and closure priority.

---

## Summary by Category

| Category | Count | Trust Level | Closure Path |
|----------|-------|-------------|--------------|
| Intrinsic contracts — core builtins | 36 | Legitimate | Requires runtime model |
| Intrinsic contracts — collections | 22 | Legitimate | Requires runtime model |
| Intrinsic contracts — composite laws | 3 | Closable | Follows from definitions |
| IEEE 754 / hardware | 1 | Legitimate | Hardware property |
| SSA well-formedness | 1 | Closable | Formalize SSA construction |
| SCCP worklist | 1 | Closable | Global worklist induction |
| Build infrastructure | 3 | Legitimate | External toolchain |
| Compile determinism | 1 | Legitimate | External toolchain |

---

## Category 1: IEEE 754 / Hardware (1 axiom)

**File:** `MoltTIR/Determinism/CrossPlatform.lean`

| # | Axiom | Line | Justification |
|---|-------|------|---------------|
| 1 | `ieee754_basic_ops_deterministic` | 173 | IEEE 754 conformance for basic float ops. Hardware property; cannot be proven in Lean. Validated by cross-platform differential tests. |

**Closure priority:** None. This is a permanent trust boundary.

---

## Category 2: Intrinsic Contracts — Core Builtins (36 axioms)

**File:** `MoltTIR/Runtime/IntrinsicContracts.lean`

These axioms model Python builtin behavior. They are declared as `axiom` because the
intrinsic functions are `opaque` (modeling the runtime's FFI boundary). Each axiom is
validated by the Python differential test suite (~3,500 test cases).

| # | Axiom | Line | Builtin | Property |
|---|-------|------|---------|----------|
| 2 | `len_nonneg` | 92 | `len` | Result is non-negative |
| 3 | `abs_int_nonneg` | 97 | `abs` | Result is non-negative (int) |
| 4 | `abs_int_of_nonneg` | 100 | `abs` | Identity for non-negative int |
| 5 | `abs_int_of_neg` | 103 | `abs` | Negation for negative int |
| 6 | `abs_float_nonneg` | 106 | `abs` | Result is non-negative (float) |
| 7 | `min_left` | 111 | `min` | Returns a when a <= b |
| 8 | `min_right` | 114 | `min` | Returns b when b < a |
| 9 | `max_right` | 117 | `max` | Returns b when a <= b |
| 10 | `max_left` | 120 | `max` | Returns a when b < a |
| 11 | `min_le_max` | 123 | `min`/`max` | min(a,b) <= max(a,b) |
| 12 | `bool_int_zero` | 128 | `bool` | bool(0) is false |
| 13 | `bool_int_nonzero` | 131 | `bool` | bool(n) is true for n != 0 |
| 14 | `bool_true` | 134 | `bool` | bool(True) is true |
| 15 | `bool_false` | 137 | `bool` | bool(False) is false |
| 16 | `bool_none` | 140 | `bool` | bool(None) is false |
| 17 | `bool_empty_str` | 143 | `bool` | bool("") is false |
| 18 | `int_of_true` | 148 | `int` | int(True) = 1 |
| 19 | `int_of_false` | 151 | `int` | int(False) = 0 |
| 20 | `int_of_int` | 154 | `int` | int(n) = n for integer n |
| 21 | `float_of_int` | 159 | `float` | float(n) succeeds for integer n |
| 22 | `str_total` | 164 | `str` | str(v) always produces a string |
| 23 | `repr_total` | 167 | `repr` | repr(v) always produces a string |
| 24 | `print_returns_none` | 172 | `print` | print(x) returns None |
| 25 | `type_int` | 177 | `type` | type(int) = "int" |
| 26 | `type_bool` | 180 | `type` | type(bool) = "bool" |
| 27 | `type_str` | 183 | `type` | type(str) = "str" |
| 28 | `type_none` | 186 | `type` | type(None) = "NoneType" |
| 29 | `type_float` | 189 | `type` | type(float) = "float" |
| 30 | `isinstance_type` | 194 | `isinstance` | isinstance(v, type(v)) is true |
| 31 | `hash_deterministic` | 200 | `hash` | Same input -> same output |
| 32 | `hash_eq_of_eq` | 204 | `hash` | Equal values -> equal hashes |
| 33 | `id_deterministic` | 210 | `id` | Same object -> same id |
| 34 | `callable_int` | 215 | `callable` | Ints are not callable |
| 35 | `callable_bool` | 218 | `callable` | Bools are not callable |
| 36 | `callable_none` | 221 | `callable` | None is not callable |
| 37 | `round_int_id` | 226 | `round` | round(int) is identity |

**Closure priority:** P4. These model an external runtime. Closure would require
replacing `opaque` intrinsic definitions with concrete implementations.

---

## Category 3: Intrinsic Contracts — Collections (22 axioms)

**File:** `MoltTIR/Runtime/IntrinsicContracts.lean`

| # | Axiom | Line | Builtin | Property |
|---|-------|------|---------|----------|
| 38 | `sorted_length` | 235 | `sorted` | Preserves length |
| 39 | `sorted_idempotent` | 239 | `sorted` | sorted(sorted(xs)) = sorted(xs) |
| 40 | `reversed_length` | 245 | `reversed` | Preserves length |
| 41 | `reversed_involution` | 249 | `reversed` | reversed(reversed(xs)) = xs |
| 42 | `reversed_nil` | 253 | `reversed` | reversed([]) = [] |
| 43 | `enumerate_length` | 258 | `enumerate` | Preserves length |
| 44 | `zip_length` | 264 | `zip` | Length = min(len(xs), len(ys)) |
| 45 | `range_length_nonneg` | 270 | `range` | Length = n for n >= 0 |
| 46 | `range_length_nonpos` | 274 | `range` | Length = 0 for n <= 0 |
| 47 | `set_length_le` | 280 | `set` | Length <= input length |
| 48 | `set_nil` | 284 | `set` | set([]) = [] |
| 49 | `set_idempotent` | 287 | `set` | set(set(xs)) = set(xs) |
| 50 | `all_nil` | 293 | `all` | all([]) = True |
| 51 | `any_nil` | 296 | `any` | any([]) = False |
| 52 | `all_implies_any` | 299 | `all`/`any` | all(xs) -> any(xs) for non-empty |
| 53 | `sum_nil` | 305 | `sum` | sum([]) = 0 |
| 54 | `map_length` | 310 | `map` | Preserves length |
| 55 | `map_nil` | 314 | `map` | map(f, []) = [] |
| 56 | `filter_length_le` | 319 | `filter` | Length <= input length |
| 57 | `filter_nil` | 323 | `filter` | filter(f, []) = [] |
| 58 | `sorted_reversed` | 339 | `sorted`/`reversed` | sorted(reversed(xs)) = sorted(xs) |
| 59 | `reversed_sorted_reversed` | 343 | `reversed`/`sorted` | reversed(reversed(sorted(xs))) = sorted(xs) |

**Closure priority:** P4. Same as core builtins — requires concrete runtime model.

---

## Category 4: Intrinsic Contracts — Composite Laws (3 axioms)

**File:** `MoltTIR/Runtime/IntrinsicContracts.lean`

| # | Axiom | Line | Property |
|---|-------|------|----------|
| 60 | `min_comm` | 347 | min(a,b) = min(b,a) |
| 61 | `max_comm` | 350 | max(a,b) = max(b,a) |
| 62 | `filter_sorted_length` | 355 | filter(f, sorted(xs)).length <= xs.length |

**Note:** `sorted_reversed` and `reversed_sorted_reversed` are counted in the
collections category above (#58, #59).

**Closure priority:** P3. These follow from the definitions if `intrinsic_min`/`intrinsic_max`
are given concrete implementations. `filter_sorted_length` follows from `filter_length_le`
and `sorted_length`.

---

## Category 5: SSA Well-Formedness (1 axiom)

**File:** `MoltTIR/Simulation/PassSimulation.lean`

| # | Axiom | Line | Property |
|---|-------|------|----------|
| 63 | `ssa_of_wellformed_tir` | 432 | Well-formed TIR functions are in SSA form |

**Justification:** Guaranteed by the compiler's SSA construction pass and validated by
the SSA verifier at compile time.

**Closure priority:** P2. Closable by formalizing the SSA construction algorithm.

---

## Category 6: SCCP Worklist (1 axiom)

**File:** `MoltTIR/Validation/SCCPValid.lean`

| # | Axiom | Line | Property |
|---|-------|------|----------|
| 64 | `sccpWorklist_env_strongSound` | 446 | Multi-block SCCP worklist produces sound abstract environments |

**Justification:** The local steps (`absEnvTop_strongSound`, `absExecInstr_strongSound`,
`absEnvJoin_sound`, `absEvalExpr_concretizes`) are all proven. The missing piece is the
global induction over the worklist iteration coupled with execution-trace reachability.

**Closure priority:** P2. Closable by implementing the reachability-conditioned global
induction. Hard effort (~1-2 weeks).

---

## Category 7: Build Infrastructure (3 axioms)

**File:** `MoltTIR/Determinism/BuildReproducibility.lean`

| # | Axiom | Line | Property |
|---|-------|------|----------|
| 65 | `cache_hit_correct` | 182 | Cache hits return correct artifacts (content-addressed) |
| 66 | `cranelift_deterministic` | 253 | Cranelift produces same machine code for same IR |
| 67 | `linker_deterministic` | 271 | Linker produces deterministic output given deterministic inputs |

**Justification:** These model external toolchain components (SHA256 cache, Cranelift
codegen, system linker). Validated by differential testing: same source produces same
binary across runs.

**Closure priority:** None. These are permanent trust boundaries (external tools).

---

## Category 8: Compile Determinism (1 axiom)

**File:** `MoltTIR/Determinism/CompileDeterminism.lean`

| # | Axiom | Line | Property |
|---|-------|------|----------|
| 68 | `no_timestamp_in_artifact` | 179 | Compiler does not embed timestamps in artifacts |

**Justification:** The compiler never calls `time.time()` or equivalent during artifact
generation. Validated by differential testing.

**Closure priority:** None. This is a property of the real implementation, not the model.

---

## Axiom Closure Priority Summary

| Priority | Axioms | Effort | Impact |
|----------|--------|--------|--------|
| **P2 — Closable, high value** | #63 (`ssa_of_wellformed_tir`), #64 (`sccpWorklist_env_strongSound`) | 2-3 weeks | Reduces trust boundary by 2 axioms |
| **P3 — Closable, moderate value** | #60-62 (composite laws) | 1-2 days | Reduces trust boundary by 3 axioms |
| **P4 — Closable with major effort** | #2-59 (intrinsic contracts) | Weeks-months | Requires replacing opaque runtime model |
| **Permanent** | #1, #65-68 (IEEE 754, toolchain) | N/A | External system properties |

### Recommended Closure Order

1. **P3 composite laws** (#60-62): Easiest wins. Give `intrinsic_min`/`intrinsic_max`
   concrete definitions and prove commutativity. `filter_sorted_length` follows from
   existing axioms.

2. **P2 ssa_of_wellformed_tir** (#63): Formalize SSA construction. Medium effort.

3. **P2 sccpWorklist_env_strongSound** (#64): Global worklist induction with
   reachability. Hard effort.

4. **P4 intrinsic contracts** (#2-59): Only if the runtime model is extended with
   concrete definitions for the builtins.
