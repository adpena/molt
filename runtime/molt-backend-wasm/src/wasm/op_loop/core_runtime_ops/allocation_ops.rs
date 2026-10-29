use super::super::result_sink::store_runtime_result;
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
) -> bool {
    match op.kind {
        molt_ir::OpKind::StackAlloc => panic!(
            "{}",
            crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED
        ),
        molt_ir::OpKind::Alloc | molt_ir::OpKind::AllocClass => {}
        _ => return false,
    }

    if op.kind == molt_ir::OpKind::AllocClass {
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
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        crate::wasm_abi_generated::WasmRuntimeImport::ObjectPublishInitialized,
    );
    true
}
