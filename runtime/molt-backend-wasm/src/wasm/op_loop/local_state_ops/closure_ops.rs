use super::LocalStateOpContext;
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_closure_local_state_op(
    context: &mut LocalStateOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "closure_load" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let tmp_ptr = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            emit_call(func, reloc_enabled, import_ids["closure_load"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "closure_store" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            let tmp_ptr = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            func.instruction(&Instruction::LocalGet(locals[&args[1]]));
            emit_call(func, reloc_enabled, import_ids["closure_store"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        _ => return false,
    }
    true
}
