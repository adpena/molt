use super::super::builder_ops::{BuilderFinish, emit_sequence_builder_from_args};
use super::super::result_sink::{finish_owned_local_result, store_runtime_result};
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::box_none;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_dataclass_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "dataclass_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let values = locals[&args[2]];
            let flags = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(fields));
            func.instruction(&Instruction::LocalGet(values));
            func.instruction(&Instruction::LocalGet(flags));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DataclassNew],
            );
            store_runtime_result(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
                crate::wasm_abi_generated::WasmRuntimeImport::DataclassNew,
            );
        }
        "dataclass_new_values" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let fields = locals[&args[1]];
            let flags = locals[&args[2]];
            let out = locals.op_result_or_sink_slot(op);
            emit_sequence_builder_from_args(
                func,
                &args[3..],
                out,
                import_ids,
                locals,
                reloc_enabled,
                BuilderFinish::Tuple,
            );
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(fields));
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::LocalGet(flags));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DataclassNew],
            );
            // Dataclass construction borrows the completed values tuple. Keep
            // its result on the stack while releasing that temporary owner.
            func.instruction(&Instruction::LocalGet(out));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj],
            );
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);
            finish_owned_local_result(func, op, locals, import_ids, reloc_enabled, out);
        }
        "dataclass_get" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DataclassGet],
            );
            store_runtime_result(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
                crate::wasm_abi_generated::WasmRuntimeImport::DataclassGet,
            );
        }
        "dataclass_set" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let idx = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(idx));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DataclassSet],
            );
            store_runtime_result(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
                crate::wasm_abi_generated::WasmRuntimeImport::DataclassSet,
            );
        }
        "dataclass_set_class" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let class_obj = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(obj));
            func.instruction(&Instruction::LocalGet(class_obj));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DataclassSetClass],
            );
            store_runtime_result(
                func,
                op,
                locals,
                import_ids,
                reloc_enabled,
                crate::wasm_abi_generated::WasmRuntimeImport::DataclassSetClass,
            );
        }
        _ => return false,
    }
    true
}
