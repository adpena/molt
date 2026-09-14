#![cfg(feature = "wasm-backend")]

use molt_backend::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_backend::tir::function::TirFunction;
use molt_backend::tir::lir::LirRepr;
use molt_backend::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction;
use molt_backend::tir::ops::AttrDict;
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::{TirValue, ValueId};
use molt_backend_wasm::test_util::lower_lir_to_wasm;
use wasm_encoder::{Instruction, ValType};

fn empty_tir_function(
    name: &str,
    blocks: std::collections::HashMap<BlockId, TirBlock>,
    return_type: TirType,
    next_value: u32,
    next_block: u32,
) -> TirFunction {
    TirFunction {
        name: name.into(),
        execution_context: Default::default(),
        param_names: vec![],
        param_types: vec![],
        return_type,
        blocks,
        entry_block: BlockId(0),
        next_value,
        next_block,
        attrs: AttrDict::new(),
        value_types: std::collections::HashMap::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    }
}

#[test]
fn wasm_lir_ref64_parameter_uses_i64_reference_word() {
    let entry_id = BlockId(0);
    let obj = ValueId(0);
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: obj,
                ty: TirType::UserClass("Point".into()),
            }],
            ops: vec![],
            terminator: Terminator::Return { values: vec![obj] },
        },
    );
    let mut func = empty_tir_function(
        "ref64_parameter",
        blocks,
        TirType::UserClass("Point".into()),
        1,
        1,
    );

    func.param_names = vec!["object".into()];
    func.param_types = vec![TirType::UserClass("Point".into())];
    let mut lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    lir.blocks.get_mut(&entry_id).unwrap().args[0].repr = LirRepr::Ref64;

    let output = lower_lir_to_wasm(&lir);

    assert_eq!(output.result_types, vec![ValType::I64]);
    assert!(output.locals.is_empty());
}

#[test]
fn wasm_lir_ref64_condition_uses_runtime_truthiness() {
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let obj = ValueId(0);
    let mut blocks = std::collections::HashMap::new();
    blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![TirValue {
                id: obj,
                ty: TirType::UserClass("Point".into()),
            }],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: obj,
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
    let mut func = empty_tir_function("ref64_truthy", blocks, TirType::None, 1, 3);

    func.param_names = vec!["object".into()];
    func.param_types = vec![TirType::UserClass("Point".into())];
    let mut lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    lir.blocks.get_mut(&entry_id).unwrap().args[0].repr = LirRepr::Ref64;

    let output = lower_lir_to_wasm(&lir);

    assert!(!output.bails_to_generic_path);
    assert_eq!(output.bail_to_generic_reason, None);
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "Ref64 condition lowering must call is_truthy instead of treating the reference word as integer nonzero; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::BrIf(_))),
        "expected br_if after Ref64 truthiness materialization"
    );
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
        execution_context: Default::default(),
        param_names: vec!["x".into()],
        param_types: vec![TirType::DynBox],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 1,
        next_block: 3,
        attrs: AttrDict::new(),
        value_types: std::collections::HashMap::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let output = lower_lir_to_wasm(&lir);

    assert!(output.locals.contains(&ValType::I32));
    assert!(
        !output.bails_to_generic_path,
        "DynBox truthiness must stay in the LIR fast lane via typed runtime dispatch"
    );
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "DynBox truthiness must call is_truthy; got {:?}",
        output.runtime_calls
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
        execution_context: Default::default(),
        param_names: vec!["i".into()],
        param_types: vec![TirType::I64],
        return_type: TirType::None,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 3,
        attrs: AttrDict::new(),
        value_types: std::collections::HashMap::new(),
        has_exception_handling: false,
        label_id_map: std::collections::HashMap::new(),
        loop_roles: std::collections::HashMap::new(),
        loop_pairs: std::collections::HashMap::new(),
        loop_break_kinds: std::collections::HashMap::new(),
        loop_cond_blocks: std::collections::HashMap::new(),
    };

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let output = lower_lir_to_wasm(&lir);

    assert_eq!(output.param_types, vec![ValType::I64]);
    assert!(output.locals.iter().all(|ty| *ty == ValType::I64));
}
