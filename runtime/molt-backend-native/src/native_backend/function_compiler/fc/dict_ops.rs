use super::super::*;

/// Single-source kind authority for [`handle_dict_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "dict_new",
    "dict_from_obj",
    "dict_get",
    "dict_set",
    "dict_update_missing",
    "dict_str_int_inc",
    "string_split_ws_dict_inc",
    "string_split_sep_dict_inc",
    "dict_pop",
    "dict_setdefault",
    "dict_setdefault_empty_list",
    "dict_update",
    "dict_clear",
    "dict_copy",
    "dict_popitem",
    "dict_update_kwstar",
    "dict_keys",
    "dict_values",
    "dict_items",
];
use super::OpFlow;

/// Hash-container operations share operand ownership, first-error cleanup,
/// and generated runtime return ownership with other boxed consumers.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_dict_op(
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
        "dict_new" => {
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
        "dict_from_obj" => "molt_dict_from_obj",
        "dict_get" => "molt_dict_get",
        "dict_str_int_inc" => "molt_dict_str_int_inc",
        "string_split_ws_dict_inc" => "molt_string_split_ws_dict_inc",
        "string_split_sep_dict_inc" => "molt_string_split_sep_dict_inc",
        "dict_pop" => "molt_dict_pop",
        "dict_setdefault" => "molt_dict_setdefault",
        "dict_setdefault_empty_list" => "molt_dict_setdefault_empty_list",
        "dict_update" => "molt_dict_update",
        "dict_clear" => "molt_dict_clear",
        "dict_copy" => "molt_dict_copy",
        "dict_popitem" => "molt_dict_popitem",
        "dict_update_kwstar" => "molt_dict_update_kwstar",
        "dict_keys" => "molt_dict_keys",
        "dict_values" => "molt_dict_values",
        "dict_items" => "molt_dict_items",
        "dict_set" => "molt_store_index",
        "dict_update_missing" => "molt_dict_update_missing",
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
