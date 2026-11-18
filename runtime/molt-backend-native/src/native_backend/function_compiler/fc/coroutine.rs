use super::super::*;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeFn;
use molt_codegen_abi::task_site::TaskConstructorLayout;

// Single-source kind authority for [`handle_coroutine_op`], consulted by
// `op_family::native_op_family`. Mirror the `match op.kind` arms below.
#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;
    OpKind::StateSwitch,
    OpKind::StateTransition,
    OpKind::StateYield,
    OpKind::StateSet,
    OpKind::IsPending,
    OpKind::TaskWait,
    OpKind::CallAsync,
);
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for coroutine, generator, and async-task
/// state-machine primitives. Shared TIR owns activation exits and cleanup.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_coroutine_op(
    op: &OpIR,
    _entry_block: Block,
    resume_blocks: &BTreeMap<i64, Block>,
    reachable_blocks: &mut BTreeSet<Block>,
    is_block_filled: &mut bool,
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    function_table: &molt_ir::FunctionTable,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let var_get_boxed_overflow_safe = |module: &mut dyn Module,
                                       import_ids: &mut RuntimeImportIds,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut NativeFunctionImports,
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

    match op.kind {
        OpKind::StateSwitch => {
            // Resume state belongs to the poll closure object passed
            // to the native poll function, not to any user-visible
            // local named `self` inside async methods.
            let self_ptr = compiled_argument(builder, 0);
            // State may live inline or in the stable aux sidecar — call through
            // the C API instead of an inline memory load.
            let get_state_ref = import_runtime_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                RuntimeFn::ObjGetState,
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
        kind @ (OpKind::StateTransition | OpKind::StateYield) => panic!(
            "native backend: `{kind}` reached codegen; the shared terminal drop pass must expose it as explicit activation exits"
        ),
        OpKind::StateSet => {
            // The state is an attribute of the transition. Saving it through
            // the poll frame neither suspends nor returns. A ready wait saves
            // its running state, which no resume dispatches to.
            let state = op.value.expect("state_set requires its state");
            let self_ptr = compiled_argument(builder, 0);
            let state_val = builder.ins().iconst(types::I64, state);
            let set_state = import_runtime_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                RuntimeFn::ObjSetState,
            );
            builder.ins().call(set_state, &[self_ptr, state_val]);
        }
        OpKind::IsPending => {
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
                OpKind::IsPending,
            );
            let pending_word = builder.ins().iconst(types::I64, pending_bits());
            let pending = builder.ins().icmp(IntCC::Equal, word, pending_word);
            if let Some(out) = op.out.as_ref() {
                let raw = builder.ins().uextend(types::I64, pending);
                def_raw_bool_value(&mut *builder, vars, representation_plan, out, raw, nbc);
            }
        }
        OpKind::TaskWait => {
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
                OpKind::TaskWait,
            );
            let self_ptr = compiled_argument(builder, 0);
            let future_ptr = unbox_ptr_value(&mut *builder, word);
            let sleep_register = import_runtime_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                RuntimeFn::SleepRegister,
            );
            builder.ins().call(sleep_register, &[self_ptr, future_ptr]);
        }
        OpKind::CallAsync => {
            let out_name = op
                .out
                .as_ref()
                .expect("call_async requires an owned result");
            let poll_func_name = op
                .reference_symbol(function_table)
                .expect("call_async target missing");
            use molt_codegen_abi::task_site::{PayloadCustody, TaskFrameContext};
            let arg_names = op.args.as_deref().unwrap_or(&[]);
            let layout = TaskConstructorLayout::for_call_async();
            let closure_size =
                layout.required_closure_size(arg_names.len(), false, GENERATOR_CONTROL_BYTES);
            let size = builder.ins().iconst(types::I64, closure_size);
            let poll_addr =
                SimpleBackend::task_poll_identity(module, builder, poll_func_name, Linkage::Import);
            // A repeated value keeps one identity (`SameAs` its first slot); a
            // raw int is boxed by the task once it exists; every other word is
            // retained by its slot unless it owns nothing.
            let mut first_slot: BTreeMap<&str, usize> = BTreeMap::new();
            let mut payload: Vec<(Value, PayloadCustody)> = Vec::with_capacity(arg_names.len());
            for (slot, arg_name) in arg_names.iter().enumerate() {
                if let Some(&first) = first_slot.get(arg_name.as_str()) {
                    payload.push((payload[first].0, PayloadCustody::SameAs(first)));
                    continue;
                }
                first_slot.insert(arg_name.as_str(), slot);
                if representation_plan.is_raw_int_carrier_name(arg_name) {
                    let raw = *var_get(builder, vars, arg_name)
                        .unwrap_or_else(|| panic!("call_async operand {arg_name} not found"));
                    payload.push((raw, PayloadCustody::RawInt));
                    continue;
                }
                let val = *var_get_boxed_overflow_safe(
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
                let custody = if representation_plan.boxed_refcount_is_noop(arg_name) {
                    PayloadCustody::Unowned
                } else {
                    PayloadCustody::Borrowed
                };
                payload.push((val, custody));
            }
            let sites = super::calls::native_frame_sites(module, import_ids, builder, import_refs);
            let tasks = super::calls::native_task_sites(module, import_ids, builder, import_refs);
            let tracking_origin = builder.current_block();
            let obj = crate::native_backend::simple_backend::emit_task_construction(
                builder,
                sealed_blocks,
                sites,
                tasks,
                layout,
                poll_addr,
                size,
                TaskFrameContext::NoFrame,
                molt_codegen_abi::task_site::TaskPayload::Words(&payload),
            );
            if let Some(continuation) = builder.current_block() {
                carry_internal_cfg_tracking(
                    tracking_origin,
                    continuation,
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
    kind: OpKind,
) -> Value {
    assert!(
        !representation_plan.is_raw_int_carrier_name(name)
            && !representation_plan.is_float_unboxed(name)
            && !representation_plan.is_bool_unboxed(name),
        "{kind} operand `{name}` must be an object carrier"
    );
    *var_get(builder, vars, name).unwrap_or_else(|| panic!("{kind} operand `{name}` not found"))
}
