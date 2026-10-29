use super::super::super::super::result_sink::store_owned_value;
use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
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

    match op.kind {
        // `molt_iter_next_unboxed(iterator, value_out)` returns the done flag
        // and writes the owned value (or None) to the out word as its last
        // action, so the shared scratch word is safe across generator code.
        molt_ir::OpKind::IterNextUnboxed => {
            let args = op.args.as_ref().unwrap();
            let mut value_name = None;
            let mut done_name = None;
            visit_simple_ir_results(op, |result| match result.field {
                SimpleIrResultField::Var => value_name = result.name,
                SimpleIrResultField::Out => done_name = result.name,
                SimpleIrResultField::Arg(_) => unreachable!("iterator has no trailing results"),
            });
            func.instruction(&Instruction::LocalGet(locals[&args[0]]));
            ctx.out_param.push_address(func);
            emit_call(
                func,
                reloc_enabled,
                import_ids[WasmRuntimeImport::IterNextUnboxed],
            );
            // The done flag is a bool, or None with an exception pending: it
            // holds no reference.
            match locals.bound_result_slot(done_name) {
                Some(done_slot) => func.instruction(&Instruction::LocalSet(done_slot)),
                None => func.instruction(&Instruction::Drop),
            };
            ctx.out_param.load(func);
            store_owned_value(
                func,
                locals.bound_result_slot(value_name),
                import_ids,
                reloc_enabled,
            );
        }
        _ => return false,
    }
    true
}
