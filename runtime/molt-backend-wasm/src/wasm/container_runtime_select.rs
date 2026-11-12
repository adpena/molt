use crate::OpIR;
use crate::repr::{ContainerKind, ContainerStorageKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_abi_generated::{
    WasmContainerRuntimeFact, WasmContainerRuntimeOp, WasmRuntimeImport, wasm_container_runtime_op,
    wasm_container_runtime_selection,
};

pub(in crate::wasm) fn selected_container_runtime_import(
    plan: &ScalarRepresentationPlan,
    op_index: usize,
    kind: molt_ir::OpKind,
    op: &OpIR,
) -> Option<WasmRuntimeImport> {
    let selector_op = wasm_container_runtime_op(kind)?;
    selected_storage_runtime(selector_op, plan, op_index, op)
        .or_else(|| selected_container_kind_runtime(selector_op, plan, op))
        .map(|selection| selection.import)
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
