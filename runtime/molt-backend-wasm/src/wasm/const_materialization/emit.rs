use crate::wasm::WasmBackend;
use crate::wasm::frame_locals::WasmLiteralScratchLocals;
use crate::wasm_abi_generated::{WasmConstLiteralPayload, WasmRuntimeImport};
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use std::sync::Arc;
use wasm_encoder::{Function, Instruction};

#[derive(Debug, Clone)]
pub(crate) struct WasmConstMaterialization {
    import: WasmRuntimeImport,
    out_local: u32,
    payload: WasmConstMaterializationPayload,
}

impl WasmConstMaterialization {
    pub(in crate::wasm::const_materialization) fn runtime_singleton(
        import: WasmRuntimeImport,
        out_local: u32,
    ) -> Self {
        Self {
            import,
            out_local,
            payload: WasmConstMaterializationPayload::RuntimeSingleton,
        }
    }

    pub(in crate::wasm::const_materialization) fn literal(
        import: WasmRuntimeImport,
        out_local: u32,
        payload: WasmConstLiteralPayload,
        bytes: Arc<[u8]>,
        scratch: WasmConstMaterializationScratch,
    ) -> Self {
        Self {
            import,
            out_local,
            payload: WasmConstMaterializationPayload::Literal {
                payload,
                bytes,
                scratch,
            },
        }
    }

    pub(crate) fn runtime_import(&self) -> WasmRuntimeImport {
        self.import
    }

    pub(crate) fn emit(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_id: u32,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        match &self.payload {
            WasmConstMaterializationPayload::RuntimeSingleton => {
                emit_call(func, reloc_enabled, import_id);
                func.instruction(&Instruction::LocalSet(self.out_local));
            }
            WasmConstMaterializationPayload::Literal {
                payload,
                bytes,
                scratch,
            } => emit_literal_materialization(
                backend,
                func,
                func_index,
                reloc_enabled,
                import_id,
                const_str_scratch_segment,
                self.out_local,
                *payload,
                bytes,
                *scratch,
            ),
        }
    }

    pub(in crate::wasm) fn emit_with_imports(
        &self,
        backend: &mut WasmBackend,
        func: &mut Function,
        func_index: u32,
        reloc_enabled: bool,
        import_ids: &TrackedImportIds,
        const_str_scratch_segment: DataSegmentRef,
    ) {
        let import_id = import_ids[self.runtime_import()];
        self.emit(
            backend,
            func,
            func_index,
            reloc_enabled,
            import_id,
            const_str_scratch_segment,
        );
    }
}

#[derive(Debug, Clone)]
enum WasmConstMaterializationPayload {
    RuntimeSingleton,
    Literal {
        payload: WasmConstLiteralPayload,
        bytes: Arc<[u8]>,
        scratch: WasmConstMaterializationScratch,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WasmConstMaterializationScratch {
    ptr_local: u32,
    len_local: u32,
}

impl WasmConstMaterializationScratch {
    pub(crate) fn new(ptr_local: u32, len_local: u32) -> Self {
        Self {
            ptr_local,
            len_local,
        }
    }
}

impl From<WasmLiteralScratchLocals> for WasmConstMaterializationScratch {
    fn from(scratch: WasmLiteralScratchLocals) -> Self {
        Self::new(scratch.ptr_local(), scratch.len_local())
    }
}

fn emit_literal_materialization(
    backend: &mut WasmBackend,
    func: &mut Function,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
    out_local: u32,
    payload: WasmConstLiteralPayload,
    bytes: &[u8],
    scratch: WasmConstMaterializationScratch,
) {
    emit_literal_ptr_len(backend, func, func_index, reloc_enabled, bytes, scratch);
    match payload {
        WasmConstLiteralPayload::String | WasmConstLiteralPayload::Bytes => {
            func.instruction(&Instruction::LocalGet(scratch.ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(scratch.len_local));
            backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
            emit_call(func, reloc_enabled, import_id);
            func.instruction(&Instruction::Drop);

            backend.emit_data_ptr_i32(reloc_enabled, func_index, func, scratch_segment);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(out_local));
        }
        WasmConstLiteralPayload::BigintDecimal => {
            func.instruction(&Instruction::LocalGet(scratch.ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(scratch.len_local));
            emit_call(func, reloc_enabled, import_id);
            func.instruction(&Instruction::LocalSet(out_local));
        }
        WasmConstLiteralPayload::None => unreachable!("literal materialization checked above"),
    }
}

fn emit_literal_ptr_len(
    backend: &mut WasmBackend,
    func: &mut Function,
    func_index: u32,
    reloc_enabled: bool,
    bytes: &[u8],
    scratch: WasmConstMaterializationScratch,
) {
    let data = backend.add_data_segment(reloc_enabled, bytes);
    backend.emit_data_ptr(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::LocalSet(scratch.ptr_local));
    func.instruction(&Instruction::I64Const(bytes.len() as i64));
    func.instruction(&Instruction::LocalSet(scratch.len_local));
}
