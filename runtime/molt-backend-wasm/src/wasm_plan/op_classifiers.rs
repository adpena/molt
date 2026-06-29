use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;

pub(crate) fn is_shared_drop_fact_marker(kind: &str) -> bool {
    matches!(kind, "drop_inserted" | "exception_region_drops_inserted")
}

pub(crate) fn wasm_scalar_integer_fast_path_for_op(
    plan: &ScalarRepresentationPlan,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        // `/=` shares `/`'s int-family fast-path gating: both produce a float on
        // int operands, so the lane is keyed on integer-family operands rather
        // than an integer result.
        "div" | "inplace_div" | "lt" | "le" | "gt" | "ge" | "eq" | "ne" => {
            plan.op_args_are_integer_family(op)
        }
        _ => plan.op_prefers_integer_runtime_lane(op),
    }
}

pub(crate) fn wasm_scalar_truthiness_fast_path_for_name(
    plan: &ScalarRepresentationPlan,
    name: &str,
) -> bool {
    plan.name_is_integer_family(name)
}
