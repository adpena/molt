use super::super::super::result_sink::store_result_or_drop;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_import_tracking::TrackedImportIds;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_method_descriptor_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    reloc_enabled: bool,
) -> bool {
    match op.kind.as_str() {
        "classmethod_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::ClassmethodNew],
            );
            store_result_or_drop(func, op, locals);
        }
        "staticmethod_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(func_bits));
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::StaticmethodNew],
            );
            store_result_or_drop(func, op, locals);
        }
        "property_new" => {
            let args = op.args.as_ref().unwrap();
            let getter = locals[&args[0]];
            let setter = locals[&args[1]];
            let deleter = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(getter));
            func.instruction(&Instruction::LocalGet(setter));
            func.instruction(&Instruction::LocalGet(deleter));
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::PropertyNew],
            );
            store_result_or_drop(func, op, locals);
        }
        "bound_method_new" => {
            let args = op.args.as_ref().unwrap();
            let func_bits = locals[&args[0]];
            let self_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::LocalGet(self_bits));
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::BoundMethodNew],
            );
            store_result_or_drop(func, op, locals);
        }
        "is_bound_method" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::IsBoundMethod],
            );
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
