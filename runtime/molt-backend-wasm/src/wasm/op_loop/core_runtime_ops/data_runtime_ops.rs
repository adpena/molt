use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::{WasmFrameLocals, WasmFrameSyntheticLocal};
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};
use wasm_encoder::{BlockType, Function, Instruction};

#[allow(unused_variables)]
pub(super) fn emit_data_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    match op.kind.as_str() {
        "json_parse" => {
            emit_scalar_parse_op(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                WasmRuntimeImport::JsonParseScalar,
                WasmRuntimeImport::JsonParseScalarObj,
            );
        }
        "msgpack_parse" => {
            emit_scalar_parse_op(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                WasmRuntimeImport::MsgpackParseScalar,
                WasmRuntimeImport::MsgpackParseScalarObj,
            );
        }
        "cbor_parse" => {
            emit_scalar_parse_op(
                func,
                op,
                import_ids,
                locals,
                reloc_enabled,
                WasmRuntimeImport::CborParseScalar,
                WasmRuntimeImport::CborParseScalarObj,
            );
        }
        _ => return false,
    }
    true
}

fn emit_scalar_parse_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
    scalar_import: WasmRuntimeImport,
    object_import: WasmRuntimeImport,
) {
    let args = op.args.as_ref().unwrap();
    let arg_name = &args[0];
    let out_ptr = locals[op.out.as_ref().unwrap()];
    if let Some(scratch) = locals.try_parse_scalar_literal_scratch(arg_name) {
        let tmp_rc = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);

        func.instruction(&Instruction::I64Const(8));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Alloc],
        );
        func.instruction(&Instruction::LocalSet(out_ptr));

        func.instruction(&Instruction::LocalGet(scratch.ptr_local()));
        func.instruction(&Instruction::I32WrapI64);
        func.instruction(&Instruction::LocalGet(scratch.len_local()));
        func.instruction(&Instruction::LocalGet(out_ptr));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
        );
        emit_call(func, reloc_enabled, import_ids[scalar_import]);
        func.instruction(&Instruction::I64ExtendI32U);
        func.instruction(&Instruction::LocalSet(tmp_rc));

        func.instruction(&Instruction::LocalGet(tmp_rc));
        func.instruction(&Instruction::I64Eqz);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::LocalGet(out_ptr));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
        );
        func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
            align: 3,
            offset: 0,
            memory_index: 0,
        }));
        func.instruction(&Instruction::LocalSet(out_ptr));
        func.instruction(&Instruction::Else);
        func.instruction(&Instruction::LocalGet(locals[arg_name]));
        emit_call(func, reloc_enabled, import_ids[object_import]);
        func.instruction(&Instruction::LocalSet(out_ptr));
        func.instruction(&Instruction::End);
    } else {
        func.instruction(&Instruction::LocalGet(locals[arg_name]));
        emit_call(func, reloc_enabled, import_ids[object_import]);
        func.instruction(&Instruction::LocalSet(out_ptr));
    }
}
