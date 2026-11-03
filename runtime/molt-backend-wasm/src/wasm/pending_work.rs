//! The admitted wasm32 runtime is single-threaded (threaded WASM is refused
//! by the runtime GIL and codegen ABI). Its aligned i32.load is the relaxed
//! word read; no shared-memory/atomic instruction may be smuggled into it.

use super::frame_locals::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

pub(in crate::wasm) fn emit_poll(
    func: &mut Function,
    locals: &WasmFrameLocals,
    imports: &TrackedImportIds,
    reloc: bool,
    payload: bool,
) {
    func.instruction(&Instruction::LocalGet(
        locals.synthetic(WasmFrameSyntheticLocal::PendingWorkPtr),
    ));
    func.instruction(&Instruction::I32Load(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
    func.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
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
