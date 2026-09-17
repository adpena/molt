#[cfg(feature = "native-backend")]
use super::*;

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
    if let Some(out) = op.out.as_ref() {
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
