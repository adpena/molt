use super::*;
use std::collections::HashMap;

fn check(label: i64) -> TirOp {
    let mut op = op(OpCode::CheckException, vec![], vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(label));
    op
}

// Follow one exceptional path, resolving actual edge arguments. This observes
// release behavior, not a prescribed number/name/layout of landing blocks.
fn released_on_exception(func: &TirFunction, check: &TirOp) -> Vec<ValueId> {
    let labels: HashMap<_, _> = func
        .label_id_map
        .iter()
        .map(|(&block, &label)| (label, BlockId(block)))
        .collect();
    let AttrValue::Int(label) = check.attrs["value"] else {
        panic!("missing target")
    };
    let mut block = labels[&label];
    let mut incoming = check.operands.clone();
    let mut released = Vec::new();
    for _ in 0..func.blocks.len() {
        let body = &func.blocks[&block];
        let bound: HashMap<_, _> = body
            .args
            .iter()
            .map(|arg| arg.id)
            .zip(incoming.iter().copied())
            .collect();
        let resolve = |value: ValueId| bound.get(&value).copied().unwrap_or(value);
        for op in &body.ops {
            if op.opcode == OpCode::DecRef {
                released.extend(op.operands.iter().copied().map(resolve));
            }
        }
        match &body.terminator {
            Terminator::Branch { target, args } => {
                incoming = args.iter().copied().map(resolve).collect();
                block = *target;
            }
            Terminator::Return { .. } => return released,
            other => panic!("unexpected exceptional path {other:?}"),
        }
    }
    panic!("exceptional cleanup must terminate")
}

#[test]
fn exception_edges_release_only_prepared_unconsumed_owners() {
    let mut func = TirFunction::new(
        "argument_failure_ownership".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let callee = func.blocks[&func.entry_block].args[0].id;
    let builder = func.fresh_value();
    let result = func.fresh_value();
    for value in [builder, result] {
        func.value_types.insert(value, TirType::DynBox);
    }
    let exit = func.fresh_block();
    func.label_id_map.insert(exit.0, 77);
    let mut call = op(OpCode::Call, vec![callee, builder], vec![result]);
    call.attrs
        .insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![
        check(77),
        original_copy("callargs_new", vec![builder]),
        check(77),
        check(77),
        call,
        check(77),
    ];
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    run(&mut func, &mut AnalysisManager::new());
    let checks: Vec<_> = func.blocks[&func.entry_block]
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::CheckException)
        .collect();
    assert_eq!(released_on_exception(&func, checks[0]), vec![]);
    assert_eq!(released_on_exception(&func, checks[1]), vec![builder]);
    assert_eq!(released_on_exception(&func, checks[2]), vec![builder]);
    assert_eq!(released_on_exception(&func, checks[3]), vec![result]);
    assert_eq!(
        checks[1].attrs["value"], checks[2].attrs["value"],
        "identical unwind states share one cleanup"
    );
    assert!(
        func.blocks[&func.entry_block]
            .ops
            .iter()
            .all(|op| op.opcode != OpCode::DecRef || !op.operands.contains(&builder)),
        "the successful call consumes the builder exactly once"
    );
}

#[test]
fn handler_live_owner_survives_normal_only_temporary_cleanup() {
    let mut func = TirFunction::new(
        "handler_keepalive".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let owner = func.fresh_value();
    let temporary = func.fresh_value();
    for value in [owner, temporary] {
        func.value_types.insert(value, TirType::DynBox);
    }
    let handler = func.fresh_block();
    func.label_id_map.insert(handler.0, 81);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![
        op(OpCode::Call, vec![], vec![owner]),
        op(OpCode::Call, vec![], vec![temporary]),
        check(81),
        op(OpCode::Call, vec![temporary], vec![]),
    ];
    entry.terminator = Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![owner], vec![])],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    run(&mut func, &mut AnalysisManager::new());
    let entry = &func.blocks[&func.entry_block];
    let at = entry
        .ops
        .iter()
        .position(|op| op.opcode == OpCode::CheckException)
        .unwrap();
    assert!(
        entry.ops[..at]
            .iter()
            .all(|op| op.opcode != OpCode::DecRef || !op.operands.contains(&owner))
    );
    assert_eq!(
        released_on_exception(&func, &entry.ops[at]),
        vec![temporary, owner]
    );
}

#[test]
fn owner_created_in_handler_is_released_on_later_exception() {
    let mut func = TirFunction::new(
        "handler_argument_failure".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let callee = func.blocks[&func.entry_block].args[0].id;
    let handler = func.fresh_block();
    let continuation = func.fresh_block();
    let exit = func.fresh_block();
    let builder = func.fresh_value();
    func.value_types.insert(builder, TirType::DynBox);
    func.label_id_map.insert(handler.0, 80);
    func.label_id_map.insert(exit.0, 81);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops = vec![check(80)];
    entry.terminator = Terminator::Return { values: vec![] };
    func.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![],
            ops: vec![original_copy("callargs_new", vec![builder])],
            terminator: Terminator::Branch {
                target: continuation,
                args: vec![],
            },
        },
    );
    let mut consume = op(OpCode::Call, vec![callee, builder], vec![]);
    consume
        .attrs
        .insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
    func.blocks.insert(
        continuation,
        TirBlock {
            id: continuation,
            args: vec![],
            ops: vec![check(81), consume],
            terminator: Terminator::Return { values: vec![] },
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
    run(&mut func, &mut AnalysisManager::new());
    let check = func.blocks[&continuation]
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::CheckException)
        .unwrap();
    assert_eq!(released_on_exception(&func, check), vec![builder]);
}
#[test]
fn fallible_operation_cleanup_belongs_to_its_observed_continuation() {
    for separate_observation_block in [false, true] {
        for discarded_result in [false, true] {
            let mut func = TirFunction::new(
                "observed_operation_cleanup".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let older = func.fresh_value();
            let younger = func.fresh_value();
            let result = func.fresh_value();
            for value in [older, younger, result] {
                func.value_types.insert(value, TirType::DynBox);
            }
            let handler = func.fresh_block();
            let continuation = func.fresh_block();
            func.label_id_map.insert(handler.0, 97);
            let entry = func.entry_block;
            let mut ops = vec![
                op(OpCode::Call, vec![], vec![older]),
                op(OpCode::Call, vec![], vec![younger]),
                op(
                    OpCode::Call,
                    vec![older],
                    if discarded_result {
                        vec![result]
                    } else {
                        vec![]
                    },
                ),
            ];
            if !separate_observation_block {
                ops.push(check(97));
            }
            func.blocks.get_mut(&entry).unwrap().ops = ops;
            func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
                target: continuation,
                args: vec![],
            };
            let mut ops = vec![];
            if separate_observation_block {
                ops.push(check(97));
            }
            ops.push(op(OpCode::Call, vec![younger], vec![]));
            func.blocks.insert(
                continuation,
                TirBlock {
                    id: continuation,
                    args: vec![],
                    ops,
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            run(&mut func, &mut AnalysisManager::new());
            let success = super::execute(&func, 0, &[]).unwrap();
            let failure = super::execute(&func, 0, &[true]).unwrap();
            let successful_order = if discarded_result {
                vec![0, 2, 1]
            } else {
                vec![0, 1]
            };
            let exceptional_order = if discarded_result {
                vec![2, 1, 0]
            } else {
                vec![1, 0]
            };
            assert_eq!(
                success,
                successful_order
                    .into_iter()
                    .map(super::Event::Freed)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                failure,
                exceptional_order
                    .into_iter()
                    .map(super::Event::Freed)
                    .collect::<Vec<_>>()
            );
        }
    }
}
