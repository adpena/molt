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
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids["callargs_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        _ => return false,
    }
    true
}
