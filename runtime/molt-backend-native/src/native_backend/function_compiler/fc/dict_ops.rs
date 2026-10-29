use super::super::*;
use molt_ir::OpKind;

// Single-source kind authority for [`handle_dict_op`], consulted by
// `op_family::native_op_family`. Mirror the `match op.kind` arms below.
#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;
    OpKind::DictNew,
    OpKind::DictFromObj,
    OpKind::DictGet,
    OpKind::DictUpdateMissing,
    OpKind::DictStrIntInc,
    OpKind::StringSplitWsDictInc,
    OpKind::StringSplitSepDictInc,
    OpKind::DictPop,
    OpKind::DictSetdefault,
    OpKind::DictSetdefaultEmptyList,
    OpKind::DictUnpack,
    OpKind::DictUpdate,
    OpKind::DictClear,
    OpKind::DictCopy,
    OpKind::DictPopitem,
    OpKind::DictUpdateKwstar,
    OpKind::DictKeys,
    OpKind::DictValues,
    OpKind::DictItems,
);
use super::OpFlow;

/// Hash-container operations share operand ownership, first-error cleanup,
/// and generated runtime return ownership with other boxed consumers.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_dict_op(
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
        OpKind::DictNew => {
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
        OpKind::DictFromObj => "molt_dict_from_obj",
        OpKind::DictGet => "molt_dict_get",
        OpKind::DictStrIntInc => "molt_dict_str_int_inc",
        OpKind::StringSplitWsDictInc => "molt_string_split_ws_dict_inc",
        OpKind::StringSplitSepDictInc => "molt_string_split_sep_dict_inc",
        OpKind::DictPop => "molt_dict_pop",
        OpKind::DictSetdefault => "molt_dict_setdefault",
        OpKind::DictSetdefaultEmptyList => "molt_dict_setdefault_empty_list",
        OpKind::DictUnpack => "molt_dict_unpack",
        OpKind::DictUpdate => "molt_dict_update",
        OpKind::DictClear => "molt_dict_clear",
        OpKind::DictCopy => "molt_dict_copy",
        OpKind::DictPopitem => "molt_dict_popitem",
        OpKind::DictUpdateKwstar => "molt_dict_update_kwstar",
        OpKind::DictKeys => "molt_dict_keys",
        OpKind::DictValues => "molt_dict_values",
        OpKind::DictItems => "molt_dict_items",
        OpKind::DictUpdateMissing => "molt_dict_update_missing",
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
