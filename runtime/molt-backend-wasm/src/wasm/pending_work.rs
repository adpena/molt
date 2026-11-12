//! The admitted wasm32 runtime is single-threaded (threaded WASM is refused
//! by the runtime GIL and codegen ABI). Its aligned i32.load is the relaxed
//! word read; no shared-memory/atomic instruction may be smuggled into it.

use super::frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{BlockType, Instruction, MemArg, ValType};

/// The permanent admitted context directly carries this thread's record.
pub(in crate::wasm) fn emit_thread_record(func: &mut Function) {
    func.instruction(&Instruction::LocalGet(0));
    func.instruction(&Instruction::I32WrapI64);
    func.instruction(&Instruction::I64Load(MemArg {
        offset: molt_ir::runtime_callable_abi_generated::ATTACHED_CONTEXT_THREAD_RECORD_OFFSET
            as u64,
        align: 3,
        memory_index: 0,
    }));
}

/// Push an i32 that is non-zero exactly when `molt_exception_pending()` is.
/// A function that holds the thread record (`record`) tests its pending byte
/// inline and calls the runtime reader only when the byte is set. The reader
/// answers 0 at once for a clear byte, and for a set byte it confirms it,
/// repairing a stale task slot, so the inline test returns exactly what the
/// call returned. Every planned frame holds the record.
pub(in crate::wasm) fn emit_exception_pending_test(
    func: &mut Function,
    record: Option<u32>,
    exception_pending: u32,
    reloc: bool,
) {
    let Some(record) = record else {
        emit_call(func, reloc, exception_pending);
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::I64Ne);
        return;
    };
    func.instruction(&Instruction::LocalGet(record));
    func.instruction(&Instruction::I32Load8U(MemArg {
        offset: molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as u64,
        align: 0,
        memory_index: 0,
    }));
    func.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    emit_call(func, reloc, exception_pending);
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::Else);
    func.instruction(&Instruction::I32Const(0));
    func.instruction(&Instruction::End);
}

/// The thread record local of a function that holds one.
pub(in crate::wasm) fn thread_record_local(locals: &WasmFrameLocals) -> Option<u32> {
    locals.optional_synthetic(WasmFrameSyntheticLocal::ThreadRecord)
}

/// Push the thread's pending-work word (i32): non-zero when work waits for a
/// poll. The one read both the poll and the `async_work_pending` test use.
pub(in crate::wasm) fn emit_pending_work_word(func: &mut Function, locals: &WasmFrameLocals) {
    func.instruction(&Instruction::LocalGet(
        locals.synthetic(WasmFrameSyntheticLocal::PendingWorkPtr),
    ));
    func.instruction(&Instruction::I32Load(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
}

/// `cold_prelude` runs at the start of the service path, before any pending
/// call or signal handler: a check's `frame_location` store goes there.
pub(in crate::wasm) fn emit_poll(
    func: &mut Function,
    locals: &WasmFrameLocals,
    imports: &TrackedImportIds,
    reloc: bool,
    payload: bool,
    cold_prelude: impl FnOnce(&mut Function),
) {
    emit_pending_work_word(func, locals);
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    cold_prelude(func);
    emit_call(
        func,
        reloc,
        imports[if payload {
            WasmRuntimeImport::AsyncWorkPollAndExceptionLastPending
        } else {
            WasmRuntimeImport::AsyncWorkPollAndExceptionPending
        }],
    );
    func.instruction(&Instruction::Else);
    if payload {
        func.instruction(&Instruction::LocalGet(
            locals.synthetic(WasmFrameSyntheticLocal::ThreadRecord),
        ));
        func.instruction(&Instruction::I32Load8U(MemArg {
            offset: molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as u64,
            align: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
        emit_call(
            func,
            reloc,
            imports[WasmRuntimeImport::ExceptionLastPending],
        );
        func.instruction(&Instruction::Else);
        func.instruction(&Instruction::I64Const(molt_codegen_abi::QNAN_TAG_NONE_I64));
        func.instruction(&Instruction::End);
    } else {
        func.instruction(&Instruction::LocalGet(
            locals.synthetic(WasmFrameSyntheticLocal::ThreadRecord),
        ));
        func.instruction(&Instruction::I64Load8U(MemArg {
            offset: molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET as u64,
            align: 0,
            memory_index: 0,
        }));
    }
    func.instruction(&Instruction::End);
}
