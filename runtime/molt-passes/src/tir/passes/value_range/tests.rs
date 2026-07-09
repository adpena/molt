use super::*;
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::IntRange;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::scev::compute_scev;
use crate::tir::values::ValueId;

#[test]
fn proves_in_bounds_const_index() {
    // Direct query on a hand-built result: container of length 3, index 2.
    let bid = BlockId(0);
    let lst = ValueId(100);
    let mut res = ValueRangeResult::default();
    res.record_container_length_constant(lst, 3);

    let idx = ValueId(101);
    res.record_global_range(idx, IntRange::point(2));
    assert!(res.proves_index_in_bounds(bid, lst, idx));

    // index 3 into len-3 container → unsafe (3 is out of bounds).
    let idx3 = ValueId(102);
    res.record_global_range(idx3, IntRange::point(3));
    assert!(!res.proves_index_in_bounds(bid, lst, idx3));

    // negative index → unsafe.
    let idxn = ValueId(103);
    res.record_global_range(idxn, IntRange::point(-1));
    assert!(!res.proves_index_in_bounds(bid, lst, idxn));

    // unknown range → unsafe.
    let idxu = ValueId(104);
    assert!(!res.proves_index_in_bounds(bid, lst, idxu));

    // unbounded-above range → unsafe even though lo >= 0.
    let idxh = ValueId(105);
    res.record_global_range(idxh, IntRange::new(0, i64::MAX));
    assert!(!res.proves_index_in_bounds(bid, lst, idxh));
}

#[test]
fn symbolic_lt_len_proof() {
    // `while i < len(lst): lst[i]` — i guarded `< len_val`, len_val=len(lst).
    let bid = BlockId(1);
    let lst = ValueId(10);
    let i = ValueId(11);
    let len_val = ValueId(12);
    let mut res = ValueRangeResult::default();
    res.record_len_of(len_val, lst);
    // i is provably >= 0 (an IV from 0).
    res.record_global_range(i, IntRange::new(0, i64::MAX));
    res.record_symbolic_lt(bid, i, len_val);
    assert!(res.proves_index_lt_len_symbolically(bid, lst, i));
    // wrong container → not proven.
    let other = ValueId(99);
    assert!(!res.proves_index_lt_len_symbolically(bid, other, i));
}

#[test]
fn unknown_everything_proves_nothing() {
    let res = ValueRangeResult::default();
    assert!(!res.proves_index_in_bounds(BlockId(0), ValueId(0), ValueId(1)));
    assert!(!res.fits_inline_int47(ValueId(0)));
    let _ = TirBlock {
        id: BlockId(0),
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
}

// -- end-to-end through compute_value_range + compute_scev ---------------
use crate::tir::blocks::TirBlock as Blk;
use crate::tir::ops::{AttrDict, Dialect, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::TirValue;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}
fn op_nsw(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut o = op(opcode, operands, results);
    o.attrs
        .insert("no_signed_wrap".into(), AttrValue::Bool(true));
    o
}
fn cint(result: ValueId, value: i64) -> TirOp {
    let mut o = op(OpCode::ConstInt, vec![], vec![result]);
    o.attrs.insert("value".into(), AttrValue::Int(value));
    o
}

fn mark_i64_values(func: &mut TirFunction, values: impl IntoIterator<Item = ValueId>) {
    for value in values {
        func.value_types.insert(value, TirType::I64);
    }
}

fn mark_bool_values(func: &mut TirFunction, values: impl IntoIterator<Item = ValueId>) {
    for value in values {
        func.value_types.insert(value, TirType::Bool);
    }
}

/// `for i in range(stop): a[i]` where `a = [0]*list_len` — built in the
/// canonical post-range_devirt shape and run through the real
/// compute_scev + compute_value_range pipeline.
fn range_loop_vr(list_len: i64, stop: i64) -> (TirFunction, BlockId, ValueId, ValueId) {
    let mut func = TirFunction::new("rl".into(), vec![], TirType::None);
    let one = func.fresh_value();
    let elem = func.fresh_value();
    let list1 = func.fresh_value();
    let lenv = func.fresh_value();
    let a = func.fresh_value();
    let start = func.fresh_value();
    let stop_v = func.fresh_value();
    let step = func.fresh_value();
    let iv = func.fresh_value();
    let cond = func.fresh_value();
    let next = func.fresh_value();
    let r = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            cint(one, 1),
            cint(elem, 0),
            op(OpCode::BuildList, vec![elem], vec![list1]),
            cint(lenv, list_len),
            op(OpCode::Mul, vec![list1, lenv], vec![a]),
            cint(start, 0),
            cint(stop_v, stop),
            cint(step, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![start],
        };
    }
    func.blocks.insert(
        header,
        Blk {
            id: header,
            args: vec![TirValue {
                id: iv,
                ty: TirType::I64,
            }],
            ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
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
        Blk {
            id: body,
            args: vec![],
            ops: vec![
                op(OpCode::Index, vec![a, iv], vec![r]),
                op_nsw(OpCode::Add, vec![iv, step], vec![next]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next],
            },
        },
    );
    func.blocks.insert(
        exit,
        Blk {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(exit, LoopRole::LoopEnd);
    (func, body, a, iv)
}

#[test]
fn e2e_range_loop_iv_in_bounds() {
    // a has length 10, for i in range(10): i in [0,9] < 10 → in bounds.
    let (func, body, a, iv) = range_loop_vr(10, 10);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    // The IV range over the body is [0, 9].
    assert_eq!(vr.range_at(body, iv), IntRange::new(0, 9));
    assert!(vr.proves_index_in_bounds(body, a, iv));
}

#[test]
fn e2e_range_loop_container_too_small_not_proven() {
    // a has length 3, for i in range(10): i can reach 9 > 2 → NOT in bounds.
    let (func, body, a, iv) = range_loop_vr(3, 10);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert!(
        !vr.proves_index_in_bounds(body, a, iv),
        "container shorter than the IV's max must NOT be provable in-bounds"
    );
}

#[test]
fn e2e_counted_loop_const_bound_proven_without_nsw() {
    // The frontend's counted-loop shape lowers `for i in range(10)` to an
    // arithmetic loop whose `Add(i, 1)` is NOT nsw-tagged (SCEV refuses the
    // AddRec). The counted-loop recognizer proves start=0/step=1/trip=10 from
    // the CONSTANT guard bound, so the IV range [0,9] is recovered soundly —
    // the producer that unblocks SROA/BCE on the dominant counted-loop shape.
    let (mut func, body, a, iv) = range_loop_vr(10, 10);
    for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
        if op.opcode == OpCode::Add {
            op.attrs.remove("no_signed_wrap");
        }
    }
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    // SCEV gives no AddRec, but the counted-loop recognizer still proves it.
    assert_eq!(
        vr.range_of(iv),
        IntRange::new(0, 9),
        "counted-loop recognizer must recover the IV range from the const bound"
    );
    assert!(
        vr.proves_index_in_bounds(body, a, iv),
        "a constant-bounded counted loop is provably in-bounds even without nsw"
    );
}

#[test]
fn counted_loop_fallback_uses_loopforest_without_loop_roles() {
    let (mut func, body, a, iv) = range_loop_vr(10, 10);
    func.loop_roles.clear();
    for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
        if op.opcode == OpCode::Add {
            op.attrs.remove("no_signed_wrap");
        }
    }

    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);

    assert!(
        !scev.is_induction_var(iv),
        "without nsw, SCEV must not be the source of this range"
    );
    assert_eq!(vr.range_of(iv), IntRange::new(0, 9));
    assert!(vr.proves_index_in_bounds(body, a, iv));
}

#[test]
fn e2e_nonconst_bound_no_nsw_not_proven() {
    // The genuinely-unprovable case: a NON-CONSTANT stop bound AND no nsw.
    // The counted-loop recognizer needs a ConstInt stop (it gets none here),
    // and SCEV needs nsw (stripped) — so NEITHER prover fires and the IV has
    // no range. BCE must NOT fire (fail-closed).
    let (mut func, body, a, iv) = range_loop_vr(10, 10);
    // Make the stop bound a non-constant: replace the `Lt(iv, stop_v)` RHS
    // with a fresh value that has no ConstInt def (an opaque parameter-like
    // value). Find the Lt op (in the header) and the stop ConstInt, and drop
    // the constant by re-pointing Lt's RHS at a never-defined value.
    let opaque = func.fresh_value();
    for block in func.blocks.values_mut() {
        for op in block.ops.iter_mut() {
            if op.opcode == OpCode::Lt && op.operands.len() == 2 {
                op.operands[1] = opaque; // RHS now has no constant/def → opaque.
            }
        }
    }
    // Strip nsw so SCEV cannot form the AddRec either.
    for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
        if op.opcode == OpCode::Add {
            op.attrs.remove("no_signed_wrap");
        }
    }
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert!(
        vr.range_of(iv).is_full(),
        "IV with neither a const bound nor nsw must have NO proven range"
    );
    assert!(
        !vr.proves_index_in_bounds(body, a, iv),
        "a possibly-wrapping IV must not yield a bounds proof"
    );
}

/// Build `for i in range(stop): ...` with extra derived ops in the body, the
/// `p.y = i + 1`-style values the forward sweep must range. Returns the func
/// plus the derived-value ids.
fn range_loop_with_derived(
    stop: i64,
) -> (TirFunction, ValueId, ValueId, ValueId, ValueId, ValueId) {
    let (mut func, body, _a, iv) = range_loop_vr(64, stop);
    let one = func.fresh_value();
    let mask = func.fresh_value();
    let m4 = func.fresh_value();
    let sh = func.fresh_value();
    let i_plus_1 = func.fresh_value();
    let i_and_15 = func.fresh_value();
    let i_mod_4 = func.fresh_value();
    let i_shl_30 = func.fresh_value();
    let block = func.blocks.get_mut(&body).unwrap();
    // Prepend the constants for the derived ops, then the derived ops.
    let mut new_ops = vec![
        cint(one, 1),
        cint(mask, 15),
        cint(m4, 4),
        cint(sh, 30),
        op(OpCode::Add, vec![iv, one], vec![i_plus_1]),
        op(OpCode::BitAnd, vec![iv, mask], vec![i_and_15]),
        op(OpCode::Mod, vec![iv, m4], vec![i_mod_4]),
        op(OpCode::Shl, vec![iv, sh], vec![i_shl_30]),
    ];
    new_ops.append(&mut block.ops);
    block.ops = new_ops;
    (func, iv, i_plus_1, i_and_15, i_mod_4, i_shl_30)
}

#[test]
fn e2e_derived_values_get_proven_ranges() {
    // for i in range(10): i in [0,9]. The forward sweep must prove the
    // derived store values that today block SROA's hot-loop promotion.
    let (func, iv, i_plus_1, i_and_15, i_mod_4, i_shl_30) = range_loop_with_derived(10);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert_eq!(vr.range_of(iv), IntRange::new(0, 9), "IV body range");
    // i + 1 ∈ [1, 10] — the `p.y = i + 1` shape.
    assert_eq!(vr.range_of(i_plus_1), IntRange::new(1, 10));
    assert!(vr.fits_inline_int47(i_plus_1));
    // i & 15 ∈ [0, 15] (mask bound — holds even if i were unknown).
    assert_eq!(vr.range_of(i_and_15), IntRange::new(0, 15));
    assert!(vr.fits_inline_int47(i_and_15));
    // i % 4 ∈ [0, 3].
    assert_eq!(vr.range_of(i_mod_4), IntRange::new(0, 3));
    assert!(vr.fits_inline_int47(i_mod_4));
    // i << 30 ∈ [0, 9 << 30] = [0, 9663676416] — still well within 2^46.
    assert_eq!(vr.range_of(i_shl_30), IntRange::new(0, 9i64 << 30));
    assert!(vr.fits_inline_int47(i_shl_30));
}

#[test]
fn e2e_shl_past_inline_window_not_proven_inline() {
    // for i in range(10): i << 45 reaches 9 << 45 ≈ 3.2e14 > 2^46-1 ⇒ the
    // range is proven but does NOT fit the inline window (must stay boxed).
    let (mut func, _iv, _p1, _a15, _m4, _shl30) = range_loop_with_derived(10);
    // Find the Shl op and bump its shift constant to 45 (overflow window).
    let mut sh_id = None;
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Shl {
                sh_id = Some(op.operands[1]);
            }
        }
    }
    let sh_id = sh_id.unwrap();
    for block in func.blocks.values_mut() {
        for op in block.ops.iter_mut() {
            if op.opcode == OpCode::ConstInt && op.results.first() == Some(&sh_id) {
                op.attrs.insert("value".into(), AttrValue::Int(45));
            }
        }
    }
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    // The Shl result is the value whose def op is Shl.
    let mut shl_res = None;
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Shl {
                shl_res = Some(op.results[0]);
            }
        }
    }
    let shl_res = shl_res.unwrap();
    // Range is proven ([0, 9<<45]) but does NOT fit the inline window.
    assert_eq!(vr.range_of(shl_res), IntRange::new(0, 9i64 << 45));
    assert!(
        !vr.fits_inline_int47(shl_res),
        "9<<45 exceeds 2^46-1 — must NOT be proven inline"
    );
}

/// `for i in range(stop): q = i // divisor`, with `q` derived in the loop
/// body. Returns the func, the IV, and the floordiv result value.
fn range_loop_with_floordiv(stop: i64, divisor: i64) -> (TirFunction, ValueId, ValueId) {
    let (mut func, body, _a, iv) = range_loop_vr(64, stop);
    let d = func.fresh_value();
    let q = func.fresh_value();
    let block = func.blocks.get_mut(&body).unwrap();
    let mut new_ops = vec![cint(d, divisor), op(OpCode::FloorDiv, vec![iv, d], vec![q])];
    new_ops.append(&mut block.ops);
    block.ops = new_ops;
    (func, iv, q)
}

#[test]
fn e2e_floordiv_const_proven_inline() {
    // (a) for i in range(1000): i // 3 ∈ [0, 333] ⊂ inline-int47, so the
    // result keeps the raw-i64 lane instead of boxing to MaybeBigInt — the
    // perf unlock for the numeric-loop class.
    let (func, iv, q) = range_loop_with_floordiv(1000, 3);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert_eq!(vr.range_of(iv), IntRange::new(0, 999), "IV body range");
    assert_eq!(vr.range_of(q), IntRange::new(0, 333), "999 // 3 == 333");
    assert!(
        vr.fits_inline_int47(q),
        "i // 3 over [0, 999] must be proven inline (raw lane fires)"
    );
}

#[test]
fn e2e_floordiv_negative_dividend_floor_exact() {
    // (b) for i in range(10): q = (-i) // 3. The IV is [0, 9] ⇒ -i ∈ [-9, 0],
    // and Python floor division rounds toward -inf, so (-i) // 3 ∈ [-3, 0]
    // (a truncating divide would mis-bound the low end). Exact negative-
    // dividend rounding through the real transfer keeps the bound sound.
    let (mut func, body, _a, iv) = range_loop_vr(64, 10);
    let neg = func.fresh_value();
    let d = func.fresh_value();
    let q = func.fresh_value();
    let block = func.blocks.get_mut(&body).unwrap();
    let mut new_ops = vec![
        cint(d, 3),
        op(OpCode::Neg, vec![iv], vec![neg]),
        op(OpCode::FloorDiv, vec![neg, d], vec![q]),
    ];
    new_ops.append(&mut block.ops);
    block.ops = new_ops;
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert_eq!(vr.range_of(neg), IntRange::new(-9, 0), "-i over i ∈ [0, 9]");
    assert_eq!(
        vr.range_of(q),
        IntRange::new(-3, 0),
        "(-i) // 3 floors toward -inf to [-3, 0]"
    );
    assert!(vr.fits_inline_int47(q));
}

#[test]
fn e2e_floordiv_divisor_spanning_zero_not_proven() {
    // (c) for i in range(10): q = i // k, where k is an opaque (unranged)
    // value. Its range is FULL (spans 0), so the divisor is not provably
    // non-zero/sign-uniform ⇒ NO range proof for q. Fail-closed: a possible
    // ZeroDivisionError or sign flip must never yield a tight bound (a false
    // bound here is the inline-int47 truncation P0).
    let (mut func, body, _a, iv) = range_loop_vr(64, 10);
    let k = func.fresh_value(); // opaque: never given a ConstInt def.
    let q = func.fresh_value();
    let block = func.blocks.get_mut(&body).unwrap();
    let mut new_ops = vec![op(OpCode::FloorDiv, vec![iv, k], vec![q])];
    new_ops.append(&mut block.ops);
    block.ops = new_ops;
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert!(
        vr.range_of(q).is_full(),
        "opaque (zero-spanning) divisor ⇒ no proof"
    );
    assert!(
        !vr.fits_inline_int47(q),
        "unproven floordiv result must stay boxed"
    );
}

/// The shift-overflow contract's count-validity gate (task #34): a `Shl`/
/// `Shr` result whose machine shift COUNT is proven outside `[0, 63]` must
/// NOT be a raw-i64-safe carrier, *even when its result range fits the
/// inline window*. `0 << 70` ranges to `[0, 0]` (fits inline) yet a raw
/// machine `shl` by 70 is LLVM poison / a wasm wrong-value mask-mod-64, so
/// the seed (`raw_i64_safe_values_for` — the single source of truth the
/// LLVM/WASM shift lanes consult) must exclude it, routing the shift to the
/// BigInt-/exception-correct boxed runtime. The proven-`[0, 63]` count case
/// (`5 << 3`) stays raw.
#[test]
fn shl_count_outside_0_63_is_not_raw_i64_safe() {
    use crate::representation_facts::raw_i64_safe_values_for;
    let mut func = TirFunction::new("shl_count_gate".into(), vec![], TirType::None);
    let zero = func.fresh_value();
    let big_count = func.fresh_value();
    let bad_res = func.fresh_value(); // 0 << 70  (result fits inline, count > 63)
    let five = func.fresh_value();
    let small_count = func.fresh_value();
    let good_res = func.fresh_value(); // 5 << 3   (result fits inline, count in [0,63])
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            cint(zero, 0),
            cint(big_count, 70),
            op(OpCode::Shl, vec![zero, big_count], vec![bad_res]),
            cint(five, 5),
            cint(small_count, 3),
            op(OpCode::Shl, vec![five, small_count], vec![good_res]),
        ];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    mark_i64_values(
        &mut func,
        [zero, big_count, bad_res, five, small_count, good_res],
    );
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    // Both results are range-proven inside the inline window...
    assert_eq!(vr.range_of(bad_res), IntRange::point(0), "0<<70 == 0");
    assert!(vr.fits_inline_int47(bad_res));
    assert_eq!(vr.range_of(good_res), IntRange::point(40), "5<<3 == 40");
    assert!(vr.fits_inline_int47(good_res));
    // ...but only the in-range-count shift may carry a raw i64.
    let raw = raw_i64_safe_values_for(&func, &vr);
    assert!(
        !raw.contains(&bad_res),
        "0<<70: machine count 70 is out of [0,63] — must NOT be raw-i64-safe"
    );
    assert!(
        raw.contains(&good_res),
        "5<<3: count 3 is in [0,63] and result fits inline — stays raw"
    );
}

#[test]
fn e2e_unbounded_accumulator_stays_unranged() {
    // The mandatory bigint_accumulator soundness gate: an accumulator phi
    // `total = total + i` whose SCEV is NOT a proven AddRec must keep its
    // FULL (absent) range — the forward sweep must NEVER prove it inline.
    let mut func = TirFunction::new("acc".into(), vec![], TirType::None);
    let start_i = func.fresh_value();
    let start_t = func.fresh_value();
    let stop_v = func.fresh_value();
    let step = func.fresh_value();
    let iv = func.fresh_value();
    let total = func.fresh_value();
    let cond = func.fresh_value();
    let next_i = func.fresh_value();
    let next_t = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            cint(start_i, 0),
            cint(start_t, 0),
            cint(stop_v, 1000000),
            cint(step, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![start_i, start_t],
        };
    }
    func.blocks.insert(
        header,
        Blk {
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
            ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
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
        Blk {
            id: body,
            args: vec![],
            // total = total + i  (an accumulator — NOT an affine recurrence).
            ops: vec![
                op(OpCode::Add, vec![total, iv], vec![next_t]),
                op_nsw(OpCode::Add, vec![iv, step], vec![next_i]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next_i, next_t],
            },
        },
    );
    func.blocks.insert(
        exit,
        Blk {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(exit, LoopRole::LoopEnd);

    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    // The IV is a proper AddRec and IS ranged.
    assert!(vr.fits_inline_int47(iv) || vr.range_of(iv).hi <= 999_999);
    // The accumulator phi and its update MUST stay un-proven (FULL).
    assert!(
        !vr.fits_inline_int47(total),
        "unbounded accumulator phi must never be proven inline"
    );
    assert!(
        !vr.fits_inline_int47(next_t),
        "accumulator update (total + i) must never be proven inline — it can \
             exceed the inline window and even i64, requiring a boxed BigInt"
    );
    assert!(
        vr.range_of(next_t).is_full(),
        "total + i range must be FULL"
    );
}

/// Build a single-latch loop whose header carries ONE phi `s` (start `s0`),
/// with a body that computes `s_next = (s << shift) & mask` (when `mask` is
/// `Some`) or `s_next = s << shift` (when `mask` is `None`), branching back
/// with `s_next`. The loop is gated by a constant counter `for _ in
/// range(trip)` so it is a recognized counted loop (header role set). Returns
/// `(func, s_phi, s_next, shl_result)`.
///
/// This is the masked-shift-accumulator shape (#43): with `mask = Some`, the
/// back-edge value is re-bounded to `[0, mask]` independently of the phi, so
/// the phi must narrow; with `mask = None`, the back-edge `s << 1` is FULL
/// (operand `s` is FULL), so the phi must NOT narrow (adversarial / bigint).
fn masked_shift_loop(
    s0: i64,
    shift: i64,
    mask: Option<i64>,
    trip: i64,
) -> (TirFunction, ValueId, ValueId, ValueId) {
    let mut func = TirFunction::new("msl".into(), vec![], TirType::None);
    // Counter machinery (drives a constant trip count so the header is a
    // recognized loop with a constant guard) + the accumulator.
    let start_i = func.fresh_value();
    let stop_v = func.fresh_value();
    let step = func.fresh_value();
    let s_start = func.fresh_value();
    let shift_c = func.fresh_value();
    let mask_c = func.fresh_value();
    let iv = func.fresh_value();
    let s_phi = func.fresh_value();
    let cond = func.fresh_value();
    let shl_res = func.fresh_value();
    let s_next = func.fresh_value();
    let next_i = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut ops = vec![
            cint(start_i, 0),
            cint(stop_v, trip),
            cint(step, 1),
            cint(s_start, s0),
            cint(shift_c, shift),
        ];
        if let Some(m) = mask {
            ops.push(cint(mask_c, m));
        }
        entry.ops = ops;
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![start_i, s_start],
        };
    }
    func.blocks.insert(
        header,
        Blk {
            id: header,
            args: vec![
                TirValue {
                    id: iv,
                    ty: TirType::I64,
                },
                TirValue {
                    id: s_phi,
                    ty: TirType::I64,
                },
            ],
            ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
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
    let mut body_ops = vec![op(OpCode::Shl, vec![s_phi, shift_c], vec![shl_res])];
    // The carried value: masked (re-bounds to [0, mask]) or bare (FULL).
    if mask.is_some() {
        body_ops.push(op(OpCode::BitAnd, vec![shl_res, mask_c], vec![s_next]));
    } else {
        // No mask: the carried value IS the shift result. Use a plain copy so
        // the back-edge arg is a distinct id (mirrors `s = s << 1`).
        body_ops.push(op(OpCode::Copy, vec![shl_res], vec![s_next]));
    }
    body_ops.push(op_nsw(OpCode::Add, vec![iv, step], vec![next_i]));
    func.blocks.insert(
        body,
        Blk {
            id: body,
            args: vec![],
            ops: body_ops,
            terminator: Terminator::Branch {
                target: header,
                args: vec![next_i, s_next],
            },
        },
    );
    func.blocks.insert(
        exit,
        Blk {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles.insert(exit, LoopRole::LoopEnd);
    mark_i64_values(
        &mut func,
        [
            start_i, stop_v, step, s_start, shift_c, mask_c, iv, s_phi, shl_res, s_next, next_i,
        ],
    );
    mark_bool_values(&mut func, [cond]);
    (func, s_phi, s_next, shl_res)
}

#[test]
fn masked_back_edge_phi_narrows_and_is_raw_safe() {
    // s = (s << 1) & MASK, MASK = 2**32 - 1. The masked back-edge value is
    // [0, MASK] INDEPENDENT of the phi, so the phi must narrow to [0, MASK].
    use crate::representation_facts::raw_i64_safe_values_for;
    let mask = (1i64 << 32) - 1;
    let (func, s_phi, s_next, shl_res) = masked_shift_loop(1, 1, Some(mask), 64);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);

    // The back-edge masked value is bounded by the mask (phi-independent).
    assert_eq!(
        vr.range_of(s_next),
        IntRange::new(0, mask),
        "masked back-edge value must be [0, MASK] under the FULL-phi sweep"
    );
    // The header phi is narrowed to the JOIN of {start=[1,1], [0, MASK]}.
    assert_eq!(
        vr.range_of(s_phi),
        IntRange::new(0, mask),
        "masked-accumulator phi must narrow to the JOIN of its incomings"
    );
    assert!(
        vr.fits_inline_int47(s_phi),
        "[0, 2**32-1] fits the 2**46 inline window"
    );
    // The re-sweep ranges the shift result `s << 1` to [0, MASK<<1], which
    // fits the inline window — the value the raw-i64 shift seed needs.
    assert_eq!(vr.range_of(shl_res), IntRange::new(0, mask << 1));
    assert!(vr.fits_inline_int47(shl_res));

    // End-to-end: the shift result IS now a raw-i64-safe carrier (count 1 in
    // [0,63] AND result fits inline) — the boxed `molt_lshift` lane is gone.
    let raw = raw_i64_safe_values_for(&func, &vr);
    assert!(
        raw.contains(&shl_res),
        "the masked-accumulator shift must be raw-i64-safe post-narrowing"
    );
    assert!(
        raw.contains(&s_phi),
        "the narrowed phi must propagate to a raw-i64 carrier (all incomings raw)"
    );
}

#[test]
fn non_masked_back_edge_phi_does_not_narrow() {
    // ADVERSARIAL (the soundness gate): s = s << 1 with NO mask. The
    // back-edge value `s << 1` has FULL range (operand `s` is FULL under the
    // FULL-phi sweep), so the phi must NOT narrow — it can grow into a heap
    // BigInt (`1 << 70` overflows i64), and a false inline proof would be a
    // silent truncation miscompile.
    use crate::representation_facts::raw_i64_safe_values_for;
    let (func, s_phi, s_next, shl_res) = masked_shift_loop(1, 1, None, 70);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);

    assert!(
        vr.range_of(s_next).is_full(),
        "unmasked `s << 1` back-edge must stay FULL (phi-dependent)"
    );
    assert!(
        vr.range_of(s_phi).is_full(),
        "unmasked doubling phi must NOT narrow — it grows past i64"
    );
    assert!(
        !vr.fits_inline_int47(s_phi),
        "unbounded doubling accumulator must never be proven inline"
    );
    assert!(
        !vr.fits_inline_int47(shl_res),
        "the unproven shift result must never be proven inline"
    );
    // End-to-end: NOT raw-i64-safe — the boxed BigInt-correct lane is kept.
    let raw = raw_i64_safe_values_for(&func, &vr);
    assert!(
        !raw.contains(&shl_res),
        "an unbounded doubling shift must NOT be raw-i64-safe (would truncate)"
    );
    assert!(
        !raw.contains(&s_phi),
        "the unbounded doubling phi must NOT be raw-i64-safe"
    );
}

#[test]
fn masked_back_edge_narrows_with_wider_shift() {
    // s = (s << 4) & (2**28 - 1): a wider per-step shift, still bounded by
    // the mask. The phi narrows to [0, mask]; the shift result [0, mask<<4]
    // must still fit the inline window (mask<<4 = 2**32-16 < 2**46).
    let mask = (1i64 << 28) - 1;
    let (func, s_phi, s_next, shl_res) = masked_shift_loop(3, 4, Some(mask), 20);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert_eq!(vr.range_of(s_next), IntRange::new(0, mask));
    assert_eq!(vr.range_of(s_phi), IntRange::new(0, mask));
    assert_eq!(vr.range_of(shl_res), IntRange::new(0, mask << 4));
    assert!(vr.fits_inline_int47(shl_res));
}

#[test]
fn masked_back_edge_does_not_narrow_when_mask_overflows_window() {
    // s = (s << 1) & (2**48 - 1): the mask itself exceeds the 2**46 inline
    // window, so the phi narrows to [0, 2**48-1] (a SOUND fact) but it must
    // NOT be proven inline — the value genuinely can exceed the window.
    let mask = (1i64 << 48) - 1;
    let (func, s_phi, _s_next, shl_res) = masked_shift_loop(1, 1, Some(mask), 100);
    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);
    assert_eq!(
        vr.range_of(s_phi),
        IntRange::new(0, mask),
        "phi narrows to the (sound) masked bound even when it exceeds the window"
    );
    assert!(
        !vr.fits_inline_int47(s_phi),
        "a [0, 2**48-1] phi must NOT be proven inline (exceeds 2**46)"
    );
    assert!(
        !vr.fits_inline_int47(shl_res),
        "the shift result of an out-of-window masked phi must not be inline"
    );
}

#[test]
fn masked_back_edge_narrows_with_derived_mask_and_vestigial_loopend() {
    // REAL-COMPILE FIDELITY: the frontend lowering of
    //   MASK = (1 << 32) - 1; s = 1
    //   for _ in range(N): s = (s << 1) & MASK
    // differs from `masked_shift_loop` in two structural ways that BOTH must
    // be tolerated for the narrowing to fire end-to-end (observed in the
    // bench_masked_shift_accumulator TIR dump):
    //
    //   (1) MASK is a DERIVED constant `(1 << 32) - 1`, not a literal
    //       `ConstInt`. Its `[0, MASK]` re-bound only materializes once
    //       `collect_constants_and_lengths` folds `Shl`/`Sub` of constants so
    //       `bit_and`'s constant-mask rule sees a known non-negative mask.
    //   (2) The SSA lift keeps a VESTIGIAL `loop_end -> header` back edge whose
    //       args are fabricated `ConstNone`s. That block is UNREACHABLE (no
    //       predecessor) and so must be excluded by the
    //       `executable_reachable_blocks` oracle — otherwise its FULL ConstNone
    //       incoming poisons the phi JOIN and defeats the narrow.
    //
    // This is the adversarial mirror of the unit `masked_shift_loop`: same
    // licensing structure, but built in the shape the compiler actually emits.
    let mut func = TirFunction::new("dm".into(), vec![], TirType::None);
    // Mask materials: one=1, k=32, then mask = (one << k) - 1 (DERIVED).
    let one_c = func.fresh_value();
    let k_c = func.fresh_value();
    let mask_shl = func.fresh_value();
    let mask = func.fresh_value();
    // Counter + accumulator seeds.
    let start_i = func.fresh_value();
    let stop_v = func.fresh_value();
    let step = func.fresh_value();
    let s_start = func.fresh_value();
    let shift_c = func.fresh_value();
    // Header phis.
    let iv = func.fresh_value();
    let s_phi = func.fresh_value();
    let cond = func.fresh_value();
    // Body values.
    let shl_res = func.fresh_value();
    let s_next = func.fresh_value();
    let next_i = func.fresh_value();
    // Vestigial loop-end ConstNone.
    let dead_none = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let dead_end = func.fresh_block();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            cint(one_c, 1),
            cint(k_c, 32),
            // mask = (1 << 32) - 1 — derived, must be const-folded.
            op(OpCode::Shl, vec![one_c, k_c], vec![mask_shl]),
            cint(start_i, 0),
            op(OpCode::Sub, vec![mask_shl, one_c], vec![mask]),
            cint(stop_v, 64),
            cint(step, 1),
            cint(s_start, 1),
            cint(shift_c, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![start_i, s_start],
        };
    }
    func.blocks.insert(
        header,
        Blk {
            id: header,
            args: vec![
                TirValue {
                    id: iv,
                    ty: TirType::I64,
                },
                TirValue {
                    id: s_phi,
                    ty: TirType::I64,
                },
            ],
            ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
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
        Blk {
            id: body,
            args: vec![],
            ops: vec![
                op(OpCode::Shl, vec![s_phi, shift_c], vec![shl_res]),
                op(OpCode::BitAnd, vec![shl_res, mask], vec![s_next]),
                op_nsw(OpCode::Add, vec![iv, step], vec![next_i]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![next_i, s_next],
            },
        },
    );
    func.blocks.insert(
        exit,
        Blk {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    // The vestigial, UNREACHABLE loop-end: branches back to the header with
    // fabricated ConstNone args. No block branches INTO it.
    func.blocks.insert(
        dead_end,
        Blk {
            id: dead_end,
            args: vec![],
            ops: vec![op(OpCode::ConstNone, vec![], vec![dead_none])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![dead_none, dead_none],
            },
        },
    );
    func.loop_roles.insert(dead_end, LoopRole::LoopEnd);
    mark_i64_values(
        &mut func,
        [
            one_c, k_c, mask_shl, mask, start_i, stop_v, step, s_start, shift_c, iv, s_phi,
            shl_res, s_next, next_i,
        ],
    );
    mark_bool_values(&mut func, [cond]);

    let scev = compute_scev(&func);
    let vr = compute_value_range(&func, &scev);

    let mask_val = (1i64 << 32) - 1;
    // The derived mask folded to a constant.
    assert_eq!(
        vr.range_of(mask),
        IntRange::point(mask_val),
        "derived mask (1 << 32) - 1 must const-fold to a point range"
    );
    // The masked back-edge value is [0, MASK] under the FULL-phi sweep,
    // independent of the (unreachable) ConstNone edge.
    assert_eq!(
        vr.range_of(s_next),
        IntRange::new(0, mask_val),
        "masked back-edge `(s << 1) & MASK` must be [0, MASK]"
    );
    // The phi narrows to the JOIN despite the vestigial ConstNone back edge.
    assert_eq!(
        vr.range_of(s_phi),
        IntRange::new(0, mask_val),
        "phi must narrow to [0, MASK] — the unreachable ConstNone edge must NOT \
             poison the JOIN"
    );
    assert!(
        vr.fits_inline_int47(s_phi),
        "[0, 2**32-1] fits the inline window"
    );
    // The shift result feeds the raw-i64 seed.
    assert_eq!(vr.range_of(shl_res), IntRange::new(0, mask_val << 1));
    assert!(vr.fits_inline_int47(shl_res));
    use crate::representation_facts::raw_i64_safe_values_for;
    let raw = raw_i64_safe_values_for(&func, &vr);
    assert!(
        raw.contains(&shl_res) && raw.contains(&s_phi),
        "the derived-mask masked accumulator must reach the raw-i64 lane \
             end-to-end (shift result + phi both raw)"
    );
}
