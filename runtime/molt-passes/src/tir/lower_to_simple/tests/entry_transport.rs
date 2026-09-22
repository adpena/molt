use super::super::structured::{emit_block_arg_loads, emit_block_arg_stores};
use super::*;

fn block(id: BlockId, terminator: Terminator) -> TirBlock {
    TirBlock {
        id,
        args: vec![],
        ops: vec![],
        terminator,
    }
}

#[test]
fn entry_reentry_seeds_once_and_reloads_parallel_arguments_in_both_lowering_forms() {
    for structured in [false, true] {
        let mut func = TirFunction::new(
            "entry_argument_rotation".into(),
            vec![TirType::Bool, TirType::F64, TirType::F64],
            TirType::F64,
            molt_ir::FunctionReturnAbi::Value,
        );
        func.param_names = vec!["run".into(), "left".into(), "right".into()];
        let entry = func.entry_block;
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let stop = func.fresh_value();
        let args: Vec<_> = func.blocks[&entry].args.iter().map(|arg| arg.id).collect();
        func.label_id_map.insert(entry.0, 7);
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
            cond: args[0],
            then_block: body,
            then_args: vec![],
            else_block: exit,
            else_args: vec![],
        };
        let mut body_block = block(
            body,
            Terminator::Branch {
                target: entry,
                args: vec![stop, args[2], args[1]],
            },
        );
        body_block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
            operands: vec![],
            results: vec![stop],
            attrs: AttrDict::from([
                ("value".into(), AttrValue::Bool(false)),
                ("_simple_out".into(), AttrValue::Str("stop".into())),
            ]),
            source_span: None,
        });
        func.blocks.insert(body, body_block);
        func.blocks.insert(
            exit,
            block(
                exit,
                Terminator::Return {
                    values: vec![args[1]],
                },
            ),
        );
        if structured {
            func.loop_roles.insert(entry, LoopRole::LoopHeader);
            func.loop_break_kinds
                .insert(entry, LoopBreakKind::BreakIfFalse);
        }
        let names = SimpleValueNames::for_function(&func);
        let slots = names.block_arg_slots(entry, args.len());
        let ops = lower_to_simple_ir(&func);
        assert!(validate_labels(&ops), "{ops:?}");
        let label = ops
            .iter()
            .position(|op| op.kind == "label" && op.value == Some(7))
            .unwrap();
        for (index, slot) in slots.iter().enumerate() {
            let stores: Vec<_> = ops
                .iter()
                .enumerate()
                .filter(|(_, op)| op.kind == "store_var" && op.var.as_ref() == Some(slot))
                .collect();
            assert_eq!(
                stores.len(),
                2,
                "one invocation seed and one backedge: {ops:?}"
            );
            assert!(stores[0].0 < label && stores[1].0 > label);
            assert_eq!(
                stores[0].1.args,
                Some(vec![func.param_names[index].clone()])
            );
            assert_eq!(
                stores[1].1.args,
                Some(vec![["stop", "right", "left"][index].into()])
            );
            let load = ops
                .iter()
                .position(|op| {
                    op.kind == "load_var"
                        && op.var.as_ref() == Some(slot)
                        && op.out.as_ref() == Some(&func.param_names[index])
                })
                .unwrap();
            assert!(load > label && load < stores[1].0);
            if structured {
                let loop_start = ops.iter().position(|op| op.kind == "loop_start").unwrap();
                assert!(load > loop_start, "every iteration must reload arguments");
            }
        }
        let flow = molt_ir::simple_verify::simple_ir_logical_flow(&ops);
        assert!(
            flow.edges.iter().enumerate().any(|(source, edges)| {
                edges
                    .iter()
                    .any(|edge| edge.target <= source && edge.target >= label)
            }),
            "backedge must not rerun invocation seeds: {ops:?}"
        );
    }
}

#[test]
fn implicit_invocation_prevents_consuming_entry_as_an_if_arm_or_loop_body() {
    for structured_loop in [false, true] {
        let mut func = TirFunction::new(
            "implicit_entry_predecessor".into(),
            vec![TirType::Bool],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = func.entry_block;
        let cond = func.fresh_block();
        let other = func.fresh_block();
        let flag = func.blocks[&entry].args[0].id;
        func.label_id_map.insert(entry.0, 7);
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
            target: cond,
            args: vec![],
        };
        func.blocks.insert(
            cond,
            block(
                cond,
                Terminator::CondBranch {
                    cond: flag,
                    then_block: entry,
                    then_args: vec![flag],
                    else_block: other,
                    else_args: vec![],
                },
            ),
        );
        func.blocks.insert(
            other,
            block(
                other,
                if structured_loop {
                    Terminator::Return { values: vec![] }
                } else {
                    Terminator::Branch {
                        target: cond,
                        args: vec![],
                    }
                },
            ),
        );
        if structured_loop {
            func.loop_roles.insert(cond, LoopRole::LoopHeader);
            func.loop_break_kinds
                .insert(cond, LoopBreakKind::BreakIfFalse);
        }
        let ops = lower_to_simple_ir(&func);
        assert!(validate_labels(&ops), "{ops:?}");
        assert!(
            ops.iter()
                .any(|op| op.kind == "label" && op.value == Some(7))
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op.kind.as_str(), "if" | "loop_start")),
            "entry cannot be inlined: {ops:?}"
        );
    }
}

#[test]
fn entry_join_and_structured_loop_exit_keep_explicit_backedges() {
    for structured_loop in [false, true] {
        let mut func = TirFunction::new(
            "entry_destination_edges".into(),
            vec![TirType::Bool],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = func.entry_block;
        let first = func.fresh_block();
        let second = func.fresh_block();
        let flag = func.blocks[&entry].args[0].id;
        func.label_id_map.insert(entry.0, 7);
        if structured_loop {
            func.blocks.get_mut(&entry).unwrap().terminator = Terminator::Branch {
                target: first,
                args: vec![],
            };
            func.blocks.insert(
                first,
                block(
                    first,
                    Terminator::CondBranch {
                        cond: flag,
                        then_block: second,
                        then_args: vec![],
                        else_block: entry,
                        else_args: vec![flag],
                    },
                ),
            );
            func.blocks.insert(
                second,
                block(
                    second,
                    Terminator::Branch {
                        target: first,
                        args: vec![],
                    },
                ),
            );
            func.loop_roles.insert(first, LoopRole::LoopHeader);
            func.loop_break_kinds
                .insert(first, LoopBreakKind::BreakIfFalse);
        } else {
            func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
                cond: flag,
                then_block: first,
                then_args: vec![],
                else_block: second,
                else_args: vec![],
            };
            for arm in [first, second] {
                func.blocks.insert(
                    arm,
                    block(
                        arm,
                        Terminator::Branch {
                            target: entry,
                            args: vec![flag],
                        },
                    ),
                );
            }
        }
        let ops = lower_to_simple_ir(&func);
        assert!(validate_labels(&ops), "{ops:?}");
        if structured_loop {
            let end = ops.iter().position(|op| op.kind == "loop_end").unwrap();
            assert_eq!(ops[end + 1].kind, "jump");
            assert_eq!(ops[end + 1].value, Some(7));
        } else {
            assert!(!ops.iter().any(|op| op.kind == "if"));
            assert_eq!(
                ops.iter()
                    .filter(|op| op.kind == "jump" && op.value == Some(7))
                    .count(),
                2,
                "both arms must transfer to entry: {ops:?}"
            );
        }
    }
}

#[test]
fn inlined_if_arms_preserve_incoming_and_join_argument_transport() {
    let mut func = TirFunction::new(
        "inline_argument_edges".into(),
        vec![TirType::Bool, TirType::F64, TirType::F64],
        TirType::F64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = func.entry_block;
    let left = func.fresh_block();
    let right = func.fresh_block();
    let join = func.fresh_block();
    let left_value = func.fresh_value();
    let right_value = func.fresh_value();
    let result = func.fresh_value();
    let args: Vec<_> = func.blocks[&entry].args.iter().map(|arg| arg.id).collect();
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond: args[0],
        then_block: left,
        then_args: vec![args[1]],
        else_block: right,
        else_args: vec![args[2]],
    };
    for (id, value) in [(left, left_value), (right, right_value)] {
        let mut arm = block(
            id,
            Terminator::Branch {
                target: join,
                args: vec![value],
            },
        );
        arm.args.push(TirValue {
            id: value,
            ty: TirType::F64,
        });
        func.blocks.insert(id, arm);
    }
    let mut join_block = block(
        join,
        Terminator::Return {
            values: vec![result],
        },
    );
    join_block.args.push(TirValue {
        id: result,
        ty: TirType::F64,
    });
    func.blocks.insert(join, join_block);
    let names = SimpleValueNames::for_function(&func);
    let ops = lower_to_simple_ir(&func);
    let if_index = ops.iter().position(|op| op.kind == "if").unwrap();
    let else_index = ops.iter().position(|op| op.kind == "else").unwrap();
    let end_index = ops.iter().position(|op| op.kind == "end_if").unwrap();
    for (arm, incoming, value, start, end) in [
        (left, args[1], left_value, if_index, else_index),
        (right, args[2], right_value, else_index, end_index),
    ] {
        let emitted = &ops[start + 1..end];
        assert_eq!(emitted.len(), 3, "store, load, join store: {ops:?}");
        assert_eq!(emitted[0].var, Some(names.block_arg_slot(arm, 0)));
        assert_eq!(emitted[0].args, Some(vec![names.value_name(incoming)]));
        assert_eq!(emitted[1].kind, "load_var");
        assert_eq!(emitted[1].out, Some(names.value_name(value)));
        assert_eq!(emitted[2].var, Some(names.block_arg_slot(join, 0)));
        assert_eq!(emitted[2].args, Some(vec![names.value_name(value)]));
    }
}

#[test]
fn block_argument_transport_rejects_missing_short_and_excess_vectors_before_emission() {
    let target = BlockId(3);
    let value = ValueId(0);
    let mut target_block = block(target, Terminator::Return { values: vec![] });
    target_block.args.push(TirValue {
        id: value,
        ty: TirType::F64,
    });
    let slots = HashMap::from([(target, vec!["slot".to_string()])]);
    for args in [vec![], vec![value, value]] {
        let mut out = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_block_arg_stores(target, &args, &slots, &mut out);
        }));
        assert!(
            result.is_err(),
            "invalid edge arity must not use stale slots"
        );
        assert!(out.is_empty(), "invalid transport must not partially emit");
    }
    for invalid_slots in [
        HashMap::new(),
        HashMap::from([(target, vec![])]),
        HashMap::from([(target, vec!["first".to_string(), "second".to_string()])]),
    ] {
        let mut out = Vec::new();
        let stores = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_block_arg_stores(target, &[value], &invalid_slots, &mut out);
        }));
        assert!(stores.is_err());
        assert!(out.is_empty());
        let loads = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_block_arg_loads(&target_block, &invalid_slots, &mut out);
        }));
        assert!(loads.is_err());
        assert!(out.is_empty());
    }
}

#[test]
#[should_panic(expected = "invalid label lowering")]
fn missing_label_after_lowering_is_never_an_optional_warning() {
    let mut func = TirFunction::new(
        "missing_destination".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    func.blocks
        .get_mut(&func.entry_block)
        .unwrap()
        .ops
        .push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::from([("value".into(), AttrValue::Int(999))]),
            source_span: None,
        });
    lower_to_simple_ir(&func);
}
