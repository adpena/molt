use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use crate::tir::verify::verify_function;

use super::run;

#[test]
fn full_width_or_unknown_integer_boxing_is_never_elided_even_when_dead() {
    for value in [Some(i64::MIN), Some(i64::MAX), None] {
        for used in [false, true] {
            let mut func =
                TirFunction::new("fallible_box".into(), vec![TirType::I64], TirType::I64);
            let raw = if value.is_some() {
                func.fresh_value()
            } else {
                ValueId(0)
            };
            let boxed = func.fresh_value();
            let unboxed = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            if let Some(value) = value {
                entry.ops.push(const_int_op(raw, value));
            }
            entry.ops.push(box_op(raw, boxed));
            if used {
                entry.ops.push(unbox_op(boxed, unboxed));
            }
            entry.terminator = Terminator::Return {
                values: vec![if used { unboxed } else { raw }],
            };
            assert_eq!(
                run(&mut func).ops_removed,
                0,
                "value={value:?}, used={used}"
            );
            assert!(
                func.blocks[&func.entry_block]
                    .ops
                    .iter()
                    .any(|op| op.opcode == OpCode::BoxVal)
            );
        }
    }
}

#[test]
fn full_width_box_keeps_its_cross_block_exception_observer() {
    use crate::tir::blocks::TirBlock;
    let mut func = TirFunction::new("box_exception".into(), vec![], TirType::I64);
    let next = func.fresh_block();
    let raw = func.fresh_value();
    let boxed = func.fresh_value();
    let unboxed = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_int_op(raw, i64::MAX), box_op(raw, boxed)]);
    entry.terminator = Terminator::Branch {
        target: next,
        args: vec![],
    };
    func.blocks.insert(
        next,
        TirBlock {
            id: next,
            args: vec![],
            ops: vec![
                TirOp {
                    opcode: OpCode::CheckException,
                    operands: vec![],
                    results: vec![],
                    ..box_op(raw, boxed)
                },
                unbox_op(boxed, unboxed),
            ],
            terminator: Terminator::Return {
                values: vec![unboxed],
            },
        },
    );
    assert_eq!(run(&mut func).ops_removed, 0);
    assert_eq!(func.blocks[&next].ops[0].opcode, OpCode::CheckException);
    assert!(verify_function(&func).is_ok());
}

#[test]
fn proven_nonallocating_scalar_pairs_remain_optimizable() {
    for (opcode, value, ty) in [
        (OpCode::ConstInt, Some(AttrValue::Int(42)), TirType::I64),
        (
            OpCode::ConstBool,
            Some(AttrValue::Bool(true)),
            TirType::Bool,
        ),
        (
            OpCode::ConstFloat,
            Some(AttrValue::Float(1.25)),
            TirType::F64,
        ),
        (OpCode::ConstNone, None, TirType::None),
    ] {
        let mut func = TirFunction::new("safe_box".into(), vec![], ty.clone());
        let raw = func.fresh_value();
        let boxed = func.fresh_value();
        let unboxed = func.fresh_value();
        func.value_types.insert(raw, ty.clone());
        func.value_types
            .insert(boxed, TirType::Box(Box::new(ty.clone())));
        func.value_types.insert(unboxed, ty);
        let mut constant = const_int_op(raw, 0);
        constant.opcode = opcode;
        constant.attrs.clear();
        if let Some(value) = value {
            constant.attrs.insert("value".into(), value);
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([constant, box_op(raw, boxed), unbox_op(boxed, unboxed)]);
        entry.terminator = Terminator::Return {
            values: vec![unboxed],
        };
        assert_eq!(run(&mut func).ops_removed, 2, "{opcode:?}");
    }
}

#[test]
fn box_unbox_elision_requires_matching_semantic_result_and_target() {
    for mismatch_hint in [false, true] {
        let mut func = TirFunction::new("mismatched_unbox".into(), vec![], TirType::F64);
        let raw = func.fresh_value();
        let boxed = func.fresh_value();
        let unboxed = func.fresh_value();
        let mut unbox = unbox_op(boxed, unboxed);
        if mismatch_hint {
            unbox
                .attrs
                .insert("type".into(), AttrValue::Str("f64".into()));
        } else {
            func.value_types.insert(unboxed, TirType::F64);
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_int_op(raw, 1), box_op(raw, boxed), unbox]);
        entry.terminator = Terminator::Return {
            values: vec![unboxed],
        };
        assert_eq!(run(&mut func).ops_removed, 0);
    }
}

fn box_op(operand: ValueId, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![operand],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn unbox_op(operand: ValueId, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::UnboxVal,
        operands: vec![operand],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn const_int_op(result: ValueId, value: i64) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

fn add_op(lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![lhs, rhs],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    }
}

#[test]
fn simple_box_unbox_pair_eliminated() {
    let mut func = TirFunction::new("test".into(), vec![], TirType::I64);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_op(v0, 42));
    entry.ops.push(box_op(v0, v1));
    entry.ops.push(unbox_op(v1, v2));
    entry.terminator = Terminator::Return { values: vec![v2] };

    assert!(
        verify_function(&func).is_ok(),
        "pre-pass verification failed"
    );

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 2, "expected 2 ops removed");
    assert_eq!(stats.values_changed, 1, "expected 1 value changed");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 1, "expected 1 op remaining");
    assert_eq!(entry.ops[0].opcode, OpCode::ConstInt);

    if let Terminator::Return { values } = &entry.terminator {
        assert_eq!(values, &[v0], "return should use original value");
    } else {
        panic!("expected Return terminator");
    }

    assert!(
        verify_function(&func).is_ok(),
        "post-pass verification failed: {:?}",
        verify_function(&func).err()
    );
}

#[test]
fn multiple_unbox_consumers_all_eliminated() {
    let mut func = TirFunction::new("test".into(), vec![], TirType::I64);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;
    let v3 = ValueId(func.next_value);
    func.next_value += 1;
    let v4 = ValueId(func.next_value);
    func.next_value += 1;
    let v5 = ValueId(func.next_value);
    func.next_value += 1;
    let v6 = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_op(v0, 10));
    entry.ops.push(box_op(v0, v1));
    entry.ops.push(unbox_op(v1, v2));
    entry.ops.push(unbox_op(v1, v3));
    entry.ops.push(unbox_op(v1, v4));
    entry.ops.push(add_op(v2, v3, v5));
    entry.ops.push(add_op(v5, v4, v6));
    entry.terminator = Terminator::Return { values: vec![v6] };

    assert!(
        verify_function(&func).is_ok(),
        "pre-pass verification failed"
    );

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 4, "expected 4 ops removed");
    assert_eq!(stats.values_changed, 3, "expected 3 values changed");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 3, "expected 3 ops remaining");

    let add1 = &entry.ops[1];
    assert_eq!(add1.opcode, OpCode::Add);
    assert_eq!(add1.operands, vec![v0, v0], "add should use original value");

    assert!(
        verify_function(&func).is_ok(),
        "post-pass verification failed: {:?}",
        verify_function(&func).err()
    );
}

#[test]
fn mixed_consumers_not_eliminated() {
    let mut func = TirFunction::new("test".into(), vec![], TirType::DynBox);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;
    let v3 = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_op(v0, 10));
    entry.ops.push(box_op(v0, v1));
    entry.ops.push(unbox_op(v1, v2));
    entry.ops.push(add_op(v1, v1, v3));
    entry.terminator = Terminator::Return { values: vec![v3] };

    assert!(
        verify_function(&func).is_ok(),
        "pre-pass verification failed"
    );

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0, "expected 0 ops removed");
    assert_eq!(stats.values_changed, 0, "expected 0 values changed");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 4, "all ops should remain");

    assert!(
        verify_function(&func).is_ok(),
        "post-pass verification failed"
    );
}

#[test]
fn no_box_ops_no_changes() {
    let mut func = TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

    let v2 = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(add_op(ValueId(0), ValueId(1), v2));
    entry.terminator = Terminator::Return { values: vec![v2] };

    assert!(
        verify_function(&func).is_ok(),
        "pre-pass verification failed"
    );

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 0);
    assert_eq!(stats.values_changed, 0);
    assert_eq!(stats.name, "unboxing");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 1);

    assert!(
        verify_function(&func).is_ok(),
        "post-pass verification failed"
    );
}

#[test]
fn nested_box_unbox_inner_pair_eliminated() {
    let mut func = TirFunction::new("test".into(), vec![], TirType::DynBox);

    let v0 = ValueId(func.next_value);
    func.next_value += 1;
    let v1 = ValueId(func.next_value);
    func.next_value += 1;
    let v2 = ValueId(func.next_value);
    func.next_value += 1;
    let v3 = ValueId(func.next_value);
    func.next_value += 1;

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_op(v0, 5));
    entry.ops.push(box_op(v0, v1));
    entry.ops.push(unbox_op(v1, v2));
    entry.ops.push(box_op(v2, v3));
    entry.terminator = Terminator::Return { values: vec![v3] };

    assert!(
        verify_function(&func).is_ok(),
        "pre-pass verification failed"
    );

    let stats = run(&mut func);

    assert_eq!(stats.ops_removed, 2, "expected inner pair removed");

    let entry = &func.blocks[&func.entry_block];
    assert_eq!(entry.ops.len(), 2, "expected const_int + outer box");
    assert_eq!(entry.ops[0].opcode, OpCode::ConstInt);
    assert_eq!(entry.ops[1].opcode, OpCode::BoxVal);
    assert_eq!(
        entry.ops[1].operands,
        vec![v0],
        "outer box should use original value"
    );

    assert!(
        verify_function(&func).is_ok(),
        "post-pass verification failed: {:?}",
        verify_function(&func).err()
    );
}
