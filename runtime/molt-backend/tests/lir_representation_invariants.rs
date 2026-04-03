use std::collections::HashMap;

use molt_backend::tir::blocks::BlockId;
use molt_backend::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_backend::tir::printer::print_lir_function;
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;
use molt_backend::tir::verify_lir::verify_lir_function;

fn lir_value(id: u32, ty: TirType, repr: LirRepr) -> LirValue {
    LirValue {
        id: ValueId(id),
        ty,
        repr,
    }
}

fn lir_op(opcode: OpCode, operands: &[u32], result_values: Vec<LirValue>) -> LirOp {
    LirOp {
        tir_op: TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: operands.iter().copied().map(ValueId).collect(),
            results: result_values.iter().map(|value| value.id).collect(),
            attrs: AttrDict::new(),
            source_span: None,
        },
        result_values,
    }
}

fn const_int(result: u32, value: i64) -> LirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("value".to_string(), AttrValue::Int(value));
    LirOp {
        tir_op: TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        },
        result_values: vec![lir_value(result, TirType::I64, LirRepr::I64)],
    }
}

#[test]
fn verify_lir_accepts_matching_branch_argument_representations() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: BlockId(1),
            args: vec![ValueId(0)],
        },
    };
    let exit = LirBlock {
        id: BlockId(1),
        args: vec![lir_value(1, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(1)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    blocks.insert(BlockId(1), exit);

    let func = LirFunction {
        name: "matching_branch_repr".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    assert!(
        verify_lir_function(&func).is_ok(),
        "matching I64 branch args should verify"
    );
}

#[test]
fn verify_lir_rejects_branch_argument_repr_mismatch() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::DynBox, LirRepr::DynBox)],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: BlockId(1),
            args: vec![ValueId(0)],
        },
    };
    let exit = LirBlock {
        id: BlockId(1),
        args: vec![lir_value(1, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(1)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    blocks.insert(BlockId(1), exit);

    let func = LirFunction {
        name: "mismatched_branch_repr".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::DynBox],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected repr mismatch");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("representation mismatch"),
        "expected repr mismatch error, got: {messages}"
    );
}

#[test]
fn verify_lir_rejects_non_bool_branch_condition() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![
            lir_value(0, TirType::I64, LirRepr::I64),
            lir_value(1, TirType::I64, LirRepr::I64),
            lir_value(2, TirType::I64, LirRepr::I64),
        ],
        ops: vec![],
        terminator: LirTerminator::CondBranch {
            cond: ValueId(0),
            then_block: BlockId(1),
            then_args: vec![ValueId(1)],
            else_block: BlockId(2),
            else_args: vec![ValueId(2)],
        },
    };
    let then_block = LirBlock {
        id: BlockId(1),
        args: vec![lir_value(3, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(3)],
        },
    };
    let else_block = LirBlock {
        id: BlockId(2),
        args: vec![lir_value(4, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(4)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    blocks.insert(BlockId(1), then_block);
    blocks.insert(BlockId(2), else_block);

    let func = LirFunction {
        name: "non_bool_cond".to_string(),
        param_names: vec!["cond".to_string(), "a".to_string(), "b".to_string()],
        param_types: vec![TirType::I64, TirType::I64, TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected bool condition error");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("Bool1"),
        "expected Bool1 branch condition error, got: {messages}"
    );
}

#[test]
fn verify_lir_rejects_entry_block_arity_mismatch() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![],
        ops: vec![],
        terminator: LirTerminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);

    let func = LirFunction {
        name: "entry_arity_mismatch".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected entry arity mismatch");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("entry block") && messages.contains("declares 1"),
        "expected entry block arity error, got: {messages}"
    );
}

#[test]
fn verify_lir_rejects_entry_block_repr_mismatch() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::I64, LirRepr::DynBox)],
        ops: vec![],
        terminator: LirTerminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);

    let func = LirFunction {
        name: "entry_repr_mismatch".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected entry repr mismatch");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("entry block") && messages.contains("representation mismatch"),
        "expected entry block repr mismatch, got: {messages}"
    );
}

#[test]
fn verify_lir_accepts_loop_carried_i64_block_params() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: BlockId(1),
            args: vec![ValueId(0)],
        },
    };
    let header = LirBlock {
        id: BlockId(1),
        args: vec![lir_value(1, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: BlockId(2),
            args: vec![ValueId(1)],
        },
    };
    let body = LirBlock {
        id: BlockId(2),
        args: vec![lir_value(2, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: BlockId(1),
            args: vec![ValueId(2)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    blocks.insert(BlockId(1), header);
    blocks.insert(BlockId(2), body);

    let func = LirFunction {
        name: "loop_i64".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![],
        blocks,
        entry_block: BlockId(0),
    };

    assert!(
        verify_lir_function(&func).is_ok(),
        "loop-carried I64 block params should verify"
    );
}

#[test]
fn verify_lir_rejects_non_dominating_branch_value_use() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::Bool, LirRepr::Bool1)],
        ops: vec![],
        terminator: LirTerminator::CondBranch {
            cond: ValueId(0),
            then_block: BlockId(1),
            then_args: vec![],
            else_block: BlockId(2),
            else_args: vec![],
        },
    };
    let then_block = LirBlock {
        id: BlockId(1),
        args: vec![],
        ops: vec![const_int(1, 7)],
        terminator: LirTerminator::Branch {
            target: BlockId(3),
            args: vec![ValueId(1)],
        },
    };
    let else_block = LirBlock {
        id: BlockId(2),
        args: vec![],
        ops: vec![],
        terminator: LirTerminator::Branch {
            target: BlockId(3),
            args: vec![ValueId(1)],
        },
    };
    let exit = LirBlock {
        id: BlockId(3),
        args: vec![lir_value(2, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(2)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    blocks.insert(BlockId(1), then_block);
    blocks.insert(BlockId(2), else_block);
    blocks.insert(BlockId(3), exit);

    let func = LirFunction {
        name: "nondominating_branch_value".to_string(),
        param_names: vec!["cond".to_string()],
        param_types: vec![TirType::Bool],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected dominance violation");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("does not dominate"),
        "expected dominance error, got: {messages}"
    );
}

#[test]
fn verify_lir_rejects_conditional_branch_with_non_bool_semantic_type() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![
            lir_value(0, TirType::Bool, LirRepr::Bool1),
            lir_value(1, TirType::I64, LirRepr::I64),
            lir_value(2, TirType::I64, LirRepr::I64),
        ],
        ops: vec![lir_op(
            OpCode::Copy,
            &[0],
            vec![lir_value(5, TirType::I64, LirRepr::Bool1)],
        )],
        terminator: LirTerminator::CondBranch {
            cond: ValueId(5),
            then_block: BlockId(1),
            then_args: vec![ValueId(1)],
            else_block: BlockId(2),
            else_args: vec![ValueId(2)],
        },
    };
    let then_block = LirBlock {
        id: BlockId(1),
        args: vec![lir_value(3, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(3)],
        },
    };
    let else_block = LirBlock {
        id: BlockId(2),
        args: vec![lir_value(4, TirType::I64, LirRepr::I64)],
        ops: vec![],
        terminator: LirTerminator::Return {
            values: vec![ValueId(4)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);
    blocks.insert(BlockId(1), then_block);
    blocks.insert(BlockId(2), else_block);

    let func = LirFunction {
        name: "wrong_bool_semantics".to_string(),
        param_names: vec!["cond".to_string(), "a".to_string(), "b".to_string()],
        param_types: vec![TirType::Bool, TirType::I64, TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected bool type error");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("semantic Bool"),
        "expected bool semantic error, got: {messages}"
    );
}

#[test]
fn verify_lir_accepts_explicit_box_unbox_ops() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::I64, LirRepr::I64)],
        ops: vec![
            lir_op(
                OpCode::BoxVal,
                &[0],
                vec![lir_value(1, TirType::DynBox, LirRepr::DynBox)],
            ),
            lir_op(
                OpCode::UnboxVal,
                &[1],
                vec![lir_value(2, TirType::I64, LirRepr::I64)],
            ),
        ],
        terminator: LirTerminator::Return {
            values: vec![ValueId(2)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);

    let func = LirFunction {
        name: "box_then_unbox".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    assert!(
        verify_lir_function(&func).is_ok(),
        "explicit box/unbox ops should verify"
    );
}

#[test]
fn verify_lir_rejects_result_id_drift_between_tir_and_lir_op_surfaces() {
    let mut bad_box = lir_op(
        OpCode::BoxVal,
        &[0],
        vec![lir_value(1, TirType::DynBox, LirRepr::DynBox)],
    );
    bad_box.tir_op.results = vec![ValueId(9)];

    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::I64, LirRepr::I64)],
        ops: vec![bad_box],
        terminator: LirTerminator::Return { values: vec![] },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);

    let func = LirFunction {
        name: "result_id_drift".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected op result drift");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("result id drift"),
        "expected result id drift error, got: {messages}"
    );
}

#[test]
fn print_lir_function_emits_representation_annotations() {
    let entry = LirBlock {
        id: BlockId(0),
        args: vec![lir_value(0, TirType::I64, LirRepr::I64)],
        ops: vec![
            lir_op(
                OpCode::BoxVal,
                &[0],
                vec![lir_value(1, TirType::DynBox, LirRepr::DynBox)],
            ),
            lir_op(
                OpCode::UnboxVal,
                &[1],
                vec![lir_value(2, TirType::I64, LirRepr::I64)],
            ),
        ],
        terminator: LirTerminator::Return {
            values: vec![ValueId(2)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);

    let func = LirFunction {
        name: "print_me".to_string(),
        param_names: vec!["x".to_string()],
        param_types: vec![TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    let rendered = print_lir_function(&func);
    assert!(
        rendered.contains("lir.func @print_me"),
        "expected LIR function header, got: {rendered}"
    );
    assert!(
        rendered.contains("%0: i64 [i64]"),
        "expected repr-annotated block arg, got: {rendered}"
    );
    assert!(
        rendered.contains("%1: dynbox [dynbox] = molt.box_val %0"),
        "expected box op rendering, got: {rendered}"
    );
    assert!(
        rendered.contains("%2: i64 [i64] = molt.unbox_val %1"),
        "expected unbox op rendering, got: {rendered}"
    );
}

#[test]
fn verify_lir_rejects_malformed_checked_i64_arithmetic_contract() {
    let mut attrs = AttrDict::new();
    attrs.insert("lir.checked_overflow".to_string(), AttrValue::Bool(true));

    let entry = LirBlock {
        id: BlockId(0),
        args: vec![
            lir_value(0, TirType::I64, LirRepr::I64),
            lir_value(1, TirType::I64, LirRepr::I64),
        ],
        ops: vec![LirOp {
            tir_op: TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![ValueId(0), ValueId(1)],
                results: vec![ValueId(2)],
                attrs,
                source_span: None,
            },
            result_values: vec![lir_value(2, TirType::I64, LirRepr::I64)],
        }],
        terminator: LirTerminator::Return {
            values: vec![ValueId(2)],
        },
    };

    let mut blocks = HashMap::new();
    blocks.insert(BlockId(0), entry);

    let func = LirFunction {
        name: "bad_checked_add".to_string(),
        param_names: vec!["a".to_string(), "b".to_string()],
        param_types: vec![TirType::I64, TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: BlockId(0),
    };

    let err = verify_lir_function(&func).expect_err("expected malformed checked add");
    let messages = err
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        messages.contains("checked i64 arithmetic"),
        "expected checked arithmetic contract error, got: {messages}"
    );
}
