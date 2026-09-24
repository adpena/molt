use super::tests::{op, op_args, op_args_out, op_val, op_val_out};
use super::*;

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
