use crate::OpIR;
use crate::Repr;
use crate::representation_plan::ScalarRepresentationPlan;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WasmScalarDirectNumericLane {
    InlineInt,
    Float,
}

pub(crate) fn is_shared_drop_fact_marker(kind: molt_ir::OpKind) -> bool {
    matches!(
        kind,
        molt_ir::OpKind::DropInserted | molt_ir::OpKind::ExceptionRegionDropsInserted
    )
}

pub(crate) fn wasm_scalar_integer_fast_path_for_op(
    plan: &ScalarRepresentationPlan,
    op: &OpIR,
) -> bool {
    match op.kind {
        // `/=` shares `/`'s int-family fast-path gating: both produce a float on
        // int operands, so the lane is keyed on integer-family operands rather
        // than an integer result.
        molt_ir::OpKind::Div
        | molt_ir::OpKind::InplaceDiv
        | molt_ir::OpKind::Lt
        | molt_ir::OpKind::Le
        | molt_ir::OpKind::Gt
        | molt_ir::OpKind::Ge
        | molt_ir::OpKind::Eq
        | molt_ir::OpKind::Ne => plan.op_args_are_integer_family(op),
        _ => plan.op_prefers_integer_runtime_lane(op),
    }
}

pub(crate) fn wasm_scalar_direct_numeric_lane_for_op(
    plan: &ScalarRepresentationPlan,
    op_index: usize,
    op: &OpIR,
) -> Option<WasmScalarDirectNumericLane> {
    match plan.op_direct_numeric_repr(op_index, op) {
        Some(Repr::RawI64Safe) => Some(WasmScalarDirectNumericLane::InlineInt),
        Some(Repr::FloatUnboxed) => Some(WasmScalarDirectNumericLane::Float),
        _ => None,
    }
}

pub(crate) fn wasm_scalar_truthiness_fast_path_for_name(
    plan: &ScalarRepresentationPlan,
    name: &str,
) -> bool {
    plan.name_is_integer_family(name)
}
