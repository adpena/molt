use crate::OpIR;
use crate::wasm_abi_generated::{
    WasmMethodIcFamily, WasmMethodIcSelection, WasmRuntimeImport, wasm_method_ic_selection,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::wasm) struct WasmMethodIcRuntime {
    pub(in crate::wasm) import: WasmRuntimeImport,
    pub(in crate::wasm) extra_arg_start: usize,
}

pub(in crate::wasm) fn selected_method_ic_runtime(op: &OpIR) -> Option<WasmMethodIcRuntime> {
    let (family, extra_arg_start) = match op.kind.as_str() {
        "call_method_ic" => (WasmMethodIcFamily::Method, 1),
        "call_super_method_ic" => (WasmMethodIcFamily::SuperMethod, 2),
        _ => return None,
    };
    let arg_count = op
        .args
        .as_ref()
        .unwrap_or_else(|| panic!("{} requires arguments", op.kind))
        .len();
    if arg_count < extra_arg_start {
        panic!(
            "{} requires at least {extra_arg_start} base argument(s)",
            op.kind
        );
    }
    let selection: WasmMethodIcSelection =
        wasm_method_ic_selection(family, arg_count - extra_arg_start);
    Some(WasmMethodIcRuntime {
        import: selection.import,
        extra_arg_start,
    })
}
