<!-- Foundation audit 30. Architect: read-only research-granted agent, 2026-06-06.
Saved verbatim. SUPERVISOR SLOT REMAPPING (internal proposed numbers collide with
taken/in-flight slots — 31=fuzzing lane in flight, 33-37 reserved by doc 29's program):
its "Doc #31 String Builder" -> slot 38; "#32 zip/enumerate devirt" -> 39; "#33 pattern-
match first-class lowering" -> 40; "#34 inplace-op completeness" -> 41; "#35 metaclass
__prepare__" -> 42. NOTE: #41 (inplace dunders) and #42 (__prepare__) are CORRECTNESS
parity bugs, not just perf — they outrank their slot order; batch with the fix-only
ledger (Batches A-E) for the next build slots. -->

# Core-Language Feature/Op Portfolio Audit — molt

**Date:** 2026-06-06. All file:line anchors verified against current HEAD (commit `951938075`). This document is the language-semantics counterpart to doc 29 (stdlib portfolio); it does not duplicate stdlib-subsystem analysis.

---

## Patterns and Conventions Established

Before the per-family scorecards, the evidence base establishes the following architectural patterns that every scoring decision below must be read against.

**Frontend emission model.** `SimpleTIRGenerator` (`/Users/adpena/Projects/molt/src/molt/frontend/__init__.py`) is the canonical visitor. It emits `MoltOp(kind="UPPERCASE_KIND", ...)` objects, which `map_ops_to_json` (`/Users/adpena/Projects/molt/src/molt/frontend/lowering/serialization.py:396`) translates to lowercase JSON kind strings. The JSON kind is the cross-component contract (per doc 25). The MRO-decomposed visitors (`visitors/calls.py`, `visitors/classes.py`, `visitors/pattern_match.py`) are F1-phase move-only extractions with no independent semantic content.

**Op-kind registry state (doc 25, phase 1 complete).** 420 frontend JSON kinds; 146 in `kind_to_opcode`; LLVM coverage gap = 28 (all fail-loud); classifier silent-fallthrough = 196 (leak-safe, not UAF). `floordiv`/`floor_div` bidirectional spelling schism is the only live correctness asymmetry. Phase 2 (toml generation) is pending a build slot.

**TIR OpCode enum.** `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs:22–268`. First-class opcodes: Add/Sub/Mul/CheckedAdd/CheckedMul, InplaceAdd/Sub/Mul, Div/FloorDiv/Mod/Pow, Neg/Pos, all comparisons, bitwise, Bool, GetIter/IterNext/IterNextUnboxed/ForIter, Yield/YieldFrom, Raise/CheckException/ExceptionPending/TryStart/TryEnd, ConstInt/ConstBigInt/ConstFloat/ConstStr/ConstBool/ConstNone/ConstBytes, ObjectNewBound/ObjectNewBoundStack. The `Copy{_original_kind}` carrier handles the remaining ~274 JSON kinds that have not yet been promoted.

**Optimization pass coverage.** range_devirt, iter_devirt, deforestation, overflow_peel, sccp, gvn, licm, bce (via value_range), block_versioning, type_guard_hoist, counted_loop, loop_unroll, sroa, mem_gvn, memory_ssa. Generator/state-machine bodies are explicitly excluded from all structural passes (`has_state_machine()` gate in `function.rs:168-204`).

**Differential corpus.** `tests/differential/basic/` — ~480+ files covering arith, comparisons, exceptions, comprehensions, closures, pattern matching, f-strings, augmented assignment, async/generators, classes, descriptors, etc.

---

## Family-by-Family Scorecards

### Family 1: Operator/Dunder Protocol Families

#### 1a. Binary arithmetic (+ - * / // % ** @)

**UPSTREAM.** Single emission site: `visit_BinOp` (frontend:10559–10734). Type-hint propagation for same-type operands. Special cases: const-string folding at frontend (`:10562–10567`); `[int]*n` → `LIST_INT_NEW` specialization (`:10573–10600`); buffer2d `@`-loop pattern recognition for `BUFFER2D_MATMUL` (`:11085–11178`, `14820`). Seven op kinds emitted: ADD/SUB/MUL/DIV/FLOORDIV/MOD/POW/MATMUL/NEG/POS. No special-casing of `__radd__`/reflected ops at the frontend — delegation is entirely in the runtime.

**MIDSTREAM.** Add/Sub/Mul are first-class TIR opcodes. Div/FloorDiv/Mod/Pow/Neg/Pos: first-class opcodes in `TirOp::OpCode`. Matmul: emits JSON kind `"matmul"`, mapped to `OpCode::Copy{_original_kind="matmul"}` — NOT promoted. CheckedAdd and CheckedMul are first-class two-result overflow-safe binary ops used by `overflow_peel`; native and LLVM lower both to hardware-exact overflow flags, Luau uses checked helper emission with conservative multiply inexactness routing, and WASM still keeps CheckedMul on the boxed lane until a raw 64x64->128 overflow helper lands.

**DOWNSTREAM.** Native (function_compiler.rs): add/sub/mul have full fast-int, fast-float, and fast-str lanes with int-primary shadows; fallback to `molt_add`/`molt_sub`/`molt_mul`. `inplace_add` at `:4414–4641`: same lanes. Div/floordiv/mod: fast-int and fast-float lanes. Pow: always boxed (`molt_pow`). Matmul: `molt_matmul` runtime call — no native fast lane. WASM: same lanes via LIR machinery. LLVM: floordiv has dedicated arm (lowering.rs:9789); `floor_div` vs `floordiv` spelling schism (doc 25 §4.1) means the frontend-emitted `"floordiv"` rides `_original_kind` and hits the `molt_floordiv` runtime symbol rather than the arithmetic fast path. Matmul on LLVM: in `llvm_coverage_gap` — **fail-loud** (no arm, no symbol).

**REFLECTED OPS.** `call_binary_dunder` (ops.rs:7927–7991) correctly implements the CPython subclass-priority reflected-op protocol (`rhs_is_subclass → prefer_rhs`). Differential corpus: `arith_reflected_ops.py`, `arith_reflected_subclass_priority.py`, `arith_dunder_precedence.py`, `arith_dunder_subclass_precedence.py`, `arith_builtin_reflected.py`. The protocol is correctly implemented but the dunder path is never shortcut at the frontend — every binary op with non-builtin operands invokes `molt_add`/etc. via the boxed runtime call even when types are statically known.

**SEMANTICS.** CPython edge classes: `NotImplemented` return (handled in `call_binary_dunder`); subclass reflected-op priority (correct); zero-div exceptions for int (correct via runtime); complex arithmetic (emits `COMPLEX_FROM_OBJ` at `:5117–5126`, carried as `DynBox`); `__matmul__` on user objects via runtime fallback.

**PERF.** The floordiv spelling schism means `n // d` with int operands NEVER hits the first-class FloorDiv arithmetic path from the frontend (it stays as `_original_kind` → runtime call). This is a concrete, measurable perf gap: every `//` operation in a hot loop boxes and calls `molt_floordiv` rather than using the native `udiv`/`idiv` fast lane that `FloorDiv` would enable. PyPy and V8 both inline integer floor-division. No microbench for this specific path exists.

**Score.** IMPORTANCE=3 (arithmetic is the hot path for all numerical code), GAP=2. Primary gap: `floordiv` spelling schism + matmul LLVM fail-loud + no inlined pow path.

**Verdict:** Fix-only task batch (not a full frontier doc). Tasks: (a) collapse `floordiv`/`floor_div` to single canonical kind per doc 25 §6.1(a) — this is the highest-leverage single fix in this family; (b) add LLVM matmul arm; (c) implement int pow fast path for small positive exponents (PyPy precedent: bitmask-loop for exponent < 64).

---

#### 1b. Comparisons + rich-compare fallback chains

**UPSTREAM.** `visit_Compare` (frontend:14173–14253). Single-comparison case: delegated to `_emit_compare_op` (`:10989–11028`). Multi-comparison chaining (e.g., `1 < x < 10`): implemented via `LIST_NEW` cell + `STORE_INDEX` pairs — NOT SSA-phi-based. Each intermediate result is materialized into a heap list and re-loaded. This is optimizer-opaque: GVN/LICM cannot see through the list cells to the scalar comparisons.

**MIDSTREAM.** Eq/Ne/Lt/Le/Gt/Ge/Is/IsNot: all first-class TIR opcodes. `In`/`NotIn`: lowered through `_emit_contains` → `CONTAINS` kind → JSON `"contains"` → `OpCode::In`/`OpCode::NotIn` in ops.rs. The `IsNot` frontend lowers as `Not(Is(a,b))` not a single `IsNot` opcode at the TIR level (frontend:11021–11022).

**DOWNSTREAM.** Native: fast-int lane for Eq/Ne/Lt/Le/Gt/Ge when both operands are int-primary. `contains`: dispatches `molt_contains` which type-switches for set/dict/list/str/bytes/range — no O(1) fast lane bypass for known-set types. WASM/LLVM: comparable.

**SEMANTICS.** Rich compare fallback (`__eq__` → `__ne__` identity fallback, `__lt__` → `__gt__` reflection, `TypeError` on failure) is implemented in `molt_eq`/`molt_lt`/etc. at the runtime level via `call_binary_dunder`. Corpus: `arith_dunder_mul_truediv.py`, `arith_dunder_floordiv_mod.py`. Missing: no differential test for `__eq__` returning `NotImplemented` → identity fallback. `is`/`is not` on small integers (CPython interns small ints, molt does not intern — potential parity gap for `a is b` when both are `1`-range ints).

**Score.** IMPORTANCE=3, GAP=2. Primary gaps: multi-comparison chaining via heap list (optimizer-opaque); contains has no known-type fast bypass; IsNot is a composed `Not(Is(...))` rather than a single primitive; `is`-on-small-int parity.

**Verdict:** Fix-only batch. Tasks: (a) multi-comparison chaining should use PHI/if-else for the intermediate, not a LIST_NEW cell; (b) type-specialized `contains` fast path for statically-known-set/dict (avoid `molt_contains` dispatch overhead).

---

#### 1c. Contains/in

**UPSTREAM.** `_emit_contains` (frontend:10984–10987): single `CONTAINS` op. The serializer annotates `container_type` for set/frozenset/dict/list/str (`:923–938`). No frontend-level contains fast path (e.g., no `str.__contains__` → `molt_str_contains` direct call).

**DOWNSTREAM.** Native: `molt_contains` runtime function; type-switched in the runtime but always a full function call. LLVM: `"contains"` is in `classifier_silent_fallthrough` (196 bucket) — no explicit arm, falls to `molt_contains` via generic symbol resolution.

**PERF.** PyPy's JIT specializes `in` for known set/dict/list type on-the-fly. Molt has the type info at compile time (via type_hints) but does not synthesize a direct hash-lookup or linear-scan call. A tight `x in some_set` in a loop always goes through `molt_contains` → type dispatch → set lookup, rather than a direct `molt_set_contains_fast` with no dispatch overhead.

**Score.** IMPORTANCE=2, GAP=2.

**Verdict:** Fix-only batch. Task: specialize `contains` in codegen for known container types (set/dict/str), emitting a direct `molt_set_contains_fast` call bypassing the type-switch dispatch.

---

#### 1d. getitem/setitem/delitem + slicing

**UPSTREAM.** `visit_Subscript` (frontend:12292–12359). Simple index: `INDEX` op. Slice without step: `SLICE` op (frontend:12319–12321). Slice with step: `SLICE_NEW` → `INDEX` (frontend:12322–12328). Augmented slice assignment: `visit_AugAssign` handles `node.target` as `ast.Subscript` (frontend:14018–14161), emitting `SLICE_NEW` ops. Intrinsic-handle class `getitem_intrinsic` bypass (`:12343–12355`).

**DOWNSTREAM.** INDEX op: native has int-index fast lanes for list/dict/str/bytes. SLICE op: maps to `molt_slice`; always boxed runtime call. LLVM: `"slice"` is in `copy_kind_mints_fresh_owned_ref_table` (op_kinds_generated.rs:154), so it IS in the FreshValue set — the LLVM classifier knows it allocates a new slice object.

**SEMANTICS.** `__getitem__` on user objects: correctly dispatched via `molt_index` → `call_dunder_getitem`. `__setitem__`/`__delitem__`: same pattern. Custom slice semantics (e.g. `__index__` coercion for slice start/stop/step): not specially handled — slice is constructed as `SLICE_NEW` with whatever values the frontend emits; `__index__` calls are not synthesized at the frontend.

**Score.** IMPORTANCE=3, GAP=1. Most common cases are correct; `__index__` coercion gap is a parity hazard.

**Verdict:** Fix-only. Task: `SLICE_NEW` construction should coerce start/stop/step through `__index__` when the values are not provably integers.

---

#### 1e. Augmented/inplace (+=, -=, *=, and others)

**UPSTREAM.** `_augassign_op_kind` (frontend:13917–13944). Three ops have dedicated inplace kinds: `INPLACE_ADD`, `INPLACE_SUB`, `INPLACE_MUL`. The others (`/=`, `//=`, `%=`, `**=`, `|=`, `&=`, `^=`, `<<=`, `>>=`, `@=`) are lowered to the plain binary ops: `"DIV"`, `"FLOORDIV"`, `"MOD"`, `"POW"`, `"INPLACE_BIT_OR"`, `"INPLACE_BIT_AND"`, `"INPLACE_BIT_XOR"`, `"LSHIFT"`, `"RSHIFT"`, `"MATMUL"`. This means `//=`, `%=`, `**=`, `<<=`, `>>=`, `@=` are always non-mutating semantics at TIR level — they never call `__ifloordiv__`, `__imod__`, `__ipow__`, `__ilshift__`, `__irshift__`, `__imatmul__`.

**RUNTIME.** `molt_inplace_add` (ops_arith.rs:~260–315): tries list/bytearray in-place extension, then `call_inplace_dunder(__iadd__)`, then falls through to `molt_add`. Same pattern for `molt_inplace_sub` and `molt_inplace_mul`. For `//=`/`%=`/`**=` etc. the runtime IS wired: `call_inplace_dunder` is called in `molt_inplace_floordiv` etc. (ops_arith.rs:1222, 1309, 2366, 2625, 3445, 3570, 3810, 3998, 4050, 4308). But the frontend does NOT emit the inplace variants for these ops — it emits the plain binary kind which calls `molt_floordiv`/`molt_mod` etc. which do NOT try the inplace dunder first.

**THE BUG CLASS.** A user class with `__ifloordiv__` but no `__floordiv__` will be silently called via the plain `__floordiv__` dunder (fallback) instead of `__ifloordiv__`. Similarly for `__imod__`, `__ipow__`, `__ilshift__`, `__irshift__`, `__imatmul__`. This is a CPython parity gap. The differential test `augassign_inplace.py` only exercises types where inplace and binary are equivalent (list, bytearray, set). No test for user-class `__ifloordiv__` exists.

**Score.** IMPORTANCE=2, GAP=2 (silent behavioral divergence for user-class `//=`, `%=`, `**=`, `<<=`, `>>=`, `@=`).

**Verdict:** Fix-only task. Add `INPLACE_FLOORDIV`, `INPLACE_MOD`, `INPLACE_POW`, `INPLACE_LSHIFT`, `INPLACE_RSHIFT`, `INPLACE_MATMUL` opcodes (mirror of `InplaceAdd/Sub/Mul` in ops.rs) and wire them in `_augassign_op_kind` and the runtime. Six new first-class opcodes + six runtime functions + serializer arms = one atomic structural arc.

---

#### 1f. Unary operators

**UPSTREAM.** `visit_UnaryOp` (frontend:14255–14289). Four cases: UAdd→`POS`, USub→`NEG`, Not→`_emit_not`, Invert→`INVERT`. All first-class opcodes (Neg/Pos/Not/BitNot in ops.rs). Type hint propagation for `int`/`float`/`complex` operands.

**Score.** IMPORTANCE=2, GAP=0. Fully adequate.

---

#### 1g. Boolean/truthiness

**UPSTREAM.** `visit_BoolOp` (frontend:16826–~16960). Short-circuit semantics correctly implemented: AND = `IF(left) { AND(left,right) } ELSE {} PHI(result, left)`. Non-phi async path uses closure slots (`:16871–16884`). `_emit_not` via `NOT` op.

**DOWNSTREAM.** `Bool` op: fast-int/fast-bool lanes in native. `molt_bool_cast` for user objects (calls `__bool__` then `__len__`). Corpus: `bool_bool_len_precedence.py`, `bool_len_fallback.py`, `bool_short_circuit_order.py`, `boolean_edges.py`. The `__bool__`→`__len__` precedence and `__len__` returning a non-int TypeError are tested.

**Score.** IMPORTANCE=3, GAP=0. Fully adequate on the correctness dimension. Performance: `molt_bool_cast` is dispatched for every non-int truthiness check; there is no IC that caches the last type's bool behavior.

---

#### 1h. Hash/eq contract

**UPSTREAM.** No explicit `__hash__`/`__eq__` lowering at the frontend. Hash computations happen entirely in the runtime (`molt_hash`, `molt_ensure_hashable`). `ensure_hashable` context parity (CPython 3.14 unhashable message context) was fixed (`5fe6b0980`).

**SEMANTICS.** Hash/eq consistency for user objects: not statically verified. A class defining `__eq__` without `__hash__` correctly sets `__hash__ = None` (runtime handles). `frozenset`/tuple hash composition: in the runtime via recursive `molt_hash`. No differential test covers deep recursive hash chain.

**Score.** IMPORTANCE=2, GAP=1.

---

### Family 2: Call Protocol

#### 2a. Positional/keyword/defaults/*args/**kwargs binding

**UPSTREAM.** The call visitor (`visitors/calls.py`) is extensive. Fast path: direct function calls with known arity emit `call_func`/`call_guarded` or direct `call` with packed arguments. Variadic path: `CALLARGS_NEW` → `CALLARGS_PUSH_POS`/`CALLARGS_PUSH_KW`/`CALLARGS_EXPAND_STAR`/`CALLARGS_EXPAND_KWSTAR` → `CALL_BIND`. IC infrastructure: `_next_ic_index` (`_types.py:104`) feeds IC slots into `call_bind` ops.

**DOWNSTREAM.** `call_bind` (IC lineage): the native backend implements a dispatch-IC for `call_bind` ops (block_versioning pass + the 7× dispatch IC improvement from 2026-06-04 swarm). The LLVM backend has separate arms.

**SEMANTICS.** Defaults: captured at function definition time as constants/closures — correct. Keyword-only args with no default: `TypeError` on missing. `*args`/`**kwargs` positional/keyword collection: correct. Missing: positional-only (`/`) parameter enforcement is not tested in the differential corpus. The `b/` syntax was added in CPython 3.8; molt should support it.

**Score.** IMPORTANCE=3, GAP=1. Positional-only enforcement gap.

**Verdict:** Fix-only. Verify positional-only parameter enforcement and add differential test.

---

#### 2b. `__call__` objects

**UPSTREAM.** Callable objects (not functions) go through `CALL_BIND` → runtime `molt_call_bind` → type-dispatch to `__call__`. No static fast path for user-defined `__call__` objects at the frontend.

**Score.** IMPORTANCE=2, GAP=1 (perf: callable objects always boxed through IC).

---

#### 2c. Bound methods / fuse_method_dispatch

**UPSTREAM.** `call_method` with `fused_method_dispatch` attribute on the op. The zero-arg `super()` fold now asks `frontend.sema.classgraph.super_fold_is_sound` over sema-owned static class-graph/C3/reachability/local class-member facts plus explicit imported-class metadata: restricted to entry-module, guards against MRO diamonds, non-method class-attribute interposition, and dynamic/decorated class-member surfaces. `CallMethod` is a first-class TIR opcode.

**Score.** IMPORTANCE=3, GAP=1.

---

#### 2d. classmethod/staticmethod/property (bootstrap-critical)

**UPSTREAM.** `IMPLICIT_CLASSMETHOD_NAMES`/`IMPLICIT_STATICMETHOD_NAMES` tables in `_types.py:313–334`. `__init_subclass__`, `__new__`, `__class_getitem__` are implicitly staticmethods; `__init_subclass__` is implicitly a classmethod. Property descriptors: `_property_field_from_method` optimization path (`classes.py:47–60`). Per CLAUDE.md bootstrap authority: `classmethod`/`staticmethod`/`property` type objects must come from runtime bootstrap intrinsics — the frontend does not probe-construct them.

**Score.** IMPORTANCE=3, GAP=0. Adequate per CLAUDE.md constraint. No new work needed here.

---

### Family 3: Classes

#### 3a. Creation (metaclasses / `__prepare__` / `__init_subclass__` / `__set_name__`)

**UPSTREAM.** `visit_ClassDef` (`classes.py:543–`). `has_metaclass_kw` detection (`:543–548`). `dynamic_build` flag when metaclass or unknown bases (`:550–568`). `class_apply_set_name` runtime function exists (`_types.py:652`), serializer emits `"class_apply_set_name"` (`:1185`). `__init_subclass__`: in `IMPLICIT_STATICMETHOD_NAMES` but not explicitly called by the frontend class builder — delegated to the runtime metaclass call.

**THE GAP.** `__prepare__` is NOT called by molt's class builder. CPython calls `metaclass.__prepare__(name, bases, **kwds)` before executing the class body to get the namespace dict. Molt uses a fresh dict unconditionally. For custom metaclasses that override `__prepare__` (e.g., `enum.EnumMeta`), this is a silent parity gap: the class body executes in a plain dict, not the metaclass-provided namespace. The `dynamic_build` path routes to the runtime's `molt_class_new_dynamic` which does call `molt_metaclass_call` — but `__prepare__` is a pre-body hook, not a post-body hook.

**Score.** IMPORTANCE=2, GAP=2 (enum.EnumMeta and any `__prepare__`-based namespace customization silently broken).

**Verdict:** Fix-only. Task: in `dynamic_build` path, call `metaclass.__prepare__` before executing the class body and pass the returned namespace to the body execution. This is a frontend change: before `_visit_block(node.body)` in the dynamic class path, emit a `CALL_METHOD(metaclass, "__prepare__", name, bases, **kwds)` and use the result as the class namespace dict.

---

#### 3b. MRO/super

**UPSTREAM.** Runtime class metadata still uses `_class_mro_names` in the class-resolution mixin. Static class facts used by the fold live in `frontend.sema.classgraph`: `ClassFacts`, `c3_merge`, `static_class_bases`, `static_mro_names`, `reachable_base_names`, `static_method_owner_after`, and `super_fold_is_sound`. Static C3 linearization and member-owner resolution are computed for fold eligibility and fail closed on opaque, ambiguous, cyclic, unknown bases, non-method class-attribute interposition, or dynamic/decorated class-member surfaces; the runtime remains authoritative for dynamic class construction.

**Score.** IMPORTANCE=3, GAP=1. Static MRO is conservatively correct (runtime is authoritative). Super fold is correctly guarded.

---

#### 3c. Descriptors (get/set/delete precedence)

**UPSTREAM.** `_expr_is_data_descriptor` (frontend:3385–3392): checks if a class attr has `__set__` or `__delete__`. `_class_attr_is_data_descriptor` (`:3395–3405`). Used at multiple attribute access sites to decide whether to bypass field fast-path. Non-data descriptors (function → bound method): handled via `bound_method_new` in the runtime.

**SEMANTICS.** Data descriptor precedence: instance dict lookup → data descriptor → instance dict → non-data descriptor → class dict. Molt's field fast-path emits `guarded_field_get` which reads directly from the instance layout offset, bypassing any descriptor. For non-descriptor fields this is correct and fast. For fields that ARE data descriptors (unlikely in normal Python but valid), the fast path silently skips the descriptor.

**Score.** IMPORTANCE=2, GAP=1.

---

#### 3d. `__slots__`

**UPSTREAM.** `__slots__` on user-defined classes: the frontend tracks `fields` in `ClassInfo` (populated from `__init__` assignments and type annotations). The `dataclass(slots=True)` path is handled (`:719`, `:3200–3239`). For manually declared `__slots__ = [...]` in a class body, the frontend does NOT explicitly parse the `__slots__` assignment to derive fields — it uses the `__init__` assignment analysis. This means `class C: __slots__ = ['x']; def __init__(self): pass` does not produce a field layout; it falls through to the dynamic layout path.

**Score.** IMPORTANCE=2, GAP=2. Manual `__slots__` declarations are not honored for static field layout optimization.

**Verdict:** Fix-only. Task: parse `__slots__ = [...]` assignments in the class body during the first-pass scan, populate `fields` from them, and set `static=True` for the class layout.

---

#### 3e. Dataclass

**UPSTREAM.** The dataclass path (`classes.py:285–400`, `:3200–3399`) handles `@dataclass`, `@dataclasses.dataclass`, and `@dataclass(...)` with recognized options. Generated `__init__`, `__repr__`, `__eq__` are synthesized by the frontend at compile time via the dataclass shim template. `frozen`, `order`, `slots` dataclass options are handled (`:3200`, `:3399`).

**Score.** IMPORTANCE=3, GAP=1. Missing `kw_only`, `match_args` options. No differential test for `@dataclass(kw_only=True)`.

---

### Family 4: Closures/Cells/Scoping

#### 4a. Cell representation and nonlocal/global

**UPSTREAM.** Free variables are represented as indices into the closure frame (1-element list cells, loaded/stored via `LOAD_CLOSURE`/`STORE_CLOSURE`). `nonlocal_decls`, `global_decls` tracked. `_box_local` creates a 1-element list cell for variables that escape into closures or are assigned in `try`/`with` blocks. 

**SEMANTICS.** Nonlocal: `LOAD_CLOSURE(cell_index)` / `STORE_CLOSURE(cell_index, val)`. Global: `_emit_module_attr_get`/`_emit_module_attr_set`. The `comp_shadow_locals` mechanism (`__init__.py:276`) handles the comprehension scope isolation (walrus operator in comprehension sees outer scope per PEP 572). Inliner closure env-misbind fix: `0920ce213` (patched). Corpus: `closure_cell_sharing.py`, `free_vars_basic.py`, `nonlocal_del_binding.py`, `comprehension_walrus_scope.py`, `comprehension_walrus_nested_targets.py`.

**Score.** IMPORTANCE=3, GAP=1. The comp-walrus class had a real miscompile (`99723d589`, now fixed). No standing structural gap; the evidence base confirms working coverage.

---

#### 4b. Lambda

**UPSTREAM.** `visit_Lambda` is handled as an anonymous function def. Lambda closures use the same cell mechanism. Lambda `__iadd__` was an inliner env-misbind source — fix landed.

**Score.** IMPORTANCE=2, GAP=0.

---

### Family 5: Strings/F-Strings

#### 5a. F-string lowering

**UPSTREAM.** `visit_JoinedStr` (frontend:11180–11211). Parts assembled into `parts: list[MoltValue]`, joined via `_emit_string_join`. Conversions `!r`/`!s`/`!a` handled (`:11191–11196`). Format spec via `_emit_format_spec_value` (`:11030–11070`) and `_emit_string_format_value`. The `{expr=}` debug format: CPython 3.8+ emits `{expr=}` as a `Constant(" expr=")` + the value repr/str in the AST (the AST already inlines the expression text as a string literal before the value). Molt handles this correctly because `visit_JoinedStr` sees the Constant nodes the CPython parser inserts.

**THE MULTISITE MISCOMPILE (historical, baton still open).** The f-string `{expr=}` multi-site miscompile (`memory/project_inliner_fstring_multisite_miscompile.md`): inlining a function containing `{expr=}` f-strings caused the expression text to be replicated across call sites. This was a consequence of the inliner sharing string constants rather than remapping them. The baton documents this but does not indicate a committed fix on HEAD. Corpus: `fstring_debug_format_spec.py`, `fstring_eval_order.py`, `pep701_fstrings_full_grammar.py`.

**DOWNSTREAM.** `string_join` always allocates a new string. No string builder / rope optimization. Loop-based string building (`s += x` in a loop) goes through `molt_inplace_add` which does in-place list extension for list but for strings creates a new object per iteration — O(n²) behavior.

**Score.** IMPORTANCE=3, GAP=2. O(n²) string concatenation in loops; {expr=} inliner multisite baton still open.

**Verdict:** Frontier-doc commission. The string builder / loop concatenation problem is structural: it requires either (a) a peephole that recognizes the `inplace_add(str, str)` loop pattern and converts it to a `[parts].join("")` pattern (deforestation-style), or (b) a runtime `StringBuilder` type. Requires a full design doc (doc slot: #31 or adjacent to doc 26).

---

#### 5b. % formatting and .format()

**UPSTREAM.** `%` with string lhs: treated as `MOD` op → `molt_mod` → type-dispatch to `molt_str_percent_format`. No frontend specialization. `.format()` call: goes through the general call path → `molt_str_format` runtime. No compile-time format string analysis.

**Score.** IMPORTANCE=2, GAP=1. No fast path; always runtime-dispatched.

---

#### 5c. String interning

**UPSTREAM.** The runtime has `intern_static_name` for dunder names (used in attribute lookups). General string interning (Python's `sys.intern`) is available but not automatically applied. String literals in TIR are `ConstStr` ops with a raw string value; no deduplication.

**Score.** IMPORTANCE=1, GAP=1.

---

### Family 6: Iteration Protocol

#### 6a. for-loop desugaring

**UPSTREAM.** `_emit_iter_new` (frontend:9443–9458) + `_emit_iter_next_checked` (`:9460–9475`) + `_emit_for_loop` (`:10195`). The exception-pending path after `ITER_NEXT` correctly routes via `_emit_raise_if_pending` (C2 fix landed: `430e09793`).

**DOWNSTREAM.** `range_devirt` (passes/range_devirt.rs): eliminates range object + iterator allocations for `for i in range(...)`. `iter_devirt` (passes/iter_devirt.rs): converts `for x in list` to indexed loop. Both are proven, working passes.

**Score.** IMPORTANCE=3, GAP=1. zip/enumerate iteration not devirtualized.

---

#### 6b. zip/enumerate fast paths

**UPSTREAM.** zip/enumerate are lowered as regular builtin calls producing iterators. No frontend specialization. The deforestation pass does not handle `zip`/`enumerate` patterns.

**GAP.** `for i, x in enumerate(lst)` allocates an enumerate iterator per element and produces a 2-tuple per next call. PyPy and LuaJIT both inline enumerate as `(i, lst[i])` with a scalar counter. This is a known compiler optimization (precedent: Julia, LuaJIT, PyPy for pure list enumerate).

**Score.** IMPORTANCE=3, GAP=2.

**Verdict:** Frontier-doc commission. zip/enumerate fast-path design should be a dedicated doc (doc slot: #32). Pattern: recognize `for i, x in enumerate(lst)` in iter_devirt as a variant of the list-loop devirt + synthesize a scalar counter IV; recognize `for a, b in zip(lst1, lst2)` as a dual-index loop.

---

#### 6c. Unpacking / starred assignment

**UPSTREAM.** Star targets in assignment: `visit_Assign` with `ast.Starred`. Error propagation tests: `assignment_unpack_error_propagation.py`, `assignment_unpack_error_order.py`, `assignment_unpack_error_custom_iter.py`. Exact unpack diagnostic parity is covered by `tests/differential/basic/unpack_error_messages.py`, including target-version-gated known-length too-many messages, generic-iterator no-count too-many messages, too-few/starred-too-few messages, and non-iterable runtime type names.

**Score.** IMPORTANCE=2, GAP=1.

---

### Family 7: Exceptions

#### 7a. raise / except / chaining

**UPSTREAM.** `visit_Raise` (frontend:17074), `visit_Try` (`:15890`), `visit_TryStar` (`:16275`). Exception chains (`__cause__`/`__context__`): handled in `visit_Raise` via `RAISE_FROM` ops. `visit_TryStar` is fully implemented for `except*` (ExceptionGroup). TryScope mechanism tracks context marks for unwinding.

**DOWNSTREAM.** Native: TryStart/TryEnd bracket exception-guarded regions. LLVM: the `__cause__`/`__context__` chain is correctly maintained via runtime ops. There is an LLVM-specific gap noted in MEMORY.md: "exceptions/ret_void tail" on LLVM — specific LLVM path around exception handling on the tail return is incomplete. Corpus: 37 exception-family differential tests.

**Score.** IMPORTANCE=3, GAP=1. LLVM exception-chain tail gap is known and batonned.

---

#### 7b. finally semantics

**UPSTREAM.** `TryScope.finalbody` propagated. `EXCEPTION_PUSH`/`EXCEPTION_POP` (frontend:15956). Sync try/except split path for the common case. Finalizer unwind on `break`/`continue`/`return` inside `try`: handled via `_emit_unwind_try_scopes`.

**Score.** IMPORTANCE=3, GAP=1.

---

#### 7c. with / context managers (sync + async)

**UPSTREAM.** `visit_With` (frontend:14499), `visit_AsyncWith` (`:14644`). `__enter__`/`__exit__` calls emitted; exception path via `EXCEPTION_POP`. `contextlib.suppress` and `contextlib.contextmanager` patterns: no frontend specialization (always dynamic path).

**Score.** IMPORTANCE=3, GAP=1.

---

#### 7d. assert

**UPSTREAM.** `visit_Assert` (frontend:17215). Emits a `CONST_BOOL` check and a conditional `RAISE`. `AssertionError` with message. No `__debug__` optimization-level elision (Python's `-O` flag elides asserts; molt does not honor this).

**Score.** IMPORTANCE=1, GAP=1.

---

### Family 8: Pattern Matching (match/case)

**UPSTREAM.** `PatternMatchMixin` (`visitors/pattern_match.py`). All pattern types implemented: `MatchAs`, `MatchOr`, `MatchSequence`, `MatchMapping`, `MatchClass`, `MatchValue`, `MatchSingleton`, `MatchStar`. OR-pattern binding validation (`:108–125`). MatchClass kwd_attrs deduplication check (`:127–133`).

**DOWNSTREAM.** Lowered entirely through the generic `LIST_NEW`/`STORE_INDEX`/`INDEX`/`IF`/`END_IF` path — no first-class match opcodes. Each match check is a runtime `is`/`isinstance`/`contains` call. The result flags are heap list cells (the `_emit_match_cell` pattern, pattern_match.py:33–38) — not SSA booleans. This means the entire match block is fully optimizer-opaque: no SCCP, no GVN, no LICM on match conditions.

**SEMANTICS.** MatchClass: `__match_args__` protocol for positional patterns (`:408–466`). Corpus: `pattern_matching_core_matrix.py`, `pattern_matching_basic.py`, `match_patterns_extended.py`, `match_or_guard_branch_eval.py`, `match_class_pattern_errors.py`.

**Score.** IMPORTANCE=2, GAP=2. The heap-cell implementation is correctness-complete but optimizer-opaque and allocates O(nesting-depth) heap cells per match statement.

**Verdict:** Frontier-doc commission (doc slot: #33). End-state design should lower match/case to a CFG diamond using SSA-phi booleans and typed `isinstance` type guards (enabling `type_guard_hoist` and `block_versioning` to fire on the post-match branches). The heap-cell carrier is technically an `_original_kind`-era artifact applied to a structural control flow construct.

---

### Family 9: Decorators, Generators/Coroutines, Comprehensions

#### 9a. Decorators

**UPSTREAM.** Decorator application: emit decorator function value, emit `CALL_BIND(decorator, decorated)`. Stack of decorators applied bottom-to-top. `@classmethod`/`@staticmethod`/`@property`: handled specially — NOT applied as runtime calls for statically-known methods (`:226`, `:841`). `@dataclass` and `@typing.overload`: handled at the frontend as compile-time transforms.

**Score.** IMPORTANCE=2, GAP=0.

---

#### 9b. Generators and coroutines

POINT to doc 26 (`/Users/adpena/Projects/molt/docs/design/foundation/26_real-async-generators.md`) — the complete audit, end-state design, and implementation blueprint are specified there. Key findings summarized: generator bodies are fully optimizer-opaque (no structural passes fire); pair-per-yield allocates 40 bytes each; exception-context swap on every resume regardless of try-content; deforestation covers only pure-body builtin-consumer patterns; generator fusion (D1) is the structural fix.

**Score.** IMPORTANCE=3, GAP=3. The highest-gap family in this document. Doc 26 is the commissioned frontier doc.

---

#### 9c. Comprehensions (list/set/dict/genexpr)

**UPSTREAM.** `visit_ListComp`/`visit_SetComp`/`visit_DictComp`/`visit_GeneratorExp` (frontend:11841–11939). Simple single-clause comprehensions without conditions use `_emit_inline_list_comp`/`_emit_inline_set_comp`/`_emit_inline_dict_comp` (`:11770–11839`) — these are the fast inlined paths. Multi-clause or conditioned comprehensions fall to the full scoped function path.

**MIDSTREAM.** Deforestation pass (passes/deforestation.rs): `sum/any/all/list/len/set/tuple/sorted/reversed(genexpr)` patterns. The `List`/`Set`/`Tuple` fusable builtins (deforestation.rs:40–54) cover `list(genexpr)` → inline loop with append. Pure-body requirement is the gating condition (`:119`); impure bodies (with side-effecting calls) are not fused.

**SEMANTICS.** Walrus scope: fully tested (`:comp_walrus_scope.py`, `:comprehension_walrus_nested_targets.py`). Comprehension outer-scope capture: tested. Nested async comprehensions: tested.

**Score.** IMPORTANCE=3, GAP=1. Multi-clause conditioned comprehensions allocate a scope function; impure genexpr bodies not fused.

---

### Family 10: Numeric Tower

#### 10a. int/float/bool coercion lattice

**UPSTREAM.** Type-hint propagation in `visit_BinOp`: `int+int→int`, `float+float→float`, `int+float→float`, `complex+any→complex`. Bool subtype of int: bool operands participate in int type hints but `[True]*n` is excluded from LIST_INT_NEW specialization (frontend:10573). Numeric tower coercion (`int→float` implicit in arithmetic) is handled in the runtime (`molt_add`'s type dispatch).

**Score.** IMPORTANCE=3, GAP=1. The coercion lattice is correct at the runtime level but not always visible to the optimizer (e.g. `int+float` result is typed `float` but the LLVM/WASM RawI64Safe promotion does not fire on `float`-typed results).

---

#### 10b. BigInt boundary (the 2^46 history)

The `Repr` lattice / overflow_peel / ValueRange arc is the substrate. Current state:
- `ConstBigInt`: first-class TIR opcode (ops.rs:174), correctly materializes via `molt_bigint_from_str`.
- `RawI64Safe` / `MaybeBigInt` (representation_plan.rs): the P0/P1 work described in MEMORY.md.
- `overflow_peel` (passes/overflow_peel.rs): dual-loop rewrite for accumulator loops; fires on native; WASM/LLVM keep boxed carrier until `RawI64Full` lattice extension.
- Bug #15 (bench_sum IV cliff): not yet fully resolved — `overflow_peel` fires on native but the bug is open for WASM/LLVM.
- BCE via ValueRange: S6 landed, correctly elides bounds checks when value range proves index in bounds.

**Score.** IMPORTANCE=3, GAP=1 (WASM/LLVM overflow_peel not yet firing → 2.2× perf cliff on WASM/LLVM accumulator loops).

---

#### 10c. complex

**UPSTREAM.** `COMPLEX_FROM_OBJ` op (frontend:5117–5126, 10770–10779). Complex literals handled. Arithmetic type propagation: `complex_in` flag in `visit_BinOp` sets `res_type="complex"` for the result. No complex fast lane — always `DynBox` carrier.

**Score.** IMPORTANCE=1, GAP=1.

---

#### 10d. divmod / round / abs / `__index__` protocols

**UPSTREAM.** `divmod` is a builtin call → `molt_divmod` runtime. `round` → `molt_round` (calls `__round__`). `abs` → `molt_abs` (calls `__abs__`). `__index__` coercion: not systematically synthesized by the frontend — the runtime calls it where CPython specifies. This means `x[some_custom_index_obj]` does NOT call `__index__` at compile time (the slice/index path emits INDEX without coercion).

**Score.** IMPORTANCE=2, GAP=1. The `__index__` gap (noted also in Family 1d) is the structural issue.

---

## The Sequenced Program

### Top-5 Commission-Immediately Frontier Docs

1. **Doc #31: String Builder / Loop Concatenation (Family 5a-b).** Brief: `s += x` in a loop creates O(n²) allocation traffic; `%`/`.format()` have no compile-time analysis. The design must specify either (a) a `StringBuilder` runtime type with a deforestation pattern that recognizes the `inplace_add(str, str)` loop and rewires to `[parts].join("")`, or (b) an IR-level string fusion pass analogous to iter_devirt for string accumulation. Precedent: LuaJIT's string buffer, Julia's string interpolation lowering. **Dependency:** deforestation.rs (extend the `FusableBuiltin` set), or a new `str_builder_devirt` pass analogous to `iter_devirt`.

2. **Doc #32: zip/enumerate Devirtualization (Family 6b).** Brief: `for i, x in enumerate(lst)` and `for a, b in zip(lst1, lst2)` should lower to zero-allocation index loops via an extension of the `iter_devirt` pattern. enumerate → counter IV + list index; zip → dual-index IV with synchronized termination. **Dependency:** `iter_devirt.rs` extension. Precedent: PyPy's tracejit inlines enumerate/zip; Julia's broadcast fuses them.

3. **Doc #33: Pattern Matching First-Class Lowering (Family 8).** Brief: `match/case` currently lowers to heap-cell boolean flags (optimizer-opaque). The design must specify a first-class IR lowering using SSA-phi booleans and typed `isinstance` type guards, enabling `type_guard_hoist` and `block_versioning` to fire on the post-match branches. This is the correct structural fix — stacking peephole opts on the heap-cell carrier is the rejected path. **Dependency:** doc 26 (L4 TypeGuard-gen needed for the `isinstance` guard chain).

4. **Doc #34: Inplace Op Completeness — `//=`, `%=`, `**=`, `<<=`, `>>=`, `@=` (Family 1e).** Brief: six augmented assignment operators silently skip their `__ifloordiv__`/`__imod__`/`__ipow__`/`__ilshift__`/`__irshift__`/`__imatmul__` dunders by lowering to plain binary ops. This is a correctness gap (CPython parity). The design is: six new first-class opcodes in `ops.rs` (mirroring InplaceAdd/Sub/Mul), six `molt_inplace_*` runtime functions (already partially present in `ops_arith.rs`), wiring through `_augassign_op_kind`, serializer, op_kinds.toml, and all backends. This is a complete structural arc (not a point fix) because the opcode, frontend, serializer, registry, runtime, and all 4 backends must be updated atomically. **Dependency:** doc 25 (op_kinds.toml phase 2 is the right venue to land these).

5. **Doc #35: metaclass `__prepare__` (Family 3a).** Brief: `__prepare__` is not called before class body execution in the dynamic-build path. `enum.EnumMeta`, `ABCMeta`, and any user metaclass overriding `__prepare__` silently receive a plain dict namespace. The design must: in the `dynamic_build` path, detect whether `metaclass.__prepare__` returns a non-dict or is non-trivially overridden, call it before `_visit_block(node.body)`, and pass the result as the class namespace. **Dependency:** class builder in `visitors/classes.py`, runtime `molt_metaclass_call`, and a differential test against `enum.Enum` subclassing.

---

### Fix-Only Task Batches (no frontier doc needed)

The following are structural fixes that do not require a full design document but require an atomic implementation arc:

**Batch A — Arithmetic/comparison correctness:**
- Collapse `floordiv`/`floor_div` to one canonical JSON kind (doc 25 §6.1(a)); this is the single highest-leverage arith fix.
- Multi-comparison chaining: replace `LIST_NEW` cell pattern with PHI/if-else in `visit_Compare`.
- Matmul LLVM arm (add to `lower_preserved_simpleir_op`).
- `IsNot` should be a single first-class op or at minimum the `Not(Is(...))` composition should be folded at SCCP.

**Batch B — `__slots__` layout (Family 3d):**
- Parse `__slots__ = [...]` in `visit_ClassDef` first-pass, populate `fields`, set `static=True`.

**Batch C — `__index__` coercion (Families 1d, 10d):**
- `SLICE_NEW` and `INDEX` with non-integer operands should synthesize `__index__` calls on the operands.

**Batch D — contains fast path (Family 1c):**
- When `container_type` is `set`/`frozenset`/`dict` and the type hint is proven, emit `molt_set_contains_fast`/`molt_dict_contains_fast` directly rather than routing through `molt_contains` type dispatch.

**Batch E — Positional-only param enforcement (Family 2a):**
- Add differential test; fix enforcement if gap is confirmed.

---

### Microbench Lanes to Create

The following microbench programs are missing and should be added to `tests/perf/` or equivalent:

1. `bench_floordiv_loop.py`: `for i in range(N): a = i // 3` — measures the floordiv spelling-schism impact on native vs LLVM vs WASM.
2. `bench_str_concat_loop.py`: `s = ""; for i in range(N): s += str(i)` — measures O(n²) string concatenation.
3. `bench_enumerate_loop.py`: `for i, x in enumerate(lst): total += x` — measures cost of enumerate iterator vs ideal.
4. `bench_zip_loop.py`: `for a, b in zip(lst1, lst2): total += a + b` — same for zip.
5. `bench_contains_set.py`: `for x in queries: x in big_set` — measures contains dispatch overhead.
6. `bench_match_simple.py`: `match obj.tag: case "a": ... case "b": ...` — measures match/case overhead vs if/elif chain.

---

### Dependency Edges to In-Flight Arcs

- **Doc #31 (string builder)** is independent; no blocking in-flight dependency.
- **Doc #32 (zip/enumerate devirt)** depends on iter_devirt.rs being stable (it is, as of HEAD).
- **Doc #33 (pattern match lowering)** depends on **doc 04b / L4 TypeGuard-gen** being landed — the isinstance guard chain requires `TypeGuard` producers in the pipeline, which L4 provides.
- **Doc #34 (inplace ops)** depends on **doc 25 phase 2** (op_kinds.toml) being the right landing venue; can proceed independently if phase 2 is delayed.
- **Doc #35 (metaclass `__prepare__`)** is independent.
- **Batch A (floordiv canonical kind)** is the prerequisite for measurable FloorDiv arithmetic fast-path gains on all backends; it should land in the same build slot as doc 25 §6.1(a).
- **Generator/coroutine families** are fully covered by **doc 26** (D1 coroelide); cross-reference only.
- **BigInt boundary families** are covered by the **repr-promotion arc** (RawI64Safe/ValueRange) and **overflow_peel** (WASM/LLVM extension); both are in progress.

---

## Cross-References to In-Flight Arcs

| Arc | Families affected | Status |
|---|---|---|
| Doc 25 — op_kinds registry phase 2 | 1a floordiv schism, 1e inplace batch D | Phase 1 complete; phase 2 pending build slot |
| Doc 26 — Real async/generators | Family 9b, deforestation | Design complete; implementation pending E1 activation verification |
| D1 — CoroElide generator fusion | Family 9b, 9c | Blueprint complete in doc 07 |
| L4 — TypeGuard-gen + loop canon | Family 8 (pattern match end-state) | Gated on TypeGuard producers in prod pipeline |
| S5 phase 2–5 — MemSSA/MemGVN/SROA | Families 1–2 general perf | S5 phase 1 landed; phases 2–5 in design |
| E1 activation (inliner) | All families (inlining unlocks fused fast paths) | Active; native+WASM landed |
| overflow_peel WASM/LLVM | Family 10b BigInt boundary | Partial (native fires; WASM/LLVM blocked on RawI64Full lattice) |
| RC drop insertion (design 20) | Family 9b generators | `drop_insertion.rs:450` bails on state machines; unblocked by D1 |

---

## Relevant File Paths

- `/Users/adpena/Projects/molt/src/molt/frontend/__init__.py` (SimpleTIRGenerator, 26,000+ lines)
- `/Users/adpena/Projects/molt/src/molt/frontend/visitors/calls.py` (call protocol, super fold)
- `/Users/adpena/Projects/molt/src/molt/frontend/visitors/classes.py` (class definition)
- `/Users/adpena/Projects/molt/src/molt/frontend/visitors/pattern_match.py` (match/case)
- `/Users/adpena/Projects/molt/src/molt/frontend/lowering/serialization.py` (JSON kind emission, map_ops_to_json:396)
- `/Users/adpena/Projects/molt/src/molt/frontend/lowering/op_kinds_generated.py` (canonical kind constants)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs` (OpCode enum, TirOp)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/op_kinds_generated.rs` (kind_to_opcode table)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/iter_devirt.rs` (list loop devirt)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/range_devirt.rs` (range loop devirt)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/deforestation.rs` (genexpr fusion)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/overflow_peel.rs` (dual-loop int accumulator)
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops.rs` (call_binary_dunder, call_inplace_dunder:7927/7993)
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/object/ops_arith.rs` (molt_add, molt_inplace_add:260, reflected op chain)
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs` (inplace_add handler:4414, add handler:~4300)
- `/Users/adpena/Projects/molt/docs/design/foundation/25_op_kind_registry.md` (drift matrix, floordiv schism §4.1, phase-2 spec)
- `/Users/adpena/Projects/molt/docs/design/foundation/26_real-async-generators.md` (generator/coroutine audit + end-state)
- `/Users/adpena/Projects/molt/tests/differential/basic/arith_reflected_ops.py`
- `/Users/adpena/Projects/molt/tests/differential/basic/augassign_inplace.py`
- `/Users/adpena/Projects/molt/tests/differential/basic/fstring_debug_format_spec.py`
