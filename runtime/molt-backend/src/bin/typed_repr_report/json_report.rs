use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::stats::{FunctionStats, OpcodeStats};

pub(crate) fn function_stats_json(stats: &FunctionStats) -> Value {
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

pub(crate) fn aggregate_functions(functions: &[Value]) -> Value {
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
