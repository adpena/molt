use melior::{
    Context as MlirContext,
    ir::{
        Block, Location, Type, Value,
        operation::{OperationBuilder, OperationLike},
    },
};
use molt_backend::tir::{
    op_kinds_generated::opcode_canonical_kind_table,
    ops::{Dialect as TirDialect, TirOp},
};

use super::values::{ValueMap, resolve_value};

pub(super) fn emit_opaque_molt_op<'c, 'a>(
    _ctx: &'c MlirContext,
    block: &'a Block<'c>,
    op: &TirOp,
    value_map: &mut ValueMap<'c, 'a>,
    i64_type: Type<'c>,
    location: Location<'c>,
) -> Result<(), String> {
    let dialect_name = match op.dialect {
        TirDialect::Molt => "molt",
        TirDialect::Scf => "scf",
        TirDialect::Gpu => "molt_gpu",
        TirDialect::Par => "molt_par",
        TirDialect::Simd => "molt_simd",
    };
    let op_name = opcode_canonical_kind_table(op.opcode);
    let full_name = format!("{dialect_name}.{op_name}");

    let operands: Result<Vec<Value<'c, '_>>, String> = op
        .operands
        .iter()
        .map(|&vid| resolve_value(value_map, vid))
        .collect();
    let operands = operands?;
    let result_types: Vec<Type<'c>> = op.results.iter().map(|_| i64_type).collect();

    let mlir_op = block.append_operation(
        OperationBuilder::new(&full_name, location)
            .add_operands(&operands)
            .add_results(&result_types)
            .build()
            .map_err(|e| format!("Failed to build {full_name}: {e}"))?,
    );

    for (i, &result_id) in op.results.iter().enumerate() {
        value_map.insert(result_id, mlir_op.result(i).unwrap().into());
    }
    Ok(())
}
