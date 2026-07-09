use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};
use wasm_encoder::Function;

#[path = "attribute_ops/generic.rs"]
mod generic;
#[path = "attribute_ops/named.rs"]
mod named;

pub(super) fn emit_attribute_op(
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
    if generic::emit_generic_attribute_op(
        backend,
        func,
        op,
        func_ir,
        import_ids,
        locals,
        func_index,
        reloc_enabled,
        op_idx,
    ) {
        return true;
    }

    named::emit_named_attribute_op(func, op, import_ids, locals, reloc_enabled)
}
