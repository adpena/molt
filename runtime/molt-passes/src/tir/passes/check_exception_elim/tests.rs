use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::{
    classify::{const_int_values, op_may_raise},
    run,
};

fn make_check_exception() -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(100));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn make_const_int(value: i64, out: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![out],
        attrs,
        source_span: None,
    }
}

fn make_call(callee: &str, out: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str(callee.to_string()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![],
        results: vec![out],
        attrs,
        source_span: None,
    }
}

fn make_module_get_attr(module: ValueId, attr_name: ValueId, out: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ModuleGetAttr,
        operands: vec![module, attr_name],
        results: vec![out],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_binary(opcode: OpCode, lhs: ValueId, rhs: ValueId, out: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![lhs, rhs],
        results: vec![out],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_original_kind(kind: &str) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs,
        source_span: None,
    }
}

fn make_func_with_block(ops: Vec<TirOp>) -> TirFunction {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![],
        ops,
        terminator: Terminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    TirFunction {
        name: "test".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 100,
        next_block: 1,
        ..TirFunction::new(
            "test".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        )
    }
}

fn make_two_block_func(entry_ops: Vec<TirOp>, successor_ops: Vec<TirOp>) -> TirFunction {
    let entry_id = BlockId(0);
    let successor_id = BlockId(1);
    let entry = TirBlock {
        id: entry_id,
        args: vec![],
        ops: entry_ops,
        terminator: Terminator::Branch {
            target: successor_id,
            args: vec![],
        },
    };
    let successor = TirBlock {
        id: successor_id,
        args: vec![],
        ops: successor_ops,
        terminator: Terminator::Return { values: vec![] },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, entry);
    blocks.insert(successor_id, successor);
    TirFunction {
        name: "two_block_test".into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 100,
        next_block: 2,
        ..TirFunction::new(
            "two_block_test".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        )
    }
}

#[test]
fn first_check_kept() {
    let mut func =
        make_func_with_block(vec![make_const_int(1, ValueId(0)), make_check_exception()]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
}

#[test]
fn redundant_check_after_pure_ops_dropped() {
    let mut func = make_func_with_block(vec![
        make_const_int(1, ValueId(0)),
        make_check_exception(),
        make_const_int(2, ValueId(1)),
        make_const_int(3, ValueId(2)),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn full_width_boxing_preserves_local_and_successor_exception_edges() {
    use crate::tir::op_kinds_generated::{GvnNumberingRole, opcode_gvn_numbering_role_table};

    assert_eq!(
        opcode_gvn_numbering_role_table(OpCode::BoxVal),
        GvnNumberingRole::Never,
        "a repeated box can independently fail allocation"
    );
    for split_block in [false, true] {
        let boxed = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::BoxVal,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs: AttrDict::new(),
            source_span: None,
        };
        let mut ops = vec![
            make_const_int(i64::MAX, ValueId(0)),
            make_check_exception(),
            boxed,
        ];
        let mut func = if split_block {
            make_two_block_func(ops, vec![make_check_exception()])
        } else {
            ops.push(make_check_exception());
            make_func_with_block(ops)
        };
        let stats = run(&mut func);
        assert_eq!(stats.ops_removed, 0, "split_block={split_block}");
        assert_eq!(
            func.blocks
                .values()
                .flat_map(|block| &block.ops)
                .filter(|op| op.opcode == OpCode::CheckException)
                .count(),
            2,
            "successful input materialization cannot prove boxing non-throwing"
        );
    }
}

#[test]
fn untargeted_observers_preserve_literal_and_poll_failures_locally_and_across_cfg() {
    for literal in [
        None,
        Some(OpCode::ConstStr),
        Some(OpCode::ConstBytes),
        Some(OpCode::ConstBigInt),
    ] {
        for split_block in [false, true] {
            let mut observer = make_check_exception();
            observer.attrs.remove("value");
            if literal.is_none() {
                observer.mark_async_work_poll();
            }
            let mut ops = vec![make_check_exception()];
            if let Some(opcode) = literal {
                ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![],
                    results: vec![ValueId(0)],
                    attrs: AttrDict::from([("s_value".into(), AttrValue::Str("123".into()))]),
                    source_span: None,
                });
            }
            ops.push(observer);
            let mut func = if split_block {
                make_two_block_func(ops, vec![make_check_exception()])
            } else {
                ops.push(make_check_exception());
                make_func_with_block(ops)
            };
            let stats = run(&mut func);
            assert_eq!(
                stats.ops_removed, 0,
                "{literal:?} split_block={split_block}: observation does not clear pending state"
            );
            assert_eq!(
                func.blocks
                    .values()
                    .flat_map(|block| &block.ops)
                    .filter(|op| op.opcode == OpCode::CheckException)
                    .count(),
                3
            );
        }
    }
}

#[test]
fn result_bearing_checks_and_untargeted_observers_survive_clean_state() {
    for targeted in [false, true] {
        let mut observer = make_check_exception();
        observer.results.push(ValueId(0));
        if !targeted {
            observer.attrs.remove("value");
        }
        let mut func = make_func_with_block(vec![make_check_exception(), observer]);
        func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        let stats = run(&mut func);
        assert_eq!(
            stats.ops_removed, 0,
            "observer result must retain its definition"
        );
        assert_eq!(func.blocks[&BlockId(0)].ops[1].results, vec![ValueId(0)]);
    }
    let mut observer = make_check_exception();
    observer.attrs.remove("value");
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        observer,
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(
        stats.ops_removed, 1,
        "passive observer preserves an already clean state"
    );
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
}

#[test]
fn targeted_poll_proves_clean_fallthrough_but_must_execute() {
    let mut poll = make_check_exception();
    poll.mark_async_work_poll();
    let mut func = make_two_block_func(
        vec![make_check_exception(), poll],
        vec![make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert!(func.blocks[&BlockId(1)].ops.is_empty());
    assert!(func.blocks[&BlockId(0)].ops[1].is_async_work_poll());
}

#[test]
fn preserved_copy_fallbacks_fail_closed_with_or_without_poll_marker() {
    let unmarked = make_original_kind("exception_finally_pending_observer");
    let mut observer = make_original_kind("exception_finally_pending_observer");
    observer.mark_async_work_poll();
    let value_types = HashMap::new();
    let probe = make_func_with_block(vec![unmarked.clone(), observer.clone()]);
    let const_ints = const_int_values(&probe);

    assert!(op_may_raise(&value_types, &const_ints, &unmarked));
    assert!(
        op_may_raise(&value_types, &const_ints, &observer),
        "polling can introduce a pending exception"
    );
}

#[test]
fn preserved_copy_transport_does_not_bypass_shared_effects() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_original_kind("store_var"),
        make_original_kind("load_var"),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_call_kept() {
    let mut func = make_func_with_block(vec![
        make_const_int(1, ValueId(0)),
        make_check_exception(),
        make_call("foo", ValueId(1)),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_module_get_attr_is_kept() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_module_get_attr(ValueId(0), ValueId(1), ValueId(2)),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 3);
}

#[test]
fn many_redundant_checks_collapsed() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_const_int(1, ValueId(0)),
        make_check_exception(),
        make_const_int(2, ValueId(1)),
        make_check_exception(),
        make_const_int(3, ValueId(2)),
        make_check_exception(),
        make_call("foo", ValueId(3)),
        make_check_exception(),
        make_check_exception(),
    ]);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 4);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 6);
}

#[test]
fn first_check_in_normal_successor_dropped_after_checked_predecessor() {
    let mut func = make_two_block_func(
        vec![make_check_exception()],
        vec![make_const_int(2, ValueId(1)), make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 1);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 1);
}

#[test]
fn first_check_in_successor_kept_when_predecessor_may_raise() {
    let mut func = make_two_block_func(
        vec![make_call("foo", ValueId(1))],
        vec![make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 1);
}

#[test]
fn exception_target_entry_remains_conservative() {
    let mut func = make_two_block_func(
        vec![make_check_exception()],
        vec![make_const_int(2, ValueId(1)), make_check_exception()],
    );
    func.label_id_map.insert(1, 100);
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 2);
}

#[test]
fn explicit_exception_clear_feeds_successor_elision() {
    let mut func = make_two_block_func(
        vec![
            make_check_exception(),
            make_original_kind("exception_clear"),
        ],
        vec![make_check_exception()],
    );
    let stats = run(&mut func);
    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(1)].ops.len(), 0);
}

#[test]
fn check_after_i64_mod_by_nonzero_const_is_dropped() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::Mod, lhs, rhs, out),
        make_check_exception(),
    ]);
    func.value_types.insert(lhs, TirType::I64);
    func.value_types.insert(rhs, TirType::I64);
    func.value_types.insert(out, TirType::I64);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_i64_floor_div_by_nonzero_const_is_dropped() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::FloorDiv, lhs, rhs, out),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn check_after_true_div_by_nonzero_integer_const_is_kept() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        make_binary(OpCode::Div, lhs, rhs, out),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
}

#[test]
fn malformed_floor_div_result_arity_fails_closed() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let mut malformed = make_binary(OpCode::FloorDiv, lhs, rhs, ValueId(2));
    malformed.results.clear();
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(3, rhs),
        make_check_exception(),
        malformed,
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
}

#[test]
fn check_after_i64_mod_by_zero_const_is_kept() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_const_int(0, rhs),
        make_check_exception(),
        make_binary(OpCode::Mod, lhs, rhs, out),
        make_check_exception(),
    ]);
    func.value_types.insert(lhs, TirType::I64);
    func.value_types.insert(rhs, TirType::I64);
    func.value_types.insert(out, TirType::I64);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 5);
}

#[test]
fn check_after_i64_mod_by_dynamic_rhs_is_kept() {
    let lhs = ValueId(0);
    let rhs = ValueId(1);
    let out = ValueId(2);
    let mut func = make_func_with_block(vec![
        make_const_int(9, lhs),
        make_check_exception(),
        make_binary(OpCode::Mod, lhs, rhs, out),
        make_check_exception(),
    ]);
    func.value_types.insert(lhs, TirType::I64);
    func.value_types.insert(rhs, TirType::I64);
    func.value_types.insert(out, TirType::I64);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 4);
}

#[test]
fn class_allocation_preserves_pending_exception_checks_across_blocks() {
    let class_alloc = |opcode| {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(16));
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs,
            source_span: None,
        }
    };
    let mut generic = make_original_kind("alloc_class");
    generic.operands = vec![ValueId(0)];
    generic.results = vec![ValueId(1)];
    generic.attrs.insert("value".into(), AttrValue::Int(16));
    for allocation in [class_alloc(OpCode::ObjectNewBound), generic] {
        assert!(op_may_raise(&HashMap::new(), &HashMap::new(), &allocation));
        let mut same_block = make_func_with_block(vec![
            make_check_exception(),
            allocation.clone(),
            make_check_exception(),
        ]);
        assert_eq!(run(&mut same_block).ops_removed, 0);
        let mut across_blocks = make_two_block_func(
            vec![make_check_exception(), allocation],
            vec![make_check_exception()],
        );
        assert_eq!(run(&mut across_blocks).ops_removed, 0);
        assert_eq!(across_blocks.blocks[&BlockId(1)].ops.len(), 1);
    }
}
