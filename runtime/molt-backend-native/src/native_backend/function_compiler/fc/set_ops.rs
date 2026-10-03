use super::super::*;

/// Single-source kind authority for [`handle_set_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "set_new",
    "frozenset_new",
    "set_add",
    "set_add_probe",
    "frozenset_add",
    "set_discard",
    "set_remove",
    "set_pop",
    "set_update",
    "set_intersection_update",
    "set_difference_update",
    "set_symdiff_update",
];
use super::OpFlow;

/// Hash-container operations share operand ownership, first-error cleanup,
/// and generated runtime return ownership with other boxed consumers.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_set_op(
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
) -> OpFlow {
    let symbol = match op.kind.as_str() {
        "set_new" | "frozenset_new" => {
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
        "set_add" => "molt_set_add",
        "set_add_probe" => "molt_set_add_probe",
        "frozenset_add" => "molt_frozenset_add",
        "set_discard" => "molt_set_discard",
        "set_remove" => "molt_set_remove",
        "set_pop" => "molt_set_pop",
        "set_update" => "molt_set_update",
        "set_intersection_update" => "molt_set_intersection_update",
        "set_difference_update" => "molt_set_difference_update",
        "set_symdiff_update" => "molt_set_symdiff_update",
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
