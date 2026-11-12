use std::collections::HashMap;

use molt_backend::tir::blocks::Terminator;
use molt_backend::tir::function::TirFunction;
use molt_backend::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;
use molt_ir::repr::Repr;
use serde_json::json;

use super::json_report::aggregate_functions;
use super::stats::collect_function_stats;

fn op(opcode: OpCode, origin: Option<molt_ir::OpKind>, operands: &[u32], results: &[u32]) -> TirOp {
    TirOp {
        origin,
        runtime: None,
        dialect: Dialect::Molt,
        opcode,
        operands: operands.iter().copied().map(ValueId).collect(),
        results: results.iter().copied().map(ValueId).collect(),
        attrs: AttrDict::new(),
        source_span: None,
    }
}

/// A function whose entry block holds `ops` and returns `result`.
fn function(params: Vec<TirType>, ops: Vec<TirOp>, result: u32) -> TirFunction {
    let mut func = TirFunction::new(
        "report_case".into(),
        params,
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let entry = func.entry_block;
    let block = func.blocks.get_mut(&entry).unwrap();
    block.ops = ops;
    block.terminator = Terminator::Return {
        values: vec![ValueId(result)],
    };
    func
}

#[test]
fn counts_immortal_words_as_boxed_transport() {
    let func = function(vec![], vec![op(OpCode::ConstNone, None, &[], &[0])], 0);
    let repr = HashMap::from([(ValueId(0), Repr::BoxedImmortal)]);
    let stats = collect_function_stats(&func, &repr);
    assert_eq!(stats.scalar_values, 0);
    assert_eq!(stats.boxed_values, 1);
    assert_eq!(stats.values_by_repr["boxed_immortal"], 1);
    assert_eq!(stats.opcodes["ConstNone"].result_reprs["boxed_immortal"], 1);
    assert_eq!(stats.opcodes["ConstNone"].boxed_result_values, 1);
}

#[test]
fn counts_raw_scalar_carriers() {
    let func = function(
        vec![TirType::I64, TirType::I64],
        vec![op(OpCode::Add, None, &[0, 1], &[2])],
        2,
    );
    let repr = HashMap::from([
        (ValueId(0), Repr::RawI64Safe),
        (ValueId(1), Repr::RawI64Safe),
        (ValueId(2), Repr::RawI64FullDeopt),
    ]);
    let stats = collect_function_stats(&func, &repr);
    assert_eq!(stats.scalar_values, 3);
    assert_eq!(stats.boxed_values, 0);
    assert_eq!(stats.values_by_repr["raw_i64_safe"], 2);
    assert_eq!(
        stats.opcodes["Add"].operand_repr_tuples["raw_i64_safe,raw_i64_safe"],
        1
    );
    assert_eq!(stats.opcodes["Add"].result_reprs["raw_i64_full_deopt"], 1);
}

#[test]
fn maybe_bigint_is_boxed_and_a_missing_carrier_is_unknown() {
    let func = function(
        vec![TirType::I64],
        vec![op(OpCode::Neg, None, &[0], &[1])],
        1,
    );
    let repr = HashMap::from([(ValueId(0), Repr::MaybeBigInt)]);
    let stats = collect_function_stats(&func, &repr);
    assert_eq!(stats.boxed_values, 1);
    assert_eq!(stats.values_by_repr["maybe_bigint"], 1);
    assert_eq!(stats.values_by_repr["unknown"], 1);
    assert_eq!(stats.opcodes["Neg"].result_reprs["unknown"], 1);
}

#[test]
fn separates_plain_copy_from_fallback_semantic_copy() {
    let func = function(
        vec![TirType::DynBox],
        vec![
            op(OpCode::Copy, None, &[0], &[1]),
            op(
                OpCode::Copy,
                Some(molt_ir::OpKind::UnpackSequence),
                &[1],
                &[2],
            ),
        ],
        2,
    );
    let repr = HashMap::from([
        (ValueId(0), Repr::DynBox),
        (ValueId(1), Repr::DynBox),
        (ValueId(2), Repr::DynBox),
    ]);
    let stats = collect_function_stats(&func, &repr);
    assert_eq!(stats.opcodes["Copy"].total, 1);
    assert_eq!(stats.opcodes["Copy::unpack_sequence"].total, 1);
    assert_eq!(stats.boxed_values, 3);
}

#[test]
fn typed_report_schema_stays_stable() {
    let function = json!({
            "name": "add_ints",
            "stats": {
                "values_by_repr": {"raw_i64_safe": 1},
                "values_by_type": {"i64": 1},
                "scalar_values": 1,
                "boxed_values": 0,
                "opcodes": {
                    "ConstInt": {
                        "total": 1,
                        "result_reprs": {"raw_i64_safe": 1},
                        "operand_repr_tuples": {"": 1},
                        "boxed_result_values": 0
                    }
                }
            },
            "verification": {"tir_errors": []}
    });
    let aggregate = aggregate_functions(&[function]);

    assert_eq!(aggregate["functions"], 1);
    assert_eq!(aggregate["scalar_values"], 1);
    assert_eq!(aggregate["tir_errors"], 0);
    assert_eq!(
        aggregate["opcodes"]["ConstInt"]["result_reprs"]["raw_i64_safe"],
        1
    );
}
