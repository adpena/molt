use crate::OpIR;
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use wasm_encoder::Function;

mod closure_ops;
mod field_ops;
mod guarded_field_ops;
mod state_machine_ops;

pub(super) struct LocalStateOpContext<'a> {
    pub(super) backend: &'a mut WasmBackend,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a WasmFrameLocals,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) func_index: u32,
    pub(super) reloc_enabled: bool,
}

#[allow(unused_variables)]
pub(super) fn emit_local_state_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    func_index: u32,
    reloc_enabled: bool,
) -> bool {
    let mut context = LocalStateOpContext {
        backend,
        import_ids,
        locals,
        const_cache,
        func_index,
        reloc_enabled,
    };

    if field_ops::emit_field_local_state_op(&mut context, func, op) {
        return true;
    }
    if closure_ops::emit_closure_local_state_op(&mut context, func, op) {
        return true;
    }
    if guarded_field_ops::emit_guarded_field_local_state_op(&mut context, func, op) {
        return true;
    }
    state_machine_ops::emit_state_machine_local_state_op(&mut context, func, op)
}
