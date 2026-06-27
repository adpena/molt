use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, Read};

use molt_backend::SimpleIR;
use molt_backend::tir::lir::{LirFunction, LirRepr};
use molt_backend::tir::ops::{AttrValue, OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;
use molt_tir::ir_rewrites::{rewrite_annotate_stubs, rewrite_phi_to_store_load};
use serde_json::{Value, json};

#[derive(Default)]
struct OpcodeStats {
    total: usize,
    result_reprs: BTreeMap<String, usize>,
    operand_repr_tuples: BTreeMap<String, usize>,
    boxed_result_values: usize,
}

#[derive(Default)]
struct FunctionStats {
    values_by_repr: BTreeMap<String, usize>,
    values_by_type: BTreeMap<String, usize>,
    opcodes: BTreeMap<String, OpcodeStats>,
    scalar_values: usize,
    reference_values: usize,
    boxed_values: usize,
}

fn main() {
    let outcome = run();
    match outcome {
        Ok((payload, verified)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).expect("report JSON must serialize")
            );
            if !verified {
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("typed_repr_report: {err}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<(Value, bool), String> {
    let input = read_input()?;
    let mut ir: SimpleIR =
        serde_json::from_str(&input).map_err(|err| format!("invalid SimpleIR JSON: {err}"))?;
    rewrite_annotate_stubs(&mut ir);

    let mut function_reports = Vec::with_capacity(ir.functions.len());
    let mut verified = true;
    for func in &mut ir.functions {
        if func.ops.iter().any(|op| op.kind == "phi") {
            rewrite_phi(func);
        }

        let mut tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(func);
        molt_backend::tir::type_refine::refine_types(&mut tir_func);
        let pass_stats = molt_backend::tir::passes::run_pipeline(
            &mut tir_func,
            &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
        );
        molt_backend::tir::type_refine::refine_types(&mut tir_func);
        let lir_func =
            molt_backend::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction(
                &tir_func,
            );

        let lir_errors = molt_backend::tir::verify_lir::verify_lir_function(&lir_func)
            .err()
            .unwrap_or_default();
        let repr_violations =
            molt_backend::tir::verify_lir_repr::verify_register_passable(&lir_func);
        if !lir_errors.is_empty() || !repr_violations.is_empty() {
            verified = false;
        }

        function_reports.push(json!({
            "name": lir_func.name,
            "blocks": lir_func.blocks.len(),
            "passes": pass_stats.iter().map(|stat| {
                json!({
                    "name": stat.name,
                    "values_changed": stat.values_changed,
                    "ops_removed": stat.ops_removed,
                    "ops_added": stat.ops_added,
                })
            }).collect::<Vec<_>>(),
            "stats": function_stats_json(&collect_function_stats(&lir_func)),
            "verification": {
                "lir_errors": lir_errors.iter().map(|err| format!("{err:?}")).collect::<Vec<_>>(),
                "repr_violations": repr_violations.iter().map(|violation| {
                    json!({
                        "block": violation.block.0,
                        "value": violation.value_id.0,
                        "expected_type": type_name(&violation.expected_type),
                        "expected_repr": repr_name(violation.expected_repr),
                        "actual_repr": repr_name(violation.actual_repr),
                    })
                }).collect::<Vec<_>>(),
            },
        }));
    }

    let aggregate = aggregate_functions(&function_reports);
    Ok((
        json!({
            "schema": "molt.typed_repr_report.v1",
            "verified": verified,
            "functions": function_reports,
            "aggregate": aggregate,
        }),
        verified,
    ))
}

fn read_input() -> Result<String, String> {
    let mut args = env::args().skip(1);
    let mut input_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stdin" => {}
            "--ir-file" => {
                input_path = Some(
                    args.next()
                        .ok_or_else(|| "--ir-file requires a path".to_string())?,
                );
            }
            "--json" => {}
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if let Some(path) = input_path {
        fs::read_to_string(&path).map_err(|err| format!("failed to read {path}: {err}"))
    } else {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|err| format!("failed to read stdin: {err}"))?;
        Ok(input)
    }
}

fn rewrite_phi(func: &mut molt_backend::FunctionIR) {
    rewrite_phi_to_store_load(&mut func.ops);
}

fn collect_function_stats(func: &LirFunction) -> FunctionStats {
    let mut stats = FunctionStats::default();
    let mut value_reprs: HashMap<ValueId, LirRepr> = HashMap::new();

    let mut block_ids = func.blocks.keys().copied().collect::<Vec<_>>();
    block_ids.sort_by_key(|bid| bid.0);
    for block_id in block_ids {
        let block = &func.blocks[&block_id];
        for arg in &block.args {
            record_value(&mut stats, &mut value_reprs, arg.id, &arg.ty, arg.repr);
        }

        for op in &block.ops {
            let opcode_name = report_opcode_name(&op.tir_op);
            let operand_tuple = op
                .tir_op
                .operands
                .iter()
                .map(|operand| {
                    value_reprs
                        .get(operand)
                        .copied()
                        .map(repr_name)
                        .unwrap_or("unknown")
                })
                .collect::<Vec<_>>()
                .join(",");

            {
                let opcode_stats = stats.opcodes.entry(opcode_name.clone()).or_default();
                opcode_stats.total += 1;
                *opcode_stats
                    .operand_repr_tuples
                    .entry(operand_tuple)
                    .or_insert(0) += 1;
            }

            for result in &op.result_values {
                record_value(
                    &mut stats,
                    &mut value_reprs,
                    result.id,
                    &result.ty,
                    result.repr,
                );
                let result_repr = repr_name(result.repr).to_string();
                let opcode_stats = stats.opcodes.entry(opcode_name.clone()).or_default();
                *opcode_stats.result_reprs.entry(result_repr).or_insert(0) += 1;
                if result.repr == LirRepr::DynBox {
                    opcode_stats.boxed_result_values += 1;
                }
            }
        }
    }

    stats
}

fn report_opcode_name(op: &TirOp) -> String {
    if op.opcode != OpCode::Copy {
        return format!("{:?}", op.opcode);
    }
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => format!("Copy::{kind}"),
        _ => "Copy".to_string(),
    }
}

fn record_value(
    stats: &mut FunctionStats,
    value_reprs: &mut HashMap<ValueId, LirRepr>,
    value: ValueId,
    ty: &TirType,
    repr: LirRepr,
) {
    value_reprs.insert(value, repr);
    *stats
        .values_by_repr
        .entry(repr_name(repr).to_string())
        .or_insert(0) += 1;
    *stats.values_by_type.entry(type_name(ty)).or_insert(0) += 1;
    match repr {
        LirRepr::I64 | LirRepr::F64 | LirRepr::Bool1 => stats.scalar_values += 1,
        LirRepr::Ref64 => stats.reference_values += 1,
        LirRepr::DynBox => {
            stats.reference_values += 1;
            stats.boxed_values += 1;
        }
    }
}

fn function_stats_json(stats: &FunctionStats) -> Value {
    json!({
        "values_by_repr": stats.values_by_repr,
        "values_by_type": stats.values_by_type,
        "scalar_values": stats.scalar_values,
        "reference_values": stats.reference_values,
        "boxed_values": stats.boxed_values,
        "opcodes": stats.opcodes.iter().map(|(name, opcode)| {
            (name.clone(), json!({
                "total": opcode.total,
                "result_reprs": opcode.result_reprs,
                "operand_repr_tuples": opcode.operand_repr_tuples,
                "boxed_result_values": opcode.boxed_result_values,
            }))
        }).collect::<serde_json::Map<_, _>>(),
    })
}

fn aggregate_functions(functions: &[Value]) -> Value {
    let mut stats = FunctionStats::default();
    let mut lir_errors = 0usize;
    let mut repr_violations = 0usize;

    for function in functions {
        let function_stats = &function["stats"];
        merge_count_map(&mut stats.values_by_repr, &function_stats["values_by_repr"]);
        merge_count_map(&mut stats.values_by_type, &function_stats["values_by_type"]);
        stats.scalar_values += function_stats["scalar_values"].as_u64().unwrap_or(0) as usize;
        stats.reference_values += function_stats["reference_values"].as_u64().unwrap_or(0) as usize;
        stats.boxed_values += function_stats["boxed_values"].as_u64().unwrap_or(0) as usize;
        merge_opcode_maps(&mut stats.opcodes, &function_stats["opcodes"]);

        lir_errors += function["verification"]["lir_errors"]
            .as_array()
            .map_or(0, Vec::len);
        repr_violations += function["verification"]["repr_violations"]
            .as_array()
            .map_or(0, Vec::len);
    }

    let mut aggregate = function_stats_json(&stats);
    aggregate["functions"] = json!(functions.len());
    aggregate["lir_errors"] = json!(lir_errors);
    aggregate["repr_violations"] = json!(repr_violations);
    aggregate
}

fn merge_count_map(target: &mut BTreeMap<String, usize>, value: &Value) {
    if let Some(map) = value.as_object() {
        for (key, count) in map {
            *target.entry(key.clone()).or_insert(0) += count.as_u64().unwrap_or(0) as usize;
        }
    }
}

fn merge_opcode_maps(target: &mut BTreeMap<String, OpcodeStats>, value: &Value) {
    let Some(map) = value.as_object() else {
        return;
    };
    for (opcode, raw_stats) in map {
        let entry = target.entry(opcode.clone()).or_default();
        entry.total += raw_stats["total"].as_u64().unwrap_or(0) as usize;
        entry.boxed_result_values +=
            raw_stats["boxed_result_values"].as_u64().unwrap_or(0) as usize;
        merge_count_map(&mut entry.result_reprs, &raw_stats["result_reprs"]);
        merge_count_map(
            &mut entry.operand_repr_tuples,
            &raw_stats["operand_repr_tuples"],
        );
    }
}

fn repr_name(repr: LirRepr) -> &'static str {
    match repr {
        LirRepr::DynBox => "dynbox",
        LirRepr::Ref64 => "ref64",
        LirRepr::I64 => "i64",
        LirRepr::F64 => "f64",
        LirRepr::Bool1 => "bool1",
    }
}

fn type_name(ty: &TirType) -> String {
    match ty {
        TirType::I64 => "i64".to_string(),
        TirType::F64 => "f64".to_string(),
        TirType::Bool => "bool".to_string(),
        TirType::None => "none".to_string(),
        TirType::Str => "str".to_string(),
        TirType::Bytes => "bytes".to_string(),
        TirType::List(inner) => format!("list[{}]", type_name(inner)),
        TirType::Dict(key, value) => format!("dict[{},{}]", type_name(key), type_name(value)),
        TirType::Set(inner) => format!("set[{}]", type_name(inner)),
        TirType::Tuple(items) => {
            let inner = items.iter().map(type_name).collect::<Vec<_>>().join(",");
            format!("tuple[{inner}]")
        }
        TirType::Iterator(inner) => format!("iterator[{}]", type_name(inner)),
        TirType::Box(inner) => format!("box[{}]", type_name(inner)),
        TirType::DynBox => "dynbox".to_string(),
        TirType::UserClass(name) => format!("userclass[{name}]"),
        TirType::Func(signature) => format!(
            "func[({})->{}]",
            signature
                .params
                .iter()
                .map(type_name)
                .collect::<Vec<_>>()
                .join(","),
            type_name(&signature.return_type)
        ),
        TirType::BigInt => "bigint".to_string(),
        TirType::Ptr(inner) => format!("ptr[{}]", type_name(inner)),
        TirType::Union(items) => {
            let inner = items.iter().map(type_name).collect::<Vec<_>>().join("|");
            format!("union[{inner}]")
        }
        TirType::Never => "never".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_backend::tir::blocks::BlockId;
    use molt_backend::tir::lir::{LirBlock, LirOp, LirTerminator, LirValue};
    use molt_backend::tir::ops::{AttrDict, Dialect};

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
}
