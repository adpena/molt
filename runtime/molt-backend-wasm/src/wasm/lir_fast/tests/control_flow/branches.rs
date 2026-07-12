use super::super::*;

#[test]
fn conditional_branch() {
    let mut func = TirFunction::new("cond_branch".into(), vec![TirType::Bool], TirType::I64);

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();

    let ret_then = func.fresh_value();
    let ret_else = func.fresh_value();

    // Patch entry block to branch on param.
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    };

    let then_block = TirBlock {
        id: then_id,
        args: vec![],
        ops: vec![TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ret_then],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(1));
                m
            },
            source_span: None,
        }],
        terminator: Terminator::Return {
            values: vec![ret_then],
        },
    };

    let else_block = TirBlock {
        id: else_id,
        args: vec![],
        ops: vec![TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ret_else],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(0));
                m
            },
            source_span: None,
        }],
        terminator: Terminator::Return {
            values: vec![ret_else],
        },
    };

    func.blocks.insert(then_id, then_block);
    func.blocks.insert(else_id, else_block);

    let output = lower_tir_to_wasm(&func).test_view();

    // Should contain br_if for the conditional branch.
    let has_br_if = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::BrIf(_)));
    assert!(
        has_br_if,
        "expected br_if instruction for conditional branch"
    );
}

#[test]
fn dynbox_bool_uses_lir_truthiness_without_generic_bail() {
    let mut func = TirFunction::new("bool_dynbox".into(), vec![TirType::DynBox], TirType::Bool);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Bool,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "boxed bool() must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "boxed bool() must dispatch non-bool objects through is_truthy; got {:?}",
        output.runtime_calls
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == QNAN_TAG_MASK_I64)
        ),
        "boxed truthiness must retain the inline boxed-bool path"
    );
}

#[test]
fn dynbox_conditional_branch_uses_lir_truthiness_without_generic_bail() {
    let mut func = TirFunction::new(
        "cond_branch_dynbox".into(),
        vec![TirType::DynBox],
        TirType::I64,
    );

    let then_id = func.fresh_block();
    let else_id = func.fresh_block();
    let ret_then = func.fresh_value();
    let ret_else = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    };

    func.blocks.insert(
        then_id,
        TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_then],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(1));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_then],
            },
        },
    );
    func.blocks.insert(
        else_id,
        TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_else],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), AttrValue::Int(0));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_else],
            },
        },
    );

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "boxed conditional branch must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"is_truthy"),
        "boxed conditional branch must dispatch non-bool objects through is_truthy; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::BrIf(_))),
        "conditional branch must still emit br_if"
    );
}

#[test]
fn comparison_i64_emits_native() {
    let func = make_lt_two_consts_func(20, 22);

    let output = lower_tir_to_wasm(&func).test_view();

    let has_lt = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64LtS));
    assert!(has_lt, "expected i64.lt_s instruction");
}
