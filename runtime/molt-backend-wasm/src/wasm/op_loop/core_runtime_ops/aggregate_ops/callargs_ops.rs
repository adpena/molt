use super::super::super::result_sink::store_runtime_result;
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_callargs_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "callargs_new" => {
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
