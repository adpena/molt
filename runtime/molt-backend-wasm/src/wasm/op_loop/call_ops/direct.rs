use super::frame::{
    collect_live_object_locals_for_call, release_live_object_locals, retain_live_object_locals,
};
use super::*;

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
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let func_map = call_ctx.func_map;
    let table_base = call_ctx.table_base;
    let import_ids = call_ctx.import_ids;
    let locals = call_ctx.locals;
    let reloc_enabled = call_ctx.reloc_enabled;

    let payload_len = op.args.as_ref().map(|args| args.len()).unwrap_or(0);
    let target_name = op.s_value.as_ref().expect("call_async target missing");
    let table_slot = *func_map
        .get(target_name)
        .unwrap_or_else(|| panic!("call_async table target not found: {target_name}"));
    let table_idx = table_base + table_slot;
    emit_table_index_i64(func, reloc_enabled, table_idx);
    func.instruction(&Instruction::I64Const((payload_len * 8) as i64));
    func.instruction(&Instruction::I64Const(TASK_KIND_FUTURE));
    emit_call(func, reloc_enabled, import_ids["task_new"]);
    let res = if let Some(out) = op.out.as_ref() {
        let r = locals[out];
        func.instruction(&Instruction::LocalSet(r));
        r
    } else {
        func.instruction(&Instruction::Drop);
        0
    };
    if let Some(args) = op.args.as_ref() {
        for (idx, arg) in args.iter().enumerate() {
            let arg_val = locals[arg];
            func.instruction(&Instruction::LocalGet(res));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
            func.instruction(&Instruction::I32Const((idx * 8) as i32));
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::LocalGet(arg_val));
            func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalGet(arg_val));
            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
        }
    }
    CallOpEmission::Handled
}

fn emit_plain_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let func_ir = call_ctx.func_ir;
    let ctx = call_ctx.ctx;
    let func_indices = call_ctx.func_indices;
    let import_ids = call_ctx.import_ids;
    let locals = call_ctx.locals;
    let reloc_enabled = call_ctx.reloc_enabled;
    let last_use_local = call_ctx.last_use_local;
    let rel_idx = call_ctx.rel_idx;

    let target_name = op.s_value.as_ref().unwrap();
    let args_names = op.args.as_ref().unwrap();
    let out = locals[op.out.as_ref().unwrap()];
    let live_object_locals =
        collect_live_object_locals_for_call(locals, last_use_local, rel_idx, op.out.as_ref());
    retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    let returns_alias_param = returns_alias_param(ctx, target_name, args_names);
    if returns_alias_param && std::env::var("MOLT_DEBUG_WASM_RETURN_ALIAS").as_deref() == Ok("1") {
        eprintln!(
            "[molt wasm return-alias] kind=call caller={} callee={}",
            func_ir.name, target_name
        );
    }
    let func_idx = *func_indices.get(target_name).unwrap_or_else(|| {
        panic!(
            "call target not found: '{}' in func '{}'",
            target_name, func_ir.name
        )
    });
    let bootstrap_call = func_idx == import_ids["runtime_init"];
    if bootstrap_call {
        push_call_args(func, locals, args_names);
        emit_call(func, reloc_enabled, func_idx);
        func.instruction(&Instruction::LocalSet(out));
        return CallOpEmission::Handled;
    }

    push_call_args(func, locals, args_names);
    emit_call(func, reloc_enabled, func_idx);
    store_call_result(func, import_ids, reloc_enabled, out, returns_alias_param);
    release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    CallOpEmission::Handled
}

fn emit_internal_call(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let func_ir = call_ctx.func_ir;
    let ctx = call_ctx.ctx;
    let func_indices = call_ctx.func_indices;
    let import_ids = call_ctx.import_ids;
    let locals = call_ctx.locals;
    let multi_return_candidates = call_ctx.multi_return_candidates;
    let multi_return = call_ctx.multi_return;
    let reloc_enabled = call_ctx.reloc_enabled;
    let arena_local = call_ctx.arena_local;
    let tail_call_count = call_ctx.tail_call_count;
    let last_use_local = call_ctx.last_use_local;
    let rel_idx = call_ctx.rel_idx;

    let target_name = op.s_value.as_ref().unwrap();
    let args_names = op.args.as_ref().unwrap();
    let out_name = op.out.as_ref().unwrap();
    let out = locals[out_name];
    let live_object_locals =
        collect_live_object_locals_for_call(locals, last_use_local, rel_idx, op.out.as_ref());
    retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    let returns_alias_param = returns_alias_param(ctx, target_name, args_names);
    if returns_alias_param && std::env::var("MOLT_DEBUG_WASM_RETURN_ALIAS").as_deref() == Ok("1") {
        eprintln!(
            "[molt wasm return-alias] kind=call_internal caller={} callee={}",
            func_ir.name, target_name
        );
    }
    let func_idx = *func_indices
        .get(target_name)
        .expect("call_internal target not found");
    let is_tail_call = is_tail_call_candidate(
        call_ctx,
        target_name,
        args_names,
        out_name,
        multi_return_candidates,
    );

    if is_tail_call && let Some(arena_idx) = arena_local {
        func.instruction(&Instruction::LocalGet(arena_idx));
        emit_call(func, reloc_enabled, import_ids["arena_free"]);
    }

    push_call_args(func, locals, args_names);

    if is_tail_call {
        emit_return_call(func, reloc_enabled, func_idx);
        tail_call_count.set(tail_call_count.get() + 1);
        return CallOpEmission::HandledAndSkipNext;
    }

    emit_call(func, reloc_enabled, func_idx);
    if multi_return.is_promoted_call_tuple(out_name) {
        let ret_count = multi_return_candidates[target_name];
        for k in (0..ret_count).rev() {
            let local_idx = multi_return
                .promoted_call_value_local(out_name, k as i64)
                .expect("multi-return call result local missing");
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::LocalSet(out));
    } else {
        store_call_result(func, import_ids, reloc_enabled, out, returns_alias_param);
    }
    release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
    CallOpEmission::Handled
}

fn returns_alias_param(
    ctx: &CompileFuncContext<'_>,
    target_name: &str,
    args_names: &[String],
) -> bool {
    ctx.return_alias_summaries
        .get(target_name)
        .and_then(|summary| match summary {
            crate::passes::ReturnAliasSummary::Param(param_idx)
                if *param_idx < args_names.len() =>
            {
                Some(*param_idx)
            }
            _ => None,
        })
        .is_some()
}

fn is_tail_call_candidate(
    call_ctx: &CallOpContext<'_, '_, '_>,
    target_name: &str,
    args_names: &[String],
    out_name: &str,
    multi_return_candidates: &BTreeMap<String, usize>,
) -> bool {
    call_ctx.tail_call_eligible
        && call_ctx.try_stack_is_empty
        && call_ctx.rel_idx + 1 < call_ctx.ops.len()
        && call_ctx.ops[call_ctx.rel_idx + 1].kind == "ret"
        && call_ctx.ops[call_ctx.rel_idx + 1].var.as_deref() == Some(out_name)
        && !multi_return_candidates.contains_key(target_name)
        && !target_name.contains("__molt_chunk_")
        && args_names.len() == call_ctx.func_ir.params.len()
}

fn push_call_args(func: &mut Function, locals: &WasmFrameLocals, args_names: &[String]) {
    for arg_name in args_names {
        let arg = locals[arg_name];
        func.instruction(&Instruction::LocalGet(arg));
    }
}

fn store_call_result(
    func: &mut Function,
    import_ids: &TrackedImportIds,
    reloc_enabled: bool,
    out: u32,
    returns_alias_param: bool,
) {
    if returns_alias_param {
        func.instruction(&Instruction::LocalTee(out));
        emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
    } else {
        func.instruction(&Instruction::LocalSet(out));
    }
}
