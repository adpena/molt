use crate::FunctionIR;
use crate::wasm::WasmBackend;
use wasm_encoder::ExportKind;

pub(super) fn register_function(
    backend: &mut WasmBackend,
    func_ir: &FunctionIR,
    type_idx: u32,
    reloc_enabled: bool,
) -> u32 {
    let func_index = backend.func_count;
    if std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref() == Some(func_ir.name.as_str()) {
        eprintln!(
            "WASM_SIG_FUNC name={} type_idx={} params={:?} param_types={:?}",
            func_ir.name, type_idx, func_ir.params, func_ir.param_types
        );
    }
    backend.funcs.function(type_idx);
    let needs_entry_wrapper = reloc_enabled || backend.module_registry.is_some();
    if needs_entry_wrapper && func_ir.name == "molt_main" {
        backend.molt_main_index = Some(func_index);
    } else if needs_entry_wrapper && func_ir.name == "molt_host_init" {
        backend.molt_host_init_index = Some(func_index);
    } else {
        backend
            .exports
            .export(&func_ir.name, ExportKind::Func, backend.func_count);
    }
    backend.func_count += 1;
    func_index
}
