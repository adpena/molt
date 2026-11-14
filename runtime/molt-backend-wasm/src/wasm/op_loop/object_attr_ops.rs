use super::super::context::CompileFuncContext;
use crate::wasm::{WasmBackend, WasmFrameLocals};
use crate::wasm_binary::Function;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};

#[path = "object_attr_ops/attribute_ops.rs"]
mod attribute_ops;
#[path = "object_attr_ops/class_ops/mod.rs"]
mod class_ops;
#[path = "object_attr_ops/method_ops/mod.rs"]
mod method_ops;

pub(super) fn emit_object_attr_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    ctx: &CompileFuncContext<'_>,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) -> bool {
    match op.kind {
        molt_ir::OpKind::StaticCellBase => {
            crate::wasm::global_slot::emit_static_cell_base(
                backend,
                func,
                op,
                locals,
                func_index,
                reloc_enabled,
            );
            return true;
        }
        molt_ir::OpKind::StaticCellSet => {
            crate::wasm::global_slot::emit_static_cell_set(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
            );
            return true;
        }
        _ => {}
    }
    if class_ops::emit_class_object_op(
        func,
        op,
        ctx,
        &backend.code_slots,
        import_ids,
        locals,
        reloc_enabled,
    ) {
        return true;
    }
    if method_ops::emit_method_op(
        backend,
        func,
        op,
        func_ir,
        import_ids,
        locals,
        func_index,
        reloc_enabled,
        op_idx,
        ctx.call_site_abi.call_func_spill_offset(),
    ) {
        return true;
    }
    attribute_ops::emit_attribute_op(
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
