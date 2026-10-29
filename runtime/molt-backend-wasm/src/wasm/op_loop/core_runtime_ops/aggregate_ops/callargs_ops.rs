use super::super::super::result_sink::store_runtime_result;
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_callargs_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind {
        molt_ir::OpKind::CallargsPushMethodReceiver => {
            let [builder, receiver] = op.args.as_deref().unwrap_or(&[]) else {
                panic!("validated method receiver prefix takes builder and receiver");
            };
            let push = crate::wasm_abi_generated::WasmRuntimeImport::CallargsPushPos;
            let release = crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj;
            func.instruction(&Instruction::LocalGet(locals[receiver]));
            func.instruction(&Instruction::I64Const(
                molt_codegen_abi::loaded_method_null_bits(),
            ));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(locals[builder]));
            func.instruction(&Instruction::LocalGet(locals[receiver]));
            emit_call(func, reloc_enabled, import_ids[push]);
            store_runtime_result(func, op, locals, import_ids, reloc_enabled, push);
            // push_pos borrows and retains. Settle the loaded pair's original
            // receiver credit on success and failure, before user arguments.
            func.instruction(&Instruction::LocalGet(locals[receiver]));
            emit_call(func, reloc_enabled, import_ids[release]);
            func.instruction(&Instruction::End);
        }
        molt_ir::OpKind::CallargsNew => {
            // The source call form picks the builder: a CALL_FUNCTION_EX call
            // site's arguments are its own tuple and mapping.
            let constructor = match op
                .call_argument_form()
                .expect("validated callargs_new call form")
            {
                molt_ir::CallArgumentForm::Stack => {
                    crate::wasm_abi_generated::WasmRuntimeImport::CallargsNew
                }
                molt_ir::CallArgumentForm::Expanded => {
                    crate::wasm_abi_generated::WasmRuntimeImport::CallargsNewExpanded
                }
            };
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids[constructor]);
            store_runtime_result(func, op, locals, import_ids, reloc_enabled, constructor);
        }
        _ => return false,
    }
    true
}
