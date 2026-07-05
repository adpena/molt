use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::effect_proof::EffectProof;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::run;

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

fn make_effect_proven_module_get_attr(module: ValueId, attr_name: ValueId, out: ValueId) -> TirOp {
    let mut op = make_module_get_attr(module, attr_name, out);
    op.attrs.insert(
        "effect_proof".into(),
        AttrValue::Str(EffectProof::StaticModuleClassBinding.name().into()),
    );
    op
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
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 100,
        next_block: 1,
        ..TirFunction::new("test".into(), vec![], TirType::None)
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
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 100,
        next_block: 2,
        ..TirFunction::new("two_block_test".into(), vec![], TirType::None)
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
fn check_after_effect_proven_static_module_class_read_is_dropped() {
    let mut func = make_func_with_block(vec![
        make_check_exception(),
        make_effect_proven_module_get_attr(ValueId(0), ValueId(1), ValueId(2)),
        make_check_exception(),
    ]);

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 1);
    assert_eq!(func.blocks[&BlockId(0)].ops.len(), 2);
}

#[test]
fn check_after_unproven_module_get_attr_is_kept() {
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
