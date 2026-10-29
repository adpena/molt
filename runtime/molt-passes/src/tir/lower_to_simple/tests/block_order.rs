use super::super::cfg::lowering_block_order;
use super::*;
use molt_ir::simple_verify::{EdgeRole, simple_ir_executable_flow};

fn block(id: BlockId, ops: Vec<TirOp>, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops,
        terminator,
    }
}

fn operation(opcode: OpCode, operands: Vec<ValueId>, value: Option<i64>) -> TirOp {
    TirOp {
        origin: None,
        dialect: Dialect::Molt,
        opcode,
        operands,
        results: vec![],
        attrs: value
            .map(|value| AttrDict::from([("value".into(), AttrValue::Int(value))]))
            .unwrap_or_default(),
        source_span: None,
    }
}

fn branch(target: BlockId) -> Terminator {
    Terminator::Branch {
        target,
        args: vec![],
    }
}

#[test]
fn suffix_first_exception_cleanup_is_forward_and_preserves_each_release_route() {
    let mut func = TirFunction::new(
        "cleanup_dag".into(),
        vec![TirType::DynBox; 3],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.param_names = vec!["left".into(), "shared".into(), "right".into()];
    func.has_exception_handling = true;
    let entry = func.entry_block;
    // Match the ownership producer's suffix-first allocation, independently of
    // the desired execution order: both prefixes release into shared -> handler.
    let handler = func.fresh_block();
    let shared = func.fresh_block();
    let left = func.fresh_block();
    let right = func.fresh_block();
    let normal = func.fresh_block();
    for (id, label) in [
        (handler, 100),
        (shared, 200),
        (left, 300),
        (right, 400),
        (normal, 500),
    ] {
        func.label_id_map.insert(id.0, label);
    }
    let values: Vec<_> = func.blocks[&entry].args.iter().map(|arg| arg.id).collect();
    let entry_block = func.blocks.get_mut(&entry).unwrap();
    entry_block.ops = vec![
        operation(OpCode::CheckException, vec![], Some(300)),
        operation(OpCode::CheckException, vec![], Some(400)),
    ];
    entry_block.terminator = branch(normal);
    for (id, value, target) in [
        (shared, values[1], handler),
        (left, values[0], shared),
        (right, values[2], shared),
    ] {
        func.blocks.insert(
            id,
            block(
                id,
                vec![operation(OpCode::DecRef, vec![value], None)],
                branch(target),
            ),
        );
    }
    for id in [handler, normal] {
        func.blocks
            .insert(id, block(id, vec![], Terminator::Return { values: vec![] }));
    }

    let order = lowering_block_order(&func, &HashMap::new());
    let position = |id| order.iter().position(|&block| block == id).unwrap();
    for (source, target) in [
        (entry, left),
        (entry, right),
        (left, shared),
        (right, shared),
        (shared, handler),
        (entry, normal),
    ] {
        assert!(position(source) < position(target), "{order:?}");
    }
    let ops = lower_to_simple_ir(&func);
    molt_ir::ir_schema::validate_normalized_simple_ir_ops(&func.name, &ops).unwrap();
    assert!(validate_labels(&ops));
    let flow = simple_ir_executable_flow(&ops);
    assert!(
        flow.edges
            .iter()
            .flatten()
            .all(|edge| edge.target > edge.source),
        "cleanup DAG gained a backward transfer: {ops:?}"
    );

    // Follow each actual exceptional edge, checking literal expected release
    // names and the final handler, rather than comparing against another sort.
    for (label, expected) in [(300, ["left", "shared"]), (400, ["right", "shared"])] {
        let observation = ops
            .iter()
            .position(|op| op.kind == molt_ir::OpKind::CheckException && op.value == Some(label))
            .unwrap();
        let mut cursor = flow.edges[observation]
            .iter()
            .find(|edge| edge.role == EdgeRole::Exception)
            .unwrap()
            .target;
        let mut releases = Vec::new();
        let mut last_label = None;
        for _ in 0..ops.len() {
            let op = &ops[cursor];
            if op.kind == molt_ir::OpKind::Label {
                last_label = op.value;
            } else if op.kind == molt_ir::OpKind::DecRef {
                releases.push(op.args.as_ref().unwrap()[0].as_str());
            } else if op.kind == molt_ir::OpKind::RetVoid {
                break;
            }
            assert_eq!(
                flow.edges[cursor].len(),
                1,
                "cleanup route must be unambiguous: {op:?}"
            );
            cursor = flow.edges[cursor][0].target;
        }
        assert_eq!(ops[cursor].kind, molt_ir::OpKind::RetVoid);
        assert_eq!(last_label, Some(100));
        assert_eq!(releases, expected);
    }
}

#[test]
fn ordinary_cycle_keeps_its_real_backward_transfer() {
    let mut func = TirFunction::new(
        "cycle".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let entry = func.entry_block;
    let body = func.fresh_block();
    func.label_id_map.insert(entry.0, 10);
    func.label_id_map.insert(body.0, 20);
    func.blocks.get_mut(&entry).unwrap().terminator = branch(body);
    func.blocks.insert(body, block(body, vec![], branch(entry)));
    assert_eq!(
        lowering_block_order(&func, &HashMap::new()),
        vec![entry, body]
    );
    let ops = lower_to_simple_ir(&func);
    molt_ir::ir_schema::validate_normalized_simple_ir_ops(&func.name, &ops).unwrap();
    let entry_label = ops
        .iter()
        .position(|op| op.kind == molt_ir::OpKind::Label && op.value == Some(10))
        .unwrap();
    let flow = simple_ir_executable_flow(&ops);
    assert!(
        flow.edges
            .iter()
            .flatten()
            .any(|edge| edge.target == entry_label && edge.source > edge.target),
        "real cycle disappeared: {ops:?}"
    );
}

fn suspension_chain(cyclic: bool) -> (TirFunction, Vec<BlockId>, HashMap<BlockId, BlockId>) {
    let mut func = TirFunction::new(
        "suspension_chain".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    func.param_names = vec!["payload".into()];
    let entry = func.entry_block;
    let initial = func.fresh_block();
    let metadata = func.fresh_block();
    let first = func.fresh_block();
    let second = func.fresh_block();
    let final_resume = if cyclic { first } else { func.fresh_block() };
    for (id, label) in [(initial, 40), (metadata, 50), (first, 91), (second, 72)] {
        func.label_id_map.insert(id.0, label);
    }
    if !cyclic {
        func.label_id_map.insert(final_resume.0, 133);
    }
    let payload = func.blocks[&entry].args[0].id;
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![
            (5, first, vec![]),
            (9, second, vec![]),
            (12, final_resume, vec![]),
        ],
        default: initial,
        default_args: vec![],
    };
    for (id, state) in [(initial, 5), (first, 9), (second, 12)] {
        func.blocks.insert(
            id,
            block(
                id,
                vec![operation(OpCode::StateYield, vec![payload], Some(state))],
                Terminator::Unreachable,
            ),
        );
    }
    if !cyclic {
        func.blocks.insert(
            final_resume,
            block(
                final_resume,
                vec![],
                Terminator::Return {
                    values: vec![payload],
                },
            ),
        );
    }
    func.blocks.insert(
        metadata,
        block(metadata, vec![], Terminator::Return { values: vec![] }),
    );
    let mut expected = vec![entry, initial, first, second];
    if !cyclic {
        expected.push(final_resume);
    }
    expected.push(metadata);
    (
        func,
        expected,
        HashMap::from([(initial, first), (first, second), (second, final_resume)]),
    )
}

#[test]
fn three_suspensions_keep_resume_adjacency_default_and_metadata() {
    let (func, expected, resumes) = suspension_chain(false);
    assert_eq!(lowering_block_order(&func, &resumes), expected);
    let ops = lower_to_simple_ir(&func);
    molt_ir::ir_schema::validate_normalized_simple_ir_ops(&func.name, &ops).unwrap();
    molt_ir::ir_schema::validate_state_dispatch(&ops).unwrap();
    let switch = ops
        .iter()
        .position(|op| op.kind == molt_ir::OpKind::StateSwitch)
        .unwrap();
    assert_eq!(
        ops[switch].state_targets.as_deref(),
        Some([(5, 91), (9, 72), (12, 133)].as_slice())
    );
    assert_eq!(ops[switch + 1].kind, molt_ir::OpKind::Jump);
    assert_eq!(ops[switch + 1].value, Some(40));
    for (state, label) in [(5, 91), (9, 72), (12, 133)] {
        let suspension = ops
            .iter()
            .position(|op| op.kind == molt_ir::OpKind::StateYield && op.value == Some(state))
            .unwrap();
        assert_eq!(ops[suspension + 1].kind, molt_ir::OpKind::StateLabel);
        assert_eq!(ops[suspension + 1].value, Some(label));
    }
}

#[test]
fn shared_cyclic_resume_targets_keep_explicit_identity_without_duplicate_blocks() {
    let (func, expected, resumes) = suspension_chain(true);
    let order = lowering_block_order(&func, &resumes);
    assert_eq!(order, expected);
    assert_eq!(
        order.iter().copied().collect::<HashSet<_>>().len(),
        func.blocks.len()
    );
    let ops = lower_to_simple_ir(&func);
    molt_ir::ir_schema::validate_state_dispatch(&ops).unwrap();
    let targets = ops
        .iter()
        .find(|op| op.kind == molt_ir::OpKind::StateSwitch)
        .unwrap()
        .state_targets
        .as_deref()
        .unwrap();
    assert_eq!(targets, [(5, 91), (9, 72), (12, 91)]);
    for label in [91, 72] {
        assert_eq!(
            ops.iter()
                .filter(|op| op.kind == molt_ir::OpKind::StateLabel && op.value == Some(label))
                .count(),
            1
        );
    }
}
