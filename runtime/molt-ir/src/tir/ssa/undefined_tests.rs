use super::tests::{op, op_args, op_args_out, op_val, op_val_out};
use super::*;

#[test]
fn exception_self_edges_transport_updated_values_at_the_transfer() {
    for kind in ["check_exception", "async_work_poll", "try_start"] {
        let ops = vec![
            op_val_out("const", 10, "x"),
            op_val("label", 7),
            op_args_out("add", &["x", "x"], "x"),
            op_val(kind, 7),
            // This definition is on the normal continuation only. The
            // exception backedge must carry the Add result, never this value.
            op_val_out("const", 99, "x"),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let loop_block = output
            .blocks
            .iter()
            .find(|block| block.ops.iter().any(|op| op.opcode == OpCode::Add))
            .expect("loop block");
        assert_eq!(loop_block.args.len(), 1, "{kind}: loop-carried x");
        let phi = loop_block.args[0].id;
        let add = loop_block
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Add)
            .unwrap();
        assert_eq!(add.operands, vec![phi, phi], "{kind}");
        let transfer = loop_block.ops.last().unwrap();
        assert_eq!(transfer.operands, add.results, "{kind}: updated backedge x");
        let entry = &output.blocks[cfg.entry];
        let initial = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ConstInt)
            .unwrap()
            .results[0];
        assert!(
            matches!(&entry.terminator, Terminator::Branch { target, args }
            if *target == loop_block.id && args == &vec![initial]),
            "{kind}: initial x"
        );
        assert_ne!(
            phi, initial,
            "{kind}: first iteration cannot replace the loop phi"
        );
        assert_no_placeholder(&output);
    }
}

#[test]
fn exception_self_edge_to_entry_preserves_parameter_order_and_updated_values() {
    for kind in ["check_exception", "async_work_poll", "try_start", "br_if"] {
        let ops = vec![
            op_val("label", 7),
            op_args_out("sub", &["z", "a"], "z"),
            OpIR {
                args: (kind == "br_if").then(|| vec!["a".into()]),
                ..op_val(kind, 7)
            },
            op_args("ret", &["z"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa_with_params(&cfg, &ops, &["z".into(), "a".into()]);
        let entry = &output.blocks[cfg.entry];
        assert_eq!(entry.args.len(), 2, "{kind}");
        let Terminator::Branch { target, args } = &entry.terminator else {
            panic!("{kind}: invocation must enter the re-entrant source block");
        };
        let body = &output.blocks[target.0 as usize];
        assert_ne!(body.id, entry.id);
        // The internal join order is canonical, while the invocation order is
        // the authored signature (z, a). Map the edge, never infer the order.
        let incoming: HashMap<_, _> = body
            .args
            .iter()
            .zip(args)
            .map(|(arg, &value)| (arg.id, value))
            .collect();
        let sub = body.ops.iter().find(|op| op.opcode == OpCode::Sub).unwrap();
        assert_eq!(
            sub.operands
                .iter()
                // An unchanged regular-edge input can use its dominating
                // invocation definition directly; it need not acquire a phi.
                .map(|id| incoming.get(id).copied().unwrap_or(*id))
                .collect::<Vec<_>>(),
            vec![entry.args[0].id, entry.args[1].id],
            "{kind}"
        );
        let expected: Vec<_> = body
            .args
            .iter()
            .map(|arg| {
                if incoming[&arg.id] == entry.args[0].id {
                    sub.results[0]
                } else {
                    arg.id
                }
            })
            .collect();
        if kind == "br_if" {
            assert!(
                matches!(&body.terminator, Terminator::CondBranch { then_block, then_args, .. }
                if *then_block == body.id && *then_args == expected),
                "{kind}"
            );
        } else {
            let transfer = body.ops.last().unwrap();
            assert_eq!(transfer.operands, expected, "{kind}");
        }
        assert_no_placeholder(&output);
    }
}

#[test]
fn reentry_local_is_seeded_by_invocation_without_expanding_the_signature() {
    for kind in ["check_exception", "async_work_poll", "try_start", "jump"] {
        let ops = vec![
            op_val("label", 7),
            op_args_out("add", &["x", "x"], "x"),
            op_val(kind, 7),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        let entry = &output.blocks[cfg.entry];
        assert!(
            entry.args.is_empty(),
            "{kind}: x is not a callable parameter"
        );
        let seed = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ConstNone)
            .expect("undefined invocation seed")
            .results[0];
        let Terminator::Branch { target, args } = &entry.terminator else {
            panic!("invocation branch")
        };
        assert_eq!(*args, vec![seed], "{kind}");
        let body = &output.blocks[target.0 as usize];
        assert_eq!(body.args.len(), 1);
        let add = body.ops.iter().find(|op| op.opcode == OpCode::Add).unwrap();
        assert_eq!(add.operands, vec![body.args[0].id; 2], "{kind}");
        if kind == "jump" {
            assert!(
                matches!(&body.terminator, Terminator::Branch { target, args }
                if *target == body.id && *args == add.results),
                "{kind}"
            );
        } else {
            assert_eq!(body.ops.last().unwrap().operands, add.results, "{kind}");
        }
        assert_no_placeholder(&output);
    }
}

fn assert_no_placeholder(output: &SsaOutput) {
    // SSA reserves zero solely for construction and does not publish its type.
    assert!(!output.types.contains_key(&ValueId(0)));
    for block in &output.blocks {
        assert!(block.args.iter().all(|arg| arg.id != ValueId(0)));
        for op in &block.ops {
            assert!(!op.operands.contains(&ValueId(0)));
            assert!(!op.results.contains(&ValueId(0)));
        }
        block
            .terminator
            .for_each_value(|value| assert_ne!(value, ValueId(0)));
    }
}

#[test]
fn checked_entry_does_not_materialize_unused_none() {
    let ops = vec![
        op_val("trace_enter_slot", 0),
        op_val("check_exception", 1),
        op_val_out("const", 3, "result"),
        op_args("ret", &["result"]),
        op_val("label", 1),
        op("ret_void"),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[cfg.entry];
    assert_eq!(
        entry.ops[0].attrs.get("_original_kind"),
        Some(&AttrValue::Str("trace_enter_slot".into()))
    );
    assert_eq!(entry.ops[1].opcode, OpCode::CheckException);
    assert!(
        output
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .all(|op| op.opcode != OpCode::ConstNone)
    );
    assert_no_placeholder(&output);
}

#[test]
fn checked_entry_keeps_later_missing_uses_in_the_success_continuation() {
    let ops = vec![
        op_val("trace_enter_slot", 0),
        op_val("check_exception", 1),
        op_args_out("add", &["missing", "missing"], "sum"),
        op_args("ret", &["sum"]),
        op_val("label", 1),
        op("ret_void"),
        op_val("label", 2),
        op_val_out("const", 4, "missing"),
        op("ret_void"),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[cfg.entry];
    assert_eq!(
        entry.ops[0].attrs.get("_original_kind"),
        Some(&AttrValue::Str("trace_enter_slot".into()))
    );
    assert_eq!(entry.ops[1].opcode, OpCode::CheckException);
    assert!(entry.ops.iter().all(|op| op.opcode != OpCode::ConstNone));
    let continuation = output
        .blocks
        .iter()
        .find(|block| block.ops.iter().any(|op| op.opcode == OpCode::Add))
        .expect("success continuation");
    assert_ne!(continuation.id, entry.id);
    assert_eq!(continuation.ops[0].opcode, OpCode::ConstNone);
    assert_eq!(continuation.ops[1].opcode, OpCode::Add);
    assert_eq!(
        continuation.ops[1].operands,
        vec![continuation.ops[0].results[0]; 2]
    );
    assert_no_placeholder(&output);
}

#[test]
fn later_exception_edge_materializes_its_missing_environment_locally() {
    let ops = vec![
        op_val("trace_enter_slot", 0),
        op_val("check_exception", 1),
        op_val("check_exception", 3),
        op("ret_void"),
        op_val("label", 1),
        op("ret_void"),
        op_val("label", 3),
        op_args("ret", &["missing"]),
        op_val("label", 4),
        op_val_out("const", 4, "missing"),
        op("ret_void"),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    assert!(
        output.blocks[cfg.entry]
            .ops
            .iter()
            .all(|op| op.opcode != OpCode::ConstNone)
    );
    let block = output
        .blocks
        .iter()
        .find(|block| {
            block
                .ops
                .iter()
                .any(|op| op.opcode == OpCode::CheckException && !op.operands.is_empty())
        })
        .expect("exception edge with a missing handler argument");
    let check_index = block
        .ops
        .iter()
        .position(|op| op.opcode == OpCode::CheckException)
        .unwrap();
    assert!(check_index > 0);
    let definition = &block.ops[check_index - 1];
    assert_eq!(definition.opcode, OpCode::ConstNone);
    assert_eq!(block.ops[check_index].operands, definition.results);
    assert_no_placeholder(&output);
}

#[test]
fn transfer_family_keeps_point_specific_handler_values_and_join_arguments() {
    for kind in ["check_exception", "async_work_poll", "try_start"] {
        let ops = vec![
            op_val_out("const", 1, "x"),
            op_val(kind, 100),
            op_val_out("const", 2, "x"),
            op_val(kind, 100),
            op_val_out("const", 3, "x"),
            op_val("jump", 200),
            op_val("label", 100),
            op_val("jump", 200),
            op_val("label", 200),
            op_args("ret", &["x"]),
        ];
        let cfg = CFG::build(&ops);
        let bid_at = |index| {
            cfg.blocks
                .iter()
                .position(|block| block.start_op <= index && index < block.end_op)
                .unwrap()
        };
        let first = bid_at(1);
        let second = bid_at(3);
        let success = bid_at(4);
        let handler = bid_at(6);
        let join = bid_at(8);
        let mut context = SsaContext::new("transfer_snapshots", &cfg, &ops, &[]);
        context.run();
        assert_ne!(first, second, "{kind}: distinct transfer program points");
        assert_ne!(
            second, success,
            "{kind}: definitions follow the later transfer"
        );
        assert_eq!(
            context.aug_dominators[handler],
            Some(first),
            "{kind}: later definitions must not dominate the handler"
        );
        assert_eq!(
            context.aug_dominators[join],
            Some(first),
            "{kind}: the join can be reached through the early handler"
        );
        let output = context.into_output();
        let handler_block = &output.blocks[handler];
        let join_block = &output.blocks[join];
        assert_eq!(handler_block.args.len(), 1, "{kind}: handler environment");
        assert_eq!(join_block.args.len(), 1, "{kind}: normal/exception merge");
        for (bid, expected_literal) in [(first, 1), (second, 2)] {
            let block = &output.blocks[bid];
            let definition = block
                .ops
                .iter()
                .find(|op| {
                    op.opcode == OpCode::ConstInt
                        && op.attrs.get("value") == Some(&AttrValue::Int(expected_literal))
                })
                .expect("definition preceding this transfer");
            let transfer = block.ops.last().expect("transfer ends block");
            assert!(crate::tir::dominators::is_exception_transfer_edge(
                transfer.opcode
            ));
            assert_eq!(
                transfer.operands, definition.results,
                "{kind}: handler receives the definition at this transfer, not a later one"
            );
        }
        let normal_definition = output.blocks[success]
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::ConstInt)
            .unwrap()
            .results[0];
        assert!(matches!(&output.blocks[success].terminator,
            Terminator::Branch { target, args }
                if *target == join_block.id && args == &[normal_definition]));
        assert!(matches!(&handler_block.terminator,
            Terminator::Branch { target, args }
                if *target == join_block.id && args == &[handler_block.args[0].id]));
        assert!(matches!(&join_block.terminator,
            Terminator::Return { values } if values == &[join_block.args[0].id]));
        assert!(
            output
                .blocks
                .iter()
                .flat_map(|block| &block.ops)
                .all(|op| op.opcode != OpCode::ConstNone)
        );
        assert_no_placeholder(&output);
    }
}

#[test]
fn missing_operands_and_return_share_only_their_local_definition() {
    let ops = vec![
        op_val_out("const", 3, "seed"),
        op_args_out("add", &["missing", "missing"], "sum"),
        op_args("ret", &["missing"]),
        op_val("label", 8),
        op_val_out("const", 4, "missing"),
        op("ret_void"),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    let entry = &output.blocks[cfg.entry];
    assert_eq!(entry.ops[0].opcode, OpCode::ConstInt);
    assert_eq!(entry.ops[1].opcode, OpCode::ConstNone);
    let undef = entry.ops[1].results[0];
    assert_eq!(entry.ops[2].operands, vec![undef, undef]);
    assert!(matches!(&entry.terminator, Terminator::Return { values } if values == &[undef]));
    assert_no_placeholder(&output);
}

#[test]
fn disconnected_return_materializes_in_its_own_block() {
    let ops = vec![
        op_val_out("const", 3, "missing"),
        op("ret_void"),
        op_val("label", 7),
        op_args("ret", &["missing"]),
    ];
    let cfg = CFG::build(&ops);
    let output = convert_to_ssa(&cfg, &ops);
    assert!(
        output.blocks[cfg.entry]
            .ops
            .iter()
            .all(|op| op.opcode != OpCode::ConstNone)
    );
    let disconnected = output
        .blocks
        .iter()
        .find(|block| block.id != BlockId(cfg.entry as u32))
        .expect("disconnected root");
    assert_eq!(disconnected.ops.len(), 1);
    assert_eq!(disconnected.ops[0].opcode, OpCode::ConstNone);
    let undef = disconnected.ops[0].results[0];
    assert!(
        matches!(&disconnected.terminator, Terminator::Return { values } if values == &[undef])
    );
    assert_no_placeholder(&output);
}

#[test]
fn complete_and_dead_phi_edges_do_not_materialize_none() {
    for used in [false, true] {
        let mut ops = vec![
            op_val_out("const", 1, "condition"),
            op_args("if", &["condition"]),
            op_val_out("const", 3, "x"),
        ];
        if used {
            ops.extend([op("else"), op_val_out("const", 4, "x")]);
        }
        ops.push(op("end_if"));
        ops.push(if used {
            op_args("ret", &["x"])
        } else {
            op("ret_void")
        });
        let cfg = CFG::build(&ops);
        let output = convert_to_ssa(&cfg, &ops);
        assert!(
            output
                .blocks
                .iter()
                .flat_map(|block| &block.ops)
                .all(|op| op.opcode != OpCode::ConstNone)
        );
        assert_no_placeholder(&output);
    }
}

#[test]
fn undefined_materialization_covers_every_terminator_value_projection() {
    let ops = [];
    let cfg = CFG::build(&ops);
    let mut context = SsaContext::new("terminators", &cfg, &ops, &[]);
    let undef = context.fresh_value();
    let target = BlockId(99);
    let terms = vec![
        Terminator::Branch {
            target,
            args: vec![undef],
        },
        Terminator::CondBranch {
            cond: undef,
            then_block: target,
            then_args: vec![undef],
            else_block: target,
            else_args: vec![undef],
        },
        Terminator::Switch {
            value: undef,
            cases: vec![(1, target, vec![undef])],
            default: target,
            default_args: vec![undef],
        },
        Terminator::StateDispatch {
            cases: vec![(1, target, vec![undef])],
            default: target,
            default_args: vec![undef],
        },
        Terminator::Return {
            values: vec![undef],
        },
        Terminator::Unreachable,
    ];
    let mut blocks: Vec<_> = terms
        .into_iter()
        .enumerate()
        .map(|(id, terminator)| TirBlock {
            id: BlockId(id as u32),
            args: vec![],
            ops: vec![],
            terminator,
        })
        .collect();
    context.materialize_undefined_uses(&mut blocks, undef);
    let mut definitions = HashSet::new();
    for block in &blocks[..5] {
        assert_eq!(block.ops.len(), 1);
        let value = block.ops[0].results[0];
        assert!(
            definitions.insert(value),
            "each disconnected root owns its definition"
        );
        block
            .terminator
            .for_each_value(|operand| assert_eq!(operand, value));
    }
    assert!(blocks[5].ops.is_empty());
    assert!(!context.value_types.contains_key(&undef));
}
