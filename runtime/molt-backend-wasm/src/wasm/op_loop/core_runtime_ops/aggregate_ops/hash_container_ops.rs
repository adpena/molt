use super::super::super::result_sink::{discard_runtime_result, finish_owned_local_result};
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_none;
use wasm_encoder::{BlockType, Function, Instruction};

/// Hash-container construction keeps one owner through every insertion. A
/// mutator's borrowed receiver/None return is never a replacement owner.
pub(super) fn emit_hash_container_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let (constructor, insert, entry_width) = match op.kind.as_str() {
        "dict_new" => (WasmRuntimeImport::DictNew, WasmRuntimeImport::DictSet, 2),
        "set_new" => (WasmRuntimeImport::SetNew, WasmRuntimeImport::SetAdd, 1),
        "frozenset_new" => (
            WasmRuntimeImport::FrozensetNew,
            WasmRuntimeImport::FrozensetAdd,
            1,
        ),
        _ => return false,
    };
    let args = op.args.as_deref().unwrap_or(&[]);
    assert_eq!(
        args.len() % entry_width,
        0,
        "{} requires complete entries",
        op.kind
    );
    let imports = ctx.import_ids;
    let locals = ctx.locals;
    let reloc = ctx.reloc_enabled;
    let out = locals.op_result_or_sink_slot(op);
    func.instruction(&Instruction::I64Const((args.len() / entry_width) as i64));
    emit_call(func, reloc, imports[constructor]);
    func.instruction(&Instruction::LocalSet(out));

    if !args.is_empty() {
        // Failed allocation must not materialize or hash any element.
        func.instruction(&Instruction::LocalGet(out));
        func.instruction(&Instruction::I64Const(box_none()));
        func.instruction(&Instruction::I64Ne);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::Block(BlockType::Empty));
        for entry in args.chunks_exact(entry_width) {
            func.instruction(&Instruction::LocalGet(out));
            for value in entry {
                func.instruction(&Instruction::LocalGet(locals[value]));
            }
            emit_call(func, reloc, imports[insert]);
            discard_runtime_result(func, imports, reloc, insert);
            emit_call(func, reloc, imports[WasmRuntimeImport::ExceptionPending]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Empty));
            // Preserve the pending exception, retire the partial container,
            // and skip all remaining user-observable hash/equality calls.
            func.instruction(&Instruction::LocalGet(out));
            emit_call(func, reloc, imports[WasmRuntimeImport::DecRefObj]);
            func.instruction(&Instruction::I64Const(box_none()));
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::Br(1));
            func.instruction(&Instruction::End);
        }
        func.instruction(&Instruction::End);
        func.instruction(&Instruction::End);
    }
    finish_owned_local_result(func, op, locals, imports, reloc, out);
    true
}
