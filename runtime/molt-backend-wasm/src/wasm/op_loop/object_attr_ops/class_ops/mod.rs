mod allocation;
mod definition;
mod metadata;
mod relationships;

use super::super::super::context::CompileFuncContext;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::Function;

pub(super) fn emit_class_object_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &CompileFuncContext<'_>,
    code_slots: &molt_ir::code_slots::CodeSlotBindings,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind {
        molt_ir::OpKind::ClassDef => {
            definition::emit_class_def(func, op, ctx, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::BuiltinType => {
            metadata::emit_builtin_type(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::TypeOf => {
            metadata::emit_type_of(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::ClassSetDefinition => metadata::emit_class_set_definition(
            func,
            op,
            code_slots,
            import_ids,
            locals,
            reloc_enabled,
        ),
        molt_ir::OpKind::Isinstance => {
            relationships::emit_isinstance(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::ExceptionMatchBuiltin => {
            relationships::emit_exception_match_builtin(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::Issubclass => {
            relationships::emit_issubclass(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::ObjectNew => {
            allocation::emit_object_new(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::ObjectNewBound => {
            allocation::emit_object_new_bound(func, op, import_ids, locals, reloc_enabled)
        }
        _ => return false,
    }
    true
}
