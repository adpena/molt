use super::super::super::super::builder_ops::{WordRangeConstructor, emit_word_range_constructor};
use super::super::super::super::result_sink::finish_owned_local_result;
use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_abi_generated::WasmRuntimeImport;
use wasm_encoder::Function;

pub(super) fn emit_list_op(func: &mut Function, op: &OpIR, ctx: &AggregateRuntimeContext<'_>) {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    let empty_args_ln: Vec<String> = Vec::new();
    let args = op.args.as_ref().unwrap_or(&empty_args_ln);
    let out = locals.op_result_or_sink_slot(op);
    emit_word_range_constructor(
        func,
        args,
        out,
        WordRangeConstructor {
            import: WasmRuntimeImport::ListFromValues,
            leading: &[],
            trailing: &[],
        },
        import_ids,
        locals,
        reloc_enabled,
    );
    finish_owned_local_result(func, op, locals, import_ids, reloc_enabled, out);
}
