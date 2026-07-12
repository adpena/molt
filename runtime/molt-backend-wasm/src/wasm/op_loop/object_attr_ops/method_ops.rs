mod descriptor_ops;
mod inline_cache_ops;

use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};
use wasm_encoder::Function;

pub(super) fn emit_method_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) -> bool {
    if descriptor_ops::emit_method_descriptor_op(func, op, import_ids, locals, reloc_enabled) {
        return true;
    }
    inline_cache_ops::emit_method_inline_cache_op(
        backend,
        func,
        op,
        func_ir,
        import_ids,
        locals,
        func_index,
        reloc_enabled,
        op_idx,
    )
}
