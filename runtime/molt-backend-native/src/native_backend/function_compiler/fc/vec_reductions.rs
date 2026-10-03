use super::super::*;

/// The fused-loop reduction kinds — the SINGLE authority for this family,
/// routed to `NativeOpFamily::Arith` (whose [`super::arith::handle_arith_op`]
/// delegates here). Each calls its `molt_<kind>` runtime kernel with
/// `(it, acc, target)` and defines the kernel's owned
/// `(result, last, count, more)` tuple for one bounded chunk of the loop; the
/// kernel alone decides how many items it consumes (`object/ops_vec.rs`).
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["vec_sum", "vec_prod", "vec_min", "vec_max"];

/// Every fused-loop reduction kernel takes `(it, acc, target)`.
#[cfg(feature = "native-backend")]
const VEC_REDUCTION_ARITY: usize = 3;

/// Call each fused-loop kernel through the shared operand transaction so
/// borrowed input boxes and the owned result follow canonical cleanup.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_vec_reduction(
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
    let symbol = match op.kind.as_str() {
        "vec_sum" => "molt_vec_sum",
        "vec_prod" => "molt_vec_prod",
        "vec_min" => "molt_vec_min",
        "vec_max" => "molt_vec_max",
        _ => unreachable!("handler invoked with non-matching op.kind"),
    };
    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    assert_eq!(
        args.len(),
        VEC_REDUCTION_ARITY,
        "{} takes (it, acc, target)",
        op.kind
    );
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
