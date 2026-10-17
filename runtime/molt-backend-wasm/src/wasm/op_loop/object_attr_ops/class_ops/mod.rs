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
    match op.kind.as_str() {
        "class_def" => definition::emit_class_def(func, op, ctx, import_ids, locals, reloc_enabled),
        "builtin_type" => metadata::emit_builtin_type(func, op, import_ids, locals, reloc_enabled),
        "type_of" => metadata::emit_type_of(func, op, import_ids, locals, reloc_enabled),
        "class_set_definition" => metadata::emit_class_set_definition(
            func,
            op,
            code_slots,
            import_ids,
            locals,
            reloc_enabled,
        ),
        "isinstance" => relationships::emit_isinstance(func, op, import_ids, locals, reloc_enabled),
        "exception_match_builtin" => {
            relationships::emit_exception_match_builtin(func, op, import_ids, locals, reloc_enabled)
        }
        "issubclass" => relationships::emit_issubclass(func, op, import_ids, locals, reloc_enabled),
        "object_new" => allocation::emit_object_new(func, op, import_ids, locals, reloc_enabled),
        "object_new_bound" => {
            allocation::emit_object_new_bound(func, op, import_ids, locals, reloc_enabled)
        }
        _ => return false,
    }
    true
}
