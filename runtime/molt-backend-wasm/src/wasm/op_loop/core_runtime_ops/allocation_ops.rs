use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_allocation_runtime_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    arena_local: Option<u32>,
) -> bool {
    match op.kind.as_str() {
        "alloc" | "stack_alloc" | "alloc_class" => {}
        _ => return false,
    }

    // Arena fast path: NoEscape allocations marked
    // `arena_eligible` go through `molt_arena_alloc_object`
    // (same NaN-boxed contract as `molt_alloc` but bumps
    // out of the per-function ScopeArena). The arena is
    // freed once at every return in O(1).
    if op.kind == "alloc_class" {
        func.instruction(&Instruction::I64Const(op.value.unwrap()));
        let class_name = op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .expect("alloc_class missing class operand");
        func.instruction(&Instruction::LocalGet(locals[class_name]));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::AllocClass],
        );
    } else if op.arena_eligible == Some(true)
        && let Some(arena_idx) = arena_local
    {
        func.instruction(&Instruction::LocalGet(arena_idx));
        func.instruction(&Instruction::I64Const(op.value.unwrap()));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ArenaAllocObject],
        );
    } else {
        func.instruction(&Instruction::I64Const(op.value.unwrap()));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Alloc],
        );
    }
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjectPublishInitialized],
    );
    if let Some(out) = op.out.as_ref() {
        func.instruction(&Instruction::LocalSet(locals[out]));
    } else {
        func.instruction(&Instruction::Drop);
    }
    true
}
