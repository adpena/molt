use std::collections::HashMap;

use molt_backend::tir::blocks::BlockId;
use molt_backend::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;
use serde_json::json;

use super::json_report::aggregate_functions;
use super::stats::collect_function_stats;

#[test]
fn counts_lir_scalar_representations() {
    let entry = BlockId(0);
    let mut blocks = HashMap::new();
    blocks.insert(
        entry,
        LirBlock {
            id: entry,
            args: vec![
                LirValue {
                    id: ValueId(0),
                    ty: TirType::I64,
                    repr: LirRepr::I64,
                },
                LirValue {
                    id: ValueId(1),
                    ty: TirType::I64,
                    repr: LirRepr::I64,
                },
            ],
            ops: vec![LirOp {
                tir_op: TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Add,
                    operands: vec![ValueId(0), ValueId(1)],
                    results: vec![ValueId(2)],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
                result_values: vec![LirValue {
                    id: ValueId(2),
                    ty: TirType::I64,
                    repr: LirRepr::I64,
                }],
            }],
            terminator: LirTerminator::Return {
                values: vec![ValueId(2)],
            },
        },
    );
    let lir_func = LirFunction {
        name: "add_ints".into(),
        param_names: vec!["a".into(), "b".into()],
        param_types: vec![TirType::I64, TirType::I64],
        return_types: vec![TirType::I64],
        blocks,
        entry_block: entry,
        label_id_map: HashMap::new(),
    };

    let stats = collect_function_stats(&lir_func);

    assert_eq!(stats.scalar_values, 3);
    assert_eq!(stats.reference_values, 0);
    assert_eq!(stats.values_by_repr.get("i64").copied(), Some(3));
    assert_eq!(stats.opcodes["Add"].operand_repr_tuples["i64,i64"], 1);
    assert_eq!(stats.opcodes["Add"].result_reprs["i64"], 1);
}

#[test]
fn counts_ref64_as_reference_not_semantic_scalar() {
    let entry = BlockId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("_type_hint".into(), AttrValue::Str("Point".into()));
    attrs.insert("value".into(), AttrValue::Int(24));
    let mut blocks = HashMap::new();
    blocks.insert(
        entry,
        LirBlock {
            id: entry,
            args: vec![],
            ops: vec![LirOp {
                tir_op: TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ObjectNewBoundStack,
                    operands: vec![],
                    results: vec![ValueId(0)],
                    attrs,
                    source_span: None,
                },
                result_values: vec![LirValue {
                    id: ValueId(0),
                    ty: TirType::UserClass("Point".into()),
                    repr: LirRepr::Ref64,
                }],
            }],
            terminator: LirTerminator::Return {
                values: vec![ValueId(0)],
            },
        },
    );
    let lir_func = LirFunction {
        name: "alloc_point".into(),
        param_names: vec![],
        param_types: vec![],
        return_types: vec![TirType::UserClass("Point".into())],
        blocks,
        entry_block: entry,
        label_id_map: HashMap::new(),
    };

    let stats = collect_function_stats(&lir_func);

    assert_eq!(stats.scalar_values, 0);
    assert_eq!(stats.reference_values, 1);
    assert_eq!(stats.boxed_values, 0);
    assert_eq!(stats.values_by_repr.get("ref64").copied(), Some(1));
    assert_eq!(
        stats.opcodes["ObjectNewBoundStack"].result_reprs["ref64"],
        1
    );
}

#[test]
fn separates_plain_copy_from_fallback_semantic_copy() {
    let entry = BlockId(0);
    let mut fallback_attrs = AttrDict::new();
    fallback_attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("unpack_sequence".into()),
    );
    let mut blocks = HashMap::new();
    blocks.insert(
        entry,
        LirBlock {
            id: entry,
            args: vec![LirValue {
                id: ValueId(0),
                ty: TirType::DynBox,
                repr: LirRepr::DynBox,
            }],
            ops: vec![
                LirOp {
                    tir_op: TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![ValueId(0)],
                        results: vec![ValueId(1)],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                    result_values: vec![LirValue {
                        id: ValueId(1),
                        ty: TirType::DynBox,
                        repr: LirRepr::DynBox,
                    }],
                },
                LirOp {
                    tir_op: TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Copy,
                        operands: vec![ValueId(1)],
                        results: vec![ValueId(2)],
                        attrs: fallback_attrs,
                        source_span: None,
                    },
                    result_values: vec![LirValue {
                        id: ValueId(2),
                        ty: TirType::DynBox,
                        repr: LirRepr::DynBox,
                    }],
                },
            ],
            terminator: LirTerminator::Return {
                values: vec![ValueId(2)],
            },
        },
    );
    let lir_func = LirFunction {
        name: "copy_kinds".into(),
        param_names: vec!["value".into()],
        param_types: vec![TirType::DynBox],
        return_types: vec![TirType::DynBox],
        blocks,
        entry_block: entry,
        label_id_map: HashMap::new(),
    };

    let stats = collect_function_stats(&lir_func);

    assert_eq!(stats.opcodes["Copy"].total, 1);
    assert_eq!(stats.opcodes["Copy::unpack_sequence"].total, 1);
}

#[test]
fn typed_report_schema_stays_stable() {
    let function = json!({
            "name": "add_ints",
            "stats": {
                "values_by_repr": {"i64": 1},
                "values_by_type": {"i64": 1},
                "scalar_values": 1,
                "reference_values": 0,
                "boxed_values": 0,
                "opcodes": {
                    "ConstInt": {
                        "total": 1,
                        "result_reprs": {"i64": 1},
                        "operand_repr_tuples": {"": 1},
                        "boxed_result_values": 0
                    }
                }
            },
            "verification": {"lir_errors": [], "repr_violations": []}
    });
    let aggregate = aggregate_functions(&[function]);

    assert_eq!(aggregate["functions"], 1);
    assert_eq!(aggregate["scalar_values"], 1);
    assert_eq!(aggregate["reference_values"], 0);
    assert_eq!(aggregate["opcodes"]["ConstInt"]["result_reprs"]["i64"], 1);
}
