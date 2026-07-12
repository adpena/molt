use super::compute_scev;
use crate::tir::blocks::{BlockId, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{ScevExpr, TripCount};
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

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

fn const_int(result: ValueId, value: i64) -> TirOp {
    let mut o = op(OpCode::ConstInt, vec![], vec![result]);
    o.attrs.insert("value".into(), AttrValue::Int(value));
    o
}

/// Build the canonical post-range_devirt shape for `for i in range(stop)`:
///
/// ```text
/// entry:  start = const 0; stop = const STOP; br header(start)
/// header(iv): cond = Lt(iv, stop); condbr cond -> body, exit
/// body:   next = Add(iv, step=1) [nsw]; br header(next)
/// exit:   ret
/// ```
///
/// Returns (func, header, iv, body).
fn range_loop(stop: i64, nsw: bool) -> (TirFunction, BlockId, ValueId, BlockId) {
    let mut func = TirFunction::new("rl".into(), vec![], TirType::None);
    let start = func.fresh_value();
    let stop_v = func.fresh_value();
    let step = func.fresh_value();
    let iv = func.fresh_value();
    let cond = func.fresh_value();
    let next = func.fresh_value();

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            const_int(start, 0),
            const_int(stop_v, stop),
            const_int(step, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![start],
        };
    }

    func.blocks.insert(
        header,
        TirBlock {
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

    let add = if nsw {
        op_nsw(OpCode::Add, vec![iv, step], vec![next])
    } else {
        op(OpCode::Add, vec![iv, step], vec![next])
    };
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![add],
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

    (func, header, iv, body)
}

#[test]
fn detects_canonical_induction_variable() {
    let (func, header, iv, _body) = range_loop(10, true);
    let scev = compute_scev(&func);
    let e = scev.scev_of(iv);
    match e {
        ScevExpr::AddRec {
            start,
            step,
            loop_header,
        } => {
            assert_eq!(*start, ScevExpr::Constant(0));
            assert_eq!(*step, ScevExpr::Constant(1));
            assert_eq!(loop_header, header);
        }
        other => panic!("expected AddRec, got {other:?}"),
    }
    assert!(scev.is_induction_var(iv));
}

#[test]
fn detects_backedge_loop_without_loop_roles() {
    let (mut func, header, iv, _body) = range_loop(10, true);
    func.loop_roles.clear();

    let scev = compute_scev(&func);

    assert!(scev.is_induction_var(iv));
    assert_eq!(scev.trip_count(header), TripCount::Constant(10));
}

#[test]
fn wrapping_increment_is_not_addrec() {
    // Without no_signed_wrap, we MUST NOT form an AddRec.
    let (func, _header, iv, _body) = range_loop(10, false);
    let scev = compute_scev(&func);
    assert!(
        !scev.is_induction_var(iv),
        "a possibly-wrapping increment must not be an AddRec"
    );
}

#[test]
fn constant_trip_count_for_range() {
    let (func, header, _iv, _body) = range_loop(10, true);
    let scev = compute_scev(&func);
    assert_eq!(scev.trip_count(header), TripCount::Constant(10));
}

#[test]
fn empty_range_trip_count_zero() {
    let (func, header, _iv, _body) = range_loop(0, true);
    let scev = compute_scev(&func);
    assert_eq!(scev.trip_count(header), TripCount::Constant(0));
}

#[test]
fn degree_two_recurrence_is_unknown() {
    // Build `total += i` inside the IV loop: total is a second header-arg
    // whose back-edge value is Add(total, iv) — step (iv) is itself an
    // AddRec → must classify total as Unknown (not an AddRec).
    let (mut func, header, iv, body) = range_loop(10, true);
    let total = func.fresh_value();
    let total_start = func.fresh_value();
    let total_next = func.fresh_value();

    // total_start = const 0 in entry; pass to header as 2nd arg.
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int(total_start, 0));
        if let Terminator::Branch { args, .. } = &mut entry.terminator {
            args.push(total_start);
        }
    }
    // header gets a 2nd block arg `total`.
    func.blocks.get_mut(&header).unwrap().args.push(TirValue {
        id: total,
        ty: TirType::I64,
    });
    // body: total_next = Add(total, iv) [nsw]; pass back as 2nd arg.
    {
        let b = func.blocks.get_mut(&body).unwrap();
        b.ops
            .push(op_nsw(OpCode::Add, vec![total, iv], vec![total_next]));
        if let Terminator::Branch { args, .. } = &mut b.terminator {
            args.push(total_next);
        }
    }

    let scev = compute_scev(&func);
    // iv is still a clean AddRec.
    assert!(scev.is_induction_var(iv));
    // total (degree-2: step is the iv AddRec) must be Unknown.
    assert_eq!(
        scev.scev_of(total),
        ScevExpr::Unknown,
        "accumulator total += i is degree-2 and must be Unknown (loop-IV OOM hazard)"
    );
}

#[test]
fn loopless_function_has_no_scev() {
    let mut func = TirFunction::new("f".into(), vec![], TirType::None);
    let v = func.fresh_value();
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![const_int(v, 5)];
        entry.terminator = Terminator::Return { values: vec![] };
    }
    let scev = compute_scev(&func);
    assert!(scev.headers().is_empty());
    assert_eq!(scev.trip_count(BlockId(0)), TripCount::Unknown);
}

#[test]
fn non_unit_step_constant_trip_count() {
    // for i in range(0, 10, 2): step 2, trip = 5.
    let (mut func, header, _iv, body) = range_loop(10, true);
    // Rewrite the step const to 2 and re-mark the add nsw (still sound: the
    // guard bounds it; for the unit test we trust the nsw attr).
    // Find the step const op in entry and set it to 2.
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for o in entry.ops.iter_mut() {
            if o.opcode == OpCode::ConstInt && o.attrs.get("value") == Some(&AttrValue::Int(1)) {
                o.attrs.insert("value".into(), AttrValue::Int(2));
            }
        }
    }
    let _ = body;
    let scev = compute_scev(&func);
    // ceil((10-0)/2) = 5.
    assert_eq!(scev.trip_count(header), TripCount::Constant(5));
}
