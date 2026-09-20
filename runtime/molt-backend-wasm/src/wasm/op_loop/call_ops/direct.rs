use super::super::result_sink::{
    finish_owned_local_result, store_owned_result_or_release, store_result_or_drop,
    store_runtime_result,
};
use super::site::{
    collect_live_object_locals_for_call, push_call_args, release_live_object_locals,
    retain_live_object_locals,
};
use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm::task_runtime::{
    WasmTaskRuntimeLayout, emit_store_task_payload_local, emit_task_payload_base,
};
use crate::wasm_abi_generated::wasm_runtime_import;
use crate::wasm_binary::{emit_call, emit_return_call};
use crate::wasm_values::emit_boxed_none;
use molt_tir::trampolines::TaskConstructorLayout;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_direct_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "call_async" => emit_call_async(call_ctx, func, op),
        "call" => emit_plain_call(call_ctx, func, op),
        "call_internal" => emit_internal_call(call_ctx, func, op),
        _ => CallOpEmission::NotHandled,
    }
}

fn emit_call_async(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let call_site_abi = call_ctx.call_site_abi;
    let import_ids = call_ctx.import_ids;
    let locals = call_ctx.locals;
    let reloc_enabled = call_ctx.reloc_enabled;

    let payload_len = op.args.as_ref().map(|args| args.len()).unwrap_or(0);
    let payload_size = TaskConstructorLayout::for_call_async().required_closure_size(
        payload_len,
        false,
        crate::wasm_abi::GEN_CONTROL_SIZE,
    );
    let layout = WasmTaskRuntimeLayout::for_call_async();
    let target_name = op.s_value.as_ref().expect("call_async target missing");
    let table_target = call_site_abi.table_target(target_name, "call_async");
    layout.emit_task_new(
        func,
        import_ids,
        call_ctx.table_relocations,
        reloc_enabled,
        call_ctx.func_import_count,
        call_ctx.func_index,
        &table_target,
        payload_size,
    );
    let res = locals.op_result_or_sink_slot(op);
    func.instruction(&Instruction::LocalSet(res));
    if let Some(args) = op.args.as_ref()
        && !args.is_empty()
    {
        func.instruction(&Instruction::LocalGet(res));
        emit_boxed_none(func);
        func.instruction(&Instruction::I64Ne);
        func.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
        let base_local = locals.synthetic(WasmFrameSyntheticLocal::WasmAllocResolve);
        emit_task_payload_base(func, import_ids, reloc_enabled, res, base_local);
        for (idx, arg) in args.iter().enumerate() {
            let arg_val = locals[arg];
            emit_store_task_payload_local(
                func,
                import_ids,
                reloc_enabled,
                base_local,
                layout.payload_base_offset() + (idx as i32) * 8,
                arg_val,
            );
        }
        func.instruction(&Instruction::End);
    }
    finish_owned_local_result(func, op, locals, import_ids, reloc_enabled, res);
    CallOpEmission::Handled
}

fn emit_plain_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let call_site_abi = call_ctx.call_site_abi;
    let import_ids = call_ctx.import_ids;
    let locals = call_ctx.locals;
    let reloc_enabled = call_ctx.reloc_enabled;
    let call_liveness = call_ctx.call_liveness;
    let call_live_idx = call_ctx.call_live_idx;

    let target_name = op.s_value.as_ref().unwrap();
    let args_names = op.args.as_deref().unwrap_or(&[]);
    let runtime_import = wasm_runtime_import(target_name);
    if let Some(import) = runtime_import {
        crate::wasm_abi::assert_runtime_i64_call(import, args_names.len());
    }
    let func_idx = call_site_abi.function_index(target_name, "call");
    let bootstrap_call =
        func_idx == import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RuntimeInit];
    if bootstrap_call {
        push_call_args(func, locals, args_names);
        emit_call(func, reloc_enabled, func_idx);
        store_runtime_result(
            func,
            op,
            locals,
            import_ids,
            reloc_enabled,
            runtime_import.expect("runtime_init import"),
        );
        return CallOpEmission::Handled;
    }

    let live_object_locals =
        collect_live_object_locals_for_call(locals, call_liveness, call_live_idx, op.out.as_ref());
    retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    push_call_args(func, locals, args_names);
    emit_call(func, reloc_enabled, func_idx);
    if let Some(import) = runtime_import {
        store_runtime_result(func, op, locals, import_ids, reloc_enabled, import);
    } else {
        let abi_returns_value = call_site_abi.function_abi_returns_value(target_name);
        finish_direct_call_result(call_ctx, func, op, abi_returns_value);
    }
    release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    CallOpEmission::Handled
}

fn emit_internal_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let call_site_abi = call_ctx.call_site_abi;
    let import_ids = call_ctx.import_ids;
    let locals = call_ctx.locals;
    let reloc_enabled = call_ctx.reloc_enabled;
    let tail_call_count = call_ctx.tail_call_count;
    let call_liveness = call_ctx.call_liveness;
    let call_live_idx = call_ctx.call_live_idx;

    let target_name = op.s_value.as_ref().unwrap();
    let args_names = op.args.as_deref().unwrap_or(&[]);
    assert!(
        wasm_runtime_import(target_name).is_none()
            && call_site_abi.positional_arity(target_name).is_some(),
        "call_internal requires a compiled function ABI: {target_name}"
    );
    let abi_returns_value = call_site_abi.function_abi_returns_value(target_name);
    let live_object_locals =
        collect_live_object_locals_for_call(locals, call_liveness, call_live_idx, op.out.as_ref());
    retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    let func_idx = call_site_abi.function_index(target_name, "call_internal");
    let is_tail_call = abi_returns_value
        && locals.bound_op_result_slot(op).is_some()
        && op.out.as_deref().is_some_and(|out_name| {
            is_tail_call_candidate(call_ctx, target_name, args_names, out_name)
        });

    if is_tail_call {
        call_ctx
            .frame
            .emit_const_anchor_releases(func, import_ids, reloc_enabled);
        push_call_args(func, locals, args_names);
        emit_return_call(func, reloc_enabled, func_idx);
        tail_call_count.set(tail_call_count.get() + 1);
        return CallOpEmission::HandledAndSkipNext;
    }

    push_call_args(func, locals, args_names);
    emit_call(func, reloc_enabled, func_idx);
    finish_direct_call_result(call_ctx, func, op, abi_returns_value);
    release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    CallOpEmission::Handled
}

fn finish_direct_call_result(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
    abi_returns_value: bool,
) {
    if !abi_returns_value {
        if call_ctx.locals.bound_op_result_slot(op).is_some() {
            emit_boxed_none(func);
            store_result_or_drop(func, op, call_ctx.locals);
        }
    } else {
        store_owned_result_or_release(
            func,
            op,
            call_ctx.locals,
            call_ctx.import_ids,
            call_ctx.reloc_enabled,
        );
    }
}

fn is_tail_call_candidate(
    call_ctx: &CallOpContext<'_, '_, '_>,
    target_name: &str,
    args_names: &[String],
    out_name: &str,
) -> bool {
    call_ctx.tail_call_enabled
        && call_ctx.tail_call_eligible
        && call_ctx.try_stack_is_empty
        && call_ctx.op_idx + 1 < call_ctx.ops.len()
        && molt_ir::tir::op_kinds_generated::simpleir_return_shape(
            call_ctx.ops[call_ctx.op_idx + 1].kind.as_str(),
        ) == molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Value
        && call_ctx.ops[call_ctx.op_idx + 1]
            .args
            .as_ref()
            .is_some_and(|args| args == &[out_name])
        && !target_name.contains("__molt_chunk_")
        && args_names.len() == call_ctx.func_ir.params.len()
}
