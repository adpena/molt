use super::super::*;
use super::list_index_fast_path::store_index_fallback_runtime_fn;
#[cfg(test)]
use molt_ir::OpKind;

#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;OpKind::StoreIndex);

/// Cranelift codegen for subscript write (`store_index`). Container, key and
/// value are borrowed through one operand transaction: a repeated source is
/// boxed once, a failed box skips the store, and a minted value box is
/// released after the container retained it. The statement's borrowed
/// container return acquires nothing.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_subscript_store_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    let args = op
        .args
        .as_deref()
        .and_then(|args| args.get(..3))
        .unwrap_or_else(|| {
            panic!("store_index in {func_name} op {op_idx} needs a container, key and value")
        });
    // Runtime dispatch is the live representation authority: compact int/bool
    // lists store directly only while their physical type remains specialized;
    // ABI publication promotes them to the generic transactional list.
    // The generic entry is the miss of the shared list store
    // (`molt_codegen_abi::list_item`), which returns the container as
    // `molt_store_index` does.
    let symbol = store_index_fallback_runtime_fn(representation_plan, op);
    emit_operand_transaction_with(
        op,
        args,
        symbol,
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        representation_plan,
        nbc,
        block_tracked_obj,
        block_tracked_ptr,
        |call| {
            let &[list, index, value] = call.words else {
                unreachable!("store_index takes a container, key and value")
            };
            if symbol != RuntimeFn::StoreIndex {
                let site = call.builder.ins().call(call.callee, call.words);
                return call.builder.inst_results(site)[0];
            }
            let sites = super::calls::native_frame_sites(
                &mut *call.module,
                &mut *call.import_ids,
                &mut *call.builder,
                &mut *call.import_refs,
            );
            let callee = call.callee;
            crate::native_backend::simple_backend::emit_list_item_set(
                call.builder,
                call.sealed_blocks,
                sites,
                list,
                index,
                value,
                list,
                |builder| {
                    let site = builder.ins().call(callee, &[list, index, value]);
                    builder.inst_results(site)[0]
                },
            )
        },
    );
}
