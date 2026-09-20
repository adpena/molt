use super::super::super::super::builder_ops::{BuilderFinish, emit_sequence_builder_from_args};
use super::super::super::super::result_sink::{finish_owned_local_result, store_runtime_result};
use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_tuple_op(func: &mut Function, op: &OpIR, ctx: &AggregateRuntimeContext<'_>) {
    match op.kind.as_str() {
        "tuple_new" => emit_tuple_new(func, op, ctx),
        "tuple_index" => emit_tuple_index(func, op, ctx),
        _ => unreachable!("non-tuple aggregate op routed to tuple emitter"),
    }
}

fn emit_tuple_new(func: &mut Function, op: &OpIR, ctx: &AggregateRuntimeContext<'_>) {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    let empty_args: Vec<String> = Vec::new();
    let args = op.args.as_ref().unwrap_or(&empty_args);
    let out = locals.op_result_or_sink_slot(op);
    emit_sequence_builder_from_args(
        func,
        args,
        out,
        import_ids,
        locals,
        reloc_enabled,
        BuilderFinish::Tuple,
    );
    finish_owned_local_result(func, op, locals, import_ids, reloc_enabled, out);
}

fn emit_tuple_index(func: &mut Function, op: &OpIR, ctx: &AggregateRuntimeContext<'_>) {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    let args = op.args.as_ref().unwrap();
    let tuple_var = &args[0];
    let tuple = locals[tuple_var];
    let val = locals[&args[1]];
    func.instruction(&Instruction::LocalGet(tuple));
    func.instruction(&Instruction::LocalGet(val));
    emit_call(
        func,
        reloc_enabled,
        import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TupleIndex],
    );
    store_runtime_result(
        func,
        op,
        locals,
        import_ids,
        reloc_enabled,
        crate::wasm_abi_generated::WasmRuntimeImport::TupleIndex,
    );
}
