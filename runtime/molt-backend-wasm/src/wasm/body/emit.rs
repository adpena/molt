use super::{WasmBody, WasmBodyOp, WasmCallTarget};
use crate::wasm::WasmBackend;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use wasm_encoder::Function;

impl WasmBody {
    pub(crate) fn emit_into(
        &self,
        func_name: &str,
        backend: &mut WasmBackend,
        func_index: u32,
        reloc_enabled: bool,
        const_str_scratch_segment: DataSegmentRef,
        mut import_index_for: impl FnMut(WasmRuntimeImport) -> u32,
        func: &mut Function,
    ) {
        for op in &self.ops {
            match op {
                WasmBodyOp::Instruction(instruction) => {
                    func.instruction(instruction);
                }
                WasmBodyOp::Call(WasmCallTarget::RuntimeImport(call)) => {
                    let import = call.import();
                    let import_index = import_index_for(import);
                    assert!(
                        import_index != u32::MAX,
                        "LIR fast body for '{func_name}' calls runtime import '{}' which was skipped/pruned from the import set",
                        import.name()
                    );
                    emit_call(func, reloc_enabled, import_index);
                }
                WasmBodyOp::ConstMaterialization(materialization) => {
                    let import = materialization.runtime_import();
                    let import_index = import_index_for(import);
                    assert!(
                        import_index != u32::MAX,
                        "LIR fast body for '{func_name}' materializes const through runtime import '{}' which was skipped/pruned from the import set",
                        import.name()
                    );
                    materialization.emit(
                        backend,
                        func,
                        func_index,
                        reloc_enabled,
                        import_index,
                        const_str_scratch_segment,
                    );
                }
                WasmBodyOp::DataPtrI32(bytes) => {
                    let data = backend.add_data_segment(reloc_enabled, bytes.as_ref());
                    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, data);
                }
                WasmBodyOp::Call(WasmCallTarget::BailToGenericPath(reason)) => {
                    panic!(
                        "LIR fast body for '{func_name}' reached a generic-path bail marker during emission: {}",
                        reason.diagnostic_name()
                    );
                }
            }
        }
    }
}
