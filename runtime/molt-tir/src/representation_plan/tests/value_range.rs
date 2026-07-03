//! Value-keyed `RawI64Safe` promotion via the value-range analysis (S6).
//!
//! Moved verbatim from `representation_plan/tests.rs` during the move-only
//! split of that god-file; no logic changes.

use std::collections::HashMap;

use super::super::*;
use crate::tir::values::ValueId;

// ======================================================================
// Value-keyed RawI64Safe promotion via the value-range analysis (S6).
//
// These exercise the SOLE proof source for the WASM/LLVM backends:
// `repr_by_value_for(.., Some(&value_range))`. They directly assert the
// soundness invariant (no false RawI64Safe → no heap-BigInt truncation)
// and the perf invariant (range-loop IVs stay RawI64Safe), and that WASM
// and LLVM derive an identical map from the same `ValueRange` (single
// source of truth — a divergence would re-create the native-vs-wasm
// trusted-unbox bug, 2bf51b730).
// ======================================================================

use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue as TirAttrValue, Dialect, OpCode as TirOpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::TirValue;

fn tir_op(opcode: TirOpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}
fn tir_op_nsw(opcode: TirOpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut o = tir_op(opcode, operands, results);
    o.attrs
        .insert("no_signed_wrap".into(), TirAttrValue::Bool(true));
    o
}
fn tir_cint(result: ValueId, value: i64) -> TirOp {
    let mut o = tir_op(TirOpCode::ConstInt, vec![], vec![result]);
    o.attrs.insert("value".into(), TirAttrValue::Int(value));
    o
}

/// Build the canonical post-range_devirt `for i in range(stop): i + 1`
/// loop in TIR: a header block-arg IV with a `no_signed_wrap` increment,
/// the shape SCEV recognises as an `AddRec` and value-range turns into a
/// proven `[start, last]` range.
fn range_loop_tir(start_v: i64, stop: i64) -> (TirFunction, ValueId, ValueId) {
    let mut func = TirFunction::new("rl".into(), vec![], TirType::None);
    let startc = func.fresh_value();
    let stopc = func.fresh_value();
    let stepc = func.fresh_value();
    let iv = func.fresh_value();
    let cond = func.fresh_value();
    let body_val = func.fresh_value();
    let one = func.fresh_value();
    let next = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            tir_cint(startc, start_v),
            tir_cint(stopc, stop),
            tir_cint(stepc, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![startc],
        };
    }
    // Type every integer value as I64 (faithful to real lowered TIR, where
    // `type_refine` types every int) so the representation floor maps them to
    // `MaybeBigInt` rather than the unknown-type `DynBox`.
    for v in [startc, stopc, stepc, iv, body_val, one, next] {
        func.value_types.insert(v, TirType::I64);
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: iv,
                ty: TirType::I64,
            }],
            ops: vec![tir_op(TirOpCode::Lt, vec![iv, stopc], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                tir_cint(one, 1),
                tir_op(TirOpCode::Add, vec![iv, one], vec![body_val]),
                tir_op_nsw(TirOpCode::Add, vec![iv, stepc], vec![next]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(exit, LoopRole::LoopEnd);
    (func, iv, next)
}

/// The overflow_peel'd loop's carrier cycle must admit into the native
/// int-primary set — the slots, their loads, and the checked sums — while
/// the bool flag lane, the exit-merge slot, and the boxed slow loop must
/// all be refused. If the fast-lane names are missing the native arm
/// silently takes the boxed lane (no speedup); if the refused names leak
/// in, the trusted raw carrier meets boxed values (the 2^47 truncation
/// miscompile class). Both directions are load-bearing.
#[test]
fn checked_loop_seed_admits_peeled_fast_loop_only() {
    let func_ir = super::super::test_fixtures::peeled_compute_func_ir();
    let plan = ScalarRepresentationPlan::for_function_ir(&func_ir);
    let primary = plan.primary_name_sets();
    let int_primary = &primary.int;

    for name in [
        "_bb1_arg0",
        "_bb1_arg1",
        "_bb1_arg3",
        "_bb1_arg4", // fast slots
        "_v16",
        "_v17",
        "_v41",
        "_v42", // their loads
        "_v22",
        "_v25", // checked sums
    ] {
        assert!(
            int_primary.contains(name),
            "{name} must be int-primary (fast-lane admission); got {int_primary:?}"
        );
        assert!(
            primary.int_full_deopt.contains(name),
            "{name} must be full-deopt, not inline-safe; got {:?}",
            primary.int_full_deopt
        );
        assert!(
            !primary.int_inline_safe.contains(name),
            "{name} must not seed RawI64Safe; got {:?}",
            primary.int_inline_safe
        );
    }
    for name in [
        "_bb1_arg2",
        "_v40",
        "_v48", // overflow-flag lane (bool)
        "_bb5_arg0",
        "_v51", // exit merge (fed by the boxed slow loop)
        "_bb7_arg0",
        "_bb7_arg1",
        "_v29",
        "_v30",
        "v114",
        "v118", // slow loop
    ] {
        assert!(
            !int_primary.contains(name),
            "{name} must NOT be int-primary (boxed lane); got {int_primary:?}"
        );
    }

    // The overflow-flag chain must admit into the RAW BOOL lane — without
    // it the break condition costs ~4 runtime calls per iteration
    // (inc_ref + is_truthy + not + or-select) and the peel's fast loop
    // loses its win.
    let bool_primary = plan.primary_name_sets().bool_;
    for name in [
        "_v46",
        "_v47",      // checked_add overflow flags
        "_v48",      // or fan-in
        "_v40",      // of-slot load
        "_v44",      // not(of)
        "_v45",      // and(cond, not_of) — the break condition
        "v111",      // the guard compare
        "_bb1_arg2", // the carried of slot
    ] {
        assert!(
            bool_primary.contains(name),
            "{name} must be bool-primary (raw flag lane); got {bool_primary:?}"
        );
    }
}

fn is_inline_safe(map: &HashMap<ValueId, Repr>, id: ValueId) -> bool {
    map.get(&id) == Some(&Repr::RawI64Safe)
}

fn is_full_deopt(map: &HashMap<ValueId, Repr>, id: ValueId) -> bool {
    map.get(&id) == Some(&Repr::RawI64FullDeopt)
}

fn is_raw_carrier(map: &HashMap<ValueId, Repr>, id: ValueId) -> bool {
    map.get(&id).is_some_and(|repr| repr.is_raw_i64_carrier())
}

/// PERF + SOUNDNESS: a bounded `for i in range(10)` induction variable is
/// proven `RawI64Safe` (so the loop keeps the bare-i64 lane and beats
/// CPython), AND that proof flows to its `no_signed_wrap` back-edge update.
#[test]
fn range_loop_iv_is_raw_i64_safe_from_value_range() {
    let (func, iv, next) = range_loop_tir(0, 10);
    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        is_inline_safe(&repr, iv),
        "range(10) IV must be RawI64Safe (range [0,9] ⊂ inline-int47)"
    );
    assert!(
        is_inline_safe(&repr, next),
        "the no_signed_wrap IV update must inherit RawI64Safe (propagated phi)"
    );
}

/// PERF: a bounded `for i in range(12): i // 3` floor-division result is proven
/// `RawI64Safe` through the value-keyed carrier authority. The value-range
/// `FloorDiv` transfer bounds `i // 3` over `i ∈ [0, 11]` to `[0, 3] ⊂
/// inline-int47`, so the result keeps the bare-i64 lane instead of boxing to
/// `MaybeBigInt`. This is the value-keyed half of the floordiv carrier proof
/// (its SimpleIR→name half is `name_scalar_kind`'s `floordiv ⇒ Int`
/// classification); together they let `floordiv_const_loop_iv.py`'s
/// carrier-aware fallback store fire instead of the boxed runtime call. A
/// non-power-of-two divisor is the case strength-reduction does NOT rewrite to a
/// shift, so it exercises the `FloorDiv` transfer specifically.
#[test]
fn range_loop_floordiv_const_is_raw_i64_safe_from_value_range() {
    let (mut func, iv, _next) = range_loop_tir(0, 12);
    let d = func.fresh_value();
    let q = func.fresh_value();
    func.value_types.insert(d, TirType::I64);
    func.value_types.insert(q, TirType::I64);
    // Prepend `d = 3; q = i // 3` to the loop body — the block carrying the
    // `no_signed_wrap` IV update.
    let body_id = *func
        .blocks
        .iter()
        .find(|(_, b)| {
            b.ops
                .iter()
                .any(|o| o.opcode == TirOpCode::Add && o.attrs.contains_key("no_signed_wrap"))
        })
        .map(|(id, _)| id)
        .expect("loop body with the nsw IV update");
    let block = func.blocks.get_mut(&body_id).unwrap();
    let mut new_ops = vec![
        tir_cint(d, 3),
        tir_op(TirOpCode::FloorDiv, vec![iv, d], vec![q]),
    ];
    new_ops.append(&mut block.ops);
    block.ops = new_ops;

    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        is_inline_safe(&repr, q),
        "i // 3 over i ∈ [0,11] ⇒ [0,3] ⊂ inline-int47 must be proven RawI64Safe"
    );
}

/// SOUNDNESS (the 2bf51b760 truncation bug-class): an induction variable
/// whose proven range exceeds 2^46 must NOT be RawI64Safe — it could be a
/// heap BigInt, so it stays `MaybeBigInt` and uses the boxed path. This is
/// the `apply(1<<60, 7) == 1152921504606846983` invariant expressed at the
/// representation boundary: a > 2^46 value is never trusted-unboxed.
#[test]
fn above_inline_int47_iv_is_not_raw_i64_safe() {
    // start at 2^46 so even iteration 0 is at the inline-int47 ceiling and
    // the very next value (2^46) is outside the window.
    let huge_start = 1i64 << 46;
    let (func, iv, _next) = range_loop_tir(huge_start, huge_start + 10);
    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        !is_inline_safe(&repr, iv),
        "an IV reaching/exceeding 2^46 must stay MaybeBigInt (no trusted unbox of a possible heap BigInt)"
    );
    assert_eq!(
        repr.get(&iv),
        Some(&Repr::MaybeBigInt),
        "the unproven int floors to the boxed BigInt-safe carrier"
    );
}

/// SOUNDNESS: with NO value-range supplied (`None`), nothing is promoted —
/// every int floors to `MaybeBigInt`. This is the conservative pre-TIR /
/// unanalysed path that can never miscompile.
#[test]
fn no_value_range_leaves_everything_maybe_bigint() {
    let (func, iv, next) = range_loop_tir(0, 10);
    let repr = repr_by_value_for(&func, None);
    assert_eq!(repr.get(&iv), Some(&Repr::MaybeBigInt));
    assert_eq!(repr.get(&next), Some(&Repr::MaybeBigInt));
    assert!(
        repr.values().all(|r| !r.is_raw_i64_safe()),
        "None means no RawI64Safe raise anywhere"
    );
}

#[test]
fn bool_select_range_proof_does_not_promote_to_raw_i64() {
    let mut func = TirFunction::new(
        "bool_select".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::Bool,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(tir_op(
        TirOpCode::And,
        vec![ValueId(0), ValueId(1)],
        vec![result],
    ));
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    crate::tir::type_refine::refine_types(&mut func);

    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert_eq!(
        repr.get(&result),
        Some(&Repr::Bool),
        "bool values can have [0,1] ranges but must stay in the Bool carrier, not RawI64Safe"
    );
}

/// SOUNDNESS: an unbounded accumulator (`total = total + i`, a degree-2
/// recurrence) is classified `Unknown` by SCEV → no value-range proof →
/// stays `MaybeBigInt`. This is the loop-IV OOM hazard the strict-subset
/// property guards against: a wrapping/unbounded accumulator must never be
/// carried as a raw i64.
#[test]
fn unbounded_accumulator_stays_maybe_bigint() {
    // for i in range(10): total = total + i  — `total` is a 2nd phi whose
    // step is the IV itself (not a constant), so it has no proven range.
    let mut func = TirFunction::new("acc".into(), vec![], TirType::None);
    let startc = func.fresh_value();
    let stopc = func.fresh_value();
    let stepc = func.fresh_value();
    let total0 = func.fresh_value();
    let iv = func.fresh_value();
    let total = func.fresh_value();
    let cond = func.fresh_value();
    let total_next = func.fresh_value();
    let next = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            tir_cint(startc, 0),
            tir_cint(stopc, 10),
            tir_cint(stepc, 1),
            tir_cint(total0, 0),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![startc, total0],
        };
    }
    // Type every integer value as real post-refine TIR would. The
    // value-keyed carrier authority is semantically typed; range proof alone
    // must never mint a raw carrier for an unknown-typed value.
    for v in [startc, stopc, stepc, total0, iv, total, total_next, next] {
        func.value_types.insert(v, TirType::I64);
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![
                TirValue {
                    id: iv,
                    ty: TirType::I64,
                },
                TirValue {
                    id: total,
                    ty: TirType::I64,
                },
            ],
            ops: vec![tir_op(TirOpCode::Lt, vec![iv, stopc], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                tir_op(TirOpCode::Add, vec![total, iv], vec![total_next]),
                tir_op_nsw(TirOpCode::Add, vec![iv, stepc], vec![next]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next, total_next],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(exit, LoopRole::LoopEnd);

    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    // The counted IV is fine; the unbounded accumulator must NOT be raw.
    assert!(
        is_inline_safe(&repr, iv),
        "the counted IV is still proven inline-safe"
    );
    assert!(
        !is_raw_carrier(&repr, total),
        "the unbounded accumulator phi must stay MaybeBigInt (degree-2 recurrence → Unknown range)"
    );
    assert!(
        !is_raw_carrier(&repr, total_next),
        "the accumulator update must stay MaybeBigInt"
    );
}

/// PERF: GPU thread/block-id intrinsics are pre-seeded RawI64Safe even
/// though the value-range analysis has no model for them — their results
/// are hardware lane indices, structurally bounded. Without this seed a GPU
/// kernel's index arithmetic would regress to the boxed runtime path.
#[test]
fn gpu_index_intrinsics_are_pre_seeded_raw_i64_safe() {
    let mut func = TirFunction::new("k".into(), vec![], TirType::None);
    let tid = func.fresh_value();
    func.value_types.insert(tid, TirType::I64);
    let mut call = tir_op(TirOpCode::Call, vec![], vec![tid]);
    call.attrs.insert(
        "s_value".into(),
        TirAttrValue::Str("molt_gpu_thread_id".into()),
    );
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![call];
        entry.terminator = Terminator::Return { values: vec![tid] };
    }
    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        is_inline_safe(&repr, tid),
        "molt_gpu_thread_id result must be pre-seeded RawI64Safe"
    );

    // A non-GPU runtime call result is NOT pre-seeded — only the bounded
    // GPU index intrinsics are.
    let mut func2 = TirFunction::new("k2".into(), vec![], TirType::None);
    let r = func2.fresh_value();
    func2.value_types.insert(r, TirType::I64);
    let mut other = tir_op(TirOpCode::Call, vec![], vec![r]);
    other.attrs.insert(
        "s_value".into(),
        TirAttrValue::Str("molt_some_runtime".into()),
    );
    {
        let entry = func2.blocks.get_mut(&func2.entry_block).unwrap();
        entry.ops = vec![other];
        entry.terminator = Terminator::Return { values: vec![r] };
    }
    let vr2 = value_range_for(&func2);
    let repr2 = repr_by_value_for(&func2, Some(&vr2));
    assert!(
        !is_raw_carrier(&repr2, r),
        "an arbitrary runtime-call result must NOT be pre-seeded raw (only bounded GPU index intrinsics are)"
    );
}

/// Build the live frontend-peeled accumulator shape: a CheckedAdd loop
/// whose header phi is fed by (a) a proven `ConstInt 0` init, (b) the
/// CheckedAdd wrapping sum (full-range raw seed), and (c) a vestigial
/// `LoopEnd` block passing a fabricated `ConstNone` — exactly the edge the
/// SSA lift keeps as loop metadata. `reachable_vestige` controls whether
/// that block is wired into the executable CFG or left detached.
fn checked_loop_with_none_vestige(reachable_vestige: bool) -> (TirFunction, ValueId, ValueId) {
    let mut func = TirFunction::new("cl".into(), vec![], TirType::None);
    let init = func.fresh_value();
    let acc = func.fresh_value();
    let cond = func.fresh_value();
    let step = func.fresh_value();
    let sum = func.fresh_value();
    let of = func.fresh_value();
    let none_v = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let vestige = func.fresh_block();

    for v in [init, acc, step, sum] {
        func.value_types.insert(v, TirType::I64);
    }
    func.value_types.insert(of, TirType::Bool);
    func.value_types.insert(none_v, TirType::None);

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![tir_cint(init, 0)];
        entry.terminator = if reachable_vestige {
            // Wire the vestige into the executable CFG: its None arg can
            // now genuinely flow, so it MUST poison the phi.
            Terminator::CondBranch {
                cond: init,
                then_block: header,
                then_args: vec![init],
                else_block: vestige,
                else_args: vec![],
            }
        } else {
            Terminator::Branch {
                target: header,
                args: vec![init],
            }
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: acc,
                ty: TirType::I64,
            }],
            ops: vec![tir_op(TirOpCode::Lt, vec![acc, init], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                tir_cint(step, -20_000_000),
                tir_op(TirOpCode::CheckedAdd, vec![acc, step], vec![sum, of]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![sum],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    // The vestigial loop-end: materializes a None and re-enters the header
    // with it. In the live lift this block has NO executable predecessor —
    // it survives purely as loop metadata.
    func.blocks.insert(
        vestige,
        TirBlock {
            id: vestige,
            args: vec![],
            ops: vec![tir_op(TirOpCode::ConstNone, vec![], vec![none_v])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![none_v],
            },
        },
    );
    func.loop_roles.insert(vestige, LoopRole::LoopEnd);
    (func, acc, sum)
}

/// PERF (the boxed-lane OOM class): the vestigial UNREACHABLE
/// `loop_end → header` edge passing a fabricated `ConstNone` must NOT
/// poison the all-incomings phi rule — dead edges deliver no values
/// (standard SCCP phi semantics). Without dead-edge insensitivity every
/// frontend-peeled accumulator demotes to the boxed `molt_add` lane on the
/// value-keyed backends: 30M-iteration loops then leak a boxed int per
/// iteration (observed: 2.1GB RSS → OOM kill on `sum_negative` @ llvm).
#[test]
fn unreachable_none_vestige_does_not_poison_checked_loop_phi() {
    let (func, acc, sum) = checked_loop_with_none_vestige(false);
    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        is_full_deopt(&repr, sum),
        "the CheckedAdd wrapping sum is the unconditional full-range seed"
    );
    assert!(
        is_full_deopt(&repr, acc),
        "the header phi must be raised: its only REACHABLE incomings are the \
             proven ConstInt init and the CheckedAdd sum; the unreachable \
             ConstNone vestige delivers no value"
    );
}

/// SOUNDNESS (the dual of the above): the SAME None-passing edge, made
/// executable, MUST poison the phi — a `None` can genuinely flow, and a
/// raw-i64 carrier fed a NaN-boxed None is the trusted-unbox miscompile
/// class. Reachability is the load-bearing distinction.
#[test]
fn reachable_none_edge_still_poisons_checked_loop_phi() {
    let (func, acc, _sum) = checked_loop_with_none_vestige(true);
    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        !is_raw_carrier(&repr, acc),
        "a REACHABLE None incoming must keep the phi boxed (MaybeBigInt floor)"
    );
}

/// SOUNDNESS (native/WASM variable-keyed phi invariant): a loop-header phi
/// cannot be carried as raw i64 unless every reachable incoming uses the raw
/// carrier. A single reachable heap/DynBox incoming must force the phi to the
/// boxed lane, even when the ordinary entry and back-edge values are raw.
#[test]
fn reachable_heap_incoming_poisons_raw_loop_phi() {
    let mut func = TirFunction::new("mixed_phi".into(), vec![], TirType::None);
    let init = func.fresh_value();
    let acc = func.fresh_value();
    let cond = func.fresh_value();
    let step = func.fresh_value();
    let sum = func.fresh_value();
    let overflow = func.fresh_value();
    let heap_value = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let heap_pred = func.fresh_block();
    let exit = func.fresh_block();

    for v in [init, acc, step, sum] {
        func.value_types.insert(v, TirType::I64);
    }
    func.value_types.insert(cond, TirType::Bool);
    func.value_types.insert(overflow, TirType::Bool);
    func.value_types.insert(heap_value, TirType::DynBox);

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![tir_cint(init, 0)];
        entry.terminator = Terminator::CondBranch {
            cond: init,
            then_block: header,
            then_args: vec![init],
            else_block: heap_pred,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: acc,
                ty: TirType::I64,
            }],
            ops: vec![tir_op(TirOpCode::Lt, vec![acc, init], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                tir_cint(step, 1),
                tir_op(TirOpCode::CheckedAdd, vec![acc, step], vec![sum, overflow]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![sum],
            },
        },
    );
    func.blocks.insert(
        heap_pred,
        TirBlock {
            id: heap_pred,
            args: vec![],
            ops: vec![tir_op(TirOpCode::Call, vec![], vec![heap_value])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![heap_value],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let vr = value_range_for(&func);
    let repr = repr_by_value_for(&func, Some(&vr));
    assert!(
        is_full_deopt(&repr, sum),
        "CheckedAdd's wrapping sum remains a valid raw carrier"
    );
    assert!(
        !is_raw_carrier(&repr, heap_value),
        "the heap incoming itself must not be raw"
    );
    assert_eq!(
        repr.get(&acc),
        Some(&Repr::MaybeBigInt),
        "a reachable heap incoming must keep the loop phi boxed; otherwise \
             native/WASM variable-keyed phis can receive raw and heap carriers"
    );
}

/// CROSS-BACKEND SINGLE SOURCE OF TRUTH: the WASM path (`repr_by_value_for`)
/// and the LLVM path (`LlvmReprFacts::build` → same `repr_by_value_for` with
/// the same `ValueRange`) derive the IDENTICAL `Repr` per `ValueId`. A
/// divergence here is the native-vs-wasm trusted-unbox bug; this test is the
/// firewall against it.
#[test]
#[cfg(feature = "llvm")]
fn wasm_and_llvm_derive_identical_repr_from_one_value_range() {
    let (func, _iv, _next) = range_loop_tir(0, 10);
    let vr = value_range_for(&func);
    let wasm_map = repr_by_value_for(&func, Some(&vr));
    let llvm_facts = LlvmReprFacts::build(&func);
    assert_eq!(
        wasm_map, llvm_facts.repr_by_value,
        "WASM and LLVM must derive the same Repr per ValueId from the same ValueRange"
    );
}
