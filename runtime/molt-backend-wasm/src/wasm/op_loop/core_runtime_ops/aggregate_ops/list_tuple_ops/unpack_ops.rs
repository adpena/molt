use super::super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_int;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_unpack_sequence(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    // args[0] is the sequence, args[1..] are output variable names.
    // op.value holds the expected element count.
    // The sequence may be a list (from _emit_list_from_iter) or
    // a tuple, so use the general-purpose `index` import which
    // handles both via __getitem__.
    let args = op.args.as_ref().unwrap();
    let seq = locals[&args[0]];
    let expected_count = op.value.unwrap() as usize;
    for i in 0..expected_count {
        let out = locals[&args[1 + i]];
        func.instruction(&Instruction::LocalGet(seq));
        func.instruction(&Instruction::I64Const(box_int(i as i64)));
        emit_call(
            func,
            reloc_enabled,
            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::Index],
        );
        func.instruction(&Instruction::LocalSet(out));
    }
}
