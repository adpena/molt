use std::collections::{BTreeMap, HashMap};

use molt_backend::tir::lir::{LirFunction, LirRepr};
use molt_backend::tir::ops::{AttrValue, OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;

use super::names::{repr_name, type_name};

#[derive(Default)]
pub(crate) struct OpcodeStats {
    pub(crate) total: usize,
    pub(crate) result_reprs: BTreeMap<String, usize>,
    pub(crate) operand_repr_tuples: BTreeMap<String, usize>,
    pub(crate) boxed_result_values: usize,
}

#[derive(Default)]
pub(crate) struct FunctionStats {
    pub(crate) values_by_repr: BTreeMap<String, usize>,
    pub(crate) values_by_type: BTreeMap<String, usize>,
    pub(crate) opcodes: BTreeMap<String, OpcodeStats>,
    pub(crate) scalar_values: usize,
    pub(crate) reference_values: usize,
    pub(crate) boxed_values: usize,
}

pub(crate) fn collect_function_stats(func: &LirFunction) -> FunctionStats {
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

pub(crate) fn report_opcode_name(op: &TirOp) -> String {
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
