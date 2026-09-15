use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::{box_int, box_none};
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

    // Constructor failure leaves the pending exception for the enclosing
    // SimpleIR exception route. Do not even materialize operands unless the
    // builder exists.
    func.instruction(&Instruction::LocalGet(out));
    func.instruction(&Instruction::I64Const(box_none()));
    func.instruction(&Instruction::I64Ne);
    func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
    func.instruction(&Instruction::Block(wasm_encoder::BlockType::Empty));
    for value_name in value_names {
        let value = locals[value_name];
        func.instruction(&Instruction::LocalGet(out));
        func.instruction(&Instruction::LocalGet(value));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ListBuilderAppend],
        );

        // Append returns 0 on success and 1 after installing an exception.
        // Destroying the partial builder releases every element retained by
        // earlier successful appends. Branch past all remaining operands and
        // the consuming finish, leaving None for the existing exception edge.
        func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        func.instruction(&Instruction::LocalGet(out));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj],
        );
        func.instruction(&Instruction::I64Const(box_none()));
        func.instruction(&Instruction::LocalSet(out));
        func.instruction(&Instruction::Br(1));
        func.instruction(&Instruction::End);
    }
    func.instruction(&Instruction::LocalGet(out));
    emit_call(func, reloc_enabled, import_ids[finish.import()]);
    func.instruction(&Instruction::LocalSet(out));
    func.instruction(&Instruction::End);
    func.instruction(&Instruction::End);
}
