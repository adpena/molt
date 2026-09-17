#[cfg(feature = "llvm")]
use super::fixed::fixed_runtime_import_return_abi;
use crate::runtime_import_abi::{
    MOLT_ASYNCGEN_NEW, MOLT_TASK_NEW, RuntimeImportSignature, RuntimeReturnAbi, runtime_sig,
};
/// Residual runtime symbols that lowering may declare on demand.
///
/// Fixed imports live in `fixed::FIXED_RUNTIME_IMPORTS`; this table is only the
/// dedicated all-i64 machine surface not covered by generated boxed contracts.
/// These facts never authorize generic boxing: raw pointers, indices, sizes and
/// results share the same carrier. Keep it disjoint from the fixed table and
/// the generated boxed authority to avoid mirrored signature ownership.
#[cfg(feature = "llvm")]
pub(crate) const CONSERVATIVE_RUNTIME_IMPORTS: &[RuntimeImportSignature] = &[
    runtime_sig("molt_builtin_type", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_layout_version", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_set_layout_version", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_closure_load", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_closure_store", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_new", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_new_from_values", 5, RuntimeReturnAbi::I64),
    runtime_sig("molt_ellipsis", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_match_builtin", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_builtin", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_builtin_empty", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_builtin_one", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_frozenset_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_func_new_builtin", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_future_poll", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_gen_locals_register", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_generator_close", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_generator_send", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_generator_throw", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_get_attr_name_default", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_guard_layout", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_guard_type", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_class_def", 8, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_field_get", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_field_init_ptr", 7, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_field_set", 7, RuntimeReturnAbi::I64),
    runtime_sig("molt_iter_next_unboxed", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_not_implemented", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_obj_get_state", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_obj_set_state", 2, RuntimeReturnAbi::Void),
    // Both tagged-object and raw-address accessor families use u64 carriers.
    runtime_sig("molt_object_field_get", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_field_get_ptr", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_field_init", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_field_init_ptr", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_field_set", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_field_set_ptr", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_new", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_super_new", 2, RuntimeReturnAbi::I64),
    MOLT_TASK_NEW,
    runtime_sig("molt_type_of", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_unpack_sequence", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_max_int", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_max_int_range", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_max_int_range_trusted", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_max_int_trusted", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_min_int", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_min_int_range", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_min_int_range_trusted", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_min_int_trusted", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_prod_int", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_prod_int_range", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_prod_int_range_trusted", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_prod_int_trusted", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_float", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_float_range", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_float_range_iter", 2, RuntimeReturnAbi::I64),
    runtime_sig(
        "molt_vec_sum_float_range_iter_trusted",
        2,
        RuntimeReturnAbi::I64,
    ),
    runtime_sig("molt_vec_sum_float_range_trusted", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_float_trusted", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_int", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_int_range", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_int_range_iter", 2, RuntimeReturnAbi::I64),
    runtime_sig(
        "molt_vec_sum_int_range_iter_trusted",
        2,
        RuntimeReturnAbi::I64,
    ),
    runtime_sig("molt_vec_sum_int_range_trusted", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_vec_sum_int_trusted", 2, RuntimeReturnAbi::I64),
    // Additional dedicated preserved-op imports. Availability and an integer
    // machine signature do not imply a positional boxed-value call contract.
    runtime_sig("molt_alloc_class", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_asyncgen_locals_register", 3, RuntimeReturnAbi::I64),
    MOLT_ASYNCGEN_NEW,
    runtime_sig("molt_function_closure_bits", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_iter", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_set_class", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_format", 2, RuntimeReturnAbi::I64),
];

#[cfg(feature = "llvm")]
pub(crate) fn runtime_import_return_abi(
    name: &str,
    param_count: usize,
) -> Option<RuntimeReturnAbi> {
    fixed_runtime_import_return_abi(name, param_count)
        .or_else(|| {
            CONSERVATIVE_RUNTIME_IMPORTS
                .iter()
                .find(|sig| sig.name == name && sig.param_count == param_count)
                .map(|sig| sig.return_abi)
        })
        .or_else(|| {
            // Generated object-value contracts also fully specify their machine
            // signatures. Do not mirror the callable registry in a native list.
            molt_ir::runtime_boxed_abi_generated::runtime_boxed_abi(name, param_count).map(|abi| {
                match abi.result {
                    molt_ir::runtime_boxed_abi_generated::RuntimeBoxedReturn::OwnedValue => {
                        RuntimeReturnAbi::I64
                    }
                    molt_ir::runtime_boxed_abi_generated::RuntimeBoxedReturn::Void => {
                        RuntimeReturnAbi::Void
                    }
                }
            })
        })
}

#[cfg(feature = "llvm")]
pub(crate) fn is_runtime_import_abi(
    name: &str,
    param_count: usize,
    return_abi: RuntimeReturnAbi,
) -> bool {
    runtime_import_return_abi(name, param_count) == Some(return_abi)
}
