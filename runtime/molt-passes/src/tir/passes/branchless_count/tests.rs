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
    let mut func = TirFunction::new(
        "test_count".into(),
        vec![TirType::Bool],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );

    let const_zero_id = ValueId(1);
    let const_one_id = ValueId(2);
    let add_result_id = ValueId(3);
    let merge_arg_id = ValueId(4);
    let condition_id = ValueId(5);
    func.next_value = 6;

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
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Bool,
            operands: vec![ValueId(0)],
            results: vec![condition_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::CondBranch {
            cond: condition_id,
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
    assert_eq!(
        entry.ops.len(),
        4,
        "the original truth conversion must remain"
    );
    assert_eq!(entry.ops[2].opcode, OpCode::Bool);
    assert_eq!(entry.ops[3].opcode, OpCode::Add);
    assert_eq!(entry.ops[3].operands[0], ValueId(1));
    assert_eq!(entry.ops[3].operands[1], ValueId(5));
    assert!(matches!(entry.terminator, Terminator::Branch { .. }));
    crate::tir::verify::verify_function(&func).expect("rewritten diamond preserves SSA");
}

#[test]
fn branchless_count_skips_non_bool_cond() {
    let mut func = make_bool_counting_func();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.args[0].ty = TirType::I64;
    if let Terminator::CondBranch { cond, .. } = &mut entry.terminator {
        *cond = ValueId(0);
    }
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
        molt_ir::FunctionReturnAbi::Value,
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

    let mut annotated = func.clone();
    assert_eq!(
        run(&mut annotated, &TargetInfo::native_release_fast()).values_changed,
        0,
        "annotation-only comparison may return a custom Python object"
    );
    let left = func.fresh_value();
    let right = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops[0].operands = vec![left, right];
    for (value, integer) in [(left, 3), (right, 7)] {
        entry.ops.insert(
            0,
            TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![value],
                attrs: AttrDict::from([("value".into(), AttrValue::Int(integer))]),
                source_span: None,
            },
        );
    }
    let stats = run(&mut func, &TargetInfo::native_release_fast());
    assert_eq!(stats.values_changed, 1, "exact comparison must fuse");
    assert_eq!(func.blocks.len(), 2);
    crate::tir::verify::verify_function(&func).expect("exact comparison diamond preserves SSA");
}

#[test]
fn branchless_count_does_not_trust_bool_annotation_or_dynamic_counter() {
    for dynamic_counter in [false, true] {
        let mut func = make_bool_counting_func();
        if dynamic_counter {
            func.blocks.get_mut(&BlockId(1)).unwrap().ops[0].operands[0] = ValueId(0);
            if let Terminator::Branch { args, .. } =
                &mut func.blocks.get_mut(&BlockId(2)).unwrap().terminator
            {
                args[0] = ValueId(0);
            }
        } else if let Terminator::CondBranch { cond, .. } =
            &mut func.blocks.get_mut(&func.entry_block).unwrap().terminator
        {
            *cond = ValueId(0);
        }
        assert_eq!(
            run(&mut func, &TargetInfo::native_release_fast()).values_changed,
            0
        );
        assert_eq!(func.blocks.len(), 4);
    }
}

#[test]
fn branchless_count_preserves_heap_counter_and_shared_arm_custody() {
    for counter in [crate::tir::numeric_facts::INLINE_INT47_HI, i64::MAX] {
        let mut func = make_bool_counting_func();
        func.blocks.get_mut(&func.entry_block).unwrap().ops[0]
            .attrs
            .insert("value".into(), AttrValue::Int(counter));
        assert_eq!(
            run(&mut func, &TargetInfo::native_release_fast()).values_changed,
            0,
            "false path must not allocate or execute an overflowing increment"
        );
    }
    for arm in [BlockId(1), BlockId(2)] {
        let mut func = make_bool_counting_func();
        let other = func.fresh_block();
        func.blocks.insert(
            other,
            TirBlock {
                id: other,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: arm,
                    args: vec![],
                },
            },
        );
        assert_eq!(
            run(&mut func, &TargetInfo::native_release_fast()).values_changed,
            0,
            "a shared arm cannot be deleted"
        );
        assert!(func.blocks.contains_key(&arm));
    }
}

#[test]
fn branchless_count_preserves_same_source_exception_and_structural_labels() {
    for opcode in [OpCode::CheckException, OpCode::TryStart, OpCode::TryEnd] {
        for arm in [BlockId(1), BlockId(2)] {
            let mut func = make_bool_counting_func();
            func.label_id_map.insert(arm.0, 91);
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .push(TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![],
                    results: vec![],
                    attrs: AttrDict::from([("value".into(), AttrValue::Int(91))]),
                    source_span: None,
                });
            assert_eq!(
                crate::tir::dominators::build_pred_map(&func)[&arm],
                vec![func.entry_block],
                "a predecessor set cannot prove exclusive edge ownership"
            );
            assert_eq!(
                run(&mut func, &TargetInfo::native_release_fast()).values_changed,
                0,
                "{opcode:?} still owns the arm's function-local label"
            );
            assert!(func.blocks.contains_key(&arm));
            assert_eq!(func.label_id_map[&arm.0], 91);
        }
    }
}

#[test]
fn branchless_count_preserves_every_loop_metadata_endpoint() {
    use crate::tir::blocks::{LoopBreakKind, LoopRole};

    for arm in [BlockId(1), BlockId(2)] {
        for metadata in 0..6 {
            let mut func = make_bool_counting_func();
            let entry = func.entry_block;
            match metadata {
                0 => {
                    func.loop_roles.insert(arm, LoopRole::LoopEnd);
                }
                1 => {
                    func.loop_pairs.insert(arm, entry);
                }
                2 => {
                    func.loop_pairs.insert(entry, arm);
                }
                3 => {
                    func.loop_break_kinds
                        .insert(arm, LoopBreakKind::BreakIfTrue);
                }
                4 => {
                    func.loop_cond_blocks.insert(arm, entry);
                }
                5 => {
                    func.loop_cond_blocks.insert(entry, arm);
                }
                _ => unreachable!(),
            }
            assert_eq!(
                run(&mut func, &TargetInfo::native_release_fast()).values_changed,
                0,
                "loop metadata endpoint {metadata} must survive"
            );
            assert!(func.blocks.contains_key(&arm));
        }
    }
}

#[test]
fn branchless_count_retires_unreferenced_labels_and_removed_value_facts() {
    let mut func = make_bool_counting_func();
    func.label_id_map.extend([(1, 81), (2, 82), (3, 83)]);
    func.value_types.insert(ValueId(3), TirType::I64);
    func.value_types.insert(ValueId(4), TirType::I64);
    assert_eq!(
        run(&mut func, &TargetInfo::native_release_fast()).values_changed,
        1
    );
    assert!(!func.label_id_map.contains_key(&1));
    assert!(!func.label_id_map.contains_key(&2));
    assert_eq!(func.label_id_map[&3], 83);
    assert!(!func.value_types.contains_key(&ValueId(3)));
    assert_eq!(func.value_types[&ValueId(4)], TirType::I64);
    crate::tir::verify::verify_function(&func).expect("retired metadata remains coherent");
}

#[test]
fn branchless_count_accepts_nonallocating_inline_endpoints() {
    use crate::tir::numeric_facts::{INLINE_INT47_HI, INLINE_INT47_LO};

    for counter in [INLINE_INT47_LO, INLINE_INT47_HI - 1] {
        let mut func = make_bool_counting_func();
        func.blocks.get_mut(&func.entry_block).unwrap().ops[0]
            .attrs
            .insert("value".into(), AttrValue::Int(counter));
        assert_eq!(
            run(&mut func, &TargetInfo::native_release_fast()).values_changed,
            1,
            "both old and incremented inline endpoints are nonallocating"
        );
        crate::tir::verify::verify_function(&func).expect("inline endpoint rewrite preserves SSA");
    }
}
