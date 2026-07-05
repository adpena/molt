use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::run;

/// Build a function that models:
///   count = 0
///   if cond: count += 1
///   return count
fn make_bool_counting_func() -> TirFunction {
    let mut func = TirFunction::new("test_count".into(), vec![TirType::Bool], TirType::I64);

    let const_zero_id = ValueId(1);
    let const_one_id = ValueId(2);
    let add_result_id = ValueId(3);
    let merge_arg_id = ValueId(4);
    func.next_value = 5;

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();
    let merge_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![const_zero_id],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            },
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![const_one_id],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            },
        ];
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };
    }

    func.blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![const_zero_id, const_one_id],
                results: vec![add_result_id],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: merge_id,
                args: vec![add_result_id],
            },
        },
    );

    func.blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: merge_id,
                args: vec![const_zero_id],
            },
        },
    );

    func.blocks.insert(
        merge_id,
        TirBlock {
            id: merge_id,
            args: vec![TirValue {
                id: merge_arg_id,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg_id],
            },
        },
    );

    func
}

#[test]
fn branchless_count_fuses_bool_increment() {
    let mut func = make_bool_counting_func();
    assert_eq!(func.blocks.len(), 4);

    let stats = run(&mut func, &TargetInfo::native_release_fast());

    assert_eq!(stats.values_changed, 1, "should report one value changed");
    assert_eq!(func.blocks.len(), 2, "should have bb0 and bb3 only");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 3, "entry should have 3 ops");
    assert_eq!(entry.ops[2].opcode, OpCode::Add);
    assert_eq!(entry.ops[2].operands[0], ValueId(1));
    assert_eq!(entry.ops[2].operands[1], ValueId(0));
    assert!(matches!(entry.terminator, Terminator::Branch { .. }));
}

#[test]
fn branchless_count_skips_non_bool_cond() {
    let mut func = make_bool_counting_func();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.args[0].ty = TirType::I64;
    func.param_types = vec![TirType::I64];

    let stats = run(&mut func, &TargetInfo::native_release_fast());

    assert_eq!(stats.values_changed, 0);
    assert_eq!(func.blocks.len(), 4);
}

#[test]
fn branchless_count_skips_multi_op_then_block() {
    let mut func = make_bool_counting_func();
    let then_id = BlockId(1);
    let then_block = func.blocks.get_mut(&then_id).unwrap();
    then_block.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![ValueId(99)],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(42));
            m
        },
        source_span: None,
    });

    let stats = run(&mut func, &TargetInfo::native_release_fast());

    assert_eq!(stats.values_changed, 0);
    assert_eq!(func.blocks.len(), 4);
}

#[test]
fn branchless_count_skips_non_unit_increment() {
    let mut func = make_bool_counting_func();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    if let Some(AttrValue::Int(v)) = entry.ops[1].attrs.get_mut("value") {
        *v = 2;
    }

    let stats = run(&mut func, &TargetInfo::native_release_fast());

    assert_eq!(stats.values_changed, 0);
    assert_eq!(func.blocks.len(), 4);
}

#[test]
fn branchless_count_handles_inplace_add() {
    let mut func = make_bool_counting_func();
    let then_id = BlockId(1);
    let then_block = func.blocks.get_mut(&then_id).unwrap();
    then_block.ops[0].opcode = OpCode::InplaceAdd;

    let stats = run(&mut func, &TargetInfo::native_release_fast());

    assert_eq!(stats.values_changed, 1);
    assert_eq!(func.blocks.len(), 2);
}

#[test]
fn branchless_count_works_with_comparison_cond() {
    let mut func = TirFunction::new(
        "test_cmp_count".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
    );
    let cmp_result = ValueId(2);
    let counter_val = ValueId(3);
    let const_one = ValueId(4);
    let add_result = ValueId(5);
    let merge_arg = ValueId(6);
    func.next_value = 7;

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();
    let merge_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Lt,
                operands: vec![ValueId(0), ValueId(1)],
                results: vec![cmp_result],
                attrs: AttrDict::new(),
                source_span: None,
            },
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![counter_val],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            },
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![const_one],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            },
        ];
        entry.terminator = Terminator::CondBranch {
            cond: cmp_result,
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };
    }

    func.blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![counter_val, const_one],
                results: vec![add_result],
                attrs: AttrDict::new(),
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: merge_id,
                args: vec![add_result],
            },
        },
    );

    func.blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: merge_id,
                args: vec![counter_val],
            },
        },
    );

    func.blocks.insert(
        merge_id,
        TirBlock {
            id: merge_id,
            args: vec![TirValue {
                id: merge_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![merge_arg],
            },
        },
    );

    let stats = run(&mut func, &TargetInfo::native_release_fast());

    assert_eq!(
        stats.values_changed, 1,
        "comparison-cond pattern should fuse"
    );
    assert_eq!(func.blocks.len(), 2);
}
