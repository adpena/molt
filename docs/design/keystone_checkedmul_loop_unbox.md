# Keystone: Unbox Loop-Carried Scalar Arithmetic (CheckedMul peel + float-primary + IV)

Design output of the `keystone-design-loop-scalar-unbox` orchestration (2026-06-25,
4 investigators + synthesis). Implementation-ready for STEP 0-1; STEP 3 is the gate.

## Problem (CLIF-confirmed)

molt boxes loop-carried scalar arithmetic. Both a pure-int loop (`s += i*j + (i+1)`)
and a pure-float loop (`x += 1.5; s += x*0.5`) run ~3.1-3.6x SLOWER than CPython.
The CLIF shows every loop-variant scalar (IVs `i`/`j`, accumulators `s`/`x`) carried
as a NaN-tagged SMI (`0x7ff9_...`), unboxed each iteration via a 4-op serial
`bxor -> ishl -> sshr` chain: ~20 ops/iter vs 2 real arithmetic ops. The
`overflow_peel` TIR pass already heals unbounded `+=` int accumulators via a
`CheckedAdd` (hardware-overflow-flag) fast loop + boxed slow-clone, but it REFUSES
`Mul` bodies, and the float lane never admits loop-carried float accumulators.

## Fix (three gaps)

- **GAP 1 (int-mul) — CheckedMul peel.** Mirror `CheckedAdd` into a `CheckedMul` op
  and admit `Mul` to the overflow_peel pure-body set. Overflow-SAFE: the fast loop
  uses a hardware-overflow-flagged multiply; on overflow the boxed slow-clone
  re-executes the iteration BigInt-exact (`mul` is pure, so re-exec is sound, exactly
  like `add`). **READY.**
- **GAP 2 (float) — float-primary loop accumulators.** Admit loop-carried float
  accumulators to `float_primary` (raw f64). No overflow hazard, so SOUND with no
  deopt. Designed; **measure-gated** (see risk 2).
- **GAP 3 (dynamic-IV) — NOT AIRTIGHT. This is the only blocker to full readiness.**

## File map

- **STEP 0 (op + tables):** `runtime/molt-ir/src/tir/ops.rs` (CheckedMul mirror of
  CheckedAdd L25-50), `runtime/molt-ir/src/tir/printer.rs` (L235 mirror), and the
  GENERATED op-kinds tables — `CheckedMul` MUST be `purity="impure"` +
  `GvnNumberingRole=Never` (TOML-driven; regenerate). See risk 6.
- **STEP 1 (peel + all-backend lowering):**
  - `runtime/molt-passes/src/tir/passes/overflow_peel.rs` (pure-set L351, phi-qual L461,
    swap L896-904).
  - `runtime/molt-tir/src/representation_plan.rs` (`checked_loop_seed_names`
    L3911/L3921; `raw_i64_safe_value_seed` full-range gate L1231).
  - native `.../fc/arith.rs` (HANDLED_KINDS L12 + `checked_mul` handler after L576).
  - `.../simple_backend.rs` (factor `imul_overflow64` from `imul_checked_inline`
    L1149 — **64-bit-exact flag, NOT 47-bit**; `smul_overflow` does not exist in
    Cranelift 0.131, use the `smulhi` pattern — see risk 5).
  - `.../llvm_backend/lowering.rs` (`emit_checked_mul` mirror L4638 + dispatch L1310).
  - `.../tir/lower_to_wasm.rs` (CheckedMul arm after L659 — **BOXED-LANE-ONLY v1**).
  - `.../luau.rs` (`molt_checked_i64_mul` prelude L579 + arm after L4863 —
    **conservative flag=true on precision loss**, see risk 4).
- **STEP 2 (float):** `representation_plan.rs` `vars_with_non_float_defs` L2873-2895.
- **STEP 3 (IV):** `value_range.rs` (`iv_range_from_recurrence` L1573 symbolic-bound +
  `narrow_from_header_guards` L1612 global_range IV promotion), `scev.rs`
  (symbolic trip-count L864-873). NOT the spec's representation_plan re-seed.
- **STEP 4 (verify):** `tests/differential/overflow_scalar/` (7 probes — the
  silent-wrong-answer guards), `bench/measure_overflow_fix.py`.

## Correctness risks (ALL must be honored)

1. **GAP 3 design is DEAD CODE.** `fits_inline_int47(phi.id)` reads only `global_range`;
   for a dynamic bound `narrow_from_header_guards` writes no numeric range, and
   `narrow_loop_header_phis` refuses phi-dependent-increment IVs under the full-phi
   sweep. Real fix = a recurrence/value_range global-range install gated on a
   proven-inline symbolic bound — and it touches the BCE memory-safety query that
   shares `fits_inline_int47`: **a wrong widening = silent OOB (memory-safety P0)**.
   Implement only after a unit test reproduces the dead-code finding.
2. **Float<->IV entanglement.** `probe_float` runs inside `for _ in range(N)`; STEP 2
   alone may leave it RED if the boxed dynamic IV forces a boxed loop shape around the
   float accumulator. Sequence: STEP 2 then MEASURE; if RED, the residual is STEP 3.
   Do NOT claim the float gap healed on a green unit test — require the warm
   CPython-ratio to flip.
3. **WASM CheckedMul is boxed-only v1** (no speedup until a RawI64Full lattice +
   64x64->128 overflow helper). A DOCUMENTED target limitation per the Performance
   Constitution — name it in the landing report's backend scoreboard, don't bury it.
4. **Luau f64-precision.** A structural `return a*b, false` is a SILENT-WRONG-ANSWER
   (f64 loses bits in the i64 range). Use flag=true-on-unproven-exactness (forces the
   sound boxed slow loop); verify with a Luau differential probe at products near 2^53.
5. **Cranelift 0.131 has no `smul_overflow`** (verified) — use the `smulhi` pattern.
   The 64-bit-exact-vs-47-bit-fits flag predicate is the subtle point: reusing
   `imul_checked_inline` verbatim (which ANDs in `fits_47`) deopts the accumulator
   2^16x too early (perf bug) — extract the 64-bit-only half.
6. **GVN/LICM multi-result safety.** CheckedMul (like CheckedAdd) must be
   `GvnNumberingRole=Never` + `purity="impure"` so a 2-result op is never
   value-numbered/hoisted; STEP 0's table regeneration must preserve this.

## Readiness

- STEP 0-1 (int-mul CheckedMul): **READY** — implement + differential-verify
  overflow->bigint on every backend (2^63 boundary, Luau 2^53) before commit.
- STEP 2 (float): ready but MEASURE-GATED (risk 2).
- STEP 3 (dynamic-IV): **BLOCKED** — `ready_to_implement = false` was driven solely by
  this gap. Needs the recurrence/global-range analysis + a reproducing unit test +
  the BCE memory-safety check first.

Full design transcript: workflow `wf_adb0f65d` output. Context: `spectral-norm-perf-red.md`.
