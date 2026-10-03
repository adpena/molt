use super::*;

fn set_state(mut op: TirOp, state: i64) -> TirOp {
    op.attrs.insert("value".into(), AttrValue::Int(state));
    op
}

fn activation() -> (TirFunction, BlockId, BlockId, ValueId) {
    let mut func = TirFunction::new(
        "activation_owners".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let frame = func.blocks[&func.entry_block].args[0].id;
    let first = func.fresh_block();
    let resume = func.fresh_block();
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(9, resume, vec![])],
        default: first,
        default_args: vec![],
    };
    for id in [first, resume] {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    (func, first, resume, frame)
}

fn fresh(func: &mut TirFunction, ty: TirType) -> ValueId {
    let id = func.fresh_value();
    func.value_types.insert(id, ty);
    id
}

#[test]
fn activation_dispatch_transport_preserves_alias_states_and_rejects_bad_targets() {
    use molt_ir::OpIR;
    let (mut func, first, resume, _) = activation();
    func.label_id_map.insert(resume.0, 71);
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(9, resume, vec![]), (4001, resume, vec![])],
        default: first,
        default_args: vec![],
    };
    let ops = molt_passes::tir::lower_to_simple::lower_to_simple_ir(&func);
    let switch = ops.iter().find(|op| op.kind == "state_switch").unwrap();
    assert_eq!(switch.state_targets, Some(vec![(9, 71), (4001, 71)]));
    let encoded = serde_json::to_string(switch).unwrap();
    assert_eq!(serde_json::from_str::<OpIR>(&encoded).unwrap(), *switch);
    let cfg = molt_passes::tir::cfg::CFG::build(&ops);
    assert_eq!(cfg.state_resume_edges.len(), 2);
    assert_eq!(cfg.state_resume_edges[0].1, cfg.state_resume_edges[1].1);
    for (kind, targets, expected) in [
        ("state_switch", vec![(9, 999)], "requires one control label"),
        (
            "state_switch",
            vec![(9, 71), (9, 71)],
            "duplicate saved state",
        ),
        ("line", vec![], "state_targets requires state_switch"),
    ] {
        let mut malformed = ops.clone();
        let switch = malformed
            .iter_mut()
            .find(|op| op.kind == "state_switch")
            .unwrap();
        switch.kind = kind.into();
        switch.state_targets = Some(targets);
        assert!(
            molt_ir::ir_schema::validate_state_dispatch(&malformed)
                .unwrap_err()
                .contains(expected)
        );
    }
}

#[test]
fn activation_yield_transfers_pair_and_releases_element_owner_before_suspending() {
    let (mut func, first, resume, frame) = activation();
    let element = fresh(&mut func, TirType::DynBox);
    let pair = fresh(&mut func, TirType::Tuple(vec![TirType::DynBox]));
    let reloaded = fresh(&mut func, TirType::DynBox);
    func.blocks.get_mut(&first).unwrap().ops = vec![
        op(OpCode::Call, vec![], vec![element]),
        set_state(op(OpCode::ClosureStore, vec![frame, element], vec![]), 24),
        op(OpCode::BuildTuple, vec![element], vec![pair]),
        set_state(op(OpCode::StateYield, vec![pair], vec![]), 9),
    ];
    func.blocks.get_mut(&first).unwrap().terminator = Terminator::Unreachable;
    func.blocks.get_mut(&resume).unwrap().ops = vec![set_state(
        op(OpCode::ClosureLoad, vec![frame], vec![reloaded]),
        24,
    )];
    func.blocks.get_mut(&resume).unwrap().terminator = Terminator::Return {
        values: vec![reloaded],
    };
    run(&mut func, &mut AnalysisManager::new());
    assert_eq!(func.attrs[DROP_INSERTED_ATTR], AttrValue::Bool(true));
    let first = &func.blocks[&first];
    assert!(matches!(&first.terminator, Terminator::Return { values } if values == &[pair]));
    assert_eq!(
        first
            .ops
            .iter()
            .filter(|op| op.opcode == OpCode::DecRef && op.operands == [element])
            .count(),
        1
    );
    assert!(!first.ops.iter().any(|op| matches!(op.opcode, OpCode::IncRef | OpCode::DecRef) && op.operands == [pair]));
    assert!(
        !func.blocks[&resume]
            .ops
            .iter()
            .any(|op| matches!(op.opcode, OpCode::IncRef | OpCode::DecRef)
                && op.operands == [reloaded])
    );
    assert!(func.blocks.values().all(|block| {
        block
            .ops
            .iter()
            .all(|op| !matches!(op.opcode, OpCode::StateYield | OpCode::StateTransition))
    }));
    let simple = molt_passes::tir::lower_to_simple::lower_to_simple_ir(&func);
    let targets = simple
        .iter()
        .find_map(|op| op.state_targets.as_ref())
        .expect("typed dispatch map survives lowering");
    let label = targets.iter().find(|(state, _)| *state == 9).unwrap().1;
    let resume_index = simple
        .iter()
        .position(|op| op.kind == "state_label" && op.value == Some(label))
        .expect("resume remains a declared target after yield becomes Return");
    let cfg = molt_passes::tir::cfg::CFG::build(&simple);
    assert!(
        cfg.state_resume_edges
            .iter()
            .any(|&(_, target, state)| state == 9
                && cfg.blocks[target].start_op <= resume_index
                && resume_index < cfg.blocks[target].end_op)
    );
    let before = molt_passes::tir::printer::print_function(&func);
    run(&mut func, &mut AnalysisManager::new());
    assert_eq!(
        molt_passes::tir::printer::print_function(&func),
        before,
        "activation ownership is idempotent"
    );
}

#[test]
fn activation_wait_pending_and_ready_paths_both_release_invocation_future() {
    let (mut func, first, resume, frame) = activation();
    // Both first entry and resumption reach the same poll site; every invocation
    // reloads an owned reference from the persistent frame.
    func.blocks.get_mut(&first).unwrap().terminator = Terminator::Branch {
        target: resume,
        args: vec![],
    };
    let future = fresh(&mut func, TirType::DynBox);
    let pending = fresh(&mut func, TirType::I64);
    let result = fresh(&mut func, TirType::DynBox);
    let block = func.blocks.get_mut(&resume).unwrap();
    block.ops = vec![
        set_state(op(OpCode::ClosureLoad, vec![frame], vec![future]), 24),
        set_state(op(OpCode::ConstInt, vec![], vec![pending]), 9),
        set_state(
            op(OpCode::StateTransition, vec![future, pending], vec![result]),
            10,
        ),
    ];
    block.terminator = Terminator::Return {
        values: vec![result],
    };
    run(&mut func, &mut AnalysisManager::new());
    let poll = &func.blocks[&resume];
    let Terminator::CondBranch {
        then_block,
        else_block,
        ..
    } = poll.terminator
    else {
        panic!("poll must expose both exits")
    };
    for (target, waiting) in [(then_block, true), (else_block, false)] {
        let block = &func.blocks[&target];
        assert!(matches!(&block.terminator, Terminator::Return { values } if values == &[result]));
        let releases: Vec<_> = block
            .ops
            .iter()
            .enumerate()
            .filter(|(_, op)| op.opcode == OpCode::DecRef && op.operands == [future])
            .collect();
        assert_eq!(releases.len(), 1, "one invocation owner on each path");
        if waiting {
            let wait = block
                .ops
                .iter()
                .position(|op| op.opcode == OpCode::TaskWait)
                .unwrap();
            assert!(wait < releases[0].0, "registration borrows a live future");
        }
        assert!(
            !block
                .ops
                .iter()
                .any(|op| matches!(op.opcode, OpCode::IncRef | OpCode::DecRef)
                    && op.operands == [result]),
            "poll result is transferred exactly once"
        );
    }
}
