#[cfg(feature = "native-backend")]
use super::*;

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
