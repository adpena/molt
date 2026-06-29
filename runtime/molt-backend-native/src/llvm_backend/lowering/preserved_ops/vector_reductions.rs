use super::*;

/// The closed set of vectorized-reduction op kinds emitted by the frontend's
/// `_match_vector_reduction_loop` (`VEC_SUM/PROD/MIN/MAX_*`, lower-cased by the
/// SimpleIR→TIR path). Each entry is `(op_kind, arity)` where `arity` is the
/// operand count and the runtime symbol is always `molt_<op_kind>`. This list
/// is the single LLVM-side authority for the family and MUST stay in lock-step
/// with the `molt_vec_*` runtime surface (`object/ops_vec.rs`) and the native
/// dispatch (`function_compiler.rs`). The arity split is structural: the
/// `_range` forms additionally pass the `start` bound (3 operands), while the
/// plain, `_trusted`, and `_range_iter` forms pass only `(seq, acc)`.
pub(super) const VEC_REDUCTION_OPS: &[(&str, usize)] = &[
    ("vec_sum_int", 2),
    ("vec_sum_int_trusted", 2),
    ("vec_sum_int_range", 3),
    ("vec_sum_int_range_trusted", 3),
    ("vec_sum_int_range_iter", 2),
    ("vec_sum_int_range_iter_trusted", 2),
    ("vec_sum_float", 2),
    ("vec_sum_float_trusted", 2),
    ("vec_sum_float_range", 3),
    ("vec_sum_float_range_trusted", 3),
    ("vec_sum_float_range_iter", 2),
    ("vec_sum_float_range_iter_trusted", 2),
    ("vec_prod_int", 2),
    ("vec_prod_int_trusted", 2),
    ("vec_prod_int_range", 3),
    ("vec_prod_int_range_trusted", 3),
    ("vec_min_int", 2),
    ("vec_min_int_trusted", 2),
    ("vec_min_int_range", 3),
    ("vec_min_int_range_trusted", 3),
    ("vec_max_int", 2),
    ("vec_max_int_trusted", 2),
    ("vec_max_int_range", 3),
    ("vec_max_int_range_trusted", 3),
];

/// Returns the `molt_*` runtime symbol for a vectorized-reduction op kind, or
/// `None` if `kind` is not a member of the family. The returned symbol is a
/// `&'static str` so it can be passed straight to `ensure_runtime_i64_fn`.
fn vec_reduction_runtime_symbol(kind: &str) -> Option<&'static str> {
    VEC_REDUCTION_RUNTIME_SYMBOLS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, sym)| *sym)
}

/// Operand count for a vectorized-reduction op kind. Panics in debug builds if
/// `kind` is not a member of the family — callers must gate on
/// [`vec_reduction_runtime_symbol`] first.
fn vec_reduction_arity(kind: &str) -> usize {
    VEC_REDUCTION_OPS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, arity)| *arity)
        .expect("vec_reduction_arity called on non-vec kind")
}

/// Static `(kind, "molt_<kind>")` table derived from [`VEC_REDUCTION_OPS`].
/// Computed once at first use so the runtime symbols are leak-free `'static`
/// strings (the lowering needs `&'static str` for `ensure_runtime_i64_fn`).
static VEC_REDUCTION_RUNTIME_SYMBOLS: std::sync::LazyLock<Vec<(&'static str, &'static str)>> =
    std::sync::LazyLock::new(|| {
        VEC_REDUCTION_OPS
            .iter()
            .map(|(kind, _)| {
                let symbol: &'static str = Box::leak(format!("molt_{kind}").into_boxed_str());
                (*kind, symbol)
            })
            .collect()
    });

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_vec_reduction_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let Some(symbol) = vec_reduction_runtime_symbol(kind) else {
            return false;
        };
        debug_assert_eq!(
            op.operands.len(),
            vec_reduction_arity(kind),
            "vec reduction {kind} must carry exactly {} operands",
            vec_reduction_arity(kind),
        );
        if op.operands.len() != vec_reduction_arity(kind) {
            return false;
        }
        let arg_bits: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
            .operands
            .iter()
            .map(|&id| self.materialize_dynbox_operand(id).into())
            .collect();
        let call_fn = self.ensure_runtime_i64_fn(symbol, op.operands.len());
        let result = self
            .backend
            .builder
            .build_call(call_fn, &arg_bits, symbol)
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if let Some(&result_id) = op.results.first() {
            self.values.insert(result_id, result);
            self.value_types.insert(result_id, TirType::DynBox);
        }
        true
    }
}
