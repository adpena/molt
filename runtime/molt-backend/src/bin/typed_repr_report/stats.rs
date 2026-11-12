use std::collections::{BTreeMap, HashMap};

use molt_backend::tir::function::TirFunction;
use molt_backend::tir::ops::{OpCode, TirOp};
use molt_backend::tir::types::TirType;
use molt_backend::tir::values::ValueId;
use molt_ir::repr::Repr;

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
    pub(crate) boxed_values: usize,
}

/// Count each value's carrier from the representation plan that every backend
/// consumes (`repr_by_value_for` over the final TIR).
pub(crate) fn collect_function_stats(
    func: &TirFunction,
    repr: &HashMap<ValueId, Repr>,
) -> FunctionStats {
    let mut stats = FunctionStats::default();
    let mut types = func.value_types.clone();
    for block in func.blocks.values() {
        for arg in &block.args {
            types.entry(arg.id).or_insert_with(|| arg.ty.clone());
        }
    }

    let mut block_ids = func.blocks.keys().copied().collect::<Vec<_>>();
    block_ids.sort_by_key(|bid| bid.0);
    for block_id in block_ids {
        let block = &func.blocks[&block_id];
        for arg in &block.args {
            record_value(&mut stats, &arg.ty, repr.get(&arg.id).copied());
        }

        for op in &block.ops {
            let opcode_name = report_opcode_name(op);
            let operand_tuple = op
                .operands
                .iter()
                .map(|operand| repr.get(operand).copied().map_or("unknown", repr_name))
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

            for result in &op.results {
                let result_repr = repr.get(result).copied();
                let ty = types.get(result).cloned().unwrap_or(TirType::DynBox);
                record_value(&mut stats, &ty, result_repr);
                let opcode_stats = stats.opcodes.entry(opcode_name.clone()).or_default();
                *opcode_stats
                    .result_reprs
                    .entry(result_repr.map_or("unknown", repr_name).to_string())
                    .or_insert(0) += 1;
                if result_repr.is_some_and(is_boxed) {
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
    match op.origin {
        Some(kind) => format!("Copy::{kind}"),
        _ => "Copy".to_string(),
    }
}

/// A NaN-boxed word: the carrier pays boxing and tag checks.
fn is_boxed(repr: Repr) -> bool {
    matches!(repr, Repr::MaybeBigInt | Repr::BoxedImmortal | Repr::DynBox)
}

fn record_value(stats: &mut FunctionStats, ty: &TirType, repr: Option<Repr>) {
    *stats
        .values_by_repr
        .entry(repr.map_or("unknown", repr_name).to_string())
        .or_insert(0) += 1;
    *stats.values_by_type.entry(type_name(ty)).or_insert(0) += 1;
    match repr {
        Some(Repr::RawI64FullDeopt | Repr::RawI64Safe | Repr::Bool | Repr::FloatUnboxed) => {
            stats.scalar_values += 1
        }
        Some(repr) if is_boxed(repr) => stats.boxed_values += 1,
        _ => {}
    }
}
