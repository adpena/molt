use crate::OpIR;
use crate::repr::{ContainerKind, ContainerStorageKind};
use crate::representation_plan::ScalarRepresentationPlan;

pub(crate) fn is_shared_drop_fact_marker(kind: &str) -> bool {
    matches!(kind, "drop_inserted" | "exception_region_drops_inserted")
}

pub(crate) fn gpu_runtime_call_symbol(kind: &str) -> Option<&'static str> {
    match kind {
        "gpu_thread_id" => Some("molt_gpu_thread_id"),
        "gpu_block_id" => Some("molt_gpu_block_id"),
        "gpu_block_dim" => Some("molt_gpu_block_dim"),
        "gpu_grid_dim" => Some("molt_gpu_grid_dim"),
        "gpu_barrier" => Some("molt_gpu_barrier"),
        _ => None,
    }
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

pub(crate) fn wasm_specialized_container_import(
    plan: &ScalarRepresentationPlan,
    op_index: usize,
    kind: &str,
    op: &OpIR,
) -> Option<&'static str> {
    match kind {
        "index"
            if plan.op_has_container_storage(op_index, op, ContainerStorageKind::FlatListInt) =>
        {
            Some("list_int_getitem")
        }
        "store_index"
            if plan.op_has_container_storage(op_index, op, ContainerStorageKind::FlatListInt) =>
        {
            Some("list_int_setitem")
        }
        "contains" | "len" | "index" | "store_index" => {
            let container = op.args.as_ref()?.first()?;
            let container_kind = plan.name_container_kind(container)?;
            match kind {
                "contains" => match container_kind {
                    ContainerKind::Set => Some("set_contains"),
                    ContainerKind::Dict => Some("dict_contains"),
                    ContainerKind::List => Some("list_contains"),
                    ContainerKind::Str => Some("str_contains"),
                    ContainerKind::Tuple => None,
                },
                "len" => match container_kind {
                    ContainerKind::List => Some("len_list"),
                    ContainerKind::Str => Some("len_str"),
                    ContainerKind::Dict => Some("len_dict"),
                    ContainerKind::Tuple => Some("len_tuple"),
                    ContainerKind::Set => Some("len_set"),
                },
                "index" => match container_kind {
                    ContainerKind::Dict => Some("dict_getitem"),
                    ContainerKind::Tuple => Some("tuple_getitem"),
                    ContainerKind::List | ContainerKind::Set | ContainerKind::Str => None,
                },
                "store_index" => match container_kind {
                    ContainerKind::Dict => Some("dict_setitem"),
                    ContainerKind::List
                    | ContainerKind::Set
                    | ContainerKind::Tuple
                    | ContainerKind::Str => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}
