use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_dict_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "dict_new" => {
            let empty_args_dn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_dn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const((args.len() / 2) as i64));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DictNew],
            );
            func.instruction(&Instruction::LocalSet(out));
            for pair in args.chunks(2) {
                let key = locals[&pair[0]];
                let val = locals[&pair[1]];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(key));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DictSet],
                );
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        _ => return false,
    }
    true
}
