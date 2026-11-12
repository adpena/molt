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
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
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
        OpKind::DictFromObj => RuntimeFn::DictFromObj,
        OpKind::DictGet => RuntimeFn::DictGet,
        OpKind::DictStrIntInc => RuntimeFn::DictStrIntInc,
        OpKind::StringSplitWsDictInc => RuntimeFn::StringSplitWsDictInc,
        OpKind::StringSplitSepDictInc => RuntimeFn::StringSplitSepDictInc,
        OpKind::DictPop => RuntimeFn::DictPop,
        OpKind::DictSetdefault => RuntimeFn::DictSetdefault,
        OpKind::DictSetdefaultEmptyList => RuntimeFn::DictSetdefaultEmptyList,
        OpKind::DictUnpack => RuntimeFn::DictUnpack,
        OpKind::DictUpdate => RuntimeFn::DictUpdate,
        OpKind::DictClear => RuntimeFn::DictClear,
        OpKind::DictCopy => RuntimeFn::DictCopy,
        OpKind::DictPopitem => RuntimeFn::DictPopitem,
        OpKind::DictUpdateKwstar => RuntimeFn::DictUpdateKwstar,
        OpKind::DictKeys => RuntimeFn::DictKeys,
        OpKind::DictValues => RuntimeFn::DictValues,
        OpKind::DictItems => RuntimeFn::DictItems,
        OpKind::DictUpdateMissing => RuntimeFn::DictUpdateMissing,
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
