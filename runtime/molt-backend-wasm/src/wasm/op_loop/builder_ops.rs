use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::box_int;
use wasm_encoder::{Function, Instruction};

#[derive(Clone, Copy)]
pub(super) enum BuilderFinish {
    List,
    Tuple,
}

impl BuilderFinish {
    const fn import(self) -> WasmRuntimeImport {
        match self {
            Self::List => WasmRuntimeImport::ListBuilderFinish,
            Self::Tuple => WasmRuntimeImport::TupleBuilderFinish,
        }
    }
}

pub(super) fn emit_sequence_builder_from_args(
    func: &mut Function,
    value_names: &[String],
    out: u32,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    finish: BuilderFinish,
) {
    func.instruction(&Instruction::I64Const(box_int(value_names.len() as i64)));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ListBuilderNew],
    );
    func.instruction(&Instruction::LocalSet(out));
    for name in value_names {
        let val = locals[name];
        func.instruction(&Instruction::LocalGet(out));
        func.instruction(&Instruction::LocalGet(val));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ListBuilderAppend],
        );
    }
    func.instruction(&Instruction::LocalGet(out));
    emit_call(func, reloc_enabled, import_ids[finish.import()]);
    func.instruction(&Instruction::LocalSet(out));
}
