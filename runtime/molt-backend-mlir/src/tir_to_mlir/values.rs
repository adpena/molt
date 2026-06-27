use std::collections::HashMap;

use melior::{
    Context as MlirContext,
    ir::{Type, Value},
};
use molt_backend::tir::values::ValueId;

pub(super) type ValueMap<'c, 'a> = HashMap<ValueId, Value<'c, 'a>>;

pub(super) fn resolve_value<'c, 'a>(
    value_map: &ValueMap<'c, 'a>,
    vid: ValueId,
) -> Result<Value<'c, 'a>, String> {
    value_map
        .get(&vid)
        .copied()
        .ok_or_else(|| format!("TIR ValueId %{} not found in MLIR value map", vid.0))
}

/// Infer whether a binary TIR op should use float arithmetic based on operand types.
///
/// We check the TIR function's type information: if either operand came from an
/// op that produced F64, we use float ops. As a fallback, we check the MLIR value
/// type directly.
pub(super) fn operand_is_float<'c>(val: Value<'c, '_>, ctx: &'c MlirContext) -> bool {
    val.r#type() == Type::float64(ctx)
}
