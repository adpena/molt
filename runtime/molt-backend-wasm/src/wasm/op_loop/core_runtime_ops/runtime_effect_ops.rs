use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::emit_box_bool_from_i32;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_runtime_effect_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "exception_pending" => emit_exception_pending(func, op, import_ids, locals, reloc_enabled),
        "print" => emit_print(func, op, import_ids, locals, reloc_enabled),
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
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_box_bool_from_i32(func);
    if let Some(out) = op.out.as_ref() {
        let res = locals[out];
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
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
