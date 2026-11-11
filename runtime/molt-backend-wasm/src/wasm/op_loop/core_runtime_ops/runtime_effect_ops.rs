use super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::emit_box_bool_from_i32;
use wasm_encoder::Instruction;

pub(super) fn emit_runtime_effect_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind {
        molt_ir::OpKind::ExceptionPending => {
            emit_exception_pending(func, op, import_ids, locals, reloc_enabled)
        }
        molt_ir::OpKind::AsyncWorkPending => {
            // Test the thread's pending-work word; service nothing. The
            // word's address is a frame local of every function that reads it.
            crate::wasm::pending_work::emit_pending_work_word(func, locals);
            func.instruction(&Instruction::I32Const(0));
            func.instruction(&Instruction::I32Ne);
            emit_box_bool_from_i32(func);
            store_result_or_drop(func, op, locals);
        }
        molt_ir::OpKind::Print => emit_print(func, op, import_ids, locals, reloc_enabled),
        _ => return false,
    }
    true
}

fn emit_exception_pending(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    // Read the runtime exception-pending flag as a NaN-boxed
    // bool: `box_bool(molt_exception_pending() != 0)`.
    // Produced by the TIR `ExceptionPending` op (round-tripped
    // to SimpleIR by lower_to_simple when an iterator-consumer
    // loop carries a `loop_break_if_exception`); consumed as
    // the condition of the `br_if`/`if` that breaks the loop on
    // a mid-iteration raise.  Boxing to a proper bool (rather
    // than leaving the raw i64 0/1) is required because the
    // downstream `br_if`/`if` truthiness path calls
    // `is_truthy`, which interprets its operand as a NaN-boxed
    // value.  Non-foldable: it observes mutable runtime state.
    crate::wasm::pending_work::emit_exception_pending_test(
        func,
        crate::wasm::pending_work::thread_record_local(locals),
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
        reloc_enabled,
    );
    emit_box_bool_from_i32(func);
    store_result_or_drop(func, op, locals);
}

fn emit_print(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) {
    let args = op.args.as_ref().unwrap();
    if let Some(&idx) = locals.get(&args[0]) {
        func.instruction(&Instruction::LocalGet(idx));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::PrintObj],
        );
    }
}
