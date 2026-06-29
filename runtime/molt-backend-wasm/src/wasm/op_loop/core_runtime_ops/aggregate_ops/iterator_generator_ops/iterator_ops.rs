use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_int;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_iterator_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "iter_next_unboxed" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            let pair = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IterNext],
            );
            func.instruction(&Instruction::LocalSet(pair));
            if let Some(done_name) = op.out.as_ref()
                && done_name != "none"
            {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(1)));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Index],
                );
                func.instruction(&Instruction::LocalSet(locals[done_name]));
            }
            if let Some(val_name) = op.var.as_ref()
                && val_name != "none"
            {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(0)));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Index],
                );
                func.instruction(&Instruction::LocalSet(locals[val_name]));
            }
            func.instruction(&Instruction::LocalGet(pair));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj],
            );
        }
        _ => return false,
    }
    true
}
