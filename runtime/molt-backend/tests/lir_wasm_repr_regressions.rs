#![cfg(feature = "wasm-backend")]

use molt_backend::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_backend::tir::function::TirFunction;
use molt_backend::tir::lower_to_lir::lower_function_to_lir;
use molt_backend::tir::lower_to_wasm::lower_lir_to_wasm;
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::{TirValue, ValueId};
use wasm_encoder::{Instruction, ValType};

fn make_op(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: AttrDict,
) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

#[test]
fn wasm_lir_truthiness_materialization_uses_bool_local_and_br_if() {
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            },
        },
    );
    blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    let func = TirFunction {
        name: "truthy_branch".into(),
        param_names: vec!["x".into()],
        param_types: vec![TirType::DynBox],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 1,
        next_block: 3,
        attrs: AttrDict::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    let lir = lower_function_to_lir(&func);
    let output = lower_lir_to_wasm(&lir);

    assert!(output.locals.contains(&ValType::I32));
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_))),
        "DynBox truthiness must lower through runtime truthiness"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::BrIf(_))),
        "expected br_if from Bool1 condition"
    );
}

#[test]
fn wasm_lir_loop_carried_i64_params_stay_i64() {
    let entry_id = BlockId(0);
    let header_id = BlockId(1);
    let body_id = BlockId(2);
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: ValueId(0),
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: header_id,
                args: vec![ValueId(0)],
            },
        },
    );
    blocks.insert(
        header_id,
        TirBlock {
            id: header_id,
            args: vec![TirValue {
                id: ValueId(1),
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: body_id,
                args: vec![ValueId(1)],
            },
        },
    );
    blocks.insert(
        body_id,
        TirBlock {
            id: body_id,
            args: vec![TirValue {
                id: ValueId(2),
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    let func = TirFunction {
        name: "loop_i64".into(),
        param_names: vec!["i".into()],
        param_types: vec![TirType::I64],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 3,
        attrs: AttrDict::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    let lir = lower_function_to_lir(&func);
    let output = lower_lir_to_wasm(&lir);

    assert_eq!(output.param_types, vec![ValType::I64]);
    assert!(output.locals.iter().all(|ty| *ty == ValType::I64));
}

#[test]
fn wasm_lir_checked_i64_add_does_not_emit_plain_i64_add() {
    let entry_id = BlockId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(1));
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![],
            ops: vec![
                make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], attrs.clone()),
                make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], attrs),
                make_op(
                    OpCode::Add,
                    vec![ValueId(0), ValueId(1)],
                    vec![ValueId(2)],
                    AttrDict::new(),
                ),
            ],
            terminator: Terminator::Return {
                values: vec![ValueId(2)],
            },
        },
    );
    let func = TirFunction {
        name: "checked_add".into(),
        param_names: vec![],
        param_types: vec![],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 1,
        attrs: AttrDict::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    let lir = lower_function_to_lir(&func);
    let output = lower_lir_to_wasm(&lir);

    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::Call(_))),
        "checked arithmetic should materialize an overflow slow path call"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Add)),
        "checked arithmetic should still compute the fast i64 sum"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::If(_))),
        "checked arithmetic should branch on inline-overflow state"
    );
}
