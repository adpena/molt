<!-- Foundation blueprint (architect swarm wf_18b24759-006, 2026-06-04). Arc: L2 real SIMD codegen across LLVM / Cranelift / WASM simd128 (vectorize is currently dead) -->

# Real SIMD Codegen Architecture Blueprint

## 1. Precise Problem Statement

`vectorize.rs` is a `ReadOnly` TIR pass that annotates loop-header ops with attrs (`vectorize=true`, `element_type`, `simd_width`, `reduction`, `promoted`). Every backend reads exactly zero of those attrs for codegen purposes:

- `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs:2698` — `let _ = has_attr(op, "vectorize");` (dead read, explicitly discarded)
- The Cranelift `function_compiler.rs` 4x-unroll at line 30815 is a manually written scalar unroll of 4 elements, not SIMD — it emits `iadd_imm / load / iadd` four times sequentially, with no `I64X2`/`F64X2` types
- `lower_to_wasm.rs` imports only `BlockType, Ieee64, Instruction, ValType` from `wasm_encoder` — no `V128` instructions

`Dialect::Simd` at `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs:17` has zero ops defined. The existing 4x unroll in `function_compiler.rs:30815-30980` is the only "vectorization" in the entire system, and it is a scalar loop with width 4 on the SimpleIR path, entirely disjoint from TIR.

This matters because:
- molt's perf contract is "faster than CPython on every benchmark on every target." The numeric benchmarks (`bench_sum`, `bench_sum_list`, `bench_prod_list`, `bench_matrix_math`) are the primary regression canaries. Without SIMD these loop bodies bottleneck at ~1 ADD/cycle throughput; AVX2 delivers 4 64-bit adds/cycle or 8 32-bit; the theoretical win is 4–8x on register-resident arithmetic.
- The `vectorize` pass is already in the pipeline at position 21 of 25 (after all cleanups and type refinement), `TargetInfo.vector_width_*` is already populated from `SimdCaps::detect_host()` (verified at `simple_backend.rs:2544-2545` and `main.rs:2201-2202`), and `LirRepr::I64`/`F64` are the proven representations for exactly the values that can safely enter SIMD lanes. The substrate is ready. The backends just emit nothing.

## 2. Structurally-Correct Design

### End-State Architecture

The system has two orthogonal axes to respect:

1. **Repr correctness**: only `Repr::RawI64Safe` and `Repr::FloatUnboxed` values enter SIMD lanes. `MaybeBigInt` values (unproven ints) never do. This is already enforced by `LirRepr::for_type` and the repr override path in `lower_to_lir.rs`. The SIMD lowering must check this.

2. **Backend specificity**: Each backend has its own SIMD ISA:
   - Cranelift: `types::I64X2` / `types::F64X2` (128-bit), `I64X4` / `F64X4` (AVX2, 256-bit), `I64X8`/`F64X8` (AVX-512F, 512-bit). Ops: `vconst`, `splat`, `iadd`, `fadd`, `extract_lane`, `vhigh_bits`, `load`, `store`, with SIMD load/store using `types::I64X2` on standard memory.
   - WASM simd128: `wasm_encoder::Instruction::I64x2Add`, `F64x2Add`, `V128Const`, `V128Load`, `V128Store`, `I64x2ExtractLane`, `F64x2ExtractLane`, `I64x2Splat`, `F64x2Splat`. Always 128-bit (2 lanes for i64/f64).
   - LLVM: emit `llvm.loop.vectorize.enable` metadata via `LLVMSetMetadata` on the back-edge branch, letting LLVM's own loop vectorizer handle the ISA-specific vector selection. Optionally emit explicit `<2 x i64>` / `<2 x double>` vector types for the reduction patterns.

### Architecture: TIR Dialect Ops + Lowering Specialization

The design introduces exactly three new TIR dialect ops in `Dialect::Simd`:

```
Simd::VecLoad { width: u8 }    // load `width` consecutive scalars into a vector value
Simd::VecAdd                    // element-wise add of two vector values
Simd::VecReduce { kind: ReductionKind }   // horizontal reduction of a vector → scalar
```

These ops are NOT inserted by the vectorize annotator. Instead, a new **vectorize-lower pass** (`passes/vectorize_lower.rs`) runs after `vectorize` (as a new `Cfg`-class pass that replaces scalar loop blocks with SIMD blocks), consuming the `vectorize=true` annotation and the proven `LirRepr` of the loop's values to decide whether to emit Simd ops or leave the loop scalar.

**Conservative first cut:** The vectorize-lower pass ONLY transforms loops that:
1. Have `vectorize=true` on their ForIter/ScfFor op
2. The annotated `element_type` is `i64` or `f64`
3. The loop iterates over a `List(I64)` or `List(F64)` with a structurally-proven `FlatListInt` storage layout (same condition already enforced by `scan_loop_int_sum_reduction` for the existing 4x unroll)
4. Have a `reduction=sum` or `reduction=product` annotation (the simplest pattern with horizontal-reduce semantics)
5. `has_exception_handling == false` on the function

This conservative cut eliminates all unsound cases — no MaybeBigInt, no boxed containers, no exception edges — and delivers the headline perf win (sum/dot reductions are the archetypal SIMD workload).

**Elementwise map support** (add to the same pass): loops annotated `vectorize=true` with no reduction, only elementwise ops (Add/Sub/Mul/Neg) over a `FlatListInt` input and `FlatListInt` output also transform. This covers `[x*2 for x in nums]` patterns.

### SIMD Ops IR Shape

```rust
// in ops.rs Dialect::Simd

// VecLoad: operands=[base_ptr, idx], attrs={width: u8, elem_ty: "i64"|"f64"}
// results=[vec_value]  — the vector register
Simd::VecLoad

// VecAdd: operands=[vec_a, vec_b], results=[vec_out]
Simd::VecAdd

// VecFAdd: operands=[vec_a, vec_b], results=[vec_out]  
Simd::VecFAdd

// VecMul: operands=[vec_a, vec_b], results=[vec_out]
Simd::VecIMul   // i64x2 multiply (lanewise)
Simd::VecFMul   // f64x2 multiply

// VecStore: operands=[base_ptr, idx, vec_val], results=[]
Simd::VecStore

// VecSplat: operands=[scalar], attrs={elem_ty}, results=[vec_out]
Simd::VecSplat  // broadcast scalar to all lanes

// VecHSum: operands=[vec_val], results=[scalar_out]  — horizontal sum
Simd::VecHSum
Simd::VecHFSum
```

These are purely internal IR nodes — they live in the TIR function for the remainder of the pipeline, are invisible to existing passes (all existing passes skip `Dialect::Simd` ops, as they are not in any `is_disqualifying`/`is_scalar_arithmetic` match arm), and are consumed exclusively in the backend lowering path.

### Pass Position

The pipeline becomes (additions in bold):

```
... vectorize (ReadOnly), polyhedral (ReadOnly), **vectorize_lower (Cfg)**, check_exception_elim, copy_prop, dce
```

`vectorize_lower` runs immediately after the annotation pass so its input is fresh, and before `check_exception_elim`/`dce` so those cleanup passes see the transformed CFG.

## 3. Exact Files to Create/Modify

### Create: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/vectorize_lower.rs`

**Responsibilities:**
- Consume `vectorize=true` annotations from ForIter/ScfFor ops
- Check `has_exception_handling == false`
- Verify the loop's values have `LirRepr::I64` or `LirRepr::F64` (query `repr_by_value_for` from `representation_plan.rs` — or equivalently check that the loop's element value has `TirType::I64` with `Repr::RawI64Safe` proven)
- Verify container storage is `FlatListInt` (check the defining op of the iterable for `BuildList`/`FlatListInt` structural proof via `alias_analysis.rs` / existing container-storage path)
- For qualifying loops: replace the scalar loop body blocks with a SIMD-vectorized CFG:
  - A prelude block that computes `vec_len = len & !(width-1)` (multiple-of-width floor)
  - A vector main loop: header + body using `Simd::VecLoad` / `Simd::VecAdd|VecFAdd|VecIMul|VecFMul` / `Simd::VecSplat` accumulator
  - A horizontal reduce at the loop exit: `Simd::VecHSum` / `Simd::VecHFSum`
  - A scalar epilogue loop for the tail (identical to existing scalar body, `len - vec_len` iterations)
- Fall back silently (leave scalar) if any condition is unmet — absence of vectorization is a perf bail, never a miscompile
- Returns `PassStats` with `ops_added` = number of SIMD ops inserted

**Key function signatures:**
```rust
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, tti: &TargetInfo) -> PassStats

fn try_vectorize_loop(
    func: &mut TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    annotation: &LoopAnnotation,   // parsed from op attrs
    tti: &TargetInfo,
) -> bool   // true = transformed

struct LoopAnnotation {
    element_type: ElemTy,   // I64 | F64
    width: u32,             // from tti.vector_width()
    reduction_op: Option<ReductionOp>,
    promoted: bool,
}
```

**Mutation class:** `Mutates::Cfg` (adds and restructures blocks).

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs`

Add to `OpCode` (under the comment marking the `Simd` dialect):
```rust
// Simd dialect ops (inserted by vectorize_lower, consumed by backend lowering)
VecLoad,    // load width scalars from memory into a vector register
VecStore,   // store vector register to width consecutive memory slots
VecSplat,   // broadcast scalar → vector
VecIAdd,    // lane-wise i64 add
VecFAdd,    // lane-wise f64 add
VecIMul,    // lane-wise i64 multiply
VecFMul,    // lane-wise f64 multiply
VecHSum,    // horizontal sum (i64 lanes → i64 scalar)
VecHFSum,   // horizontal sum (f64 lanes → f64 scalar)
```

The `Dialect::Simd` variant already exists at line 17 — no change needed there.

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs`

Add `pub mod vectorize_lower;` (line after `pub mod vectorize;`).

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/pass_manager.rs`

In `build_default_pipeline`, after the `vectorize` pass entry (currently line 345-348), insert:
```rust
pass("vectorize_lower", Cfg, |f, am, tti| {
    passes::vectorize_lower::run(f, am, tti)
}),
```

Update the canonical pass order test in `pass_manager.rs` (line 529 area) and the `mod.rs` pipeline test at line 156 area to include `"vectorize_lower"` after `"vectorize"`.

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/vectorize.rs`

No semantic changes. The `is_disqualifying` function (line 144) and `is_scalar_arithmetic` (line 177) must NOT include any new `VecLoad`/`VecStore`/etc. ops — because these are in `Dialect::Simd` and the vectorize pass only examines `Dialect::Molt` ops. This is automatic since the match arms are exhaustive over `OpCode` — the new variants will need to be handled or unreachable. Add `OpCode::VecLoad | OpCode::VecStore | ... => {}` to `is_disqualifying` (they are iteration infrastructure, not disqualifying).

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/effects.rs`

The effects oracle must classify the new SIMD ops. `VecLoad` is a memory read (same class as `Index`), `VecStore` is a memory write. `VecSplat`, `VecIAdd`, `VecFAdd`, `VecIMul`, `VecFMul`, `VecHSum`, `VecHFSum` are pure arithmetic. Add these to the appropriate arms in `effects.rs`. This is mandatory — the S3 effects oracle drives LICM/GVN/DSE decisions.

### Modify: `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs`

**This is where real SIMD codegen lands.** Add a new code-emission arm that handles `Dialect::Simd` ops:

```rust
// In the main op dispatch loop, add a Dialect::Simd arm:
Dialect::Simd => {
    match op.opcode {
        OpCode::VecLoad => emit_vec_load_cranelift(&mut builder, op, tti, ...),
        OpCode::VecStore => emit_vec_store_cranelift(&mut builder, op, tti, ...),
        OpCode::VecSplat => emit_vec_splat_cranelift(&mut builder, op, tti, ...),
        OpCode::VecIAdd => emit_vec_iadd_cranelift(&mut builder, op, ...),
        OpCode::VecFAdd => emit_vec_fadd_cranelift(&mut builder, op, ...),
        OpCode::VecIMul => emit_vec_imul_cranelift(&mut builder, op, ...),
        OpCode::VecFMul => emit_vec_fmul_cranelift(&mut builder, op, ...),
        OpCode::VecHSum => emit_vec_hsum_cranelift(&mut builder, op, ...),
        OpCode::VecHFSum => emit_vec_hfsum_cranelift(&mut builder, op, ...),
        _ => panic!("unhandled Simd op: {:?}", op.opcode),
    }
}
```

The Cranelift vector type is chosen from `tti.vector_width_i64`:
- 2 → `types::I64X2` / `types::F64X2` (128-bit SSE2/NEON)
- 4 → `types::I64X4` / `types::F64X4` (AVX2, 256-bit)
- 8 → `types::I64X8` / `types::F64X8` (AVX-512F, 512-bit)

The vector load uses `builder.ins().load(vec_type, MemFlags::trusted(), base_ptr, byte_offset)` — Cranelift loads any type from a pointer, including SIMD types.

`VecHSum` for I64X2: `extract_lane(v, 0) + extract_lane(v, 1)`. For I64X4: two 128-bit horizontal adds composed. Cranelift does not have a native `hadd` for i64; compose from `extract_lane(v, i)` calls (4 extracts + 3 adds).

**Delete the existing 4x unroll** at `function_compiler.rs:30815-31000` entirely. The `vectorize_lower` TIR pass replaces it with a structurally correct SIMD loop that handles all element types (not just int) and all widths (not just 4). This is the legacy deletion mandated by CLAUDE.md's "no dual path" rule.

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_wasm.rs`

Add WASM simd128 emission. `wasm-encoder 0.245.1` exposes `Instruction::I64x2Add`, `Instruction::F64x2Add`, `Instruction::V128Load { memarg }`, `Instruction::V128Store { memarg }`, `Instruction::I64x2Splat`, `Instruction::F64x2Splat`, `Instruction::I64x2ExtractLane { lane }`, `Instruction::F64x2ExtractLane { lane }`.

For the WASM backend, `tti.vector_width_*` is always 2 (simd128 is always 128-bit = 2 lanes for i64/f64). Add a `match op.tir_op.opcode` arm in `lower_block`:

```rust
OpCode::VecLoad => emit_v128_load_wasm(&mut instrs, op, lir_values, ...),
OpCode::VecIAdd => instrs.push(Instruction::I64x2Add),
OpCode::VecFAdd => instrs.push(Instruction::F64x2Add),
// etc.
```

WASM local allocation: a `v128` WASM local is needed per `VecSplat`/`VecLoad` result. Add `ValType::V128` as a local type. The local assignment loop in `lower_to_wasm.rs` must allocate `V128` locals for `LirRepr::I64`/`F64` values that result from SIMD ops. Since `LirRepr` doesn't have a `Vec128` variant, add one: `LirRepr::Vec128` — but only for WASM. Alternatively, and simpler: detect `Dialect::Simd` ops in the local allocation phase and allocate a `V128` local for their result values. The WASM local type for a SIMD result is always `V128` regardless of element type (i64x2 and f64x2 both occupy a 128-bit WASM local).

### Modify: `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs`

Replace the dead read at line 2698:
```rust
// BEFORE (dead):
let _ = has_attr(op, "vectorize");

// AFTER:
// The vectorize_lower TIR pass has already replaced eligible scalar loops with
// Simd dialect ops. Any ForIter op that still reaches here is non-vectorizable
// and requires no loop metadata — the LLVM auto-vectorizer handles the rest.
// No dead read, no metadata attachment needed here.
```

Add handling of `Dialect::Simd` ops in the LLVM lowering dispatch. For LLVM the simplest correct approach is to emit explicit LLVM vector IR:
- `VecLoad` → `builder.build_load(<2 x i64>*, ...)`
- `VecIAdd` → `builder.build_int_add` on LLVM vector values
- `VecHSum` → `builder.build_extract_element(v, 0) + build_extract_element(v, 1)` (for width 2)

For the initial cut: emit a call to a new pair of runtime helpers `molt_vec_hsum_i64(ptr, count)` / `molt_vec_hfsum_f64(ptr, count)` that perform the horizontal reduction in Rust (not SIMD-optimal but correct and measurable). The LLVM backend already emits `march=native` so LLVM will auto-vectorize the helper's loop. Real explicit vector IR follows as a Phase 2 LLVM hardening.

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/alias_analysis.rs`

Add `VecLoad` to the `MemRegion::Heap` / `LoadPurity::MayDispatch` classification (it reads from a container's data pointer — same memory region as a `FlatListInt` element read). `VecStore` → `MemRegion::Heap` write. This is required for S5 alias oracle soundness.

### Modify: `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/verify.rs`

The TIR verifier must accept `Dialect::Simd` ops. Add a verify arm that checks:
- `VecLoad`: 2 operands (base_ptr: I64 or Ptr, idx: I64), 1 result (must be typed with an `attrs["elem_ty"]` consistent with the result's type), `attrs["width"]` present
- `VecHSum`/`VecHFSum`: 1 operand, 1 result (I64 or F64 respectively)

### New test file: `/Users/adpena/Projects/molt/tests/differential/basic/simd_sum_correctness.py`

```python
# Tests that vectorized loops produce CPython-identical results
def sum_int(n: int) -> int:
    nums: list[int] = list(range(n))
    total = 0
    for x in nums:
        total += x
    return total

def sum_float(n: int) -> float:
    nums: list[float] = [float(i) for i in range(n)]
    total = 0.0
    for x in nums:
        total += x
    return total

# Tail remainder: length not a multiple of SIMD width
print(sum_int(7))    # 21 — 7 % 4 != 0
print(sum_int(100))  # 4950
print(sum_int(0))    # 0 — empty
print(sum_float(7))
# Bigint safety: values in [0, 2^47) stay RawI64Safe, above stays boxed
print(sum_int(1 << 10))  # must match CPython exactly
```

### New test file: `/Users/adpena/Projects/molt/tests/differential/basic/simd_elementwise.py`

```python
def map_double(n: int) -> list[int]:
    nums: list[int] = list(range(n))
    out: list[int] = [x * 2 for x in nums]
    return out

print(map_double(9))   # tail remainder test
print(map_double(8))   # exact multiple
print(map_double(0))   # empty
```

### New test file: `/Users/adpena/Projects/molt/tests/differential/basic/simd_no_vectorize_boxed.py`

```python
# Boxed ints (past 2^46) must NOT be vectorized — boxed path, bigint-correct
def sum_big(nums: list) -> int:
    total = 0
    for x in nums:
        total += x
    return total

import sys
nums = [1 << 47, 1 << 48, 1 << 49]
print(sum_big(nums))  # must not miscompile; stays on boxed path
```

## 4. Soundness Argument

The design is never-miscompile by construction:

**Gate 1 — Repr check.** `vectorize_lower` only transforms a loop when every scalar element value in the loop body has `LirRepr::I64` (proven `RawI64Safe`) or `LirRepr::F64`. `MaybeBigInt` values have `LirRepr::DynBox`, which fails this check. The `repr_by_value_for` map from `representation_plan.rs` is the same map consumed by WASM's `lower_to_lir.rs` — a single source of truth for which values are safe to treat as raw machine scalars.

**Gate 2 — Container storage check.** Only `FlatListInt` storage (same proof used by the existing 4x unroll) qualifies. A `FlatListInt` list stores raw i64 elements at `data_ptr[i * 8]` — the SIMD vector load of N consecutive 8-byte slots is safe without boxing/tagging. A generic `DynBox` list stores NaN-boxed values; loading them into a SIMD register and performing raw integer addition would be a miscompile. The `FlatListInt` proof gates this.

**Gate 3 — No exception handling.** `func.has_exception_handling == false` is checked before any transformation. An exception edge inside a vectorized loop body would be invisible to the SIMD block structure; the conservative guard keeps the transformation out of exception-bearing functions entirely until phase-c (exception-observation inlining) lands.

**Gate 4 — Tail correctness.** The scalar epilogue loop handles `len % width` remaining elements identically to the original scalar loop. The epilogue is always emitted when `len % width != 0`, which includes the case `len < width` (the vector main loop executes 0 iterations, the epilogue runs `len` scalar iterations). Empty input (`len == 0`) is correct: both loops execute 0 iterations.

**Gate 5 — Silent fallback.** Any check failure (unproven repr, boxed container, exception handling, unusual loop shape) leaves the loop as scalar — NEVER emits incorrect code. "Absence of vectorization" is a perf miss, not a miscompile.

**Gate 6 — Overflow safety.** For `RawI64Safe` i64 lanes, the existing invariant is that values are in `[-2^63, 2^63)` and raw machine add/mul are sound (with a deferred overflow-to-BigInt boundary at the function escape). Summing N values in SIMD lanes preserves this: each lane add has the same overflow semantics as a scalar add. The post-loop `VecHSum` is a sequence of scalar adds with the same semantics.

## 5. Legacy This Arc Deletes

**The 4x scalar unroll in `function_compiler.rs:30815-31000`** (approximately 190 lines in the SimpleIR path). This is the only prior "vectorization" in the system. It:
- Only handles int (`FlatListInt`) sum reductions, not float, not product, not elementwise
- Hard-codes width 4 regardless of `tti.vector_width_i64` (never reads it)
- Emits 4 sequential scalar `iadd` instructions with 4 sequential loads — not SIMD
- Lives on the SimpleIR path, entirely bypassing TIR

The TIR `vectorize_lower` pass replaces it with:
- All element types (i64, f64)
- All reduction ops (sum, product) and elementwise maps
- All target widths from `tti` (2/4/8 lanes)
- Real SIMD instructions (Cranelift `I64X2`, WASM `I64x2Add`, LLVM vector IR)
- TIR-level (correct pipeline position, after all type refinement)

The SimpleIR sum-reduction scan function `scan_loop_int_sum_reduction` and `SumReductionCandidate` struct (lines ~178-342) are deleted along with the unroll body.

## 6. Test Plan

### Rust Unit Tests (in `vectorize_lower.rs`)

**`test_sum_i64_list_transforms`**: build a TirFunction with a ForIter-annotated loop over a `List(I64)` with `FlatListInt` proof, `vectorize=true`, `reduction=sum`, `element_type=i64`. Verify that after `run(func, am, tti)` the function contains `VecLoad`, `VecIAdd`, `VecHSum` ops and a scalar epilogue block.

**`test_elementwise_float_map_transforms`**: loop over `List(F64)` with `vectorize=true`, no reduction, elementwise mul. Verify `VecLoad`, `VecFMul`, `VecStore` emitted.

**`test_no_transform_without_annotation`**: loop without `vectorize=true`. Verify no SIMD ops emitted.

**`test_no_transform_dynbox_list`**: loop over `DynBox` list (unproven repr). Verify no SIMD ops emitted.

**`test_no_transform_exception_handling`**: function with `has_exception_handling=true`. Verify no SIMD ops emitted even when loop is annotated.

**`test_tail_remainder_epilogue_always_emitted`**: verify the scalar epilogue block is always present after transformation (handles `len % width` iterations including `len < width`).

**`test_width_from_tti`**: run with `TargetInfo::native_from_simd_caps(SimdCaps{ avx2: true, .. })` (width 4). Verify `VecLoad` attrs contain `width=4`. Run with baseline (width 2). Verify `width=2`.

### Differential Tests (Python → CPython byte-identical)

**`tests/differential/basic/simd_sum_correctness.py`** (already specified above): tests sum of int list and float list at lengths that are exact multiples and non-multiples of the SIMD width; empty input; large input.

**`tests/differential/basic/simd_elementwise.py`** (above): elementwise doubling, non-multiple length, empty.

**`tests/differential/basic/simd_no_vectorize_boxed.py`** (above): boxed ints past 2^46 must not miscompile.

**Adversarial cases** (add to the suite):
- `simd_nan_float.py`: list with `float('nan')` — NaN + NaN = NaN, must match CPython's NaN propagation semantics for float reductions.
- `simd_overflow_i64.py`: list with values near `i64::MAX/4`. The sum must overflow to BigInt just as scalar would. This validates that the `RawI64Safe` gate correctly refuses lists whose elements might overflow the accumulator when summed. (Since SCEV S6 refuses degree-2 accumulators that could overflow, the vectorize_lower pass must also refuse unless the accumulator's value range fits.)
- `simd_mixed_repr.py`: list annotated `list[int]` where some elements are runtime BigInts (past 2^46). The `FlatListInt` proof should NOT apply; the loop stays scalar.
- `simd_empty_list.py`: `sum([])` = 0 — the loop body never executes; the result must be the initial accumulator.
- `simd_single_element.py`: list of length 1 — exercises the epilogue-only path when `len < width`.
- `simd_prod_list.py`: product reduction (multiplicative identity = 1, not 0).

**Cross-backend**: run the same differential tests through `--backend native`, `--backend wasm`, `--backend llvm`. All must produce identical output to CPython 3.12/3.13/3.14. The LLVM lane falls back to the auto-vectorizer (no explicit vector IR in phase 1) but the scalar epilogue must still be correct.

## 7. Perf-Gate Plan

### Benchmarks

1. **`tests/benchmarks/bench_sum_list.py`** — primary target. `list[int]`, 1M elements, sum. Expected delta: ≥2x over scalar (AVX2 4 lanes = theoretical 4x; real 2-3x after loop overhead). Must be ≥1x CPython on every target.

2. **`tests/benchmarks/bench_prod_list.py`** — product reduction, same size. Same expectations.

3. **`tests/benchmarks/bench_sum.py`** — `range(10M)` sum. This is a `ForIter` over a range, NOT a `FlatListInt` list. The conservative first cut excludes this (range iterators are not `FlatListInt`). It must NOT regress — verifies the silent-fallback contract.

4. **New bench** `tests/benchmarks/bench_sum_float_list.py`:
```python
def main() -> None:
    size = 1_000_000
    nums = [float(i) for i in range(size)]
    total = 0.0
    for x in nums:
        total += x
    print(total)
```
Expected: ≥2x scalar on native/LLVM targets with AVX2.

5. **`tests/benchmarks/bench_matrix_math.py`** — uses `molt_buffer` (not `FlatListInt`), should not regress (fallback to scalar).

### How Measured

```bash
# Per target, per profile, using molt's existing bench harness:
python3 -m molt bench tests/benchmarks/bench_sum_list.py \
  --target native --profile release-fast --compare cpython
```

Gate condition: bench_sum_list and bench_prod_list show ≥ CPython speed on native/release-fast. bench_sum (range) must not regress more than 5% vs pre-arc baseline. The WASM and LLVM gates measure correctness (differential parity) in phase 1; perf gates on those backends follow in phases 2-3 as explicit WASM simd128 and LLVM vector IR land.

## 8. Risk, Rollback, Dependencies

### Hard Dependencies (already landed)

- **S1 AnalysisManager** (`ef284d182`) — `vectorize_lower` uses `am.get::<LoopForest>()` for loop body collection, and optionally `am.get::<AliasAnalysis>()` for the `FlatListInt` proof
- **S2 TargetInfo** (`9ff5d2e00`) — `tti.vector_width()` drives the SIMD width selection
- **S3 effects.rs** (`8b6b88286`) — must register the new `VecLoad`/`VecStore` effects; `VecIAdd`/etc. are pure
- **S5 alias_analysis** (`fb574b289`) — `is_rc_barrier` / `may_observe_slot` must classify SIMD ops correctly
- **S6 SCEV + ValueRange** (`cd66f365e`) — the `FlatListInt` + `RawI64Safe` repr gate already depends on these; `vectorize_lower` queries the same facts
- **Repr promotion / E1 inliner** — orthogonal; `vectorize_lower` reads the existing repr facts

### Risks

1. **Cranelift SIMD on macOS/aarch64**: Cranelift 0.131 supports NEON 128-bit SIMD on aarch64. The `types::I64X2`/`F64X2` types are available and the ISA builder in `native_backend/mod.rs` uses `cranelift_native::builder_with_options` which auto-detects the host ISA (including NEON on Apple Silicon). Verify: build with `cargo test -p molt-backend --features native-backend -- simd` on the dev machine and confirm no "type not supported" panics.

2. **WASM simd128 runtime**: the WASM executor (Node.js / Wasmtime) must have simd128 enabled. wasm-encoder 0.245.1 encodes simd128 instructions correctly. The test harness running WASM tests must invoke Node with `--experimental-wasm-simd` if needed, or use Wasmtime which has simd128 on by default.

3. **`has_exception_handling` gate breadth**: As documented in MEMORY.md, `CheckException` ops flip `has_exception_handling=true` for virtually all real user functions (lower_from_simple.rs:319-330). This means the conservative first-cut only fires on leaf functions with no exception handling. This is by design for the first landing — it delivers a measurable win on the numeric benchmarks (which are exception-free by construction) and unblocks measurement. Exception-bearing loops are phase-c work (same as the inliner's dormancy).

4. **LLVM path**: the LLVM backend has a pre-existing link failure on `molt_app_resolve_intrinsic` (from MEMORY.md). The `Dialect::Simd` arm must be added to LLVM lowering for completeness but the LLVM perf gate is lower priority until the link issue is resolved.

5. **Rollback**: since `vectorize_lower` is a new pass and the `VecLoad`/etc. ops are new variants that existing passes never match (they fall to default `_ => {}` arms in match-exhaustive contexts or explicitly need handling), a compilation error will surface any missing match arm. The feature flag for rollback: gate `vectorize_lower` under `#[cfg(feature = "native-backend")]` initially so it only fires on the native backend.

## 9. Phased Landing Sequence

### Phase 1a — IR Extension (atomic, no behavior change)

**Files changed:**
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/ops.rs`: add 9 new `OpCode` variants under `Dialect::Simd`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/effects.rs`: classify the 9 new opcodes
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/vectorize.rs`: add the new opcodes to the `is_disqualifying` / `is_scalar_arithmetic` arms as non-disqualifying iteration infrastructure
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/alias_analysis.rs`: classify `VecLoad` → `MemRegion::Heap` read, `VecStore` → write

**Acceptance**: `cargo test -p molt-backend` stays green with no new warnings. The new opcodes are unreachable in production (no pass emits them yet).

### Phase 1b — Vectorize-Lower Pass: Cranelift Native

**Files changed:**
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/vectorize_lower.rs`: new file, full implementation for the conservative first-cut (FlatListInt + RawI64Safe + no EH + sum/product/elementwise)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/passes/mod.rs`: add `pub mod vectorize_lower`
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/pass_manager.rs`: insert `vectorize_lower` after `vectorize`, update pipeline order tests
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs`:
  - Add `Dialect::Simd` op dispatch arm with Cranelift vector instruction emission
  - **Delete** the 4x scalar unroll (lines ~30815-31000) and its helper `scan_loop_int_sum_reduction` / `SumReductionCandidate` (lines ~178-342 and tests ~37691-38125)
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/verify.rs`: accept Simd ops

**Acceptance**: all 882+ existing backend lib tests pass. `tests/differential/basic/simd_sum_correctness.py` and `simd_no_vectorize_boxed.py` differential-test green. `bench_sum_list` ≥ CPython on native/release-fast.

### Phase 1c — WASM simd128 Lowering

**Files changed:**
- `/Users/adpena/Projects/molt/runtime/molt-tir/src/tir/lower_to_wasm.rs`: add `Dialect::Simd` op emission using `wasm_encoder` V128 instructions; add `V128` local allocation

**Acceptance**: WASM differential tests for `simd_sum_correctness.py` green under Wasmtime/Node.

### Phase 1d — LLVM Lowering (explicit vector IR or auto-vectorize handoff)

**Files changed:**
- `/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs`: remove dead read, add `Dialect::Simd` arm (runtime helper calls for phase 1; explicit LLVM vector IR in phase 2)

**Acceptance**: LLVM differential tests green. No regression on the LLVM lane (excluding the pre-existing link failure).

Each phase (1a, 1b, 1c, 1d) is a complete structural piece. 1a can land alone (purely additive). 1b must land together with the deletion of the 4x unroll — these are one atomic arc (they replace the same dual path). 1c and 1d are independent follow-ons that complete cross-backend coverage.

Sources:
- [InstBuilder in cranelift::prelude - Rust](https://docs.wasmtime.dev/api/cranelift/prelude/trait.InstBuilder.html)
- [cranelift::prelude::types - Rust](https://docs.wasmtime.dev/api/cranelift/prelude/types/index.html)
