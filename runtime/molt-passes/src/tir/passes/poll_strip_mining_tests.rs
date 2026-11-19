use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{AttrDict, Dialect};

/// How the fixture's loop departs from the canonical counted loop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Canonical,
    /// `while x < stop`: the compare runs a block before its branch, on an
    /// alias copy of the counter.
    SplitTest,
    /// The SSA lift's vestigial `loop_end -> header` edge: unreachable, with a
    /// fabricated `None` argument.
    DeadEntry,
    /// A `for` loop: the body publishes the pre-step counter to its home.
    LaggedHome,
    CallInBody,
    UnprovenBound,
    Falling,
    Resumable,
}

struct Fixture {
    func: TirFunction,
    header: BlockId,
    guard: BlockId,
    latch: BlockId,
    exit: BlockId,
}

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::default(),
        source_span: None,
    }
}

fn int(func: &mut TirFunction, value: i64, proven: bool) -> (ValueId, TirOp) {
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::I64);
    let mut constant = op(OpCode::ConstInt, vec![], vec![result]);
    constant.attrs.insert("value".into(), AttrValue::Int(value));
    if proven {
        constant.mark_inline_int_proof();
    }
    (result, constant)
}

fn poll_check(label: i64) -> TirOp {
    let mut check = op(OpCode::CheckException, vec![], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(label));
    check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
    check.mark_async_work_poll();
    check
}

/// `for x in range(0, 100): pass`, after the late stage, with its back-edge
/// poll: `H(p) -> G: x = copy p; c = lt x, stop -> Body: x1 = x + 1; poll`.
fn counted_loop(variant: Variant) -> Fixture {
    let mut func = TirFunction::new(
        "counted".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let entry = func.entry_block;
    let header = func.fresh_block();
    let guard = func.fresh_block();
    let latch = func.fresh_block();
    let exit = func.fresh_block();

    let (stop, stop_op) = if variant == Variant::UnprovenBound {
        int(&mut func, 1 << 50, false)
    } else {
        int(&mut func, 100, true)
    };
    let (start, start_op) = int(&mut func, 0, true);
    let param = func.fresh_value();
    func.value_types.insert(param, TirType::I64);
    let induction = func.fresh_value();
    func.value_types.insert(induction, TirType::I64);
    let cond = func.fresh_value();
    func.value_types.insert(cond, TirType::Bool);
    let (one, one_op) = int(&mut func, 1, true);
    let next = func.fresh_value();
    func.value_types.insert(next, TirType::I64);

    let entry_block = func.blocks.get_mut(&entry).unwrap();
    entry_block.ops = vec![stop_op, start_op];
    entry_block.terminator = Terminator::Branch {
        target: header,
        args: vec![start],
    };
    let mut copy = op(OpCode::Copy, vec![param], vec![induction]);
    copy.mark_inline_int_proof();
    if variant == Variant::SplitTest {
        copy.origin = Some(molt_ir::OpKind::BindingAlias);
    }
    let compare = op(OpCode::Lt, vec![induction, stop], vec![cond]);
    let (test_ops, branch_ops) = if variant == Variant::SplitTest {
        (vec![copy, compare], vec![])
    } else {
        (vec![], vec![copy, compare])
    };
    let test = func.fresh_block();
    let mut step = op(
        if variant == Variant::Falling {
            OpCode::Sub
        } else {
            OpCode::Add
        },
        vec![induction, one],
        vec![next],
    );
    step.mark_inline_int_proof();
    let mut body_ops = vec![one_op, step];
    if variant == Variant::LaggedHome {
        let home = func.fresh_value();
        func.value_types.insert(home, TirType::I64);
        let mut store = op(OpCode::Copy, vec![induction], vec![home]);
        store.origin = Some(molt_ir::OpKind::FrameHomeStore);
        store.attrs.insert("value".into(), AttrValue::Int(0));
        body_ops.insert(0, store);
    }
    if variant == Variant::CallInBody {
        let mut call = op(OpCode::Call, vec![], vec![]);
        call.attrs
            .insert("s_value".into(), AttrValue::Str("work".into()));
        body_ops.push(call);
        body_ops.push(poll_check(90));
    }
    body_ops.push(poll_check(90));
    let blocks = [
        (
            header,
            vec![TirValue {
                id: param,
                ty: TirType::I64,
            }],
            vec![],
            Terminator::Branch {
                target: test,
                args: vec![],
            },
        ),
        (
            test,
            vec![],
            test_ops,
            Terminator::Branch {
                target: guard,
                args: vec![],
            },
        ),
        (
            guard,
            vec![],
            branch_ops,
            Terminator::CondBranch {
                cond,
                then_block: latch,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        ),
        (
            latch,
            vec![],
            body_ops,
            Terminator::Branch {
                target: header,
                args: vec![next],
            },
        ),
        (exit, vec![], vec![], Terminator::Return { values: vec![] }),
    ];
    for (id, args, ops, terminator) in blocks {
        for arg in &args {
            func.value_types.insert(arg.id, arg.ty.clone());
        }
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args,
                ops,
                terminator,
            },
        );
    }
    func.label_id_map.insert(exit.0, 90);
    if variant == Variant::DeadEntry {
        let loop_end = func.fresh_block();
        let none = func.fresh_value();
        func.value_types.insert(none, TirType::None);
        func.blocks.insert(
            loop_end,
            TirBlock {
                id: loop_end,
                args: vec![],
                ops: vec![op(OpCode::ConstNone, vec![], vec![none])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![none],
                },
            },
        );
    }
    if variant == Variant::Resumable {
        // A resumable body: a state dispatch anywhere refuses the function.
        let resume = func.fresh_block();
        func.blocks.insert(
            resume,
            TirBlock {
                id: resume,
                args: vec![],
                ops: vec![],
                terminator: Terminator::StateDispatch {
                    cases: vec![],
                    default: exit,
                    default_args: vec![],
                },
            },
        );
    }
    crate::tir::verify::verify_function(&func).expect("valid counted-loop fixture");
    Fixture {
        func,
        header,
        guard,
        latch,
        exit,
    }
}

fn polls(func: &TirFunction) -> Vec<BlockId> {
    let mut blocks: Vec<BlockId> = func
        .blocks
        .iter()
        .filter(|(_, block)| block.ops.iter().any(TirOp::is_async_work_poll))
        .map(|(id, _)| *id)
        .collect();
    blocks.sort_unstable_by_key(|id| id.0);
    blocks
}

#[test]
fn the_exit_test_carries_the_poll() {
    let Fixture {
        mut func,
        header,
        guard,
        latch,
        exit,
    } = counted_loop(Variant::Canonical);
    let stats = run(&mut func, &mut AnalysisManager::new());
    assert_eq!(stats.attrs_changed, 1, "one loop rewritten");
    crate::tir::verify::verify_function(&func).expect("rewritten loop verifies");

    // The header carries the limit; the guard tests against it.
    let limit = func.blocks[&header].args.last().unwrap().id;
    assert_eq!(func.blocks[&header].args.len(), 2);
    let compare = func.blocks[&guard]
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Lt)
        .unwrap();
    assert_eq!(compare.operands[1], limit);
    // The split `while` shape is rewritten the same way.
    let mut split = counted_loop(Variant::SplitTest);
    assert_eq!(
        run(&mut split.func, &mut AnalysisManager::new()).attrs_changed,
        1
    );
    crate::tir::verify::verify_function(&split.func).expect("rewritten while loop verifies");

    // The latch no longer polls and passes the limit unchanged.
    assert!(
        !func.blocks[&latch]
            .ops
            .iter()
            .any(TirOp::is_async_work_poll)
    );
    assert!(matches!(
        &func.blocks[&latch].terminator,
        Terminator::Branch { args, .. } if args.last() == Some(&limit)
    ));

    // One poll, in the boundary's service block: reached only when the guard
    // fails and the real test still continues.
    let [service] = polls(&func)[..] else {
        panic!("exactly one poll");
    };
    let Terminator::CondBranch {
        else_block: boundary,
        ..
    } = &func.blocks[&guard].terminator
    else {
        panic!("guard keeps its conditional branch");
    };
    assert!(matches!(
        &func.blocks[boundary].terminator,
        Terminator::CondBranch { then_block, else_block, .. }
            if *then_block == service && *else_block == exit
    ));
}

#[test]
fn the_limit_is_a_raw_int_through_its_own_back_edge() {
    let Fixture {
        mut func, header, ..
    } = counted_loop(Variant::Canonical);
    assert_eq!(run(&mut func, &mut AnalysisManager::new()).attrs_changed, 1);
    let limit = func.blocks[&header].args.last().unwrap().id;
    let raw = crate::representation_facts::non_heap_values_for(
        &func,
        &crate::representation_facts::value_range_for(&func),
    );
    assert!(
        raw.contains(&limit),
        "the latch passes the limit back unchanged; the greatest fixpoint keeps it raw"
    );
}

fn verdicts(func: &TirFunction) -> Vec<Result<(), Refusal>> {
    plans(func, &mut AnalysisManager::new())
        .into_iter()
        .map(|(_, plan)| plan.map(|_| ()))
        .collect()
}

#[test]
fn the_canonical_loop_has_a_plan() {
    assert_eq!(verdicts(&counted_loop(Variant::Canonical).func), [Ok(())]);
    assert_eq!(verdicts(&counted_loop(Variant::SplitTest).func), [Ok(())]);
    assert_eq!(verdicts(&counted_loop(Variant::DeadEntry).func), [Ok(())]);
}

#[test]
fn loops_outside_the_contract_keep_their_poll() {
    for (variant, refusal) in [
        (Variant::CallInBody, Refusal::RunsPython),
        (Variant::UnprovenBound, Refusal::NotInlineInts),
        (Variant::Falling, Refusal::NoProgress),
        (Variant::Resumable, Refusal::Resumable),
        (Variant::LaggedHome, Refusal::LaggedBinding),
    ] {
        let Fixture {
            mut func, latch, ..
        } = counted_loop(variant);
        assert_eq!(verdicts(&func), [Err(refusal)]);
        let before = polls(&func);
        let stats = run(&mut func, &mut AnalysisManager::new());
        assert_eq!(stats.total_changes(), 0);
        assert_eq!(polls(&func), before);
        assert!(before.contains(&latch));
    }
}
