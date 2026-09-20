use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_int;
use molt_tir::tir::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};
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
            let mut value_name = None;
            let mut done_name = None;
            visit_simple_ir_results(op, |result| match result.field {
                SimpleIrResultField::Var => value_name = result.name,
                SimpleIrResultField::Out => done_name = result.name,
                SimpleIrResultField::Arg(_) => unreachable!("iterator has no trailing results"),
            });
            let iter = locals[&args[0]];
            let pair = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IterNext],
            );
            func.instruction(&Instruction::LocalSet(pair));
            if let Some(done_slot) = locals.bound_result_slot(done_name) {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(1)));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Index],
                );
                func.instruction(&Instruction::LocalSet(done_slot));
            }
            if let Some(value_slot) = locals.bound_result_slot(value_name) {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(0)));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Index],
                );
                func.instruction(&Instruction::LocalSet(value_slot));
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
