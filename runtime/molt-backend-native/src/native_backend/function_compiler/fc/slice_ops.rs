use super::super::*;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["del_index", "slice", "slice_new"];

/// Cranelift codegen for delete-index, subscript-slice and slice construction.
/// Operands are borrowed through the shared operand transaction in `shared.rs`:
/// one box per distinct source, a stop at the first failed box, and minted
/// owners released on both paths. Slice construction is a fixed aggregate; the
/// subscript calls publish returns under the generated boxed ABI, so a
/// discarded slice is released and `del_index`'s borrowed container return
/// acquires nothing.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_slice_op(
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
    let (symbol, arity) = match op.kind.as_str() {
        "del_index" => ("molt_del_index", 2),
        "slice" => ("molt_slice", 3),
        "slice_new" => {
            emit_fixed_aggregate_constructor(
                op,
                FixedAggregateConstructor::Slice,
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
            return;
        }
        _ => unreachable!("unexpected indexing op kind: {}", op.kind),
    };
    let args = op
        .args
        .as_deref()
        .and_then(|args| args.get(..arity))
        .unwrap_or_else(|| {
            panic!(
                "{} in {func_name} op {op_idx} needs {arity} operands",
                op.kind
            )
        });
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
