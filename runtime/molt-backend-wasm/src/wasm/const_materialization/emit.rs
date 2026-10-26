use crate::wasm::WasmBackend;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_data::DataSegmentRef;
use crate::wasm_import_tracking::TrackedImportIds;
use molt_tir::tir::op_kinds_generated::OwnedLiteralPayloadKind;
use std::sync::Arc;
use wasm_encoder::{Function, Instruction};

#[cfg(test)]
mod tests;

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

    pub(in crate::wasm::const_materialization) fn scalar_i64(
        import: WasmRuntimeImport,
        out_local: u32,
        value: i64,
    ) -> Self {
        Self {
            import,
            out_local,
            payload: WasmConstMaterializationPayload::ScalarI64(value),
        }
    }

    pub(in crate::wasm::const_materialization) fn literal(
        import: WasmRuntimeImport,
        out_local: u32,
        payload: OwnedLiteralPayloadKind,
        bytes: Arc<[u8]>,
    ) -> Self {
        Self {
            import,
            out_local,
            payload: WasmConstMaterializationPayload::Literal { payload, bytes },
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
            WasmConstMaterializationPayload::ScalarI64(value) => {
                func.instruction(&Instruction::I64Const(*value));
                emit_call(func, reloc_enabled, import_id);
                func.instruction(&Instruction::LocalSet(self.out_local));
            }
            WasmConstMaterializationPayload::Literal { payload, bytes } => {
                emit_literal_materialization(
                    backend,
                    func,
                    func_index,
                    reloc_enabled,
                    import_id,
                    const_str_scratch_segment,
                    self.out_local,
                    *payload,
                    bytes,
                )
            }
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
    ScalarI64(i64),
    Literal {
        payload: OwnedLiteralPayloadKind,
        bytes: Arc<[u8]>,
    },
}

fn emit_literal_materialization(
    backend: &mut WasmBackend,
    func: &mut Function,
    func_index: u32,
    reloc_enabled: bool,
    import_id: u32,
    scratch_segment: DataSegmentRef,
    out_local: u32,
    payload: OwnedLiteralPayloadKind,
    bytes: &[u8],
) {
    // Payload addresses and lengths exist only as constructor operands. Keep
    // relocation recording in the data authority and consume both immediately;
    // neither needs an invocation-long frame local or an anchor alias.
    let data = backend.add_data_segment(reloc_enabled, bytes);
    backend.emit_data_ptr_i32(reloc_enabled, func_index, func, data);
    func.instruction(&Instruction::I64Const(bytes.len() as i64));
    match payload {
        OwnedLiteralPayloadKind::String | OwnedLiteralPayloadKind::Bytes => {
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
        OwnedLiteralPayloadKind::BigintBytes => {
            emit_call(func, reloc_enabled, import_id);
            func.instruction(&Instruction::LocalSet(out_local));
        }
    }
}
