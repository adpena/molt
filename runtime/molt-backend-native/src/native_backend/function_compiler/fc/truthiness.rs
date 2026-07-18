use super::super::*;
use super::list_index_fast_path::ListIndexFastPathState;

/// Emit the shared boxed-value truthiness fast path without losing unrelated
/// FunctionBuilder variables across its internal CFG.
///
/// Both structured `if` and TIR `br_if` lower through this authority. Every
/// exit into the merge transports the same liveness-derived payload, then
/// rebinds it before the caller emits its semantic branch. This keeps the
/// optimization runtime-free for bool/int values while making CFG expansion
/// transparent to values that merely live across the condition.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_boxed_truthiness(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    first_defined_at: &BTreeMap<String, usize>,
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    current_out: Option<&str>,
    list_index_fast_paths: &ListIndexFastPathState,
    cond_name: &str,
    cond: Value,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    nbc: &crate::NanBoxConsts,
) -> Value {
    let origin_block = builder.current_block();
    let live_through = collect_live_through_values(
        builder,
        vars,
        first_defined_at,
        last_use,
        op_idx,
        current_out,
    );
    let merge = builder.create_block();
    builder.append_block_param(merge, types::I8);
    append_live_through_params(builder, merge, &live_through);

    emit_conditional_list_bool_truthiness(
        builder,
        sealed_blocks,
        &list_index_fast_paths.list_is_bool_cache,
        list_index_fast_paths
            .conditional_list_bool_shadows
            .get(cond_name),
        merge,
        &live_through,
    );

    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let masked = builder.ins().band(cond, mask);
    let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);
    let bool_block = builder.create_block();
    let not_bool_block = builder.create_block();
    builder
        .ins()
        .brif(is_bool, bool_block, &[], not_bool_block, &[]);

    switch_to_block_materialized(builder, bool_block);
    seal_block_once(builder, sealed_blocks, bool_block);
    let bit0 = builder.ins().band_imm(cond, 1);
    let bool_truthy = builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0);
    let merge_args = merge_args_with_live_through(bool_truthy, &live_through);
    jump_block(builder, merge, &merge_args);

    switch_to_block_materialized(builder, not_bool_block);
    seal_block_once(builder, sealed_blocks, not_bool_block);
    let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let is_int = builder.ins().icmp(IntCC::Equal, masked, int_tag);
    let int_block = builder.create_block();
    let call_block = builder.create_block();
    builder.set_cold_block(call_block);
    builder.ins().brif(is_int, int_block, &[], call_block, &[]);

    switch_to_block_materialized(builder, int_block);
    seal_block_once(builder, sealed_blocks, int_block);
    let raw = unbox_int(builder, cond, nbc);
    let int_truthy = builder.ins().icmp_imm(IntCC::NotEqual, raw, 0);
    let merge_args = merge_args_with_live_through(int_truthy, &live_through);
    jump_block(builder, merge, &merge_args);

    switch_to_block_materialized(builder, call_block);
    seal_block_once(builder, sealed_blocks, call_block);
    let truthy_fn = SimpleBackend::import_func_id_split(
        module,
        import_ids,
        "molt_is_truthy",
        &[types::I64],
        &[types::I64],
    );
    let truthy_ref = module.declare_func_in_func(truthy_fn, builder.func);
    let call = builder.ins().call(truthy_ref, &[cond]);
    let truthy = builder.inst_results(call)[0];
    let call_truthy = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
    let merge_args = merge_args_with_live_through(call_truthy, &live_through);
    jump_block(builder, merge, &merge_args);

    switch_to_block_materialized(builder, merge);
    seal_block_once(builder, sealed_blocks, merge);
    let params = builder.block_params(merge).to_vec();
    rebind_live_through_values(builder, vars, &live_through, &params[1..]);

    if let Some(origin) = origin_block
        && origin != merge
    {
        let obj_live = block_tracked_obj.remove(&origin).unwrap_or_default();
        if !obj_live.is_empty() {
            extend_unique_tracked(block_tracked_obj.entry(merge).or_default(), obj_live);
        }
        let ptr_live = block_tracked_ptr.remove(&origin).unwrap_or_default();
        if !ptr_live.is_empty() {
            extend_unique_tracked(block_tracked_ptr.entry(merge).or_default(), ptr_live);
        }
    }
    params[0]
}
