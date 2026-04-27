# TIR Pipeline Completeness Arc — Year 1+ Plan

**Status**: in progress (Phase 3 frontend + TIR landed; Phases 1, 2, 4 outstanding).
**Owner**: autonomous Year-1 work, multi-session.
**Trigger**: `bench_class_hierarchy.py` runs 42x slower than CPython (18.76s vs 0.44s), revealing structural gaps in TIR pipeline.

---

## Background — what was wrong

`tests/benchmarks/bench_class_hierarchy.py` runs 5M iterations of `total += obj.compute(i)` through a Base/Mid/Leaf class hierarchy with `super()`. Result: 42x slower than CPython AND silently coerces the integer accumulator to float.

Decomposed via experimental ablation:
- **Plain function call** (no class): 0.08s, prints int correctly.
- **Method call on flat class** (no super): 5.07s, prints float (`24999995000000.0`).
- **Method hoisted** (`fn = obj.compute; fn(i)`): 0.55s, prints int correctly.
- **Without super**: 5.97s.

Conclusion: bound-method allocation per iteration is ~4.5s of the 5s overhead; super() adds another ~10s; type-erasure on the call result causes the float coercion.

---

## Audit summary (research agent, 2026-04-27)

The TIR pipeline runs 24 passes (`runtime/molt-backend/src/tir/passes/mod.rs:62-212`). The following are **structural completeness gaps**:

1. **No inlining pass** — every method call goes through full dispatch.
2. **No bound-method fusion** — `LoadAttr → Call(BoundMethod)` (1-use) still allocates a heap BoundMethod per iteration. CPython 3.11+ has `LOAD_METHOD/CALL_METHOD` to avoid this.
3. **No type-aware method devirtualization** — `BoundMethod:` callees fall through to IC dispatch even when class is statically known. `Func:` callees have a devirt path (`frontend/__init__.py:16230-16257`); BoundMethod does not.
4. **Conservative escape analysis** — `OpCode::Call → GlobalEscape` (`escape_analysis.rs:194-198`) prevents BoundMethod stack-allocation. Even if relaxed, BoundMethod alloc is hidden inside the runtime helper, not represented as a TIR `Alloc` op (structural gap).
5. **No PRE / code sinking** — partially-redundant attribute loads are not optimized.
6. **LICM `is_hoistable` whitelist excludes LoadAttr** (`licm.rs:51-89`) — correctly conservative for general dispatch (descriptor protocol can have side effects), but no escape valve for type-stable loads with version-guarded receivers.
7. **No method-cache (LOAD_METHOD-style) op** — `OpCode::CallMethod` (`tir/ops.rs:72`) still requires a heap-allocated BoundMethod input.
8. **No devirtualizing IC for method dispatch** — `(receiver_type_id, layout_version) → callee fn ptr` IC does not exist.
9. **No effect tracking for user-defined methods** — `passes/effects.rs` only knows builtin types.

Float-coercion bug root cause:
- `frontend/__init__.py:16205-16207, 16772-16784, 19752-19754` — three sites that drop `return_hint` for builtin scalars (`int`/`float`/`bool`/`str`/`bytes`). **Fixed in this arc's first commit.**
- Deeper TIR fix needed: `infer_result_type` returns `None` for `OpCode::Call` (`type_refine.rs:824`). **Fixed in this arc's second commit by reading `_type_hint`/`return_type` AttrValue.**

---

## Phase plan (target sequencing)

### ✅ Phase 3 — float-coercion underlying fix (DONE — commit pending push)

- Frontend: extend `res_hint` gate to `BUILTIN_TYPE_TAGS` at the three sites.
- TIR: `infer_result_type_with_attrs` reads `return_type` then `_type_hint` AttrValue for `Call`/`CallMethod`/`CallBuiltin`; pre-snapshot the seeded type in `refine_types`'s op-snapshot tuple to avoid per-round AttrDict cloning.
- Files: `src/molt/frontend/__init__.py`, `runtime/molt-backend/src/tir/type_refine.rs`.
- Status: **landed, 608/0 backend, 2055/0/21 workspace, clippy clean.**

### Phase 1 — bound-method fusion TIR pass (P0 perf)

**Goal**: eliminate per-iter BoundMethod allocation when `LoadAttr → Call(BoundMethod, args)` has 1-use.

**Design**:
- New TIR opcode `CallMethodDirect` (`tir/ops.rs:22` enum). Operands: `[receiver, ...args]`. Attrs: `method_name: Str`, `class_name: Str`, `layout_version: Int`, `ic_index: Int`. Result: same type as the original Call.
- New SimpleIR kind `call_method_direct` for the round-trip.
- TIR pass `runtime/molt-backend/src/tir/passes/bound_method_fusion.rs`:
  - For each `LoadAttr` whose result is a BoundMethod with exactly one use, AND that use is a `Call` with kind=call_bind, replace the pair with one `CallMethodDirect(obj, ...args)` op.
  - Use-count check via the existing `dce.rs` value-use map.
- Backend lowering: `runtime/molt-backend/src/native_backend/function_compiler.rs` — emit IC probe + cached fn ptr direct call; on miss, fall back to `molt_call_bind_ic`.
- Runtime helper: `molt_call_method_direct_ic(site, receiver, method_name_id, args_ptr, args_len)` in `runtime/molt-runtime/src/call/bind.rs`.

**Files to touch**: `tir/ops.rs`, `tir/passes/mod.rs` (register before gvn), `tir/passes/bound_method_fusion.rs` (new), `tir/lower_to_simple.rs`, `tir/lower_from_simple.rs`, `tir/type_refine.rs` (reads `return_type` for new opcode), `native_backend/function_compiler.rs` (lowering arm), `call/bind.rs` (new helper).

**Tests**: `tests/test_bound_method_fusion.py` — class with `compute(self, x:int)->int`; loop calls `obj.compute(i)` 1M times; verify no `alloc_bound_method_obj` calls (count via debug counter); `tests/benchmarks/bench_class_hierarchy.py` should drop 5s → ~0.5s.

**Expected**: 4.5s → ~0.3s on `bench_class_hierarchy.py`.

### Phase 4a — static super() fold (P0 perf)

**Goal**: when `super()` is called with no args inside a method body and class MRO is statically known, resolve to the parent's method directly.

**Design**:
- Frontend (`src/molt/frontend/__init__.py:18615-18664`): replace SUPER_NEW emission with direct call resolution when current class's MRO is statically determined (no metaclass, no `__init_subclass__` surprises, bases all in `self.classes`).
- Compute `Leaf.__mro__ = [Leaf, Mid, Base, object]` statically.
- For `super().compute(x)` from inside `Leaf.compute`, resolve to `Mid.compute`; emit a `CALL` directly with `self` prepended.

**Expected**: 10s → ~0.5s for super-portion of bench_class_hierarchy.

### Phase 4b — cached super-bound-method (P1 perf)

When static resolution fails (dynamic class creation, `super(type_var, obj)`), introduce an IC slot keyed by `(class_id, method_name, mro_layout_version)` that caches resolved fn ptr.

### Phase 2 — small-method inliner (P1 perf, biggest correctness ceiling)

**Predicates** (all required):
1. Caller's site has known static `class_name` for the receiver.
2. Callee body ≤ 8 ops in single basic block.
3. No `closure_load`/`closure_store`.
4. No `try_start`/`raise`/`check_exception`.
5. No `super()`.
6. No `yield`/`yield_from`.
7. Arity ≤ 4 (configurable).
8. Param types are scalars or typed user classes (no `*args`/`**kwargs`).
9. Per-site inline depth ≤ 2.

**Files**: `runtime/molt-backend/src/tir/passes/inline.rs` (new); `tir/passes/mod.rs` register after `bound_method_fusion`. Need callee TIR cache via existing `tir/cache.rs`.

### Phase 5 — escape analysis upgrade

Stop forcing every `OpCode::Call` to `GlobalEscape`. Add an effect descriptor `consumes_first_arg` for `call_bind` and similar; teach escape_analysis to honour it. Pair with the BoundMethod alloc being a real TIR `Alloc` op (so escape analysis can see it).

### Phase 6 — generalised LICM with type-version guards

When a class layout is statically known and stable, hoist `LoadAttr` operations out of loops with a layout-version guard at the loop preheader. Falls back to per-iter dispatch on guard failure (CPython-style "deopt" pattern, but at TIR level).

### Phase 7 — float arithmetic hot paths

User directive: "shouldn't float and all have hot paths too". Audit the loop fast-path code in `function_compiler.rs` (`emit_int_loop_specialized`, etc.) and ensure the F64 lane has equivalent hot-path coverage. Currently `float_like_vars` population path (line 1525-1629) is far less developed than `int_like_vars`.

### Phase 8 — full benchmark sweep + per-bench root-cause

Run `tools/bench.py` over the full 50+ benchmark corpus. Classify each:
- ≥ 1.5x faster than CPython: keep, cover with a perf regression test.
- 0.5x – 1.5x of CPython: investigate; likely lane-inference / type-hint propagation gap.
- < 0.5x of CPython: structural gap; assign to one of the phases above.

Target: every benchmark ≥ 1.0x of CPython, and ideally ≥ 2x (the project's stated competitive position).

---

## 5–10 year arc considerations

- **Year 2**: full Typed-IR redesign — eliminate `fast_int`/`raw_int`/`type_hint` transport hints; all type information lives in TIR ops. Phase 3 already begins migrating from `_type_hint` (legacy) to `return_type` (structural).
- **Year 3**: PGO + LTO + auto-vectorization at the TIR level. Phase 6's type-version-guarded hoisting is a stepping stone.
- **Year 4**: MLIR backend; formal verification of TIR passes via Z3; translation validator. Inlining (Phase 2) needs to compose with verification.
- **Year 5+**: replace CPython for AOT; self-hosting; language extensions.

---

## File:line index (for next session pickup)

| Concern | File | Lines |
|---|---|---|
| TIR pipeline registration | `runtime/molt-backend/src/tir/passes/mod.rs` | 62-212 |
| LICM whitelist | `runtime/molt-backend/src/tir/passes/licm.rs` | 51-89 |
| Escape Call→GlobalEscape | `runtime/molt-backend/src/tir/passes/escape_analysis.rs` | 194-198, 232-237 |
| TIR Call/CallMethod opcodes | `runtime/molt-backend/src/tir/ops.rs` | 71-72 |
| `infer_result_type` (now attrs-aware) | `runtime/molt-backend/src/tir/type_refine.rs` | 728+ |
| `annotate_type_flags` legacy hint | `runtime/molt-backend/src/tir/lower_to_simple.rs` | 2753-2806 |
| `op_kind_already_classified` | `runtime/molt-backend/src/tir/lower_to_simple.rs` | 2811-2905 |
| Frontend res_hint sites (Phase 3 fix) | `src/molt/frontend/__init__.py` | 16205, 16782, 19767 |
| Frontend SUPER_NEW emission | `src/molt/frontend/__init__.py` | 18615-18664 |
| Native call_bind lowering | `runtime/molt-backend/src/native_backend/function_compiler.rs` | 16322-16472 |
| Native call_method + builtin fast paths | `runtime/molt-backend/src/native_backend/function_compiler.rs` | 16474-16649 |
| `alloc_bound_method_obj` | `runtime/molt-runtime/src/object/builders.rs` | 1029-1042 |
| `molt_call_bind_ic` | `runtime/molt-runtime/src/call/bind.rs` | 2483 |
| `molt_super_new` | `runtime/molt-runtime/src/builtins/types.rs` | 1098-1147 |
| Inline cache structure | `runtime/molt-runtime/src/object/inline_cache.rs` | 25-89 |
