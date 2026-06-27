use super::super::*;

#[path = "aggregate_ops/callargs_ops.rs"]
mod callargs_ops;
#[path = "aggregate_ops/container_query_ops.rs"]
mod container_query_ops;
#[path = "aggregate_ops/dict_ops.rs"]
mod dict_ops;
#[path = "aggregate_ops/iterator_generator_ops.rs"]
mod iterator_generator_ops;
#[path = "aggregate_ops/list_tuple_ops.rs"]
mod list_tuple_ops;
#[path = "aggregate_ops/set_ops.rs"]
mod set_ops;

pub(super) struct AggregateRuntimeContext<'a> {
    pub(super) func_ir: &'a FunctionIR,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a BTreeMap<String, u32>,
    pub(super) scalar_plan: &'a ScalarRepresentationPlan,
    pub(super) is_multi_return_callee: Option<usize>,
    pub(super) multi_ret_locals: &'a [u32],
    pub(super) multi_ret_tuple_vars: &'a BTreeSet<String>,
    pub(super) multi_ret_call_locals: &'a BTreeMap<(String, i64), u32>,
    pub(super) multi_ret_call_vars: &'a BTreeSet<String>,
    pub(super) reloc_enabled: bool,
    pub(super) op_idx: usize,
}

#[allow(unused_variables)]
pub(super) fn emit_aggregate_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &BTreeMap<String, u32>,
    scalar_plan: &ScalarRepresentationPlan,
    is_multi_return_callee: Option<usize>,
    multi_ret_locals: &[u32],
    multi_ret_tuple_vars: &BTreeSet<String>,
    multi_ret_call_locals: &BTreeMap<(String, i64), u32>,
    multi_ret_call_vars: &BTreeSet<String>,
    reloc_enabled: bool,
    arena_local: Option<u32>,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    let ctx = AggregateRuntimeContext {
        func_ir,
        import_ids,
        locals,
        scalar_plan,
        is_multi_return_callee,
        multi_ret_locals,
        multi_ret_tuple_vars,
        multi_ret_call_locals,
        multi_ret_call_vars,
        reloc_enabled,
        op_idx,
    };

    if callargs_ops::emit_callargs_op(func, op, &ctx) {
        return true;
    }
    if list_tuple_ops::emit_list_tuple_op(func, op, &ctx) {
        return true;
    }
    if dict_ops::emit_dict_op(func, op, &ctx) {
        return true;
    }
    if set_ops::emit_set_op(func, op, &ctx) {
        return true;
    }
    if iterator_generator_ops::emit_iterator_generator_op(func, op, &ctx) {
        return true;
    }
    container_query_ops::emit_container_query_op(func, op, &ctx)
}
