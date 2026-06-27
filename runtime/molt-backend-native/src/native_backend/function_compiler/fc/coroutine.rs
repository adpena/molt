use super::super::*;

/// Single-source kind authority for [`handle_coroutine_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "state_switch",
    "state_transition",
    "state_yield",
    "chan_send_yield",
    "chan_recv_yield",
    "chan_new",
    "chan_drop",
    "spawn",
    "cancel_token_new",
    "cancel_token_clone",
    "cancel_token_drop",
    "cancel_token_cancel",
    "cancel_token_is_cancelled",
    "cancel_token_set_current",
    "cancel_token_get_current",
    "cancelled",
    "cancel_current",
    "call_async",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for coroutine, generator, async task, channel,
/// and cancellation state-machine ops. Extracted from `compile_func_inner` as a
/// move-only function split: block/reachability state, suspend cleanup authority,
/// and debug sealing are threaded explicitly.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_coroutine_op(
    op: &OpIR,
    ops: &[OpIR],
    op_idx: usize,
    entry_block: Block,
    master_return_block: Block,
    resume_states: &BTreeSet<i64>,
    resume_blocks: &BTreeMap<i64, Block>,
    label_blocks: &BTreeMap<i64, Block>,
    reachable_blocks: &mut BTreeSet<Block>,
    is_block_filled: &mut bool,
    rc_authority: NativeRcAuthority,
    returns_value: bool,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    entry_vars: &BTreeMap<String, Value>,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    local_exc_pending_fast: FuncRef,
    exc_flag_ptr_slot: Option<cranelift_codegen::ir::StackSlot>,
    maybe_debug_seal: &dyn Fn(&str, usize, Block),
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };

    match op.kind.as_str() {
        "state_switch" => {
            // Resume state belongs to the poll closure object passed
            // to the native poll function, not to any user-visible
            // local named `self` inside async methods.
            let self_ptr = builder.block_params(entry_block)[0];
            // State lives in the cold header (HashMap) — call through
            // the C API instead of an inline memory load.
            let get_state_ref = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_get_state",
                &[types::I64],
                &[types::I64],
            );
            let state_call = builder.ins().call(get_state_ref, &[self_ptr]);
            let state = builder.inst_results(state_call)[0];
            let self_bits = box_ptr_value(&mut *builder, self_ptr, nbc);
            def_var_named(&mut *builder, vars, "self", self_bits);

            let mut sorted_states: Vec<_> = resume_states.iter().copied().collect();
            sorted_states.sort();
            let fallback_block = builder.create_block();
            let mut switch = Switch::new();
            for id in sorted_states {
                let block = resume_blocks[&id];
                switch.set_entry((id as u64) as u128, block);
                reachable_blocks.insert(block);
            }
            reachable_blocks.insert(fallback_block);
            switch.emit(&mut *builder, state, fallback_block);
            switch_to_block_with_rebind(
                &mut *builder,
                fallback_block,
                &mut *is_block_filled,
                false,
            );
        }
        "state_transition" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let future = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Future not found");
            let future_ptr = unbox_ptr_value(&mut *builder, *future, nbc);
            let (slot_bits, pending_state_bits) = if args.len() == 2 {
                (
                    None,
                    *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[1],
                        representation_plan,
                    )
                    .expect("Pending state not found"),
                )
            } else {
                (
                    Some(
                        *var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &args[1],
                            representation_plan,
                        )
                        .expect("Await slot not found"),
                    ),
                    *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[2],
                        representation_plan,
                    )
                    .expect("Pending state not found"),
                )
            };
            let next_state_id = op.value.unwrap_or(0);
            let self_ptr = builder.block_params(entry_block)[0];

            let pending_state_id = unbox_int(&mut *builder, pending_state_bits, nbc);
            let set_state_ref = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder
                .ins()
                .call(set_state_ref, &[self_ptr, pending_state_id]);

            let poll_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_future_poll",
                &[types::I64],
                &[types::I64],
            );
            let local_poll = module.declare_func_in_func(poll_callee, builder.func);
            let poll_call = builder.ins().call(local_poll, &[*future]);
            let res = builder.inst_results(poll_call)[0];

            if let Some(target_id) = next_check_exception_target(ops, op_idx)
                && let Some(&target_block) = label_blocks.get(&target_id)
            {
                let fallthrough = builder.create_block();
                reachable_blocks.insert(target_block);
                reachable_blocks.insert(fallthrough);
                let has_exception = emit_exception_pending_condition(
                    &mut *builder,
                    local_exc_pending_fast,
                    exc_flag_ptr_slot,
                );
                brif_block(
                    &mut *builder,
                    has_exception,
                    target_block,
                    &[],
                    fallthrough,
                    &[],
                );
                if sealed_blocks.insert(fallthrough) {
                    maybe_debug_seal(
                        "state_transition_exception_fallthrough",
                        op_idx,
                        fallthrough,
                    );
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fallthrough);
                }
                switch_to_block_with_rebind(
                    &mut *builder,
                    fallthrough,
                    &mut *is_block_filled,
                    true,
                );
                *is_block_filled = false;
            }

            let pending_const = builder.ins().iconst(types::I64, pending_bits());
            let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

            let next_block = resume_blocks[&next_state_id];
            let pending_path = builder.create_block();
            let ready_path = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(pending_path, current_block);
                builder.insert_block_after(ready_path, pending_path);
            }
            reachable_blocks.insert(pending_path);
            reachable_blocks.insert(ready_path);
            reachable_blocks.insert(next_block);
            builder
                .ins()
                .brif(is_pending, pending_path, &[], ready_path, &[]);

            switch_to_block_with_rebind(&mut *builder, pending_path, &mut *is_block_filled, false);
            seal_block_once(&mut *builder, &mut *sealed_blocks, pending_path);
            let sleep_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_sleep_register",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_sleep = module.declare_func_in_func(sleep_callee, builder.func);
            builder.ins().call(local_sleep, &[self_ptr, future_ptr]);
            reachable_blocks.insert(master_return_block);
            // Suspend-boundary cleanup: an async `_poll` returns the PENDING
            // sentinel here and is re-entered on the next resume, so dead
            // per-iteration heap temporaries must be released now rather than
            // deferred to the per-await return (see
            // `drain_dead_block_temps_for_suspend`).
            drain_dead_block_temps_for_suspend(
                rc_authority,
                &mut *builder,
                &mut *block_tracked_obj,
                &mut *block_tracked_ptr,
                last_use,
                alias_roots,
                &mut *already_decrefed,
                entry_vars,
                vars,
                local_dec_ref_obj,
                op_idx,
            );
            jump_block(&mut *builder, master_return_block, &[pending_const]);

            switch_to_block_with_rebind(&mut *builder, ready_path, &mut *is_block_filled, false);
            seal_block_once(&mut *builder, &mut *sealed_blocks, ready_path);
            if let Some(bits) = slot_bits {
                let offset = unbox_int(&mut *builder, bits, nbc);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_closure_store",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder.ins().call(local_callee, &[self_ptr, offset, res]);
            }
            let state_val = builder.ins().iconst(types::I64, next_state_id);
            let set_state_ref2 = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder.ins().call(set_state_ref2, &[self_ptr, state_val]);
            if args.len() <= 1
                && let Some(out__) = op.out.as_ref()
            {
                def_var_from_boxed_transport(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    out__,
                    res,
                );
            }
            jump_block(&mut *builder, next_block, &[]);

            switch_to_block_with_rebind(&mut *builder, next_block, &mut *is_block_filled, false);
        }
        "state_yield" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let pair = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Yield pair not found");
            let next_state_id = op.value.unwrap_or(0);
            let self_ptr = builder.block_params(entry_block)[0];

            let state_val = builder.ins().iconst(types::I64, next_state_id);
            let set_state_yield = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder.ins().call(set_state_yield, &[self_ptr, state_val]);

            reachable_blocks.insert(master_return_block);
            if returns_value {
                // Suspension returns an owned value to the caller; explicitly
                // retain it here so downstream cleanup/control-flow lowering cannot
                // invalidate yielded data before next()/send()/throw() unwraps it.
                emit_inc_ref_obj(&mut *builder, *pair, local_inc_ref_obj, nbc);
            }
            // ── Suspend-boundary cleanup of dead per-iteration temporaries ──
            //
            // A `_poll` returns on every yield, so this jump-to-return is the
            // per-iteration scope exit for any heap temporary that is dead
            // before the suspend.  The headline case is the `(value, done)`
            // pair tuple built by `tuple_new` right before this op: it was
            // allocated rc=1, retained to rc=2 above (so it survives the
            // return), and is registered as a block-tracked temporary whose
            // real `last_use` is THIS op (kept un-extended for stateful
            // functions — see `stateful_per_iter_temps`).  Draining it here
            // takes its alloc reference back to rc=1, so the consumer's single
            // release frees it — closing the per-yield (and, under delegation,
            // O(iterations × depth)) tuple leak.  Loop-carried values survive
            // because their `last_use` was extended past this op and the
            // `last <= op_idx` gate keeps them live.
            drain_dead_block_temps_for_suspend(
                rc_authority,
                &mut *builder,
                &mut *block_tracked_obj,
                &mut *block_tracked_ptr,
                last_use,
                alias_roots,
                &mut *already_decrefed,
                entry_vars,
                vars,
                local_dec_ref_obj,
                op_idx,
            );
            if returns_value {
                jump_block(&mut *builder, master_return_block, &[*pair]);
            } else {
                jump_block(&mut *builder, master_return_block, &[]);
            }

            let next_block = resume_blocks[&next_state_id];
            if reachable_blocks.contains(&next_block) {
                switch_to_block_with_rebind(
                    &mut *builder,
                    next_block,
                    &mut *is_block_filled,
                    false,
                );
            } else {
                *is_block_filled = true;
            }
        }
        "chan_send_yield" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let chan = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Chan not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Val not found");
            let pending_state_bits = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Pending state not found");
            let next_state_id = op.value.unwrap_or(0);
            let self_ptr = builder.block_params(entry_block)[0];

            let pending_state_id = unbox_int(&mut *builder, pending_state_bits, nbc);
            let set_state_csend1 = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder
                .ins()
                .call(set_state_csend1, &[self_ptr, pending_state_id]);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_chan_send",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*chan, *val]);
            let res = builder.inst_results(call)[0];

            if let Some(target_id) = next_check_exception_target(ops, op_idx)
                && let Some(&target_block) = label_blocks.get(&target_id)
            {
                let fallthrough = builder.create_block();
                reachable_blocks.insert(target_block);
                reachable_blocks.insert(fallthrough);
                let has_exception = emit_exception_pending_condition(
                    &mut *builder,
                    local_exc_pending_fast,
                    exc_flag_ptr_slot,
                );
                brif_block(
                    &mut *builder,
                    has_exception,
                    target_block,
                    &[],
                    fallthrough,
                    &[],
                );
                if sealed_blocks.insert(fallthrough) {
                    maybe_debug_seal("chan_send_exception_fallthrough", op_idx, fallthrough);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fallthrough);
                }
                switch_to_block_with_rebind(
                    &mut *builder,
                    fallthrough,
                    &mut *is_block_filled,
                    true,
                );
                *is_block_filled = false;
            }

            let pending_const = builder.ins().iconst(types::I64, pending_bits());
            let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

            let next_block = resume_blocks[&next_state_id];
            let ready_path = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(ready_path, current_block);
            }
            reachable_blocks.insert(master_return_block);
            reachable_blocks.insert(ready_path);
            brif_block(
                &mut *builder,
                is_pending,
                master_return_block,
                &[pending_const],
                ready_path,
                &[],
            );

            switch_to_block_with_rebind(&mut *builder, ready_path, &mut *is_block_filled, false);
            seal_block_once(&mut *builder, &mut *sealed_blocks, ready_path);
            let state_val = builder.ins().iconst(types::I64, next_state_id);
            let set_state_csend2 = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder.ins().call(set_state_csend2, &[self_ptr, state_val]);
            if let Some(out__) = op.out.as_ref() {
                def_var_from_boxed_transport(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    out__,
                    res,
                );
            }
            reachable_blocks.insert(next_block);
            jump_block(&mut *builder, next_block, &[]);

            if reachable_blocks.contains(&next_block) {
                switch_to_block_with_rebind(
                    &mut *builder,
                    next_block,
                    &mut *is_block_filled,
                    false,
                );
            } else {
                *is_block_filled = true;
            }
        }
        "chan_recv_yield" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let chan = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Chan not found");
            let pending_state_bits = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Pending state not found");
            let next_state_id = op.value.unwrap_or(0);
            let self_ptr = builder.block_params(entry_block)[0];

            let pending_state_id = unbox_int(&mut *builder, pending_state_bits, nbc);
            let set_state_crecv1 = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder
                .ins()
                .call(set_state_crecv1, &[self_ptr, pending_state_id]);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_chan_recv",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*chan]);
            let res = builder.inst_results(call)[0];

            if let Some(target_id) = next_check_exception_target(ops, op_idx)
                && let Some(&target_block) = label_blocks.get(&target_id)
            {
                let fallthrough = builder.create_block();
                reachable_blocks.insert(target_block);
                reachable_blocks.insert(fallthrough);
                let has_exception = emit_exception_pending_condition(
                    &mut *builder,
                    local_exc_pending_fast,
                    exc_flag_ptr_slot,
                );
                brif_block(
                    &mut *builder,
                    has_exception,
                    target_block,
                    &[],
                    fallthrough,
                    &[],
                );
                if sealed_blocks.insert(fallthrough) {
                    maybe_debug_seal("chan_recv_exception_fallthrough", op_idx, fallthrough);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fallthrough);
                }
                switch_to_block_with_rebind(
                    &mut *builder,
                    fallthrough,
                    &mut *is_block_filled,
                    true,
                );
                *is_block_filled = false;
            }

            let pending_const = builder.ins().iconst(types::I64, pending_bits());
            let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

            let next_block = resume_blocks[&next_state_id];
            let ready_path = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(ready_path, current_block);
            }
            reachable_blocks.insert(master_return_block);
            reachable_blocks.insert(ready_path);
            brif_block(
                &mut *builder,
                is_pending,
                master_return_block,
                &[pending_const],
                ready_path,
                &[],
            );

            switch_to_block_with_rebind(&mut *builder, ready_path, &mut *is_block_filled, false);
            seal_block_once(&mut *builder, &mut *sealed_blocks, ready_path);
            let state_val = builder.ins().iconst(types::I64, next_state_id);
            let set_state_crecv2 = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder.ins().call(set_state_crecv2, &[self_ptr, state_val]);
            if let Some(out__) = op.out.as_ref() {
                def_var_from_boxed_transport(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    out__,
                    res,
                );
            }
            reachable_blocks.insert(next_block);
            jump_block(&mut *builder, next_block, &[]);

            if reachable_blocks.contains(&next_block) {
                switch_to_block_with_rebind(
                    &mut *builder,
                    next_block,
                    &mut *is_block_filled,
                    false,
                );
            } else {
                *is_block_filled = true;
            }
        }
        "chan_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let capacity = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Capacity not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_chan_new",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*capacity]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "chan_drop" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let chan = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Chan not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_chan_drop",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*chan]);
            let _ = builder.inst_results(call)[0];
        }
        "spawn" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let task = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Task not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_spawn",
                &[types::I64],
                &[],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*task]);
        }
        "cancel_token_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let parent = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Parent token not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_new",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*parent]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "cancel_token_clone" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let token = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Token not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_clone",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*token]);
        }
        "cancel_token_drop" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let token = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Token not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_drop",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*token]);
        }
        "cancel_token_cancel" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let token = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Token not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_cancel",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*token]);
        }
        "cancel_token_is_cancelled" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let token = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Token not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_is_cancelled",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*token]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "cancel_token_set_current" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let token = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Token not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_set_current",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*token]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "cancel_token_get_current" => {
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_token_get_current",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "cancelled" => {
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancelled",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "cancel_current" => {
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_cancel_current",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[]);
        }
        "call_async" => {
            let Some(poll_func_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            if !poll_func_name.ends_with("_poll") {
                panic!(
                    "call_async target '{poll_func_name}' is not a poll function; expected *_poll"
                );
            }
            let args = op.args.as_deref();
            let payload_len = args.map(|vals| vals.len()).unwrap_or(0);
            let size = builder.ins().iconst(types::I64, (payload_len * 8) as i64);
            let mut poll_sig = module.make_signature();
            poll_sig.params.push(AbiParam::new(types::I64));
            poll_sig.returns.push(AbiParam::new(types::I64));
            let poll_func_id = module
                .declare_function(poll_func_name, Linkage::Import, &poll_sig)
                .unwrap();
            let poll_func_ref = module.declare_func_in_func(poll_func_id, builder.func);
            let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

            let task_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_task_new",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let task_local = module.declare_func_in_func(task_callee, builder.func);
            let kind_val = builder.ins().iconst(types::I64, TASK_KIND_FUTURE);
            let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
            let obj = builder.inst_results(call)[0];
            let obj_ptr = unbox_ptr_value(&mut *builder, obj, nbc);

            if let Some(arg_names) = args
                && !arg_names.is_empty()
            {
                for (idx, arg_name) in arg_names.iter().enumerate() {
                    let val = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        arg_name,
                        representation_plan,
                    )
                    .expect("Arg not found");
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), *val, obj_ptr, (idx * 8) as i32);
                    emit_inc_ref_obj(&mut *builder, *val, local_inc_ref_obj, nbc);
                }
            }
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, obj);
        }
        _ => unreachable!("non-coroutine op routed to handle_coroutine_op"),
    }
    OpFlow::Proceed
}
