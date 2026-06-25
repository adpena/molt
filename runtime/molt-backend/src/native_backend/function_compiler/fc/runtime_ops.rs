use super::super::*;

/// Single-source kind authority for [`handle_runtime_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "env_get",
    "exception_pending",
    "function_defaults_version",
    "print",
    "warn_stderr",
    "print_newline",
    "block_on",
    "bridge_unavailable",
];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for residual runtime probe/call shims. These ops
/// read mutable runtime state or call side-effecting runtime helpers, but do not
/// terminate the current block; the parent dispatch must still run the per-op
/// epilogue after this handler returns.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_runtime_op(
    op: &OpIR,
    func_name: &str,
    is_block_filled: bool,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    local_exc_pending_fast: FuncRef,
    exc_flag_ptr_slot: Option<cranelift_codegen::ir::StackSlot>,
    nbc: &crate::NanBoxConsts,
) {
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
        "env_get" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Env key not found");
            let default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Env default not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_env_get",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*key, *default]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "exception_pending" => {
            // Read the runtime exception-pending flag as a boolean value:
            // `molt_exception_pending_fast() != 0`. Produced by the TIR
            // `ExceptionPending` op and consumed by loop-exit branches.
            let cond_bool = emit_exception_pending_condition(
                &mut *builder,
                local_exc_pending_fast,
                exc_flag_ptr_slot,
            );
            let raw_bool = builder.ins().uextend(types::I64, cond_bool);
            if let Some(out__) = op.out.as_ref() {
                def_raw_bool_value(
                    &mut *builder,
                    vars,
                    representation_plan,
                    out__,
                    raw_bool,
                    nbc,
                );
            }
        }
        "function_defaults_version" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let func_boxed = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("FunctionDefaultsVersion arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_function_defaults_version",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*func_boxed]);
            let boxed_res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                if representation_plan.is_raw_int_carrier_name(out__) {
                    let raw_res = unbox_int(&mut *builder, boxed_res, nbc);
                    def_var_named(&mut *builder, vars, out__, raw_res);
                } else {
                    def_var_named(&mut *builder, vars, out__, boxed_res);
                }
            }
        }
        "print" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = if let Some(val) = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            ) {
                *val
            } else {
                builder.ins().iconst(types::I64, box_none())
            };

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_print_obj",
                &[types::I64],
                &[],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[val]);
        }
        "warn_stderr" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            if std::env::var("MOLT_DEBUG_WARN_BACKEND").is_ok() {
                eprintln!(
                    "[WARN_BACKEND] warn_stderr op in func={} args={:?} is_block_filled={}",
                    func_name, args, is_block_filled
                );
            }
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("warn_stderr arg");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_warn_stderr",
                &[types::I64],
                &[],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*val]);
        }
        "print_newline" => {
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_print_newline",
                &[],
                &[],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[]);
        }
        "block_on" => {
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
                "molt_block_on",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*task]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bridge_unavailable" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let msg = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Message not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bridge_unavailable",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*msg]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("non-runtime op routed to handle_runtime_op"),
    }
}
