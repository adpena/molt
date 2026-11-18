use super::*;
use crate::tir::analysis::AnalysisManager;

const HANDLER_LABEL: i64 = 9;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp::new(opcode, operands, results)
}

fn copy(kind: OpKind, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut op = op(OpCode::Copy, operands, results);
    op.origin = Some(kind);
    op
}

fn store(value: ValueId, view: Option<ValueId>, slot: i64) -> TirOp {
    let mut op = copy(
        OpKind::FrameHomeStore,
        vec![value],
        view.into_iter().collect(),
    );
    op.attrs.insert("value".into(), AttrValue::Int(slot));
    op
}

fn home_load(slot: i64, result: ValueId) -> TirOp {
    let mut op = copy(OpKind::FrameHomeLoad, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(slot));
    op
}

fn const_int(value: i64, result: ValueId) -> TirOp {
    let mut op = op(OpCode::ConstInt, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Int(value));
    op
}

fn const_bool(value: bool, result: ValueId) -> TirOp {
    let mut op = op(OpCode::ConstBool, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Bool(value));
    op
}

fn poll() -> TirOp {
    let mut op = op(OpCode::CheckException, vec![], vec![]);
    op.attrs
        .insert("value".into(), AttrValue::Int(HANDLER_LABEL));
    op.attrs
        .insert("async_work_poll".into(), AttrValue::Bool(true));
    assert!(op.is_async_work_poll());
    op
}

fn call(result: ValueId) -> TirOp {
    let mut op = op(OpCode::Call, vec![], vec![result]);
    op.attrs
        .insert("s_value".into(), AttrValue::Str("work".into()));
    op
}

fn block(
    id: u32,
    args: Vec<(ValueId, TirType)>,
    ops: Vec<TirOp>,
    terminator: Terminator,
) -> TirBlock {
    TirBlock {
        id: BlockId(id),
        args: args
            .into_iter()
            .map(|(id, ty)| crate::tir::values::TirValue { id, ty })
            .collect(),
        ops,
        terminator,
    }
}

fn exit_block(id: u32) -> TirBlock {
    block(
        id,
        vec![],
        vec![copy(OpKind::TraceExit, vec![], vec![])],
        Terminator::Return { values: vec![] },
    )
}

/// A synchronous frame with one parameter `p` (value 0) and a function-level
/// handler block 9 that exits the frame:
///
/// ```text
/// bb0:  trace_enter_slot; %1 = 0; home[0] = %1 (first)
///       br bb1(%1)
/// bb1(%2): %3 = true; cond_br %3, bb2, bb4
/// bb2:  body; br bb1(%n)
/// bb4:  trace_exit; return
/// ```
fn loop_function(first: ValueId, body: Vec<TirOp>, next: ValueId) -> TirFunction {
    let mut func = TirFunction::new(
        "f".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.execution_context = ExecutionContextPolicy::Local;
    func.next_value = 100;
    func.next_block = 10;
    let header_value = ValueId(2);
    func.value_types.insert(header_value, TirType::I64);
    let entry = func.blocks.get_mut(&BlockId(0)).unwrap();
    entry.ops = vec![
        copy(OpKind::TraceEnterSlot, vec![], vec![]),
        const_int(0, ValueId(1)),
        store(first, None, 0),
    ];
    entry.terminator = Terminator::Branch {
        target: BlockId(1),
        args: vec![ValueId(1)],
    };
    func.blocks.insert(
        BlockId(1),
        block(
            1,
            vec![(header_value, TirType::I64)],
            vec![const_bool(true, ValueId(3))],
            Terminator::CondBranch {
                cond: ValueId(3),
                then_block: BlockId(2),
                then_args: vec![],
                else_block: BlockId(4),
                else_args: vec![],
            },
        ),
    );
    func.blocks.insert(
        BlockId(2),
        block(
            2,
            vec![],
            body,
            Terminator::Branch {
                target: BlockId(1),
                args: vec![next],
            },
        ),
    );
    func.blocks.insert(BlockId(4), exit_block(4));
    func.blocks.insert(BlockId(9), exit_block(9));
    func.label_id_map.insert(9, HANDLER_LABEL);
    func
}

/// The terminal phase's order: the nothrow proofs, then placement.
fn place(func: &mut TirFunction) -> PassStats {
    prove_home_writes_nothrow(func);
    run(func, &mut AnalysisManager::new(), true)
}

fn home_writes_in(func: &TirFunction, bid: BlockId) -> Vec<(i64, ValueId)> {
    func.blocks[&bid]
        .ops
        .iter()
        .filter(|op| is_home_store(op))
        .map(|op| {
            let Some(AttrValue::Int(slot)) = op.attrs.get("value") else {
                panic!("a home store names its slot");
            };
            (*slot, op.operands[0])
        })
        .collect()
}

fn poll_blocks(func: &TirFunction) -> Vec<BlockId> {
    let mut found: Vec<BlockId> = func
        .blocks
        .values()
        .filter(|block| block.ops.iter().any(TirOp::is_async_work_poll))
        .map(|block| block.id)
        .collect();
    found.sort_unstable();
    found
}

/// The loop body rebinds the local to an int each iteration. Its normal path
/// writes no home: the poll's cold block writes it before servicing work, and
/// the loop exit writes the binding the header holds.
#[test]
fn a_loop_writes_its_home_only_at_the_poll_service_and_the_exit() {
    let five = ValueId(5);
    let mut func = loop_function(
        ValueId(1),
        vec![const_int(5, five), store(five, Some(ValueId(6)), 0), poll()],
        ValueId(6),
    );
    let stats = place(&mut func);
    assert!(stats.facts_changed > 0, "{stats:?}");

    // The body keeps a pending-work test, not the store or the poll.
    let body = &func.blocks[&BlockId(2)];
    assert!(home_writes_in(&func, BlockId(2)).is_empty());
    assert!(
        body.ops
            .iter()
            .any(|op| op.opcode == OpCode::AsyncWorkPending)
    );
    assert!(!body.ops.iter().any(TirOp::is_async_work_poll));
    let Terminator::CondBranch {
        then_block: cold,
        else_block: rest,
        ..
    } = body.terminator
    else {
        panic!("the poll split the body: {:?}", body.terminator);
    };
    assert_eq!(home_writes_in(&func, cold), vec![(0, five)]);
    assert_eq!(poll_blocks(&func), vec![cold]);
    assert!(matches!(
        func.blocks[&rest].terminator,
        Terminator::Branch {
            target: BlockId(1),
            ..
        }
    ));
    // The view the store returned now names the value itself.
    assert!(
        func.blocks[&BlockId(2)]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::Copy
                && op.origin.is_none()
                && op.operands == vec![five]
                && op.results == vec![ValueId(6)])
    );
    // The exit writes the header's binding.
    assert_eq!(home_writes_in(&func, BlockId(4)), vec![(0, ValueId(2))]);
    // The store before the loop stays, with its nothrow proof; every written
    // home carries one.
    assert_eq!(home_writes_in(&func, BlockId(0)), vec![(0, ValueId(1))]);
    for block in func.blocks.values() {
        for op in block.ops.iter().filter(|op| is_home_store(op)) {
            assert!(op.has_nothrow_proof(), "{op:?}");
        }
    }
}

/// A rerun of the terminal phase finds the marker and moves nothing.
#[test]
fn placement_runs_once() {
    let five = ValueId(5);
    let mut func = loop_function(
        ValueId(1),
        vec![const_int(5, five), store(five, None, 0), poll()],
        five,
    );
    place(&mut func);
    let blocks = func.blocks.len();
    let stats = place(&mut func);
    assert_eq!(stats.facts_changed, 0);
    assert_eq!(stats.ops_added, 0);
    assert_eq!(func.blocks.len(), blocks);
}

/// The loop of [`loop_function`] whose body first takes a conditional arm:
///
/// ```text
/// bb2:  cond_br %3, bb5, bb6
/// bb5:  arm; br bb6
/// bb6:  body; br bb1(%n)      ; the latch
/// ```
fn loop_function_with_arm(
    first: ValueId,
    arm: Vec<TirOp>,
    body: Vec<TirOp>,
    next: ValueId,
) -> TirFunction {
    let mut func = loop_function(first, vec![], next);
    let latch = func.blocks.remove(&BlockId(2)).unwrap().terminator;
    func.blocks.insert(
        BlockId(2),
        block(
            2,
            vec![],
            vec![],
            Terminator::CondBranch {
                cond: ValueId(3),
                then_block: BlockId(5),
                then_args: vec![],
                else_block: BlockId(6),
                else_args: vec![],
            },
        ),
    );
    func.blocks.insert(
        BlockId(5),
        block(
            5,
            vec![],
            arm,
            Terminator::Branch {
                target: BlockId(6),
                args: vec![],
            },
        ),
    );
    func.blocks
        .insert(BlockId(6), block(6, vec![], body, latch));
    func
}

/// A call can run Python that reads the frame: on its arm the home is written
/// before it, from the binding the header holds.
#[test]
fn a_call_on_an_arm_reads_the_home() {
    let five = ValueId(5);
    let mut func = loop_function_with_arm(
        ValueId(1),
        vec![call(ValueId(7))],
        vec![const_int(5, five), store(five, None, 0), poll()],
        five,
    );
    place(&mut func);
    let arm = &func.blocks[&BlockId(5)];
    let at_call = arm
        .ops
        .iter()
        .position(|op| op.opcode == OpCode::Call)
        .unwrap();
    assert_eq!(at_call, 1);
    assert!(is_home_store(&arm.ops[0]));
    assert_eq!(arm.ops[0].operands, vec![ValueId(2)]);
    assert!(home_writes_in(&func, BlockId(6)).is_empty());
}

/// `for i in range(n)`: the local has no binding before the loop, and the
/// exit edge leaves the header, where no SSA value holds the binding. The
/// header carries it in an added parameter with a flag that only the latch
/// sets, so the exit writes the home when the body ran and leaves it unbound
/// when the loop ran no iteration.
#[test]
fn a_binding_first_made_in_the_loop_is_written_at_the_exit_only_after_an_iteration() {
    let five = ValueId(5);
    let mut func = loop_function(
        ValueId(1),
        vec![const_int(5, five), store(five, Some(ValueId(6)), 0), poll()],
        ValueId(6),
    );
    // No binding before the loop.
    func.blocks.get_mut(&BlockId(0)).unwrap().ops.pop();
    func.value_types.insert(five, TirType::I64);
    func.value_types.insert(ValueId(6), TirType::I64);
    let stats = place(&mut func);
    assert!(stats.facts_changed > 0, "{stats:?}");
    assert!(home_writes_in(&func, BlockId(2)).is_empty());

    // The header gains the flag and the carried binding.
    let header = &func.blocks[&BlockId(1)];
    let [_, flag, carried] = header.args.as_slice() else {
        panic!("the header carries a flag and a binding: {:?}", header.args);
    };
    assert_eq!((&flag.ty, &carried.ty), (&TirType::Bool, &TirType::I64));

    // The entry passes false; the latch, after the poll split, passes true and
    // the binding.
    let passed = |bid: BlockId| -> Vec<ValueId> {
        let mut found = Vec::new();
        func.blocks[&bid].terminator.for_each_edge(|to, args| {
            if to == BlockId(1) {
                found = args.to_vec();
            }
        });
        found
    };
    let flag_const = |bid: BlockId, value: ValueId| -> Option<bool> {
        func.blocks[&bid].ops.iter().find_map(|op| {
            (op.results == vec![value] && op.opcode == OpCode::ConstBool)
                .then(|| matches!(op.attrs.get("value"), Some(AttrValue::Bool(true))))
        })
    };
    let entry_args = passed(BlockId(0));
    assert_eq!(entry_args.len(), 3);
    assert_eq!(flag_const(BlockId(0), entry_args[1]), Some(false));
    let Terminator::CondBranch {
        else_block: rest, ..
    } = func.blocks[&BlockId(2)].terminator
    else {
        panic!("the poll split the body");
    };
    let latch_args = passed(rest);
    assert_eq!(flag_const(rest, latch_args[1]), Some(true));
    assert_eq!(latch_args[2], five);

    // The exit edge tests the flag; only its taken arm writes the home.
    let Terminator::CondBranch {
        else_block: split, ..
    } = header.terminator
    else {
        panic!("the header branches");
    };
    let Terminator::CondBranch {
        cond,
        then_block: write,
        else_block: exit,
        ..
    } = func.blocks[&split].terminator
    else {
        panic!("the exit edge tests the flag: {:?}", func.blocks[&split]);
    };
    assert_eq!((cond, exit), (flag.id, BlockId(4)));
    assert!(home_writes_in(&func, split).is_empty());
    assert_eq!(home_writes_in(&func, write), vec![(0, carried.id)]);
    assert!(matches!(
        func.blocks[&write].terminator,
        Terminator::Branch {
            target: BlockId(4),
            ..
        }
    ));
    assert!(home_writes_in(&func, BlockId(4)).is_empty());
}

/// A reader every iteration runs would be written as often as the store it
/// replaces: the slot keeps its stores, and the poll stays.
#[test]
fn a_reader_every_iteration_runs_keeps_the_store() {
    let five = ValueId(5);
    let mut func = loop_function(
        ValueId(1),
        vec![
            call(ValueId(7)),
            const_int(5, five),
            store(five, None, 0),
            poll(),
        ],
        five,
    );
    let stats = place(&mut func);
    assert_eq!(stats.facts_changed, 0);
    assert_eq!(home_writes_in(&func, BlockId(2)), vec![(0, five)]);
    assert_eq!(poll_blocks(&func), vec![BlockId(2)]);
}

/// The body's own read of the home sees the binding.
#[test]
fn a_home_load_reads_the_home() {
    let five = ValueId(5);
    let mut func = loop_function_with_arm(
        ValueId(1),
        vec![home_load(0, ValueId(8))],
        vec![const_int(5, five), store(five, None, 0), poll()],
        five,
    );
    place(&mut func);
    let arm = &func.blocks[&BlockId(5)];
    assert!(is_home_store(&arm.ops[0]));
    assert_eq!(arm.ops[0].operands, vec![ValueId(2)]);
    assert_eq!(arm.ops[1].origin, Some(OpKind::FrameHomeLoad));
}

/// A home that held the parameter may release a finalizer at the loop's
/// first rebinding, so the loop keeps its stores and its poll.
#[test]
fn a_binding_that_may_finalize_keeps_its_store() {
    let five = ValueId(5);
    let mut func = loop_function(
        ValueId(0),
        vec![const_int(5, five), store(five, None, 0), poll()],
        five,
    );
    let stats = place(&mut func);
    assert_eq!(stats.facts_changed, 0);
    assert_eq!(home_writes_in(&func, BlockId(2)), vec![(0, five)]);
    assert_eq!(poll_blocks(&func), vec![BlockId(2)]);
}

/// A parameter store boxes nothing: it carries the proof, though it moves.
#[test]
fn a_store_that_boxes_nothing_is_nothrow() {
    let mut func = loop_function(ValueId(0), vec![poll()], ValueId(1));
    place(&mut func);
    let entry = &func.blocks[&BlockId(0)];
    let first = entry.ops.iter().find(|op| is_home_store(op)).unwrap();
    assert!(first.has_nothrow_proof());
    let mut effects_types = HashMap::default();
    effects_types.insert(ValueId(0), TirType::DynBox);
    assert!(super::super::effects::op_effects_with_types(first, &effects_types).nothrow);
}

/// A `line` publication writes the location word: no home is written there.
#[test]
fn a_line_publication_reads_no_home() {
    let five = ValueId(5);
    let mut line = copy(OpKind::Line, vec![], vec![]);
    line.attrs.insert("value".into(), AttrValue::Int(4));
    let mut func = loop_function(
        ValueId(1),
        vec![line, const_int(5, five), store(five, None, 0), poll()],
        five,
    );
    place(&mut func);
    assert!(home_writes_in(&func, BlockId(2)).is_empty());
}

fn publication(id: i64) -> TirOp {
    let mut op = op(OpCode::FrameSetLocation, vec![], vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(id));
    op
}

fn publications_in(func: &TirFunction, bid: BlockId) -> Vec<i64> {
    func.blocks[&bid]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::FrameSetLocation)
        .filter_map(location_of)
        .collect()
}

/// The location word is a frame-record word too: a publication no reader on
/// the loop's normal path sees moves to the poll's service path and the exit,
/// and a call on an arm gets the publication that reaches it.
#[test]
fn a_loop_publishes_its_location_only_where_a_reader_can_run() {
    let five = ValueId(5);
    let mut func = loop_function_with_arm(
        ValueId(1),
        vec![call(ValueId(7))],
        vec![
            publication(14),
            const_int(5, five),
            store(five, None, 0),
            poll(),
        ],
        five,
    );
    func.blocks
        .get_mut(&BlockId(1))
        .unwrap()
        .ops
        .insert(0, publication(7));
    place(&mut func);
    assert!(publications_in(&func, BlockId(1)).is_empty());
    assert!(publications_in(&func, BlockId(6)).is_empty());
    // The arm's call sees the header's publication.
    assert_eq!(publications_in(&func, BlockId(5)), vec![7]);
    let Terminator::CondBranch {
        then_block: cold, ..
    } = func.blocks[&BlockId(6)].terminator
    else {
        panic!("the poll split the latch");
    };
    assert_eq!(publications_in(&func, cold), vec![14]);
    assert_eq!(publications_in(&func, BlockId(4)), vec![7]);
}

/// A reader on an arm inside the loop (a generic slow path), where only the
/// header's carried binding holds the local, writes it under the header's
/// flag in a block split before the reader. The body's normal path still
/// writes no home, so a cold reader no longer pins a store every iteration.
#[test]
fn a_reader_on_an_arm_writes_a_carried_binding_under_the_flag() {
    let five = ValueId(5);
    let mut func = loop_function_with_arm(
        ValueId(1),
        vec![call(ValueId(7))],
        vec![const_int(5, five), store(five, Some(ValueId(6)), 0), poll()],
        ValueId(6),
    );
    // No binding before the loop: only the carried binding holds it at the arm.
    func.blocks.get_mut(&BlockId(0)).unwrap().ops.pop();
    func.value_types.insert(five, TirType::I64);
    func.value_types.insert(ValueId(6), TirType::I64);
    let stats = place(&mut func);
    assert!(stats.facts_changed > 0, "{stats:?}");
    assert!(home_writes_in(&func, BlockId(6)).is_empty());

    let header = &func.blocks[&BlockId(1)];
    let [_, flag, carried] = header.args.as_slice() else {
        panic!("the header carries a flag and a binding: {:?}", header.args);
    };
    let Terminator::CondBranch {
        cond,
        then_block: write,
        else_block: cont,
        ..
    } = func.blocks[&BlockId(5)].terminator
    else {
        panic!(
            "the arm tests the flag before its reader: {:?}",
            func.blocks[&BlockId(5)]
        );
    };
    assert_eq!(cond, flag.id);
    assert_eq!(home_writes_in(&func, write), vec![(0, carried.id)]);
    assert!(matches!(
        func.blocks[&write].terminator,
        Terminator::Branch { target, .. } if target == cont
    ));
    assert_eq!(func.blocks[&cont].ops[0].opcode, OpCode::Call);
    assert!(
        func.blocks
            .values()
            .flat_map(|block| &block.ops)
            .all(|op| !op.attrs.contains_key(CARRIED_READER_ATTR))
    );
}
