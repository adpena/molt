use super::super::*;

/// Single-source kind authority for [`handle_funcobj_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "builtin_func",
    "func_new",
    "func_new_closure",
    "code_new",
    "code_slot_set",
    "asyncgen_locals_register",
    "gen_locals_register",
    "code_slots_init",
    "trace_enter_slot",
    "trace_exit",
    "frame_locals_set",
    "line",
    "missing",
    "function_closure_bits",
];

/// Single-source kind authority for [`handle_gpu_intrinsic_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const GPU_INTRINSIC_HANDLED_KINDS: &[&str] = &[
    "gpu_thread_id",
    "gpu_block_id",
    "gpu_block_dim",
    "gpu_grid_dim",
    "gpu_barrier",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

#[cfg(feature = "native-backend")]
fn metadata_target_signature(
    module: &ObjectModule,
    op_kind: &str,
    target: &str,
    arities: &BTreeMap<String, usize>,
    returns: &BTreeMap<String, bool>,
) -> cranelift_codegen::ir::Signature {
    let arity = *arities
        .get(target)
        .unwrap_or_else(|| panic!("{op_kind} missing target signature for `{target}`"));
    let returns_value = *returns
        .get(target)
        .unwrap_or_else(|| panic!("{op_kind} missing target return ABI for `{target}`"));
    let mut signature = module.make_signature();
    for _ in 0..arity {
        signature.params.push(AbiParam::new(types::I64));
    }
    if returns_value {
        signature.returns.push(AbiParam::new(types::I64));
    }
    signature
}

/// Cranelift codegen handlers for function objects, code metadata, frame trace
/// metadata, and adjacent pre-call runtime intrinsics. Extracted from
/// `compile_func_inner` as a move-only function split: backend state is threaded
/// explicitly and outer-loop `continue` arms return `OpFlow::Continue`.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_funcobj_op(
    op: &OpIR,
    op_idx: usize,
    owned_frame_entered: Option<Variable>,
    leading_frame_entry_preemitted: bool,
    has_frame_slot: bool,
    is_block_filled: bool,
    rc_authority: NativeRcAuthority,
    in_loop: bool,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    task_kinds: &BTreeMap<String, TrampolineKind>,
    task_closure_sizes: &BTreeMap<String, i64>,
    defined_functions: &BTreeSet<String>,
    known_function_arities: &BTreeMap<String, usize>,
    function_has_ret: &BTreeMap<String, bool>,
    trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
    declared_func_arities: &mut BTreeMap<String, usize>,
    local_closure_envs: &mut BTreeMap<String, String>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    last_use: &BTreeMap<String, usize>,
    cleanup_roots: &mut NativeCleanupRoots,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
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
        "builtin_func" => {
            let Some(func_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let arity = op.value.unwrap_or(0);
            let mut func_sig = module.make_signature();
            for _ in 0..arity {
                func_sig.params.push(AbiParam::new(types::I64));
            }
            func_sig.returns.push(AbiParam::new(types::I64));
            let func_id = declare_function_object_target(
                &mut *module,
                "builtin_func",
                func_name,
                Linkage::Import,
                &func_sig,
            );
            declared_func_arities.insert(func_name.clone(), arity as usize);
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            let func_addr = builder.ins().func_addr(types::I64, func_ref);
            let tramp_id = SimpleBackend::ensure_trampoline(
                &mut *module,
                &mut *trampoline_ids,
                &mut *import_ids,
                func_name,
                Linkage::Import,
                TrampolineSpec {
                    arity: arity as usize,
                    has_closure: false,
                    kind: TrampolineKind::Plain,
                    closure_size: 0,
                    target_has_ret: true,
                },
            );
            let tramp_ref = module.declare_func_in_func(tramp_id, builder.func);
            let tramp_addr = builder.ins().func_addr(types::I64, tramp_ref);
            let arity_val = builder.ins().iconst(types::I64, arity);

            let call = if let Some(name_var) = op.args.as_ref().and_then(|args| args.first())
                && let Some(name_bits) = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    name_var,
                    representation_plan,
                )
                .map(|value| value.0)
            {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_func_new_builtin_named",
                    &[types::I64, types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder
                    .ins()
                    .call(local_callee, &[name_bits, func_addr, tramp_addr, arity_val])
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_func_new_builtin",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder
                    .ins()
                    .call(local_callee, &[func_addr, tramp_addr, arity_val])
            };
            let res = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, res, module, import_ids, builder, vars);
        }
        "func_new" => {
            let Some(func_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let arity = op.value.unwrap_or(0);
            let kind = task_kinds
                .get(func_name)
                .copied()
                .unwrap_or(TrampolineKind::Plain);
            let is_task = matches!(kind.behavior(), TrampolineBehavior::Task(_));
            let closure_size = if is_task {
                *task_closure_sizes
                    .get(func_name)
                    .expect("task constructor requires frame size")
            } else {
                0
            };
            let target_ret = function_has_ret
                .get(func_name.as_str())
                .copied()
                .unwrap_or(true);
            let mut func_sig = module.make_signature();
            if is_task {
                func_sig.params.push(AbiParam::new(types::I64));
            } else {
                for _ in 0..arity {
                    func_sig.params.push(AbiParam::new(types::I64));
                }
            }
            if target_ret {
                func_sig.returns.push(AbiParam::new(types::I64));
            }
            declared_func_arities.insert(func_name.clone(), func_sig.params.len());
            let func_id = declare_function_object_target(
                &mut *module,
                "func_new",
                func_name,
                Linkage::Import,
                &func_sig,
            );
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            let func_addr = builder.ins().func_addr(types::I64, func_ref);
            let target_has_ret = function_has_ret
                .get(func_name.as_str())
                .copied()
                .unwrap_or(true);
            let tramp_id = SimpleBackend::ensure_trampoline(
                &mut *module,
                &mut *trampoline_ids,
                &mut *import_ids,
                func_name,
                Linkage::Export,
                TrampolineSpec {
                    arity: arity as usize,
                    has_closure: false,
                    kind,
                    closure_size,
                    target_has_ret,
                },
            );
            let tramp_ref = module.declare_func_in_func(tramp_id, builder.func);
            let tramp_addr = builder.ins().func_addr(types::I64, tramp_ref);
            let arity_val = builder.ins().iconst(types::I64, arity);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_func_new",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[func_addr, tramp_addr, arity_val]);
            let res = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, res, module, import_ids, builder, vars);
        }
        "func_new_closure" => {
            let Some(func_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let arity = op.value.unwrap_or(0);
            let kind = task_kinds
                .get(func_name)
                .copied()
                .unwrap_or(TrampolineKind::Plain);
            let is_task = matches!(kind.behavior(), TrampolineBehavior::Task(_));
            let closure_size = if is_task {
                *task_closure_sizes
                    .get(func_name)
                    .expect("task constructor requires frame size")
            } else {
                0
            };
            let closure_name = op
                .args
                .as_ref()
                .and_then(|args| args.first())
                .expect("func_new_closure expects closure arg");
            let closure_bits = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                closure_name,
                representation_plan,
            )
            .expect("closure arg not found");
            let target_ret = function_has_ret
                .get(func_name.as_str())
                .copied()
                .unwrap_or(true);
            let mut func_sig = module.make_signature();
            if is_task {
                func_sig.params.push(AbiParam::new(types::I64));
            } else {
                func_sig.params.push(AbiParam::new(types::I64));
                for _ in 0..arity {
                    func_sig.params.push(AbiParam::new(types::I64));
                }
            }
            if target_ret {
                func_sig.returns.push(AbiParam::new(types::I64));
            }
            declared_func_arities.insert(func_name.clone(), func_sig.params.len());
            // Use Export linkage only when the closure target is
            // defined in this compilation unit; otherwise Import
            // (resolved at link time for batched builds).
            let closure_linkage = if defined_functions.contains(func_name) {
                Linkage::Export
            } else {
                Linkage::Import
            };
            let func_id = declare_function_object_target(
                &mut *module,
                "func_new_closure",
                func_name,
                closure_linkage,
                &func_sig,
            );
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            let func_addr = builder.ins().func_addr(types::I64, func_ref);
            let target_has_ret = function_has_ret
                .get(func_name.as_str())
                .copied()
                .unwrap_or(true);
            let tramp_id = SimpleBackend::ensure_trampoline(
                &mut *module,
                &mut *trampoline_ids,
                &mut *import_ids,
                func_name,
                Linkage::Export,
                TrampolineSpec {
                    arity: arity as usize,
                    has_closure: true,
                    kind,
                    closure_size,
                    target_has_ret,
                },
            );
            let tramp_ref = module.declare_func_in_func(tramp_id, builder.func);
            let tramp_addr = builder.ins().func_addr(types::I64, tramp_ref);
            let arity_val = builder.ins().iconst(types::I64, arity);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_func_new_closure",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[func_addr, tramp_addr, arity_val, closure_bits],
            );
            let res = builder.inst_results(call)[0];
            // Track closure function object for direct calls
            if let Some(out_name) = op.out.as_ref() {
                local_closure_envs.insert(func_name.clone(), out_name.clone());
            }
            bind_owned_runtime_result(op, res, module, import_ids, builder, vars);
        }
        "code_new" => {
            let args = op.args.as_ref().expect("admitted code_new operands");
            let filename_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("filename not found");
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("name not found");
            let firstlineno_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("firstlineno not found");
            let linetable_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                representation_plan,
            )
            .expect("linetable not found");
            let varnames_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[4],
                representation_plan,
            )
            .expect("varnames not found");
            let names_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[5],
                representation_plan,
            )
            .expect("names not found");
            let argcount_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[6],
                representation_plan,
            )
            .expect("argcount not found");
            let posonlyargcount_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[7],
                representation_plan,
            )
            .expect("posonly not found");
            let kwonlyargcount_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[8],
                representation_plan,
            )
            .expect("kwonly not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_code_new",
                &[
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                ],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[
                    *filename_bits,
                    *name_bits,
                    *firstlineno_bits,
                    *linetable_bits,
                    *varnames_bits,
                    *names_bits,
                    *argcount_bits,
                    *posonlyargcount_bits,
                    *kwonlyargcount_bits,
                ],
            );
            let res = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, res, module, import_ids, builder, vars);
        }
        "code_slot_set" => {
            let args = op.args.as_ref().expect("admitted code_slot_set operands");
            let code_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("code bits not found");
            let globals_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("code globals not found");
            let code_id = op.value.expect("admitted code_slot_set ID");
            let code_id_val = builder.ins().iconst(types::I64, code_id);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_code_slot_set",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder
                .ins()
                .call(local_callee, &[code_id_val, *code_bits, *globals_bits]);
        }
        "asyncgen_locals_register" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let names_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("names tuple not found");
            let offsets_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("offsets tuple not found");
            let func_name = op
                .s_value
                .as_ref()
                .expect("asyncgen_locals_register expects symbol");
            let func_sig = metadata_target_signature(
                module,
                &op.kind,
                func_name,
                known_function_arities,
                function_has_ret,
            );
            let linkage = if defined_functions.contains(func_name) {
                Linkage::Export
            } else {
                Linkage::Import
            };
            let func_id = declare_function_object_target(
                &mut *module,
                "asyncgen_locals_register",
                func_name,
                linkage,
                &func_sig,
            );
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            let func_addr = builder.ins().func_addr(types::I64, func_ref);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_asyncgen_locals_register",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder
                .ins()
                .call(local_callee, &[func_addr, *names_bits, *offsets_bits]);
        }
        "gen_locals_register" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let names_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("names tuple not found");
            let offsets_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("offsets tuple not found");
            let func_name = op
                .s_value
                .as_ref()
                .expect("gen_locals_register expects symbol");
            // Pointer metadata refers to the target's physical ABI, not the
            // Python callable arity or the spelling of the symbol.
            let func_sig = metadata_target_signature(
                module,
                &op.kind,
                func_name,
                known_function_arities,
                function_has_ret,
            );
            let linkage = if defined_functions.contains(func_name) {
                Linkage::Export
            } else {
                Linkage::Import
            };
            let func_id = declare_function_object_target(
                &mut *module,
                "gen_locals_register",
                func_name,
                linkage,
                &func_sig,
            );
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            let func_addr = builder.ins().func_addr(types::I64, func_ref);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_gen_locals_register",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder
                .ins()
                .call(local_callee, &[func_addr, *names_bits, *offsets_bits]);
        }
        "code_slots_init" => {
            let count = op.value.expect("admitted code_slots_init count");
            let count_val = builder.ins().iconst(types::I64, count);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_code_slots_init",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder.ins().call(local_callee, &[count_val]);
        }
        "trace_enter_slot" => {
            let entered = owned_frame_entered
                .expect("trace_enter_slot requires local execution-context ownership");
            if leading_frame_entry_preemitted {
                debug_assert_eq!(op_idx, 0);
            } else {
                emit_owned_execution_frame_enter(
                    entered,
                    op.value.expect("admitted trace_enter_slot ID"),
                    module,
                    import_ids,
                    builder,
                );
            }
        }
        "trace_exit" => {}
        "frame_locals_set" => {
            let arg_names = op.args.as_deref().unwrap_or(&[]);
            let dict_bits = arg_names
                .first()
                .map(|name| {
                    *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    )
                    .expect("Arg not found")
                })
                .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_frame_locals_set",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder.ins().call(local_callee, &[dict_bits]);
        }
        "line" => {
            // Inside active loops, skip line tracking entirely.
            // These are debug-info calls (~3ns each) that dominate
            // inner-loop cost when inlining arithmetic and stores.
            // Exception tracebacks still get correct line info from
            // the last line op before the loop or at loop entry.
            if in_loop {
                return OpFlow::Continue;
            }
            let line = op.value.unwrap_or(0);
            let line_val = builder.ins().iconst(types::I64, line);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_trace_set_line",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder.ins().call(local_callee, &[line_val]);
            // Update frame stack line (+ column offsets) for tracebacks.
            if has_frame_slot {
                let has_col = op.col_offset.is_some() && op.end_col_offset.is_some();
                if has_col {
                    let col_val = builder.ins().iconst(types::I64, op.col_offset.unwrap());
                    let end_col_val = builder.ins().iconst(types::I64, op.end_col_offset.unwrap());
                    let frame_line_col_fn = import_func_ref(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        "molt_frame_set_line_col",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    builder
                        .ins()
                        .call(frame_line_col_fn, &[line_val, col_val, end_col_val]);
                } else {
                    let frame_line_fn = import_func_ref(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        "molt_frame_set_line",
                        &[types::I64],
                        &[types::I64],
                    );
                    builder.ins().call(frame_line_fn, &[line_val]);
                }
            }
            if !is_block_filled && let Some(block) = builder.current_block() {
                for tracked in [
                    block_tracked_obj.get_mut(&block),
                    block_tracked_ptr.get_mut(&block),
                ]
                .into_iter()
                .flatten()
                {
                    for name in
                        drain_cleanup_candidates(rc_authority, tracked, last_use, op_idx, None)
                    {
                        cleanup_roots.release(builder, local_dec_ref_obj, &name);
                    }
                }
            }
        }
        "missing" => {
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_missing",
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
        "function_closure_bits" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let func_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Func not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_function_closure_bits",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*func_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                // The runtime borrows the function's closure edge. Acquire an
                // owner only for a bound result; discarding it needs no RC.
                emit_inc_ref_obj(&mut *builder, res, local_inc_ref_obj);
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("non-function-object op routed to handle_funcobj_op"),
    }
    OpFlow::Proceed
}

/// Narrow handler for native GPU runtime intrinsics that sit in the same
/// pre-call opcode neighborhood but do not need the function-object state.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_gpu_intrinsic_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    vars: &BTreeMap<String, Variable>,
) {
    match op.kind.as_str() {
        "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" | "gpu_barrier" => {
            let symbol = match op.kind.as_str() {
                "gpu_thread_id" => "molt_gpu_thread_id",
                "gpu_block_id" => "molt_gpu_block_id",
                "gpu_block_dim" => "molt_gpu_block_dim",
                "gpu_grid_dim" => "molt_gpu_grid_dim",
                "gpu_barrier" => "molt_gpu_barrier",
                _ => unreachable!(),
            };
            let local_callee = import_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                symbol,
                &[],
                &[types::I64],
            );
            let call = builder.ins().call(local_callee, &[]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
                if op.kind != "gpu_barrier" {}
            }
        }
        _ => unreachable!("non-GPU intrinsic op routed to handle_gpu_intrinsic_op"),
    }
}
