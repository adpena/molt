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
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "class_new" => definition::emit_class_new(func, op, import_ids, locals, reloc_enabled),
        "class_def" => definition::emit_class_def(func, op, ctx, import_ids, locals, reloc_enabled),
        "class_set_base" => {
            definition::emit_class_set_base(func, op, import_ids, locals, reloc_enabled)
        }
        "class_apply_set_name" => {
            definition::emit_class_apply_set_name(func, op, import_ids, locals, reloc_enabled)
        }
        "super_new" => metadata::emit_super_new(func, op, import_ids, locals, reloc_enabled),
        "builtin_type" => metadata::emit_builtin_type(func, op, import_ids, locals, reloc_enabled),
        "type_of" => metadata::emit_type_of(func, op, import_ids, locals, reloc_enabled),
        "class_layout_version" => {
            metadata::emit_class_layout_version(func, op, import_ids, locals, reloc_enabled)
        }
        "class_set_layout_version" => {
            metadata::emit_class_set_layout_version(func, op, import_ids, locals, reloc_enabled)
        }
        "class_merge_layout" => {
            metadata::emit_class_merge_layout(func, op, import_ids, locals, reloc_enabled)
        }
        "isinstance" => relationships::emit_isinstance(func, op, import_ids, locals, reloc_enabled),
        "exception_match_builtin" => {
            relationships::emit_exception_match_builtin(func, op, import_ids, locals, reloc_enabled)
        }
        "issubclass" => relationships::emit_issubclass(func, op, import_ids, locals, reloc_enabled),
        "object_new" => allocation::emit_object_new(func, op, import_ids, locals, reloc_enabled),
        "object_new_bound" => {
            allocation::emit_object_new_bound(func, op, import_ids, locals, reloc_enabled)
        }
        "object_new_bound_stack" => {
            allocation::emit_object_new_bound_stack(func, op, import_ids, locals, reloc_enabled)
        }
        "object_set_class" => {
            allocation::emit_object_set_class(func, op, import_ids, locals, reloc_enabled)
        }
        _ => return false,
    }
    true
}
