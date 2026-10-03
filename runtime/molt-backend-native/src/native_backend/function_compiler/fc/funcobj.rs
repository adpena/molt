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
    "stateful_locals_register",
    "code_slots_init",
    "trace_enter_slot",
    "trace_exit",
    "frame_context_set",
    "frame_home_store",
    "frame_home_cell",
    "frame_home_private_cell",
    "frame_home_load",
    "frame_home_take",
    "frame_home_clear",
    "frame_locals",
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

/// The runtime entry custody of the function object a `func_new` creates for
/// `target`, from the target's own parameter declaration. A target with no
/// declaration, or one the runtime cannot encode in one bit, is a compiler
/// error: guessing "borrowed" would let a transferring body release its
/// caller's references.
#[cfg(feature = "native-backend")]
fn entry_custody_word(
    declarations: &BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration>,
    target: &str,
    has_closure: bool,
    arity: i64,
) -> u64 {
    let declaration = declarations
        .get(target)
        .unwrap_or_else(|| panic!("func_new target `{target}` has no parameter declaration"));
    usize::try_from(arity)
        .map_err(|_| molt_codegen_abi::EntryCustodyError::Signature)
        .and_then(|arity| declaration.encode(has_closure, arity))
        .unwrap_or_else(|error| {
            panic!("func_new target `{target}` has no runtime entry custody: {error:?}")
        })
}

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

/// Borrow the executing frame's binding homes for `slots` code slots into
/// `homes` and return the lent base: `molt_frame_homes` lends it, or 0 when
/// the frame entry failed (its exception pending) or the frame has fewer
/// homes (`SystemError` raised). Callers route 0 through an exception edge.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_frame_homes_lend(
    homes: Variable,
    slots: i64,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
) -> Value {
    let slots_val = builder.ins().iconst(types::I64, slots);
    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_frame_homes",
        &[types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[slots_val]);
    let base = builder.inst_results(call)[0];
    builder.def_var(homes, base);
    base
}

/// The byte offset of a frame-home op's code slot in the lent homes.
#[cfg(feature = "native-backend")]
fn frame_home_offset(op: &OpIR) -> i32 {
    op.value
        .expect("admitted frame home slot")
        .checked_mul(molt_codegen_abi::FRAME_HOME_BYTES)
        .and_then(|offset| i32::try_from(offset).ok())
        .expect("frame home slot offset overflows")
}

/// Release a displaced home binding when its kind owns a reference: after
/// the new pair is published, so the finalizer it runs sees the new binding.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn emit_frame_home_release(
    old_kind: Value,
    old_bits: Value,
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    local_dec_ref_obj: FuncRef,
) {
    let holds = builder
        .ins()
        .band_imm(old_kind, molt_codegen_abi::FRAME_HOME_HOLDS_REFERENCE);
    let origin = builder.current_block();
    let release = builder.create_block();
    let done = builder.create_block();
    builder.ins().brif(holds, release, &[], done, &[]);
    switch_to_block_materialized(builder, release);
    seal_block_once(builder, sealed_blocks, release);
    builder.ins().call(local_dec_ref_obj, &[old_bits]);
    jump_block(builder, done, &[]);
    switch_to_block_materialized(builder, done);
    seal_block_once(builder, sealed_blocks, done);
    carry_internal_cfg_tracking(origin, done, block_tracked_obj, block_tracked_ptr);
}

/// Box the raw integer a home was just given into that home, for a boxed view
/// of it: in place when it fits the inline range, otherwise through
/// `molt_frame_home_load`, which allocates the object into the home or, when
/// the allocation fails, keeps the raw binding and leaves MemoryError pending
/// for this store's authored exception check. The result is borrowed from the home
/// (the missing sentinel after a failure). A store never allocates otherwise.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn box_frame_home_raw_int(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    homes: Value,
    offset: i32,
    raw: Value,
) -> Value {
    let fits = int_value_fits_inline(builder, raw);
    let origin = builder.current_block();
    let inline = builder.create_block();
    let slow = builder.create_block();
    let done = builder.create_block();
    builder.append_block_param(done, types::I64);
    builder.set_cold_block(slow);
    builder.ins().brif(fits, inline, &[], slow, &[]);
    switch_to_block_materialized(builder, inline);
    seal_block_once(builder, sealed_blocks, inline);
    let nbc = crate::NanBoxConsts::new();
    let int_mask = builder.ins().iconst(types::I64, nbc.int_mask);
    let masked = builder.ins().band(raw, int_mask);
    let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let boxed = builder.ins().bor(int_tag, masked);
    let plain = builder
        .ins()
        .iconst(types::I64, molt_codegen_abi::FRAME_HOME_PLAIN);
    builder.ins().store(
        MemFlagsData::trusted(),
        plain,
        homes,
        offset + molt_codegen_abi::FRAME_HOME_KIND_OFFSET,
    );
    builder.ins().store(
        MemFlagsData::trusted(),
        boxed,
        homes,
        offset + molt_codegen_abi::FRAME_HOME_BITS_OFFSET,
    );
    jump_block(builder, done, &[boxed]);
    switch_to_block_materialized(builder, slow);
    seal_block_once(builder, sealed_blocks, slow);
    let home = builder.ins().iadd_imm(homes, i64::from(offset));
    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_frame_home_load",
        &[types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[home]);
    let loaded = builder.inst_results(call)[0];
    jump_block(builder, done, &[loaded]);
    switch_to_block_materialized(builder, done);
    seal_block_once(builder, sealed_blocks, done);
    carry_internal_cfg_tracking(origin, done, block_tracked_obj, block_tracked_ptr);
    builder.block_params(done)[0]
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
    frame_homes: Option<(Variable, i64)>,
    leading_frame_entry_preemitted: Option<usize>,
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
    function_entry_custody: &BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration>,
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
            // A task constructor's arguments enter through its task
            // trampoline, which retains each into the new task: it borrows.
            let entry_custody = if is_task {
                0
            } else {
                entry_custody_word(function_entry_custody, func_name, false, arity)
            };
            let custody_val = builder.ins().iconst(types::I64, entry_custody as i64);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_func_new",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[func_addr, tramp_addr, arity_val, custody_val],
            );
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
            // A task constructor's arguments enter through its task
            // trampoline, which retains each into the new task: it borrows.
            let entry_custody = if is_task {
                0
            } else {
                entry_custody_word(function_entry_custody, func_name, true, arity)
            };
            let custody_val = builder.ins().iconst(types::I64, entry_custody as i64);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_func_new_closure",
                &[types::I64, types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[func_addr, tramp_addr, arity_val, closure_bits, custody_val],
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
        "stateful_locals_register" => {
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
            let layout_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("layout tuple not found");
            let func_name = op
                .s_value
                .as_ref()
                .expect("stateful_locals_register expects symbol");
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
                "stateful_locals_register",
                func_name,
                linkage,
                &func_sig,
            );
            let func_ref = module.declare_func_in_func(func_id, builder.func);
            let func_addr = builder.ins().func_addr(types::I64, func_ref);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_stateful_locals_register",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder
                .ins()
                .call(local_callee, &[func_addr, *names_bits, *layout_bits]);
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
            if let Some(leading_frame_entry_op_idx) = leading_frame_entry_preemitted {
                debug_assert_eq!(op_idx, leading_frame_entry_op_idx);
            } else {
                emit_owned_execution_frame_enter(
                    entered,
                    op.value.expect("admitted trace_enter_slot ID"),
                    module,
                    import_ids,
                    builder,
                );
                // The entry took the frame's binding homes: lend them before
                // the entry's adjacent exception check, which leaves for the
                // entry-failure label when the lend is 0.
                if let Some((homes, slots)) = frame_homes {
                    emit_frame_homes_lend(homes, slots, module, import_ids, builder);
                }
            }
        }
        "trace_exit" => {}
        "frame_home_store" | "frame_home_cell" | "frame_home_private_cell" => {
            let homes = builder.use_var(
                frame_homes
                    .expect("frame home ops require the frame's lent homes")
                    .0,
            );
            let offset = frame_home_offset(op);
            let src_name = &op.args.as_deref().expect("admitted frame home operand")[0];
            let out_name = op.out.as_deref().filter(|name| *name != "none");
            let kind_offset = offset + molt_codegen_abi::FRAME_HOME_KIND_OFFSET;
            let bits_offset = offset + molt_codegen_abi::FRAME_HOME_BITS_OFFSET;
            let old_kind =
                builder
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), homes, kind_offset);
            let old_bits =
                builder
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), homes, bits_offset);
            // The home takes operand 0's reference: the op consumes it. A raw
            // integer carrier is published raw, holding no reference; any other
            // value is published boxed (floats and bools box inline). A boxed
            // view of a raw integer can allocate into the home below; the
            // store's authored CheckException transfers failure before the next
            // effect or lexical-region exit. Cell stores adopt existing cells.
            let raw_int = (op.kind == "frame_home_store")
                .then(|| int_raw_value(&mut *builder, vars, representation_plan, src_name))
                .flatten();
            let bits = match raw_int {
                Some(raw) => {
                    let kind_val = builder
                        .ins()
                        .iconst(types::I64, molt_codegen_abi::FRAME_HOME_RAW_INT);
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), kind_val, homes, kind_offset);
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), raw, homes, bits_offset);
                    if out_name.is_none_or(|name| representation_plan.is_raw_int_carrier_name(name))
                    {
                        raw
                    } else {
                        box_frame_home_raw_int(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *sealed_blocks,
                            &mut *block_tracked_obj,
                            &mut *block_tracked_ptr,
                            homes,
                            offset,
                            raw,
                        )
                    }
                }
                None => {
                    let kind = match op.kind.as_str() {
                        "frame_home_cell" => molt_codegen_abi::FRAME_HOME_CELL,
                        "frame_home_private_cell" => molt_codegen_abi::FRAME_HOME_PRIVATE_CELL,
                        _ => molt_codegen_abi::FRAME_HOME_PLAIN,
                    };
                    let bits = *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        src_name,
                        representation_plan,
                    )
                    .expect("frame home operand not found");
                    let kind_val = builder.ins().iconst(types::I64, kind);
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), kind_val, homes, kind_offset);
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), bits, homes, bits_offset);
                    bits
                }
            };
            emit_frame_home_release(
                old_kind,
                old_bits,
                &mut *builder,
                &mut *sealed_blocks,
                &mut *block_tracked_obj,
                &mut *block_tracked_ptr,
                local_dec_ref_obj,
            );
            // The result is a view: the operand in its own representation,
            // owning nothing, valid until this slot's next write.
            if let Some(out_name) = out_name
                && !super::value_transfer::def_unboxed_lane_from(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    representation_plan,
                    nbc,
                    src_name,
                    out_name,
                )
            {
                def_var_named(&mut *builder, vars, out_name, bits);
            }
        }
        "frame_home_load" => {
            // A borrowed view of the slot's plain binding. `PLAIN` is read
            // inline; the runtime boxes a raw integer into the home, reports an
            // unbound slot as the missing sentinel, and raises for a cell.
            let homes = builder.use_var(
                frame_homes
                    .expect("frame home ops require the frame's lent homes")
                    .0,
            );
            let offset = frame_home_offset(op);
            let kind = builder.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                homes,
                offset + molt_codegen_abi::FRAME_HOME_KIND_OFFSET,
            );
            let plain =
                builder
                    .ins()
                    .icmp_imm(IntCC::Equal, kind, molt_codegen_abi::FRAME_HOME_PLAIN);
            let origin = builder.current_block();
            let inline = builder.create_block();
            let slow = builder.create_block();
            let done = builder.create_block();
            builder.append_block_param(done, types::I64);
            builder.set_cold_block(slow);
            builder.ins().brif(plain, inline, &[], slow, &[]);
            switch_to_block_materialized(&mut *builder, inline);
            seal_block_once(&mut *builder, &mut *sealed_blocks, inline);
            let bits = builder.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                homes,
                offset + molt_codegen_abi::FRAME_HOME_BITS_OFFSET,
            );
            jump_block(&mut *builder, done, &[bits]);
            switch_to_block_materialized(&mut *builder, slow);
            seal_block_once(&mut *builder, &mut *sealed_blocks, slow);
            let home = builder.ins().iadd_imm(homes, i64::from(offset));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_frame_home_load",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[home]);
            let loaded = builder.inst_results(call)[0];
            jump_block(&mut *builder, done, &[loaded]);
            switch_to_block_materialized(&mut *builder, done);
            seal_block_once(&mut *builder, &mut *sealed_blocks, done);
            carry_internal_cfg_tracking(origin, done, block_tracked_obj, block_tracked_ptr);
            let value = builder.block_params(done)[0];
            if let Some(out_name) = op.out.as_deref() {
                def_var_named(&mut *builder, vars, out_name, value);
            }
        }
        "frame_home_take" => {
            // PEP 709's save of an enclosing binding: the runtime moves it out
            // of the home, which becomes unbound, and the result owns it.
            let homes = builder.use_var(
                frame_homes
                    .expect("frame home ops require the frame's lent homes")
                    .0,
            );
            let home = builder
                .ins()
                .iadd_imm(homes, i64::from(frame_home_offset(op)));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_frame_home_take",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[home]);
            let taken = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, taken, module, import_ids, builder, vars);
        }
        "frame_home_clear" => {
            // `del`: the slot becomes unbound, then what it held is released.
            let homes = builder.use_var(
                frame_homes
                    .expect("frame home ops require the frame's lent homes")
                    .0,
            );
            let offset = frame_home_offset(op);
            let kind_offset = offset + molt_codegen_abi::FRAME_HOME_KIND_OFFSET;
            let bits_offset = offset + molt_codegen_abi::FRAME_HOME_BITS_OFFSET;
            let old_kind =
                builder
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), homes, kind_offset);
            let old_bits =
                builder
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), homes, bits_offset);
            let unbound = builder
                .ins()
                .iconst(types::I64, molt_codegen_abi::FRAME_HOME_UNBOUND);
            builder
                .ins()
                .store(MemFlagsData::trusted(), unbound, homes, kind_offset);
            let zero = builder.ins().iconst(types::I64, 0);
            builder
                .ins()
                .store(MemFlagsData::trusted(), zero, homes, bits_offset);
            emit_frame_home_release(
                old_kind,
                old_bits,
                &mut *builder,
                &mut *sealed_blocks,
                &mut *block_tracked_obj,
                &mut *block_tracked_ptr,
                local_dec_ref_obj,
            );
        }
        "frame_locals" => {
            // `locals()`, `vars()`, `dir()`: the runtime's one authority reads
            // the executing frame's bindings; the operand pairs are for
            // targets that build the dict themselves.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_locals_builtin",
                &[],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[]);
            let dict = builder.inst_results(call)[0];
            bind_owned_runtime_result(op, dict, module, import_ids, builder, vars);
        }
        "frame_context_set" => {
            let args: Vec<_> = op
                .args
                .as_deref()
                .expect("admitted frame_context_set operands")
                .iter()
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
                    .expect("frame context operand not found")
                })
                .collect();
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_frame_context_set",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            // ABI returns borrowed None; the typed service has no SSA result
            // and follows the authored exception continuation without polling.
            let _ = builder.ins().call(local_callee, &args);
        }
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
