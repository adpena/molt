use super::super::super::super::builder_ops::{BuilderFinish, emit_sequence_builder_from_args};
use super::super::AggregateRuntimeContext;
use crate::OpIR;
use wasm_encoder::Function;

pub(super) fn emit_list_op(func: &mut Function, op: &OpIR, ctx: &AggregateRuntimeContext<'_>) {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    let empty_args_ln: Vec<String> = Vec::new();
    let args = op.args.as_ref().unwrap_or(&empty_args_ln);
    let out = locals[op.out.as_ref().unwrap()];
    emit_sequence_builder_from_args(
        func,
        args,
        out,
        import_ids,
        locals,
        reloc_enabled,
        BuilderFinish::List,
    );
}
