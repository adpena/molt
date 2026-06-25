<!-- Wave-3 recon implementation plan (wf_00af7480-2ba, 2026-06-04), live-code-verified. -->

# ARC 2: Dual-Loop Overflow Peel for Loop Accumulator `>2^47` Performance

## 1. Problem Statement and Live Code Audit

**The bug:** A function-local `while` accumulator (`total = total + i` over 30 M iterations) runs 2.2x slower than CPython when `total` exceeds `2^47` — the NaN-box inline limit — because the accumulator phi falls to boxed BigInt adds. The accumulator is NOT module-scoped (module_slot_promotion already fixed that case, delivering 3.8x over CPython). This is the remaining function-local loop case.

**Why S6 correctly refuses:** SCEV classifies `total += i` as degree-2 (step = the IV `i`, which is itself an AddRec), yielding `ScevExpr::Unknown` for `total` (scev.rs ~line 46-52). `ValueRangeResult::fits_inline_int47` returns `false` for Unknown values (`value_range.rs:290-295`). `repr_by_value_for` seeds nothing into the `RawI64Safe` set for the accumulator phi. Correct: this is not a bounded IV — the sum genuinely grows without a proven upper bound.

**Soundness trap (from baton — verified still live):** The native path for `int_primary` add is branchless `iadd` with deferred overflow check at boxing escape (`function_compiler.rs:3961`). For unbounded accumulators, bare `iadd` wraps silently at `2^63`, and `molt_int_from_i64(wrapped_i64)` at the escape boundary produces a wrong BigInt — the wrapped value is arithmetically incorrect, not just out-of-range. The fix MUST detect overflow at the add and branch to a continuation seeded with the PRE-overflow operands.

**Live state of `lir.checked_overflow` triple:**
- `lower_to_lir.rs:237-267` (`lower_checked_i64_arithmetic`): emits a 3-result LirOp `(main: I64, overflow_box: DynBox, overflow_flag: Bool1)` with `lir.checked_overflow = true`. This is gated on `lowers_to_checked_i64_arithmetic` (`lower_to_lir.rs:202-235`), which requires both operands AND result to be `Repr::RawI64Safe` when a repr override is supplied.
- `lower_to_wasm.rs:780-813`: the WASM arm that consumes this triple is LIVE and functional — it emits raw `I64Add`, range-checks the sum against `[INLINE_INT_MIN, INLINE_INT_MAX]`, takes the runtime `molt_add` slow path when out of range. The `overflow_box` and `overflow_flag` values are set and emitted.
- The native backend (`function_compiler.rs`) does NOT have a consumer for `lir.checked_overflow`. Its int-primary path emits branchless `iadd` with overflow deferred to `ensure_boxed_overflow_safe` at escape boundaries only.
- LLVM (`lowering.rs:4173`) gates raw arithmetic on `is_inline_safe_int(result_id)`, which requires `RawI64Safe`; it already routes unproven accumulators to `molt_add`.

**Root cause for the perf cliff:** The accumulator phi is `MaybeBigInt` (unproven), so on native it reaches the `else` arm of `out_is_int_primary` at `function_compiler.rs:3966`, where `var_get_boxed_overflow_safe` boxes each operand and calls `molt_add` per iteration. LLVM takes the same boxed path. WASM takes the same boxed path via `emit_lir_binary_arith`'s `_ => { emit_get_boxed_for_repr … Call(0) }` fallthrough arm.

## 2. Design: The Dual-Loop Overflow Peel (TIR Transform)

The fix is a new TIR-level loop transform (not a backend-level hack) that rewrites a qualifying function-local loop from:

```
while i < N:
    total = total + i   # carries as MaybeBigInt
```

into a dual-loop pair:

```
# Fast loop: carry total as raw i64; checked add; bail on overflow
while i < N:
    (total_raw, overflowed) = checked_i64_add(total_raw, i)
    if overflowed:
        # seed boxed continuation with pre-overflow values
        total_box = box(total_pre) + box(i_pre)  # molt_add, BigInt-correct
        break → slow_continuation_start
# Slow loop: carry total as DynBox (boxed BigInt); runs only on overflow
while i < N:
    total_box = molt_add(total_box, i_box)
```

The fast loop ends either at the loop exit (normal case; `total_raw` is the result) or at overflow (rare; the slow loop takes over from the overflow point). The two loops together cover the full mathematical range: [-(2^63), 2^63) is carried raw, >=2^63 is handled by the BigInt continuation.

**Key invariants:**
1. The fast-loop checked add detects overflow AT the add (not at a boxing escape), using Rust's `i64::checked_add` equivalent at the TIR level: a new `OpCode::CheckedAdd` that produces `(sum: I64, overflowed: Bool)`.
2. The slow loop is seeded from the last VALID values before overflow, not from the wrapped sum. The wraparound sum must never feed `molt_int_from_i64`.
3. The transform is only applied to accumulators where: (a) the loop is a function-local natural loop (LoopRole metadata present), (b) the accumulator is an integer-typed loop-carried phi (block arg), (c) every in-loop update to the accumulator is `Add` or `Sub`, (d) the loop has no exception handlers (`!has_exception_handlers()`), (e) the loop has a recognized exit guard (so the slow continuation can reuse the same exit condition). Generator/async ops disqualify the function.
4. The slow loop shares the same exit block as the fast loop. Only the loop body is duplicated.

## 3. Phase-by-Phase Implementation Plan

### Phase A: `OpCode::CheckedAdd` — the new primitive (atomic, self-contained)

**Files changed:**
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/effects.rs`
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_wasm.rs`
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/verify.rs` (if exhaustive match)
- Any `matches!`-based oracle that enumerates opcodes (CRITICAL: grep `matches!(.*opcode` exhaustively)

**`tir/ops.rs`:** Add `CheckedAdd` to `OpCode` after `Add`. Two results: `results[0]` = sum (I64), `results[1]` = overflow flag (Bool). One operand semantic: lhs and rhs are I64. Contract: sum = `lhs + rhs` as a wrapping i64; overflow flag = 1 iff the addition overflowed signed i64. This is the only safe way to detect overflow at the add — the sum when overflow_flag=1 is the wrapped value and MUST NOT be used as a Python int.

**`effects.rs`:** `CheckedAdd` is `ReadOnly` (no side effects, no memory access, CSE-safe, movable). The CRITICAL lesson from the import-error parity work: any `matches!`-based oracle that gates on opcodes defaults to `false` (or `true` for the wrong polarity). Every exhaustive `match` in `effects.rs`, `passes/mod.rs` pass pipeline, and any opcode dispatch that may silently skip `CheckedAdd` must be audited and extended. Use `cargo test` (not just `cargo build`) to catch test-module warnings.

**Native backend (`function_compiler.rs`):** Add a case for `CheckedAdd`. Use Cranelift's `sadd_overflow` instruction (produces `(sum, overflow_flag)` as a pair). This is the only sound implementation: `sadd_overflow` is a hardware-exact signed-overflow detector. Map `results[0]` → sum variable, `results[1]` → overflow flag variable. Do NOT use branchless `iadd` + post-hoc range check here; that is the existing deferred-overflow pattern which is not what this op represents.

**WASM backend (`lower_to_wasm.rs`):** The existing `lir.checked_overflow` triple already implements the semantics needed for a single arithmetic op. `CheckedAdd` at LIR level should reuse the same emission path. However, `CheckedAdd` is a TIR-level opcode; the LIR lowering pass (`lower_to_lir.rs`) must map it to a `lir.checked_overflow`-annotated triple (same as `lower_checked_i64_arithmetic` already does for proven-RawI64Safe Add). The difference: `CheckedAdd` is emitted explicitly for the peel; `lir.checked_overflow` on a regular Add is emitted only when repr is proven. This path is already functional in `lower_to_wasm.rs:780-813`.

**LLVM backend (`lowering.rs`):** Emit `llvm.sadd.with.overflow.i64` intrinsic for `CheckedAdd`. Extract sum and overflow bit. This is LLVM's canonical checked-arithmetic intrinsic, available in inkwell.

**Soundness note on results[0] when overflow_flag=1:** The sum is the mathematically-wrapped-i64 value. Callers of `CheckedAdd` MUST only use `results[0]` when `results[1]` is false (the zero/non-overflow branch). The slow-continuation peel must use the PRE-add operands when seeding `molt_add`, not the wrapped `results[0]`. Enforce this contract via a TIR verifier check: if `CheckedAdd results[0]` is used anywhere other than the non-overflow branch, flag as invalid.

### Phase B: `overflow_peel` TIR pass — the dual-loop transform

**New file:** `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/overflow_peel.rs`

**Registration:**
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs`: `pub mod overflow_peel;` + add `run` call in `run_pipeline` between `range_devirt` and `iter_devirt` (range_devirt must have fired to canonicalize IV structure; the peel should run before iter_devirt which may further simplify loop structure)

**Pass signature:** `pub fn run(func: &mut TirFunction, _tti: &TargetInfo) -> PassStats` — `Mutates::Cfg` (inserts new blocks).

**Recognition:** Walk the function's loop headers (from `func.loop_roles`). For each header, find accumulator phis: block args at the header that are I64-typed, are updated by an `Add`/`Sub` in the loop body (back-edge block or its dominated body), and are NOT already `RawI64Safe` (check via a fresh `value_range_for` + `fits_inline_int47`). Skip functions with `has_exception_handlers()`. Skip loops whose back-edge block cannot be identified (multi-latch, complex control flow).

**Accumulator qualification predicate (all must hold):**
1. `phi_ty == TirType::I64` (or MaybeBigInt — same thing in the type system)
2. Exactly one back-edge incoming to the phi, of the form `Add(phi, step)` or `Add(step, phi)` where `step` is loop-invariant
3. Loop has a single exit guard (`find_loop_guard` from scev.rs:724 succeeds — re-export or call directly)
4. No other ops in the loop body read the accumulator result in a way that requires boxing it mid-loop (a use that is not itself an `Add` feeding back into the phi is disqualifying — simplifies the first implementation)
5. `!vr.fits_inline_int47(phi_id)` — i.e., we're in the unbounded case that S6 correctly refuses

**Transform:**

The TIR CFG before the transform (schematic):
```
preheader → header(iv_start, acc_start)
header(iv, acc):
  cond = Lt(iv, stop)
  CondBranch(cond, body, exit)
body:
  acc_next = Add(acc, iv)
  iv_next = Add(iv, 1)  [nsw]
  Branch → header(iv_next, acc_next)
exit:
  return acc (boxed somehow)
```

The transform produces:

```
preheader → fast_header(iv_start, acc_start_raw: I64)
fast_header(iv, acc_raw):
  cond = Lt(iv, stop)
  CondBranch(cond, fast_body, fast_exit)
fast_body:
  (acc_sum, overflowed) = CheckedAdd(acc_raw, iv)
  iv_next = Add(iv, 1)  [nsw, reused or cloned]
  CondBranch(overflowed, overflow_bridge(iv, acc_raw), fast_back(iv_next, acc_sum))
fast_back → fast_header
fast_exit:
  # acc_raw is the final value; box it at return
  return box_raw_int(acc_raw)
overflow_bridge(iv_at_overflow, acc_pre_overflow):
  # seed slow loop from pre-overflow values
  acc_box = molt_add(box(acc_pre_overflow), box(iv_at_overflow))
  iv_after = Add(iv_at_overflow, 1)  [nsw]
  Branch → slow_header(iv_after, acc_box)
slow_header(iv, acc_box):
  cond = Lt(iv, stop)  # same stop value as fast loop
  CondBranch(cond, slow_body, slow_exit)
slow_body:
  acc_box_next = molt_add(acc_box, iv)   # Call("molt_add", [acc_box, box(iv)])
  iv_next = Add(iv, 1) [nsw]
  Branch → slow_header(iv_next, acc_box_next)
slow_exit:
  return acc_box  # already boxed
```

Both `fast_exit` and `slow_exit` must join at the original `exit` block. If the original function has a single `Return` using `acc`, the transform must replace uses of `acc` in `exit` with a phi that merges the fast-exit boxed result and the slow-exit boxed result. This requires inserting a new merge block before `exit` with a block arg, branching from both exits into it.

**Representation wiring:** The fast-loop accumulator phi is type `I64` but must be physically carried as `RawI64Safe` in the fast loop. Because this transform runs at the TIR level (before LIR), the representation plan needs to be told the fast-loop phi is proven raw. The mechanism: annotate the fast-loop header's accumulator block arg with `no_signed_wrap` on ALL its incoming values — the preheader incoming (initial value from `box_int_value` or a ConstInt), and the fast-back incoming (the `CheckedAdd` result[0], which is only used on the non-overflow branch and is structurally bounded by the overflow check above it). With all incomings proven, `propagate_raw_i64_safe_values` in `representation_plan.rs:544` will raise the phi to `RawI64Safe` automatically, routing it through the fast native/WASM/LLVM paths.

Wait — this is wrong. `CheckedAdd results[0]` is NOT range-proven by `fits_inline_int47`; it can hold any i64 value (the sum before overflow). The correct approach: add a new `Repr` variant `RawI64Checked` (or annotate the phi with a TIR attribute `"loop_peel_raw_i64" = true`) that the representation plan reads directly to seed the `RawI64Safe` set for the fast-loop phi and its update.

More precisely: the `overflow_peel` pass annotates the fast-loop header block (or its accumulator arg) with a TIR attribute that means "this phi is structurally proven raw-i64 within this loop body because the CheckedAdd precondition guarantees no silent wrap". The representation plan's `repr_by_value_for` adds a new seeding rule: a block-arg with `loop_peel_raw_i64 = true` is seeded `RawI64Safe`, bypassing `fits_inline_int47` (which cannot prove this). The `propagate_raw_i64_safe_values` fixpoint then propagates it into the `CheckedAdd results[0]`.

The TIR attribute approach is the clean one: it keeps the proof localized to where the transform was applied and does not require a new Repr variant.

**Native backend consumption of `CheckedAdd`:** With the fast-loop phi raised to `RawI64Safe` and the `CheckedAdd` op in the body:
- The phi arrives at Cranelift's `int_primary_vars` (via `primary_name_sets().int`).
- `CheckedAdd` is lowered by the new case in `function_compiler.rs` using Cranelift's `sadd_overflow`, storing `results[0]` in the main variable and `results[1]` in a bool variable.
- The overflow branch: `CondBranch(results[1], overflow_bridge, fast_back)`.
- The `overflow_bridge` block receives `(iv_at_overflow: I64, acc_pre_overflow: I64)` as block args (both `RawI64Safe` — they are copies of the phi and the IV which are both raw at that point). It boxes them via `ensure_boxed_overflow_safe` and calls `molt_add`.

This is the first time an overflow exit from the raw-fast loop uses the PRE-add values (`acc_raw`, the phi BEFORE the add was applied) as the seed for the slow-loop. The transform must explicitly thread the pre-overflow phi value as a block arg into `overflow_bridge`.

### Phase C: Observability instrument

**Env var:** `MOLT_OVERFLOW_PEEL_STATS=1` — printed to stderr per function, showing how many loops were peeled, how many accumulators qualified, and how many were refused (with the first refusal reason). This is the L4/needs_inlining lesson: synthetic tests pass while real code refuses through unpredictable layers. The instrument must fire on `bench_sum` and show `peeled=1 accumulators=1`.

**Placement:** In `overflow_peel::run`, after the recognition + transform loop, print if the env var is set: `[overflow_peel] func '{}': {} loops peeled, {} refused (first: {:?})`. Use an enum `PeelRefusal { NoLoop, MultiLatch, HasExceptionHandlers, NotUnbounded, ComplexAccumulator, ... }` to give actionable refusal codes.

### Phase D: Pass ordering integration

Current pipeline order (`passes/mod.rs:132-160`):
```
range_devirt → iter_devirt → tuple_scalarize → loop_unroll → canonicalize →
unboxing → block_versioning → canonicalize_post → gvn → licm → escape_analysis →
refcount_elim → reuse_analysis → dead_store_elim → type_guard_hoist → sccp →
strength_reduction → fast_math → branchless_count → bce → vectorize → polyhedral →
check_exception_elim → copy_prop → dce
```

Insert `overflow_peel` after `range_devirt` (so the IV shape is canonical) and before `iter_devirt` (so iter_devirt can devirtualize list access in the fast loop body). Register it as `Mutates::Cfg` so the `AnalysisManager` invalidates CFG-sensitive analyses (PredMap, ImmediateDoms, LoopForest) after it runs.

Update the test at `passes/mod.rs:132` (the pipeline name sequence test) to include `"overflow_peel"` in the correct position. The test is exhaustive and will fail at compile time if the pass is not registered or is in the wrong order.

### Phase E: Legacy deletion (none in this arc)

The `compute_i64_interval_facts` chain in `representation_plan.rs:2029` is the LEGACY proof source for the name-keyed (native) `int_primary` set. It handles bounded loop IVs (via `propagate_counted_loop_intervals`) but correctly cannot handle unbounded accumulators. This arc does NOT delete the legacy chain — that is a separate bounded arc (the full migration to value-range-based name-keyed proof). The overflow_peel pass adds a new structural fast path that BYPASSES the legacy chain for the accumulator phi, via the TIR attribute. No legacy code is deleted here.

## 4. Soundness Argument

**The wrapped-sum trap:** `CheckedAdd results[0]` when `results[1] = true` holds the mathematically-wrapped i64 (e.g., `-9223372036854775808` when `max_i64 + 1` overflows). The TIR verifier enforces that `results[0]` is NEVER used when `results[1]` may be true: the overflow branch must lead to a block that only uses `results[1]`, and the non-overflow (fast-back) branch uses `results[0]`. The `overflow_bridge` block is seeded from the CALLER'S block args (the pre-add `acc_raw` and `iv` values), NOT from `results[0]`. This is structurally enforced by how the transform wires the block args.

**The BigInt-exact slow loop:** The slow loop carries `acc_box: DynBox` (a NaN-boxed Python object, which may be a heap BigInt). Every iteration calls `molt_add(acc_box, box(iv))`, which is the runtime's BigInt-correct addition — it handles heap BigInt operands, inline-int operands, and mixed operands. The slow loop's result is byte-identical to what CPython would compute.

**Exit merge correctness:** The fast-exit boxes `acc_raw` via `molt_int_from_i64` (or the `ensure_boxed_overflow_safe` path) before passing to the merge phi. The slow-exit passes `acc_box` directly (already boxed). Both arrive at the same merge phi with `TirType::DynBox`. This is the standard "box at exit" discipline already used throughout the native backend.

**The stop/iv invariant across both loops:** Both loops use the same `stop` value (loop-invariant, defined in the preheader). The slow loop's `iv` is seeded from `iv_after_overflow = iv_at_overflow + 1`, which is the iteration AFTER the overflow. Because the slow-loop body recomputes the full Python-correct `acc_box += iv` starting from this point, the final result equals the sum `acc_start + sum(range(i_start, N))` in exact Python arithmetic — no iterations are skipped or double-counted.

## 5. Differential Test Matrix

Test file prefix: `tests/differential/loop_overflow_peel/`

| Shape | Description | Why |
|---|---|---|
| `sum_under_47bit.py` | `sum(range(1_000_000))` — stays under 2^47 | Fast path only; must not regress |
| `sum_over_47bit.py` | `sum(range(100_000_000))` — final sum ~5e15 > 2^47 | Peel fires; result byte-identical CPython |
| `sum_cross_63bit.py` | accumulate `1<<62` + many large values crossing 2^63 | Triggers slow loop; BigInt-exact required |
| `sum_negative.py` | accumulate negative values, result < -(2^47) | Peel fires; result byte-identical |
| `sum_exception_mid.py` | function with `try/except` wrapping the loop | Peel must be REFUSED (`has_exception_handlers`) |
| `sum_while_nonunit.py` | `total += i*i` — non-unit step accumulator | Peel refused (complex accumulator); must not regress |
| `sum_bigint_seeded.py` | initial `acc` is already a large Python int (BigInt) | Peel refused (initial value not provably I64); goes boxed path |
| `sum_exact_overflow_boundary.py` | last add overflows: `acc = 2^63 - 2; acc += 1` → 2^63-1; `acc += 1` → 2^63 | Triggers slow loop at last step; exact BigInt result |
| `sum_multi_accum.py` | two separate accumulators in one loop | Both peeled or both refused, no partial |
| `cross_backend_parity.py` | compile with native, wasm, llvm; run; compare outputs | All 3 backends must agree with CPython |

Each test: compile with molt, run, diff stdout against `python3 test.py`. Test runner: `python3 -m molt build --target native/wasm/llvm --output /tmp/test_out test_file.py --rebuild` plus the watchdog `python3 tools/safe_run.py --rss-mb 2048 --timeout 15`.

## 6. Performance Gate

Benchmark: `bench_sum` (30M iteration sum, function-local).

Target: `molt native` >= 2.0x faster than CPython 3.14 on `bench_sum` (currently 2.2x SLOWER). The peel should bring this to 5-10x faster (raw i64 loop with a single `sadd_overflow` per iteration plus an almost-never-taken branch to the slow loop).

Measurement: `python3 tools/safe_run.py --rss-mb 2048 --timeout 60 -- ./bench_sum_molt_native` vs `python3 bench_sum.py`. Gate: pass only if molt result < 0.5x CPython time (i.e., molt is at least 2x faster).

The instrument (`MOLT_OVERFLOW_PEEL_STATS=1`) must show `peeled=1` for `bench_sum` before the gate is run. If it shows `peeled=0`, the perf gate is meaningless — debug the recognizer first.

## 7. Backend-by-Backend Consumption of `CheckedAdd`

### Native (Cranelift)

In `function_compiler.rs`, add a new arm in the SimpleIR op dispatch (the `match op.kind.as_str()` block) for the `CheckedAdd` op when it reaches the native backend. The op arrives from the TIR→SimpleIR lowering (already handled via `lower_from_simple.rs` / `passes.rs` SimpleIR inliner roundtrip).

Wait — this is wrong. The overflow_peel pass operates at TIR level. The native backend compiles TIR through the SimpleIR round-trip (`lower_to_simple.rs` → `simple_backend.rs`). The `CheckedAdd` TIR opcode must have a round-trip representation in SimpleIR. Options:
1. Add a `checked_add` SimpleIR op kind, handled in `simple_backend.rs` using Cranelift `sadd_overflow`.
2. Lower `CheckedAdd` to a pair of SimpleIR ops: a plain `add` producing the sum (in int_primary) plus a new `i64_overflow_check` op that produces the flag.

Option 1 is cleaner and avoids a spurious two-op sequence. The SimpleIR `checked_add` op has two outputs (the sum and the flag), mirroring the TIR semantics. The native backend's `function_compiler.rs` adds a case for `op.kind == "checked_add"` that emits Cranelift `sadd_overflow`.

**For WASM and LLVM:** Both consume TIR directly (not via SimpleIR). `lower_to_lir.rs` maps `OpCode::CheckedAdd` to a `lir.checked_overflow`-annotated triple (reusing `lower_checked_i64_arithmetic`'s shape but emitting it unconditionally for the explicit opcode). The WASM consumer at `lower_to_wasm.rs:780` already handles this shape. The LLVM consumer at `lowering.rs` adds a new arm for `OpCode::CheckedAdd` using `llvm.sadd.with.overflow.i64`.

## 8. New TIR Attribute for Fast-Loop Phi Promotion

The `overflow_peel` pass annotates the fast-loop header block with:
```
attrs: {"loop_overflow_fast_phi_ids": AttrValue::List([ValueId(N), ...])}
```
listing the ValueIds of the accumulator phis that must be treated as `RawI64Safe` in the fast loop. The representation plan's `repr_by_value_for` adds a seeding rule: before the `raw_i64_safe_value_seed` call, check `tir_func`'s entry block (or loop header block) for this attribute and pre-seed those ValueIds as `RawI64Safe`. Then `propagate_raw_i64_safe_values` propagates it to the `CheckedAdd results[0]` through the non-overflow Copy/value-identity edges.

This attribute is consumed by `representation_plan::repr_by_value_for` (`representation_plan.rs:404-439`) and by `LlvmReprFacts::build`. The native backend reads the plan's `int_carrier_names()` view which already exports `RawI64Safe` values to `primary_name_sets().int`.

## 9. Files to Change (with current line anchors)

| File | Change |
|---|---|
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs:22` | Add `CheckedAdd` to `OpCode` enum after `Add` |
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/effects.rs` | Add `CheckedAdd` arm (ReadOnly, CSE-safe) to ALL exhaustive `match` on OpCode |
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_lir.rs:202` | `lowers_to_checked_i64_arithmetic`: add `OpCode::CheckedAdd` as unconditionally eligible (no repr guard needed — it is always a checked triple) |
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_wasm.rs:780` | Already handles `lir.checked_overflow` — no change needed beyond ensuring `CheckedAdd` lowers via `lir.checked_overflow` |
| `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs:4138` | Add `OpCode::CheckedAdd` arm using `llvm.sadd.with.overflow.i64` |
| `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs` | Add `checked_add` SimpleIR op emission using Cranelift `sadd_overflow` |
| `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs` | Add `"checked_add"` arm in op dispatch |
| `/Users/adpena/Projects/molt/runtime/molt-backend/src/representation_plan.rs:404` | In `repr_by_value_for`: add pre-seeding for `loop_overflow_fast_phi_ids` attribute |
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs:77` | Add `pub mod overflow_peel;` and `run_pass!(overflow_peel, Mutates::Cfg)` after `range_devirt` |
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/overflow_peel.rs` | **New file** — full pass implementation |
| `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/verify.rs` | Add `CheckedAdd` validation: 2 results (I64, Bool), 2 operands (I64, I64) |
| Tests at `passes/mod.rs:132` | Add `"overflow_peel"` in correct position |

**Exhaustive `matches!` audit required before landing Phase A:**
```
grep -r "matches!.*opcode\|matches!.*OpCode" runtime/molt-backend/src/
```
Every site that lists opcodes in a `matches!` macro without a `..` wildcard and is NOT already exhaustive must be updated to include `CheckedAdd`. The known ones are in `effects.rs`, but there may be others in `passes/bce.rs`, `passes/gvn.rs`, `passes/licm.rs`, `tir/verify_lir.rs`, `tir/lower_to_simple.rs`, and the `is_side_effecting`/`opcode_may_throw` oracles.

## 10. What Is NOT Changed in This Arc

- `scev.rs`: degree-2 accumulator continues to yield `Unknown` — correct behavior. The fast-loop IV is still an AddRec (clean); only the accumulator phi is structurally different after the transform.
- `value_range.rs`: `fits_inline_int47` still returns false for unbounded accumulators. The peel's fast-loop phi is seeded via the TIR attribute, not via the value-range analysis.
- `module_slot_promotion.rs`: module-scoped loops already get 3.8x speedup via this pass. This arc is orthogonal (function-local only).
- `lir.checked_overflow` triple semantics: unchanged; the WASM path already consumes it correctly.
- `ensure_boxed_overflow_safe` in the native backend: unchanged; still used at escape boundaries for general int-primary variables.

## 11. Landing Sequence (single atomic arc)

This arc is ONE complete structural change. The phases above (A, B, C, D) are implementation sub-steps, not half-measures to commit. The arc is not done until:
1. `cargo test` passes with 0 new warnings on all features (native-backend, wasm-backend, llvm).
2. `MOLT_OVERFLOW_PEEL_STATS=1` shows `peeled=1` for `bench_sum`.
3. All differential tests pass byte-identical against CPython 3.12, 3.13, 3.14.
4. Perf gate: molt `bench_sum` >= 2x faster than CPython 3.14.
5. `MOLT_VERIFY_ANALYSIS=1` passes on the post-transform TIR (the new blocks have correct LoopRole metadata and the AnalysisManager correctly invalidates after `overflow_peel` runs).

If phases A-D cannot land in one session, the correct baton is: leave `OpCode::CheckedAdd` unregistered in `ops.rs` (compile-error if used), with a baton note describing exactly which files and line ranges to touch. Do NOT commit a half-landed `overflow_peel.rs` that bypasses the recognizer conditions.

## 12. Essential Files

- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs` — OpCode enum
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/scev.rs` — degree-2 refusal proof, AddRec soundness rules
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/value_range.rs` — `fits_inline_int47`, `INLINE_INT47_LO/HI`
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/representation_plan.rs` — `repr_by_value_for`, `propagate_raw_i64_safe_values`, `compute_i64_interval_facts` (legacy)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_lir.rs` — `lower_checked_i64_arithmetic`, `lowers_to_checked_i64_arithmetic`, `ReprOverride`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_wasm.rs` — `emit_lir_binary_arith` (lir.checked_overflow consumer, lines 780-813)
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs` — `emit_binary_arith`, `is_inline_safe_int` gate (line 4173)
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs` — branchless iadd + `ensure_boxed_overflow_safe` (lines 3951-3965, 495-547)
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/simple_backend.rs` — `int_value_fits_inline` (line 781), `imul_checked_inline` (line 804)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs` — pipeline registration and pass name test
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/module_phase.rs` — `run_module_pipeline` (where overflow_peel slot would go at module scope if ever needed; NOT needed for this arc)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/function.rs` — `TirFunction` struct, `has_exception_handlers()`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/effects.rs` — exhaustive opcode match oracle (CRITICAL for CheckedAdd registration)

---

## IMPLEMENTATION WORKSHEET (verified 2026-06-04, post-plan grounding)

### The 2-result SimpleIR round-trip convention (CRITICAL — verified from IterNextUnboxed)
- **Emission** (`lower_to_simple.rs:1620`): `var` = results[0] (the value/sum), `out` = results[1]
  (the done/flag). `checked_add` mirrors: `var` = sum, `out` = overflow flag.
- **Lift** (`ssa.rs get_def_vars`, ~line 967 region): a special-case arm returning `[var, out]` in
  that order; `checked_add` needs the same arm + the kind→`OpCode::CheckedAdd` mapping in the
  opcode table (~ssa.rs:1808 region).
- **WHY ROUND-TRIP IS MANDATORY**: the module phase (`lower_functions_to_tir_module`) RE-LIFTS
  every function from its post-pipeline SimpleIR on EVERY build, and the TIR content cache stores
  post-pipeline SimpleIR that re-lifts on cache hits. An op that doesn't round-trip falls to the
  `OpCode::Copy` fallback and silently vanishes — the exact iterator-consumer-bug class
  (`exception_pending`'s comment in ssa.rs documents the precedent).

### The matches!-oracle audit map (15 files, classified)
- **Compiler-enforced (exhaustive `match`, will not compile until classified)**: `effects.rs`
  (assert_opcode_is_listed + all_opcodes + opcode_effects table) — classify CheckedAdd:
  side-effect-free, NOT pure-movable first cut (2-result op; keep GVN/LICM hands off until
  verified), never-throws, no-memory.
- **`false`-default SAFE (conservative for a new opcode)**: type_refine (5 sites — no type fact ⇒
  the peel seeds types explicitly), block_versioning (4), scev (2 — accumulator stays Unknown by
  design), polyhedral (2), module_slot_promotion (2 — CheckedAdd is pure_movable=false ⇒ falls to
  region check ⇒ ScalarRegister ⇒ not a barrier… VERIFY region_of default for CheckedAdd:
  `opcode_touches_memory` must return false or it lands in GenericHeap and barriers promotion —
  add it to the not-touches-memory set in alias_analysis.rs:642 region), vectorize, value_range,
  memory_ssa (partner file — coordinate; likely needs a no-memory classification arm),
  gvn, escape_analysis, branchless_count, check_exception_elim, ssa (the 2 sites are the
  multi-result handling — covered by the lift arm above).
- **Intentional new arms**: lower_to_lir (`lowers_to_checked_i64_arithmetic` + the triple
  emission), verify.rs (arity: 2 operands I64, 2 results I64+Bool), native function_compiler
  (`"checked_add"` → Cranelift `sadd_overflow`), llvm lowering (`llvm.sadd.with.overflow.i64`).

### Landing prerequisites still open (parity directive)
1. The Luau CheckedAdd lowering decision (swarm wf_971517d5-6b2 lane 2: runtime-helper vs
   target-gated refusal-with-verified-boxed-fallback) — REQUIRED before the atomic landing.
2. LLVM/Luau inheritance verification of E1+promotion (lane 1) — determines whether the peel's
   pipeline placement (per-function, after range_devirt) reaches all targets uniformly.
3. The profile matrix (dev-fast) measurement commands from lane 1 → the peel's perf gate must
   cover release-fast AND dev-fast per the directive.

### Order of work for the implementation session
1. ops.rs variant + effects.rs exhaustive arms (compiler walks you through every site).
2. alias_analysis opcode_touches_memory + the audit's intentional arms (lift/emit/verify/lir).
3. Native + LLVM lowerings; Luau per the swarm decision.
4. Unit test: construct CheckedAdd → full SimpleIR round-trip (lift→emit→lift) preserves opcode +
   both results; native compile smoke of a hand-built function.
5. THEN overflow_peel (the transform) + MOLT_OVERFLOW_PEEL_STATS + the recognizer; iterate with the
   instrument on bench_sum 30M until peeled=1 (expect refusal layers — budget for them).
6. The 11-shape differential matrix ×{native,wasm,llvm,luau} + perf gates ×{release-fast,dev-fast}.
