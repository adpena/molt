use crate::OpIR;
use crate::wasm_abi_generated::{
    LirRuntimeCall, WasmObjectNewBoundPayload, WasmObjectNewBoundSelection, WasmRuntimeImport,
    wasm_object_new_bound_selection,
};
use molt_tir::tir::lir::LirOp;
use molt_tir::tir::ops::AttrValue;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wasm) struct WasmObjectNewBoundRuntime {
    pub(in crate::wasm) import: WasmRuntimeImport,
    pub(in crate::wasm) lir_runtime_call: LirRuntimeCall,
    payload_size: Option<i64>,
}

impl WasmObjectNewBoundRuntime {
    pub(in crate::wasm) fn payload_size(&self) -> Option<i64> {
        self.payload_size
    }

    pub(in crate::wasm) fn required_payload_size(&self, kind: &str) -> i64 {
        self.payload_size
            .unwrap_or_else(|| panic!("{kind} requires positive payload byte size"))
    }
}

pub(in crate::wasm) fn selected_object_new_bound_runtime(op: &OpIR) -> WasmObjectNewBoundRuntime {
    selected_object_new_bound_runtime_for_size(op.value)
}

pub(in crate::wasm) fn required_object_new_bound_stack_runtime(
    op: &OpIR,
) -> WasmObjectNewBoundRuntime {
    let selected = selected_object_new_bound_runtime(op);
    if selected.payload_size().is_none() {
        panic!("object_new_bound_stack requires positive payload byte size");
    }
    selected
}

pub(in crate::wasm) fn selected_lir_object_new_bound_runtime(
    op: &LirOp,
) -> WasmObjectNewBoundRuntime {
    let size = match op.tir_op.attrs.get("value") {
        Some(AttrValue::Int(value)) => Some(*value),
        _ => None,
    };
    selected_object_new_bound_runtime_for_size(size)
}

fn selected_object_new_bound_runtime_for_size(size: Option<i64>) -> WasmObjectNewBoundRuntime {
    let payload_size = positive_payload_size(size);
    let payload = if payload_size.is_some() {
        WasmObjectNewBoundPayload::Sized
    } else {
        WasmObjectNewBoundPayload::Unsized
    };
    let selection: WasmObjectNewBoundSelection = wasm_object_new_bound_selection(payload);
    WasmObjectNewBoundRuntime {
        import: selection.import,
        lir_runtime_call: selection.lir_runtime_call,
        payload_size,
    }
}

fn positive_payload_size(size: Option<i64>) -> Option<i64> {
    size.filter(|value| *value > 0)
}
