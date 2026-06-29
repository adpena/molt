use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_set_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "set_new" => {
            let empty_args_sn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_sn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(args.len() as i64));
            emit_call(func, reloc_enabled, import_ids["set_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for name in args {
                let val = locals[name];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["set_add"]);
                func.instruction(&Instruction::Drop);
            }
        }
        "frozenset_new" => {
            let empty_args_fn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_fn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(args.len() as i64));
            emit_call(func, reloc_enabled, import_ids["frozenset_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for name in args {
                let val = locals[name];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["frozenset_add"]);
                func.instruction(&Instruction::Drop);
            }
        }
        _ => return false,
    }
    true
}
