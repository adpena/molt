use super::super::*;
use molt_ir::OpKind;

// Single-source kind authority for [`handle_set_op`], consulted by
// `op_family::native_op_family`. Mirror the `match op.kind` arms below.
#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;
    OpKind::SetNew,
    OpKind::FrozensetNew,
    OpKind::SetAdd,
    OpKind::SetAddProbe,
    OpKind::FrozensetAdd,
    OpKind::SetDiscard,
    OpKind::SetRemove,
    OpKind::SetPop,
    OpKind::SetUpdate,
    OpKind::SetIntersectionUpdate,
    OpKind::SetDifferenceUpdate,
    OpKind::SetSymdiffUpdate,
);
use super::OpFlow;

/// Hash-container operations share operand ownership, first-error cleanup,
/// and generated runtime return ownership with other boxed consumers.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_set_op(
    op: &OpIR,
    module: &mut dyn Module,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) -> OpFlow {
    let symbol = match op.kind {
        OpKind::SetNew | OpKind::FrozensetNew => {
            emit_hash_container_constructor(
                op,
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
            return OpFlow::Proceed;
        }
        OpKind::SetAdd => "molt_set_add",
        OpKind::SetAddProbe => "molt_set_add_probe",
        OpKind::FrozensetAdd => "molt_frozenset_add",
        OpKind::SetDiscard => "molt_set_discard",
        OpKind::SetRemove => "molt_set_remove",
        OpKind::SetPop => "molt_set_pop",
        OpKind::SetUpdate => "molt_set_update",
        OpKind::SetIntersectionUpdate => "molt_set_intersection_update",
        OpKind::SetDifferenceUpdate => "molt_set_difference_update",
        OpKind::SetSymdiffUpdate => "molt_set_symdiff_update",
        _ => unreachable!("handler invoked with non-matching op.kind"),
    };
    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
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
    OpFlow::Proceed
}
