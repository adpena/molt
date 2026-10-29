use super::super::super::result_sink::{discard_runtime_result, finish_owned_local_result};
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::emit_call;
use crate::wasm_values::box_none;
use wasm_encoder::{BlockType, Function, Instruction};

/// Dictionaries use the runtime full-pairs constructor. Incremental sets keep
/// one owner through every insertion; a mutator result never replaces it.
pub(super) fn emit_hash_container_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    if op.kind == molt_ir::OpKind::DictNew {
        use super::super::super::builder_ops::{WordRangeConstructor, emit_word_range_constructor};
        let args = op.args.as_deref().unwrap_or(&[]);
        assert!(
            args.len().is_multiple_of(2),
            "dict literal needs complete pairs"
        );
        let out = ctx.locals.op_result_or_sink_slot(op);
        emit_word_range_constructor(
            func,
            args,
            out,
            WordRangeConstructor {
                import: WasmRuntimeImport::DictFromPairs,
                leading: &[],
                trailing: &[],
            },
            ctx.import_ids,
            ctx.locals,
            ctx.reloc_enabled,
        );
        finish_owned_local_result(func, op, ctx.locals, ctx.import_ids, ctx.reloc_enabled, out);
        return true;
    }
    let (constructor, insert) = match op.kind {
        molt_ir::OpKind::SetNew => (WasmRuntimeImport::SetNew, WasmRuntimeImport::SetAdd),
        molt_ir::OpKind::FrozensetNew => (
            WasmRuntimeImport::FrozensetNew,
            WasmRuntimeImport::FrozensetAdd,
        ),
        _ => return false,
    };
    let args = op.args.as_deref().unwrap_or(&[]);
    let imports = ctx.import_ids;
    let locals = ctx.locals;
    let reloc = ctx.reloc_enabled;
    let out = locals.op_result_or_sink_slot(op);
    func.instruction(&Instruction::I64Const(args.len() as i64));
    emit_call(func, reloc, imports[constructor]);
    func.instruction(&Instruction::LocalSet(out));

    if !args.is_empty() {
        // Failed allocation must not materialize or hash any element.
        func.instruction(&Instruction::LocalGet(out));
        func.instruction(&Instruction::I64Const(box_none()));
        func.instruction(&Instruction::I64Ne);
        func.instruction(&Instruction::If(BlockType::Empty));
        func.instruction(&Instruction::Block(BlockType::Empty));
        for value in args {
            func.instruction(&Instruction::LocalGet(out));
            func.instruction(&Instruction::LocalGet(locals[value]));
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
