#[cfg(feature = "native-backend")]
use super::*;

/// Publish the exact generated import return contract before operand cleanup.
/// A borrowed result can alias a temporary argument; a bound result must own an
/// independent credit before that argument's owner is released.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn bind_runtime_import_result(
    op: &OpIR,
    result: Value,
    symbol: &str,
    arity: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    bind_runtime_import_result_name(
        crate::tir::simple_def_use::simple_ir_out_result(op),
        result,
        symbol,
        arity,
        module,
        import_ids,
        builder,
        vars,
    );
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn bind_runtime_import_result_name(
    out: Option<&str>,
    result: Value,
    symbol: &str,
    arity: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_abi};
    let contract = runtime_boxed_abi(symbol, arity)
        .unwrap_or_else(|| panic!("runtime result has no canonical boxed ABI: {symbol}/{arity}"))
        .result;
    match contract {
        RuntimeBoxedReturn::OwnedValue | RuntimeBoxedReturn::PollValue => {
            bind_owned_runtime_result_name(out, result, module, import_ids, builder, vars);
        }
        RuntimeBoxedReturn::BorrowedValue => {
            if let Some(out) = out {
                let retain = SimpleBackend::import_func_id_split(
                    module,
                    import_ids,
                    "molt_inc_ref_obj",
                    &[types::I64],
                    &[],
                );
                let retain = module.declare_func_in_func(retain, builder.func);
                builder.ins().call(retain, &[result]);
                def_var_named(builder, vars, out, result);
            }
        }
        RuntimeBoxedReturn::Void => assert!(
            out.is_none(),
            "void runtime import {symbol} cannot bind a result"
        ),
    }
}

/// One failure-atomic construction protocol for dict, set, and frozenset.
/// The aggregate keeps its original owner; mutator returns follow generated
/// ABI facts. Failed allocation/boxing/insertion skips all later entries,
/// releases every initialized temporary and aggregate, and leaves the pending
/// exception for the ordinary IR exception edge.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_hash_container_constructor(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    let (new_symbol, insert_symbol, width) = match op.kind.as_str() {
        "dict_new" => ("molt_dict_new", "molt_dict_set", 2),
        "set_new" => ("molt_set_new", "molt_set_add", 1),
        "frozenset_new" => ("molt_frozenset_new", "molt_frozenset_add", 1),
        kind => panic!("not a hash container constructor: {kind}"),
    };
    let args = op.args.as_deref().unwrap_or(&[]);
    assert!(
        args.len().is_multiple_of(width),
        "incomplete {} entry",
        op.kind
    );
    let origin = builder.current_block();
    let none = builder.ins().iconst(types::I64, box_none());
    // Only materialized raw integers own temporary boxes. Private SSA slots
    // are initialized before every possible failure edge and reused per entry.
    let temporary_owners: Vec<_> = (0..width)
        .map(|_| {
            let owner = builder.declare_var(types::I64);
            builder.def_var(owner, none);
            owner
        })
        .collect();
    let release = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_dec_ref_obj",
        &[types::I64],
        &[],
    );
    let pending = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_exception_pending_fast",
        &[],
        &[types::I64],
    );
    let create = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        new_symbol,
        &[types::I64],
        &[types::I64],
    );
    let insert_params = vec![types::I64; width + 1];
    let insert = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        insert_symbol,
        &insert_params,
        &[types::I64],
    );
    let capacity = builder
        .ins()
        .iconst(types::I64, (args.len() / width) as i64);
    let created = builder.ins().call(create, &[capacity]);
    let aggregate = builder.inst_results(created)[0];
    let abort = builder.create_block();
    builder.set_cold_block(abort);
    let initialize = builder.create_block();
    let merge = builder.create_block();
    builder.append_block_param(merge, types::I64);
    let allocated = builder
        .ins()
        .icmp_imm(IntCC::NotEqual, aggregate, box_none());
    builder.ins().brif(allocated, initialize, &[], abort, &[]);
    switch_to_block_materialized(builder, initialize);
    seal_block_once(builder, sealed_blocks, initialize);
    for entry in args.chunks(width) {
        let mut operands = vec![aggregate];
        for (index, name) in entry.iter().enumerate() {
            let value = fc::var_get_boxed_overflow_safe_fn(
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
            .unwrap_or_else(|| panic!("{} operand not found: {name}", op.kind));
            operands.push(*value);
            if representation_plan.is_raw_int_carrier_name(name) {
                builder.def_var(temporary_owners[index], *value);
                let failed = emit_exception_pending_condition(builder, pending, None);
                let next = builder.create_block();
                builder.ins().brif(failed, abort, &[], next, &[]);
                switch_to_block_materialized(builder, next);
                seal_block_once(builder, sealed_blocks, next);
            }
        }
        let inserted = builder.ins().call(insert, &operands);
        let result = builder.inst_results(inserted)[0];
        bind_runtime_import_result_name(
            None,
            result,
            insert_symbol,
            width + 1,
            module,
            import_ids,
            builder,
            vars,
        );
        for &owner in &temporary_owners {
            let value = builder.use_var(owner);
            builder.ins().call(release, &[value]);
            builder.def_var(owner, none);
        }
        let failed = emit_exception_pending_condition(builder, pending, None);
        let next = builder.create_block();
        builder.ins().brif(failed, abort, &[], next, &[]);
        switch_to_block_materialized(builder, next);
        seal_block_once(builder, sealed_blocks, next);
    }
    jump_block(builder, merge, &[aggregate]);
    switch_to_block_materialized(builder, abort);
    seal_block_once(builder, sealed_blocks, abort);
    for &owner in &temporary_owners {
        let value = builder.use_var(owner);
        builder.ins().call(release, &[value]);
    }
    builder.ins().call(release, &[aggregate]);
    jump_block(builder, merge, &[none]);
    switch_to_block_materialized(builder, merge);
    seal_block_once(builder, sealed_blocks, merge);
    let result = builder.block_params(merge)[0];
    bind_owned_runtime_result(op, result, module, import_ids, builder, vars);
    carry_internal_cfg_tracking(origin, merge, block_tracked_obj, block_tracked_ptr);
}

/// Consume a transferred runtime owner even when there is no named SSA result.
/// Borrowed results must not enter this sink without first being retained.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn bind_owned_runtime_result(
    op: &OpIR,
    result: Value,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    bind_owned_runtime_result_name(
        crate::tir::simple_def_use::simple_ir_out_result(op),
        result,
        module,
        import_ids,
        builder,
        vars,
    );
}

/// Field-role consumers pass the selected result name, not a synthetic op.
/// This is the same owned-result sink for ordinary and positional results.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn bind_owned_runtime_result_name(
    out: Option<&str>,
    result: Value,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    if let Some(out) = out.filter(|name| *name != "none") {
        def_var_named(builder, vars, out, result);
    } else {
        let release = SimpleBackend::import_func_id_split(
            module,
            import_ids,
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        let release = module.declare_func_in_func(release, builder.func);
        builder.ins().call(release, &[result]);
    }
}

/// Carry per-block ownership cleanup roots across compiler-internal CFG.
///
/// These splits are transparent to TIR, so values owned by the origin block
/// remain owned after every internal edge rejoins. Leaving them keyed by the
/// now-closed origin makes later return/exception cleanup unable to find them.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn carry_internal_cfg_tracking(
    origin: Option<Block>,
    merge: Block,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    let Some(origin) = origin.filter(|origin| *origin != merge) else {
        return;
    };
    for tracked in [block_tracked_obj, block_tracked_ptr] {
        let live = tracked.remove(&origin).unwrap_or_default();
        if !live.is_empty() {
            extend_unique_tracked(tracked.entry(merge).or_default(), live);
        }
    }
}

/// Keep task initialization off the allocation-failure edge. The original boxed
/// result reaches the ordinary exception edge, which owns frame/RC unwinding.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn begin_task_initialization(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    task: Value,
) -> Block {
    let initialize = builder.create_block();
    let done = builder.create_block();
    let allocated = builder.ins().icmp_imm(IntCC::NotEqual, task, box_none());
    builder.ins().brif(allocated, initialize, &[], done, &[]);
    switch_to_block_materialized(builder, initialize);
    seal_block_once(builder, sealed_blocks, initialize);
    done
}

/// Enter the code-slot-backed frame owned by this compiled invocation.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_owned_execution_frame_enter(
    entered: Variable,
    code_id: i64,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
) {
    let code_id_val = builder.ins().iconst(types::I64, code_id);
    let callee = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_trace_enter_slot",
        &[types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let _ = builder.ins().call(local_callee, &[code_id_val]);
    let active = builder.ins().iconst(types::I8, 1);
    builder.def_var(entered, active);
}

/// Release only a frame entered by this invocation, including early-return paths.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_owned_execution_frame_exit(
    entered: Option<Variable>,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
) {
    let Some(entered) = entered else {
        return;
    };
    let active = builder.use_var(entered);
    let pop_block = builder.create_block();
    let done_block = builder.create_block();
    builder.ins().brif(active, pop_block, &[], done_block, &[]);
    switch_to_block_materialized(builder, pop_block);
    seal_block_once(builder, sealed_blocks, pop_block);
    // Retire ownership before releasing frame-owned values can call Python.
    let inactive = builder.ins().iconst(types::I8, 0);
    builder.def_var(entered, inactive);
    let exit = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_trace_exit",
        &[],
        &[types::I64],
    );
    builder.ins().call(exit, &[]);
    jump_block(builder, done_block, &[]);
    switch_to_block_materialized(builder, done_block);
    seal_block_once(builder, sealed_blocks, done_block);
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) static EMPTY_VEC_STRING: Vec<String> = Vec::new();

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn is_cold_module_chunk_function(
    name: &str,
) -> bool {
    name.contains("__molt_module_chunk_")
}
