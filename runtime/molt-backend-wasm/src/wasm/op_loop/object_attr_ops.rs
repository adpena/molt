use super::super::context::CompileFuncContext;
use super::*;

#[path = "object_attr_ops/attribute_ops.rs"]
mod attribute_ops;
#[path = "object_attr_ops/class_ops.rs"]
mod class_ops;
#[path = "object_attr_ops/dataclass_ops.rs"]
mod dataclass_ops;
#[path = "object_attr_ops/method_ops.rs"]
mod method_ops;

pub(super) fn emit_object_attr_op(
    backend: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    ctx: &CompileFuncContext<'_>,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    func_index: u32,
    reloc_enabled: bool,
    op_idx: usize,
) -> bool {
    if dataclass_ops::emit_dataclass_op(func, op, import_ids, locals, reloc_enabled) {
        return true;
    }
    if class_ops::emit_class_object_op(func, op, ctx, import_ids, locals, reloc_enabled) {
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
