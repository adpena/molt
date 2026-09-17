use super::RuntimeServiceOpContext;
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm::task_runtime::{
    WasmTaskRuntimeLayout, emit_register_cancel_token, emit_store_task_payload_local,
    emit_task_payload_base,
};
use crate::wasm_binary::emit_call;
use crate::wasm_values::emit_boxed_none;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_async_task_runtime_op(
    context: &mut RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let call_site_abi = context.call_site_abi;
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "alloc_task" => {
            let total = op.value.unwrap_or(0);
            let layout = WasmTaskRuntimeLayout::for_alloc_task_kind(op.task_kind.as_deref());
            molt_tir::trampolines::TaskConstructorLayout::for_alloc_kind(op.task_kind.as_deref())
                .validate_closure_size(
                    total,
                    op.args.as_ref().map_or(0, Vec::len),
                    false,
                    crate::wasm_abi::GEN_CONTROL_SIZE,
                );
            let target_name = op.s_value.as_ref().expect("alloc_task target missing");
            let table_target = call_site_abi.table_target(target_name, "alloc_task");
            layout.emit_task_new(
                func,
                import_ids,
                context.table_relocations,
                reloc_enabled,
                context.func_import_count,
                context.func_index,
                &table_target,
                total,
            );
            let out = op
                .out
                .as_ref()
                .expect("alloc_task requires an owned result");
            let res = locals[out];
            func.instruction(&Instruction::LocalSet(res));
            func.instruction(&Instruction::LocalGet(res));
            emit_boxed_none(func);
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
            // Resolve the task handle pointer once when we need to
            // materialize closure/argument payload slots after the
            // runtime-owned control block.
            let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
            if has_args {
                let resolve_local = locals.synthetic(WasmFrameSyntheticLocal::WasmAllocResolve);
                emit_task_payload_base(func, import_ids, reloc_enabled, res, resolve_local);
            }
            if let Some(args) = op.args.as_ref()
                && !args.is_empty()
            {
                let resolve_local = locals.synthetic(WasmFrameSyntheticLocal::WasmAllocResolve);
                for (i, name) in args.iter().enumerate() {
                    let arg_local = locals[name];
                    emit_store_task_payload_local(
                        func,
                        import_ids,
                        reloc_enabled,
                        resolve_local,
                        layout.payload_base_offset() + (i as i32) * 8,
                        arg_local,
                    );
                }
            }
            if layout.registers_cancel_token() {
                emit_register_cancel_token(func, import_ids, reloc_enabled, res);
            }
            func.instruction(&Instruction::End);
        }
        "state_yield" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(0));
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ObjSetState],
            );
            let pair = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(pair));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
            );
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::LocalSet(locals[out]));
                func.instruction(&Instruction::LocalGet(locals[out]));
            } else {
                func.instruction(&Instruction::LocalGet(pair));
            }
            context
                .frame
                .emit_const_anchor_releases(func, import_ids, reloc_enabled);
            func.instruction(&Instruction::Return);
        }
        _ => return false,
    }
    true
}
