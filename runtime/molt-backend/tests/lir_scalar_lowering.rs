use std::collections::HashMap;

use molt_backend::tir::blocks::{BlockId, Terminator, TirBlock};
use molt_backend::tir::function::TirFunction;
use molt_backend::tir::lower_from_simple::lower_to_tir;
use molt_backend::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction;
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::{TirValue, ValueId};
use molt_backend::tir::verify_lir::verify_lir_function;
use molt_backend::{FunctionIR, OpIR};

fn single_block_func(ops: Vec<TirOp>, return_type: TirType, next_value: u32) -> TirFunction {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![],
        ops,
        terminator: Terminator::Return {
            values: if matches!(return_type, TirType::None) {
                vec![]
            } else {
                vec![ValueId(next_value - 1)]
            },
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    TirFunction {
        name: "test".into(),
        param_names: vec![],
        param_types: vec![],
        return_type,
        blocks,
        entry_block: entry_id,
        next_value,
        next_block: 1,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    }
}

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

fn int_attr(value: i64) -> AttrDict {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(value));
    attrs
}

fn float_attr(value: f64) -> AttrDict {
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Float(value));
    attrs
}

fn opir(kind: &str, args: &[&str], out: Option<&str>) -> OpIR {
    OpIR {
        kind: kind.into(),
        args: if args.is_empty() {
            None
        } else {
            Some(args.iter().map(|arg| (*arg).into()).collect())
        },
        out: out.map(str::to_string),
        ..OpIR::default()
    }
}

fn const_float_opir(out: &str, value: f64) -> OpIR {
    OpIR {
        kind: "const_float".into(),
        f_value: Some(value),
        out: Some(out.into()),
        ..OpIR::default()
    }
}

fn ret_opir(var: &str) -> OpIR {
    OpIR {
        kind: "ret".into(),
        var: Some(var.into()),
        ..OpIR::default()
    }
}

#[test]
fn lower_const_int_to_i64_repr() {
    let ops = vec![make_op(
        OpCode::ConstInt,
        vec![],
        vec![ValueId(0)],
        int_attr(42),
    )];
    let func = single_block_func(ops, TirType::I64, 1);

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&lir.entry_block];
    let op = &entry.ops[0];

    assert_eq!(op.result_values.len(), 1);
    assert_eq!(op.result_values[0].id, ValueId(0));
    assert_eq!(op.result_values[0].ty, TirType::I64);
    assert_eq!(
        op.result_values[0].repr,
        molt_backend::tir::lir::LirRepr::I64
    );
}

#[test]
fn lower_mixed_add_to_f64_repr() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(
            OpCode::ConstFloat,
            vec![],
            vec![ValueId(1)],
            float_attr(2.0),
        ),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let func = single_block_func(ops, TirType::F64, 3);

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&lir.entry_block];
    let op = &entry.ops[2];

    assert_eq!(op.result_values.len(), 1);
    assert_eq!(op.result_values[0].id, ValueId(2));
    assert_eq!(op.result_values[0].ty, TirType::F64);
    assert_eq!(
        op.result_values[0].repr,
        molt_backend::tir::lir::LirRepr::F64
    );
}

#[test]
fn lower_simple_float_param_arithmetic_return_to_f64_repr() {
    let func_ir = FunctionIR {
        name: "interpolate_like".into(),
        params: vec!["a".into(), "b".into(), "w".into()],
        param_types: Some(vec!["float".into(), "float".into(), "float".into()]),
        ops: vec![
            opir("sub", &["b", "a"], Some("v0")),
            const_float_opir("c3", 3.0),
            const_float_opir("c2", 2.0),
            opir("mul", &["w", "c2"], Some("v1")),
            opir("sub", &["c3", "v1"], Some("v2")),
            opir("mul", &["v0", "v2"], Some("v3")),
            opir("mul", &["v3", "w"], Some("v4")),
            opir("mul", &["v4", "w"], Some("v5")),
            opir("add", &["v5", "a"], Some("v6")),
            ret_opir("v6"),
        ],
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);
    assert_eq!(tir.return_type, TirType::F64);
    assert_eq!(tir.blocks[&tir.entry_block].args[0].ty, TirType::F64);

    let lir = lower_function_to_lir_for_repr_fact_extraction(&tir);
    let entry = &lir.blocks[&lir.entry_block];
    let return_value = match &entry.terminator {
        molt_backend::tir::lir::LirTerminator::Return { values } => values[0],
        other => panic!("expected return terminator, got {other:?}"),
    };
    let return_def = entry
        .ops
        .iter()
        .flat_map(|op| op.result_values.iter())
        .find(|value| value.id == return_value)
        .expect("return value should be defined by arithmetic chain");

    assert_eq!(return_def.ty, TirType::F64);
    assert_eq!(return_def.repr, molt_backend::tir::lir::LirRepr::F64);
    assert!(
        verify_lir_function(&lir).is_ok(),
        "float parameter arithmetic return must satisfy LIR verifier"
    );
}

#[test]
fn lower_dynbox_float_arithmetic_return_stays_dynbox() {
    let func_ir = FunctionIR {
        name: "dynamic_float_mix".into(),
        params: vec!["x".into()],
        param_types: None,
        ops: vec![
            const_float_opir("c", 1.5),
            opir("add", &["x", "c"], Some("v0")),
            ret_opir("v0"),
        ],
        source_file: None,
        is_extern: false,
    };

    let tir = lower_to_tir(&func_ir);
    assert_eq!(
        tir.return_type,
        TirType::DynBox,
        "unproven dynamic+float arithmetic must not create an F64 return contract"
    );

    let lir = lower_function_to_lir_for_repr_fact_extraction(&tir);
    let entry = &lir.blocks[&lir.entry_block];
    let return_value = match &entry.terminator {
        molt_backend::tir::lir::LirTerminator::Return { values } => values[0],
        other => panic!("expected return terminator, got {other:?}"),
    };
    let return_def = entry
        .ops
        .iter()
        .flat_map(|op| op.result_values.iter())
        .find(|value| value.id == return_value)
        .expect("return value should be defined by arithmetic op");

    assert_eq!(return_def.ty, TirType::DynBox);
    assert_eq!(return_def.repr, molt_backend::tir::lir::LirRepr::DynBox);
    assert!(
        verify_lir_function(&lir).is_ok(),
        "dynamic+float arithmetic return should not violate LIR verifier"
    );
}

#[test]
fn lower_comparison_to_bool1_repr() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Eq,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let func = single_block_func(ops, TirType::Bool, 3);

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&lir.entry_block];
    let op = &entry.ops[2];

    assert_eq!(op.result_values.len(), 1);
    assert_eq!(op.result_values[0].id, ValueId(2));
    assert_eq!(op.result_values[0].ty, TirType::Bool);
    assert_eq!(
        op.result_values[0].repr,
        molt_backend::tir::lir::LirRepr::Bool1
    );
}

#[test]
fn lower_dynbox_add_to_dynbox_repr() {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![
            TirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
            },
            TirValue {
                id: ValueId(1),
                ty: TirType::I64,
            },
        ],
        ops: vec![make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        )],
        terminator: Terminator::Return {
            values: vec![ValueId(2)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    let func = TirFunction {
        name: "dynbox_add".into(),
        param_names: vec!["x".into(), "y".into()],
        param_types: vec![TirType::DynBox, TirType::I64],
        return_type: TirType::DynBox,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 1,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&lir.entry_block];
    let op = &entry.ops[0];

    assert_eq!(op.result_values.len(), 1);
    assert_eq!(op.result_values[0].id, ValueId(2));
    assert_eq!(op.result_values[0].ty, TirType::DynBox);
    assert_eq!(
        op.result_values[0].repr,
        molt_backend::tir::lir::LirRepr::DynBox
    );
}

#[test]
fn lower_i64_add_with_explicit_overflow_materialization() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let func = single_block_func(ops, TirType::I64, 3);

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&lir.entry_block];
    let add = &entry.ops[2];

    assert_eq!(add.tir_op.opcode, OpCode::Add);
    assert_eq!(add.result_values.len(), 3);
    assert_eq!(add.result_values[0].id, ValueId(2));
    assert_eq!(add.result_values[0].ty, TirType::I64);
    assert_eq!(
        add.result_values[0].repr,
        molt_backend::tir::lir::LirRepr::I64
    );
    assert_eq!(add.result_values[1].ty, TirType::DynBox);
    assert_eq!(
        add.result_values[1].repr,
        molt_backend::tir::lir::LirRepr::DynBox
    );
    assert_eq!(add.result_values[2].ty, TirType::Bool);
    assert_eq!(
        add.result_values[2].repr,
        molt_backend::tir::lir::LirRepr::Bool1
    );
    assert_eq!(
        add.tir_op.attrs.get("lir.checked_overflow"),
        Some(&AttrValue::Bool(true))
    );
    assert!(
        verify_lir_function(&lir).is_ok(),
        "lowered checked add should satisfy verifier"
    );
}

#[test]
fn lower_box_and_unbox_align_with_verifier_contract() {
    let entry_id = BlockId(0);
    let block = TirBlock {
        id: entry_id,
        args: vec![TirValue {
            id: ValueId(0),
            ty: TirType::I64,
        }],
        ops: vec![
            make_op(
                OpCode::BoxVal,
                vec![ValueId(0)],
                vec![ValueId(1)],
                AttrDict::new(),
            ),
            make_op(
                OpCode::UnboxVal,
                vec![ValueId(1)],
                vec![ValueId(2)],
                AttrDict::new(),
            ),
        ],
        terminator: Terminator::Return {
            values: vec![ValueId(2)],
        },
    };
    let mut blocks = HashMap::new();
    blocks.insert(entry_id, block);
    let func = TirFunction {
        name: "box_unbox".into(),
        param_names: vec!["x".into()],
        param_types: vec![TirType::I64],
        return_type: TirType::I64,
        blocks,
        entry_block: entry_id,
        next_value: 3,
        next_block: 1,
        attrs: AttrDict::new(),
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&lir.entry_block];
    assert_eq!(
        entry.ops[0].result_values[0].repr,
        molt_backend::tir::lir::LirRepr::DynBox
    );
    assert_eq!(
        entry.ops[1].result_values[0].repr,
        molt_backend::tir::lir::LirRepr::I64
    );
    assert!(
        verify_lir_function(&lir).is_ok(),
        "lowered box/unbox should satisfy verifier"
    );
}

#[test]
fn lower_truthy_condition_materializes_bool1_before_branch() {
    let entry_id = BlockId(0);
    let then_id = BlockId(1);
    let else_id = BlockId(2);
    let mut blocks = HashMap::new();
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
        value_types: HashMap::new(),
        has_exception_handling: false,
        label_id_map: HashMap::new(),
        loop_roles: HashMap::new(),
        loop_pairs: HashMap::new(),
        loop_break_kinds: HashMap::new(),
        loop_cond_blocks: HashMap::new(),
    };

    let lir = lower_function_to_lir_for_repr_fact_extraction(&func);
    let entry = &lir.blocks[&entry_id];
    assert_eq!(
        entry.ops.len(),
        1,
        "expected explicit truthiness materialization op"
    );
    assert_eq!(entry.ops[0].tir_op.opcode, OpCode::CallBuiltin);
    assert_eq!(
        entry.ops[0].tir_op.attrs.get("lir.truthy_cond"),
        Some(&AttrValue::Bool(true))
    );
    assert!(
        verify_lir_function(&lir).is_ok(),
        "truthiness materialization should satisfy verifier"
    );
}
