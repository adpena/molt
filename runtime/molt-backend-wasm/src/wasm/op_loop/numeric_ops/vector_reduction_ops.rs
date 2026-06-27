use super::super::*;

#[allow(unused_variables)]
pub(super) fn emit_vector_reduction_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    match op.kind.as_str() {
        "vec_sum_int"
        | "vec_sum_int_trusted"
        | "vec_sum_int_range_iter"
        | "vec_sum_int_range_iter_trusted"
        | "vec_sum_int_range"
        | "vec_sum_int_range_trusted"
        | "vec_sum_float"
        | "vec_sum_float_trusted"
        | "vec_sum_float_range_iter"
        | "vec_sum_float_range_iter_trusted"
        | "vec_sum_float_range"
        | "vec_sum_float_range_trusted"
        | "vec_prod_int"
        | "vec_prod_int_trusted"
        | "vec_prod_int_range"
        | "vec_prod_int_range_trusted"
        | "vec_min_int"
        | "vec_min_int_trusted"
        | "vec_min_int_range"
        | "vec_min_int_range_trusted"
        | "vec_max_int"
        | "vec_max_int_trusted"
        | "vec_max_int_range"
        | "vec_max_int_range_trusted" => {
            let args_names = op.args.as_ref().unwrap();
            let arg_locals: Vec<u32> = args_names.iter().map(|n| locals[n]).collect();
            let out = locals[op.out.as_ref().unwrap()];
            emit_simple_call(
                func,
                reloc_enabled,
                import_ids[op.kind.as_str()],
                &arg_locals,
                out,
            );
        }
        _ => return false,
    }
    true
}
