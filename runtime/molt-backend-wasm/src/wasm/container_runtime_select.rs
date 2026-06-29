use crate::repr::{ContainerKind, ContainerStorageKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_abi_generated::{
    LirRuntimeCall, WasmContainerRuntimeFact, WasmContainerRuntimeOp,
    wasm_container_runtime_op, wasm_container_runtime_selection,
};
use crate::OpIR;
use molt_tir::tir::types::TirType;

pub(in crate::wasm) fn selected_container_runtime_import(
    plan: &ScalarRepresentationPlan,
    op_index: usize,
    kind: &str,
    op: &OpIR,
) -> Option<&'static str> {
    let selector_op = wasm_container_runtime_op(kind)?;
    selected_storage_runtime(selector_op, plan, op_index, op)
        .or_else(|| selected_container_kind_runtime(selector_op, plan, op))
        .map(|selection| selection.import_name)
}

pub(in crate::wasm) fn selected_lir_container_runtime_call(
    selector_op: WasmContainerRuntimeOp,
    has_flat_list_int_storage: bool,
    container_ty: Option<&TirType>,
) -> Option<LirRuntimeCall> {
    if has_flat_list_int_storage
        && let Some(selection) = wasm_container_runtime_selection(
            selector_op,
            WasmContainerRuntimeFact::FlatListInt,
        )
    {
        return selection.lir_runtime_call;
    }
    let fact = tir_type_container_fact(container_ty?)?;
    wasm_container_runtime_selection(selector_op, fact)?.lir_runtime_call
}

fn selected_storage_runtime(
    selector_op: WasmContainerRuntimeOp,
    plan: &ScalarRepresentationPlan,
    op_index: usize,
    op: &OpIR,
) -> Option<crate::wasm_abi_generated::WasmContainerRuntimeSelection> {
    if !plan.op_has_container_storage(op_index, op, ContainerStorageKind::FlatListInt) {
        return None;
    }
    wasm_container_runtime_selection(selector_op, WasmContainerRuntimeFact::FlatListInt)
}

fn selected_container_kind_runtime(
    selector_op: WasmContainerRuntimeOp,
    plan: &ScalarRepresentationPlan,
    op: &OpIR,
) -> Option<crate::wasm_abi_generated::WasmContainerRuntimeSelection> {
    let container = op.args.as_ref()?.first()?;
    let fact = container_kind_fact(plan.name_container_kind(container)?)?;
    wasm_container_runtime_selection(selector_op, fact)
}

fn container_kind_fact(kind: ContainerKind) -> Option<WasmContainerRuntimeFact> {
    match kind {
        ContainerKind::Dict => Some(WasmContainerRuntimeFact::Dict),
        ContainerKind::List => Some(WasmContainerRuntimeFact::List),
        ContainerKind::Set => Some(WasmContainerRuntimeFact::Set),
        ContainerKind::Str => Some(WasmContainerRuntimeFact::Str),
        ContainerKind::Tuple => Some(WasmContainerRuntimeFact::Tuple),
    }
}

fn tir_type_container_fact(ty: &TirType) -> Option<WasmContainerRuntimeFact> {
    match ty {
        TirType::Dict(_, _) => Some(WasmContainerRuntimeFact::Dict),
        TirType::List(_) => Some(WasmContainerRuntimeFact::List),
        TirType::Set(_) => Some(WasmContainerRuntimeFact::Set),
        TirType::Str => Some(WasmContainerRuntimeFact::Str),
        TirType::Tuple(_) => Some(WasmContainerRuntimeFact::Tuple),
        _ => None,
    }
}
