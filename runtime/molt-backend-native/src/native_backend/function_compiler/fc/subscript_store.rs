use super::super::*;
use super::list_index_fast_path::store_index_fallback_import_name;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &["store_index"];

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
    let symbol = store_index_fallback_import_name(representation_plan, op);
    emit_operand_transaction_call(
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
    );
}
