use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmRuntimeImport, WasmRuntimeReturn};
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

/// Consume the owned boxed value on the machine stack, not just its bits.
pub(super) fn store_owned_result_or_release(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    store_owned_value(
        func,
        locals.bound_op_result_slot(op),
        import_ids,
        reloc_enabled,
    );
}

pub(super) fn store_owned_value(
    func: &mut Function,
    output: Option<u32>,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    if let Some(out) = output {
        func.instruction(&Instruction::LocalSet(out));
    } else {
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::DecRefObj],
        );
    }
}

pub(super) fn store_result_or_drop(func: &mut Function, op: &OpIR, locals: &WasmFrameLocals) {
    store_raw_value(func, locals.bound_op_result_slot(op));
}

/// A bound borrowed value becomes an independent owner; an unobserved value
/// acquires no reference. Callers must apply this before releasing its source.
pub(super) fn store_borrowed_value(
    func: &mut Function,
    output: Option<u32>,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
) {
    if let Some(out) = output {
        func.instruction(&Instruction::LocalTee(out));
        emit_call(
            func,
            reloc_enabled,
            import_ids[WasmRuntimeImport::IncRefObj],
        );
    } else {
        func.instruction(&Instruction::Drop);
    }
}

fn store_raw_value(func: &mut Function, output: Option<u32>) {
    if let Some(res) = output {
        func.instruction(&Instruction::LocalSet(res));
    } else {
        func.instruction(&Instruction::Drop);
    }
}

/// Complete a multi-call constructor after all initialization and temporary
/// cleanup. Live results already occupy their destination; discarded results
/// must release the owner held in the scratch local.
pub(super) fn finish_owned_local_result(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    result: u32,
) {
    if let Some(bound) = locals.bound_op_result_slot(op) {
        assert_eq!(
            bound, result,
            "owned constructor result must occupy its destination"
        );
    } else {
        func.instruction(&Instruction::LocalGet(result));
        store_owned_result_or_release(func, op, locals, import_ids, reloc_enabled);
    }
}

/// Import ABI metadata, not a carrier type or per-op guess, owns disposal.
pub(super) fn store_runtime_result(
    func: &mut Function,
    op: &OpIR,
    locals: &WasmFrameLocals,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    import: WasmRuntimeImport,
) {
    emit_runtime_result(
        func,
        locals.bound_op_result_slot(op),
        import_ids,
        reloc_enabled,
        import,
    );
}

pub(super) fn discard_runtime_result(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    import: WasmRuntimeImport,
) {
    emit_runtime_result(func, None, import_ids, reloc_enabled, import);
}

fn emit_runtime_result(
    func: &mut Function,
    output: Option<u32>,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    import: WasmRuntimeImport,
) {
    match import.return_contract() {
        // Pending is an immediate tag, not a heap pointer; DecRefObj is a
        // no-op for it and consumes exactly one owner for a ready value.
        WasmRuntimeReturn::OwnedObject | WasmRuntimeReturn::PollResult => {
            store_owned_value(func, output, import_ids, reloc_enabled);
        }
        WasmRuntimeReturn::BorrowedObject => {
            store_borrowed_value(func, output, import_ids, reloc_enabled);
        }
        WasmRuntimeReturn::RawBits => store_raw_value(func, output),
        WasmRuntimeReturn::Void => assert!(
            output.is_none(),
            "void runtime import {} cannot bind a result",
            import.name()
        ),
        WasmRuntimeReturn::UnpublishedObject => panic!(
            "runtime import {} requires object publication before result binding or disposal",
            import.name()
        ),
        WasmRuntimeReturn::ScratchAllocation => panic!(
            "runtime import {} requires sized scratch-allocation custody",
            import.name()
        ),
        WasmRuntimeReturn::ExecutionToken => panic!(
            "runtime import {} requires paired execution-lane custody",
            import.name()
        ),
    }
}
