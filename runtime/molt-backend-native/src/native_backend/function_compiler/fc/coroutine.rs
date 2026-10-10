use super::super::*;
use crate::runtime_import_abi::MOLT_TASK_NEW;
use molt_tir::trampolines::TaskConstructorLayout;

/// Single-source kind authority for [`handle_coroutine_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "state_switch",
    "state_transition",
    "state_yield",
    "state_set",
    "is_pending",
    "task_wait",
    "call_async",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for coroutine, generator, and async-task
/// state-machine primitives. Shared TIR owns activation exits and cleanup.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_coroutine_op(
    op: &OpIR,
    entry_block: Block,
    resume_blocks: &BTreeMap<i64, Block>,
    reachable_blocks: &mut BTreeSet<Block>,
    is_block_filled: &mut bool,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    local_inc_ref_obj: FuncRef,
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
            // State may live inline or in the stable aux sidecar — call through
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

            let fallback_block = builder.create_block();
            let mut switch = Switch::new();
            for (&id, &block) in resume_blocks {
                switch.set_entry((id as u64) as u128, block);
                reachable_blocks.insert(block);
            }
            reachable_blocks.insert(fallback_block);
            switch.emit(&mut *builder, state, fallback_block);
            crate::switch_to_block_tracking(&mut *builder, fallback_block, &mut *is_block_filled);
        }
        kind @ ("state_transition" | "state_yield") => panic!(
            "native backend: `{kind}` reached codegen; the shared terminal drop pass must expose it as explicit activation exits"
        ),
        "state_set" => {
            // The state is an attribute of the transition. Saving it through
            // the poll frame neither suspends nor returns. A ready wait saves
            // its running state, which no resume dispatches to.
            let state = op.value.expect("state_set requires its state");
            let self_ptr = builder.block_params(entry_block)[0];
            let state_val = builder.ins().iconst(types::I64, state);
            let set_state = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_obj_set_state",
                &[types::I64, types::I64],
                &[],
            );
            builder.ins().call(set_state, &[self_ptr, state_val]);
        }
        "is_pending" => {
            // The scheduler sentinel is one exact word: no truthiness, no
            // callback and no owner.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let [poll] = args.as_slice() else {
                panic!("is_pending expects one poll result");
            };
            let word = activation_object_word(
                &mut *builder,
                vars,
                representation_plan,
                poll,
                "is_pending",
            );
            let pending_word = builder.ins().iconst(types::I64, pending_bits());
            let pending = builder.ins().icmp(IntCC::Equal, word, pending_word);
            if let Some(out) = op.out.as_ref() {
                let raw = builder.ins().uextend(types::I64, pending);
                def_raw_bool_value(&mut *builder, vars, representation_plan, out, raw, nbc);
            }
        }
        "task_wait" => {
            // Wake this activation when the future completes. Registration
            // takes the frame and the future's object address and retains
            // nothing: explicit TIR still owns the future.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let [future] = args.as_slice() else {
                panic!("task_wait expects one future");
            };
            let word = activation_object_word(
                &mut *builder,
                vars,
                representation_plan,
                future,
                "task_wait",
            );
            let self_ptr = builder.block_params(entry_block)[0];
            let future_ptr = unbox_ptr_value(&mut *builder, word);
            let sleep_register = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                "molt_sleep_register",
                &[types::I64, types::I64],
                &[types::I64],
            );
            builder.ins().call(sleep_register, &[self_ptr, future_ptr]);
        }
        "call_async" => {
            let out_name = op
                .out
                .as_ref()
                .expect("call_async requires an owned result");
            let poll_func_name = op.s_value.as_ref().expect("call_async target missing");
            let args = op.args.as_deref();
            let payload_len = args.map(|vals| vals.len()).unwrap_or(0);
            let layout = TaskConstructorLayout::for_call_async();
            let closure_size =
                layout.required_closure_size(payload_len, false, GENERATOR_CONTROL_BYTES);
            let size = builder.ins().iconst(types::I64, closure_size);
            let poll_addr =
                SimpleBackend::task_poll_identity(module, builder, poll_func_name, Linkage::Import);

            let task_callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                MOLT_TASK_NEW,
            );
            let task_local = module.declare_func_in_func(task_callee, builder.func);
            let kind_val = builder.ins().iconst(
                types::I64,
                crate::native_task_runtime_kind_bits(layout.runtime_kind()),
            );
            let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
            let obj = builder.inst_results(call)[0];

            if let Some(arg_names) = args
                && !arg_names.is_empty()
            {
                let tracking_origin = builder.current_block();
                let initialized = begin_task_initialization(builder, sealed_blocks, obj);
                let obj_ptr = unbox_ptr_value(&mut *builder, obj);
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
                    emit_inc_ref_obj(&mut *builder, *val, local_inc_ref_obj);
                }
                jump_block(builder, initialized, &[]);
                switch_to_block_materialized(builder, initialized);
                seal_block_once(builder, sealed_blocks, initialized);
                carry_internal_cfg_tracking(
                    tracking_origin,
                    initialized,
                    block_tracked_obj,
                    block_tracked_ptr,
                );
            }
            def_var_named(&mut *builder, vars, out_name, obj);
        }
        _ => unreachable!("non-coroutine op routed to handle_coroutine_op"),
    }
    OpFlow::Proceed
}

/// Scheduler words are objects: a poll result or an awaited future. A raw
/// scalar carrier could alias the sentinel's bits and would need a box that
/// nothing owns, so reaching one here is a lowering defect.
#[cfg(feature = "native-backend")]
fn activation_object_word(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
    kind: &str,
) -> Value {
    assert!(
        !representation_plan.is_raw_int_carrier_name(name)
            && !representation_plan.is_float_unboxed(name)
            && !representation_plan.is_bool_unboxed(name),
        "{kind} operand `{name}` must be an object carrier"
    );
    *var_get(builder, vars, name).unwrap_or_else(|| panic!("{kind} operand `{name}` not found"))
}
