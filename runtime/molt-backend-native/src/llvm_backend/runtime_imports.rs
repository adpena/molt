//! Declares runtime functions that compiled LLVM code calls into.
//!
//! These correspond to `extern "C"` functions in `molt-runtime/src/object/ops.rs`
//! and related modules. Fixed declarations below may use target pointers for
//! dedicated buffer helpers; the conservative classified-import table is an
//! all-`i64` ABI surface whose pointer addresses are passed as integer bits and
//! cast inside the runtime export.
//!
//! ## LLVM function attributes
//!
//! Every declared function is annotated with LLVM attributes that enable
//! interprocedural optimization:
//!
//! - **`nounwind`**: All molt runtime functions use explicit error return
//!   values (NaN-boxed sentinels) and `catch_unwind` at FFI boundaries.
//!   Panics never escape as C++ exceptions, so LLVM can omit landing pads
//!   and exception handling tables entirely.
//!
//! - **`willreturn`**: Applied to functions that always terminate (no
//!   infinite loops, no coroutine suspension). Enables more aggressive
//!   dead code elimination and code motion.
//!
//! - **`memory(read)`** (= `readonly`): Applied to functions that read
//!   memory but never write to it. Enables CSE and LICM of repeated
//!   calls with the same arguments.
//!
//! - **`memory(none)`** (= `readnone`): Applied to functions that
//!   neither read nor write memory — pure functions of their arguments.
//!   Enables full redundancy elimination.

#[cfg(feature = "llvm")]
use inkwell::attributes::{Attribute, AttributeLoc};
#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::module::Module;
#[cfg(feature = "llvm")]
use inkwell::types::FunctionType;
#[cfg(feature = "llvm")]
use inkwell::values::FunctionValue;

// ── LLVM memory effect encoding (for the `memory` enum attribute) ──
//
// The `memory` attribute in LLVM 16+ replaces the legacy `readnone`,
// `readonly`, and `writeonly` function attributes.  Its value is a
// 6-bit bitmask encoding read/write permissions for three memory
// location classes:
//
//   bits [1:0] = Default (everything not ArgMem or InaccessibleMem)
//   bits [3:2] = ArgMem  (memory pointed to by pointer arguments)
//   bits [5:4] = InaccessibleMem (e.g. errno, thread-locals)
//
// Each 2-bit field: 0 = None, 1 = Read, 2 = Write, 3 = ReadWrite.

/// `memory(none)` — the function does not access any memory.
/// Currently unused: all molt runtime functions dereference NaN-boxed
/// heap pointers in at least their fallback paths.  Retained for future
/// use when we add inline NaN-box tag extraction intrinsics.
#[cfg(feature = "llvm")]
#[allow(dead_code)]
const MEMORY_NONE: u64 = 0;

/// `memory(read)` — the function may read any memory but never writes.
/// All three location classes set to Read (01): 0b01_01_01 = 21.
#[cfg(feature = "llvm")]
const MEMORY_READ: u64 = 0b01_01_01;

/// Apply `nounwind` to a function declaration.
///
/// Safe for all molt runtime functions: panics are caught by `catch_unwind`
/// in `with_gil_entry!` and converted to pending exceptions with zeroed
/// return values.  No C++ exceptions are ever thrown.
#[cfg(feature = "llvm")]
fn add_nounwind(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("nounwind");
    func.add_attribute(AttributeLoc::Function, ctx.create_enum_attribute(kind, 0));
}

/// Apply `willreturn` to a function declaration.
///
/// Valid for functions that always terminate: no infinite loops, no
/// coroutine suspension, no `longjmp`-style control transfer.
#[cfg(feature = "llvm")]
fn add_willreturn(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("willreturn");
    func.add_attribute(AttributeLoc::Function, ctx.create_enum_attribute(kind, 0));
}

/// Apply `memory(none)` to a function — it neither reads nor writes memory.
/// See `MEMORY_NONE` for why this is currently unused.
#[cfg(feature = "llvm")]
#[allow(dead_code)]
fn add_memory_none(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("memory");
    func.add_attribute(
        AttributeLoc::Function,
        ctx.create_enum_attribute(kind, MEMORY_NONE),
    );
}

/// Apply `memory(read)` to a function — it may read memory but never writes.
#[cfg(feature = "llvm")]
fn add_memory_read(ctx: &Context, func: FunctionValue<'_>) {
    let kind = Attribute::get_named_enum_kind_id("memory");
    func.add_attribute(
        AttributeLoc::Function,
        ctx.create_enum_attribute(kind, MEMORY_READ),
    );
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeReturnAbi {
    I64,
    Void,
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeImportSignature {
    pub name: &'static str,
    /// `runtime_sig` facts are exact all-`i64` parameter ABI facts.
    pub param_count: usize,
    pub return_abi: RuntimeReturnAbi,
}

#[cfg(feature = "llvm")]
const fn runtime_sig(
    name: &'static str,
    param_count: usize,
    return_abi: RuntimeReturnAbi,
) -> RuntimeImportSignature {
    RuntimeImportSignature {
        name,
        param_count,
        return_abi,
    }
}

/// Runtime symbols that lowering may declare on demand, plus fixed-table symbols
/// whose return ABI is needed by generic preserved-op lowering.
///
/// This is not the preferred end state for high-traffic runtime imports: promote
/// those to `declare_runtime_functions` when their exact attributes are known.
/// The table exists to make the remaining conservative surface explicit and to
/// prevent typo/new-symbol drift from silently creating extern declarations.
#[cfg(feature = "llvm")]
pub const CLASSIFIED_RUNTIME_IMPORTS: &[RuntimeImportSignature] = &[
    runtime_sig("molt_abs_builtin", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_aiter", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_ascii_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_builtin_type", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_callargs_expand_kwstar", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_callargs_expand_star", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_callargs_push_kw", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_cbor_parse_scalar_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_chan_recv", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_chan_send", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_apply_set_name", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_layout_version", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_merge_layout", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_set_layout_version", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_closure_load", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_closure_store", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_complex_from_obj", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_depth", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_exit", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_unwind_to", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_new", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_new_from_values", 5, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_clear", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_copy", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_get", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_items", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_keys", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_popitem", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_set", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_setdefault", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_setdefault_empty_list", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_update", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_update_kwstar", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_update_missing", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_values", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_ellipsis", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_enumerate", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_active", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_class", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_context_set", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_match_builtin", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_builtin", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_builtin_empty", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_builtin_one", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_set_cause", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_set_last", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_float_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_format_builtin", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_frozenset_add", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_frozenset_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_function_defaults_version", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_future_poll", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_gen_locals_register", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_generator_close", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_generator_send", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_generator_throw", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_get_attr_name_default", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_get_attr_special", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_guard_layout_ptr", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_guard_type", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_class_def", 8, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_field_get_ptr", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_field_init_ptr", 7, RuntimeReturnAbi::I64),
    runtime_sig("molt_guarded_field_set_ptr", 7, RuntimeReturnAbi::I64),
    runtime_sig("molt_has_attr_name", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_int_from_obj", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_int_from_str_of_obj", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_is_callable", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_isinstance", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_issubclass", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_iter_checked", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_iter_next_unboxed", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_json_parse_scalar_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_len", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_len_dict", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_len_list", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_len_set", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_len_str", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_len_tuple", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_append", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_extend", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_fill_new", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_from_range", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_missing", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_msgpack_parse_scalar_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_not_implemented", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_obj_get_state", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_obj_set_state", 2, RuntimeReturnAbi::Void),
    runtime_sig("molt_object_field_init_ptr", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_field_set_ptr", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_new", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_new_bound", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_new_bound_sized", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_ord", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_ord_at", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_print_newline", 0, RuntimeReturnAbi::Void),
    runtime_sig("molt_print_obj", 1, RuntimeReturnAbi::Void),
    runtime_sig("molt_range_new", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_repr_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_add", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_add_probe", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_sleep_register", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_slice", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_str_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_join", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_super_new", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_task_new", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_tuple_from_list", 1, RuntimeReturnAbi::I64),
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
    // Frontend-preserved Copy spellings that reach the generic LLVM
    // `molt_<kind>` fallback. `MOLT_RUNTIME_INTRINSIC_SYMBOLS` proves only
    // active-profile availability; this table owns the boxed-call ABI.
    runtime_sig("molt_alloc_class", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_alloc_class_static", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_alloc_class_trusted", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_anext", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_asyncgen_locals_register", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_asyncgen_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_asyncgen_shutdown", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_block_on", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_bound_method_new", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bridge_unavailable", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_buffer2d_get", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_buffer2d_matmul", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_buffer2d_new", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_buffer2d_set", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_count", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_count_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_endswith", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_endswith_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_fill_range", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_find", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_find_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_from_str", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_replace", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_split", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_split_max", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_startswith", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytearray_startswith_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_count", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_count_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_endswith", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_endswith_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_find", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_find_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_from_obj", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_from_str", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_replace", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_split", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_split_max", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_startswith", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_bytes_startswith_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_callargs_new", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_callargs_push_pos", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_current", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_cancel", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_clone", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_drop", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_get_current", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_is_cancelled", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancel_token_set_current", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_cancelled", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_chan_drop", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_chr", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_class_set_base", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_classmethod_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_code_new", 9, RuntimeReturnAbi::I64),
    runtime_sig("molt_code_slot_set", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_code_slots_init", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_contains", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_closing", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_enter", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_null", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_context_unwind", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_get", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_set", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dataclass_set_class", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_inc", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_pop", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_dict_str_int_inc", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_env_get", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_clear", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_kind", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_last", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_last_pending", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_message", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_new_from_class", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_pop", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_push", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_stack_clear", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_stack_depth", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_stack_enter", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_stack_exit", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exception_stack_set_depth", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exceptiongroup_combine", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_exceptiongroup_match", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_file_close", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_file_flush", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_file_open", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_file_read", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_file_write", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_fn_ptr_code_set", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_frame_locals_set", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_func_new", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_func_new_closure", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_function_closure_bits", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_future_cancel", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_future_cancel_clear", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_future_cancel_msg", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_id", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_inplace_bit_and", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_inplace_bit_or", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_inplace_bit_xor", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_intarray_from_seq", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_invert", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_is_bound_method", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_is_generator", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_is_native_awaitable", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_iter", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_clear", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_copy", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_count", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_index", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_index_range", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_insert", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_int_new", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_pop", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_remove", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_list_reverse", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_matmul", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_inplace_matmul", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_memoryview_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_memoryview_tobytes", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_module_import_star", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_module_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_object_set_class", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_pow_mod", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_promise_new", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_promise_set_exception", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_promise_set_result", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_property_new", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_round", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_difference_update", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_discard", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_intersection_update", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_pop", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_remove", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_symdiff_update", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_set_update", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_slice_new", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_spawn", 1, RuntimeReturnAbi::Void),
    runtime_sig("molt_staticmethod_new", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_statistics_mean_slice", 5, RuntimeReturnAbi::I64),
    runtime_sig("molt_statistics_stdev_slice", 5, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_capitalize", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_count", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_count_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_endswith", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_endswith_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_find", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_find_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_format", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_lower", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_lstrip", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_replace", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_rstrip", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_field", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_field_eq", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_field_len", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_field_to_int", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_max", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_sep_dict_inc", 4, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_validate", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_split_ws_dict_inc", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_startswith", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_startswith_slice", 6, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_strip", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_string_upper", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_taq_ingest_line", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_task_register_token_owned", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_thread_submit", 3, RuntimeReturnAbi::I64),
    runtime_sig("molt_trace_enter_slot", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_trace_exit", 0, RuntimeReturnAbi::I64),
    runtime_sig("molt_trunc", 1, RuntimeReturnAbi::I64),
    runtime_sig("molt_tuple_count", 2, RuntimeReturnAbi::I64),
    runtime_sig("molt_tuple_index", 2, RuntimeReturnAbi::I64),
];

#[cfg(feature = "llvm")]
pub fn classified_runtime_import_return_abi(
    name: &str,
    param_count: usize,
) -> Option<RuntimeReturnAbi> {
    CLASSIFIED_RUNTIME_IMPORTS
        .iter()
        .find(|sig| sig.name == name && sig.param_count == param_count)
        .map(|sig| sig.return_abi)
}

#[cfg(feature = "llvm")]
pub fn is_classified_runtime_import(
    name: &str,
    param_count: usize,
    return_abi: RuntimeReturnAbi,
) -> bool {
    classified_runtime_import_return_abi(name, param_count) == Some(return_abi)
}

/// Declare a runtime symbol that is not in the fixed import table yet.
///
/// This is the only on-demand declaration path lowering may use. It deliberately
/// applies the weakest globally valid runtime attribute set: `nounwind` only.
/// Stronger facts such as `willreturn` must be encoded by adding the symbol to
/// `declare_runtime_functions` so the central table owns that proof.
#[cfg(feature = "llvm")]
pub fn declare_conservative_runtime_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    name: &str,
    fn_ty: FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(func) = module.get_function(name) {
        return func;
    }
    let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
    add_nounwind(ctx, func);
    func
}

/// Declare all runtime functions that lowered code may call.
///
/// We declare them as external linkage — the linker will resolve them
/// against the Molt runtime shared library or static archive.
///
/// Each function is annotated with LLVM attributes to enable
/// interprocedural optimization.  See module-level documentation.
#[cfg(feature = "llvm")]
pub fn declare_runtime_functions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) {
    let i64_ty = ctx.i64_type();
    let i32_ty = ctx.i32_type();
    let void_ty = ctx.void_type();

    // ── Arithmetic (DynBox dispatch: (u64, u64) -> u64) ──
    // These may allocate (e.g. bigint overflow) so no memory attribute.
    for name in &[
        "molt_add",
        "molt_str_concat",
        "molt_sub",
        "molt_mul",
        "molt_div",
        "molt_floordiv",
        "molt_mod",
        "molt_pow",
        // In-place augmented-assignment runtime entries. The boxed slow paths of
        // emit_binary_arith / emit_bitwise dispatch `x //= y` etc. through these
        // (they try the `__i<op>__` dunder before the binary protocol). They
        // must be declared here or call_runtime_2's get_function lookup panics
        // ("Runtime function not declared"). matmul rides the preserved-Copy
        // lane and is declared by the classified runtime-import table below.
        //
        // add/sub/mul: the first-class OpCode::InplaceAdd/Sub/Mul share
        // emit_binary_arith with their binary opcode and previously dispatched
        // the boxed fallback to molt_add/sub/mul (skipping __iadd__/etc. — a
        // latent LLVM-only parity bug); the boxed slow path now correctly calls
        // molt_inplace_add/sub/mul, which therefore must be declared here.
        "molt_inplace_add",
        "molt_inplace_sub",
        "molt_inplace_mul",
        "molt_inplace_div",
        "molt_inplace_floordiv",
        "molt_inplace_mod",
        "molt_inplace_pow",
        "molt_inplace_lshift",
        "molt_inplace_rshift",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Unary (u64) -> u64 ──
    // Unary ops may invoke user protocol methods (__neg__, __bool__, __invert__).
    for name in &["molt_neg", "molt_not", "molt_invert"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Integer boxing (i64 -> u64 NaN-boxed) ──
    //
    // Channel constructor: returns an opaque runtime handle encoded in Molt
    // object bits. It is not a generic boxed preserved-Copy fallback because the
    // Rust ABI advertises the semantic `ChanHandle` type; LLVM owns it through a
    // dedicated `chan_new` arm.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_chan_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // `molt_int_from_i64` boxes a raw i64 into a tagged integer handle. Used by
    // the overflow-safe integer box path (`box_i64_overflow_safe`) so the LLVM
    // backend boxes integers through the same runtime entry point the native
    // backend uses, instead of an unconditional 47-bit truncating mask.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_int_from_i64",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // Truthiness may invoke user __bool__/__len__; it must not promise
    // termination.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_is_truthy",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // Type-check helper: reads object layout only and always returns.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_is_function_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // Fast-path truthy checks: these inspect NaN-box bits and only
    // fall back to the GIL-wrapped path for unexpected types.  They
    // are read-only from LLVM's perspective (no allocation, no visible
    // mutation).
    for name in &["molt_is_truthy_int", "molt_is_truthy_bool"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
    }

    // GIL-free truthy checks: no GIL acquisition, no catch_unwind, no
    // signal checks.  Pure reads of the NaN-boxed value bits with a
    // fallback to the fast-path check above.  Read-only: they inspect
    // memory but never write.
    //
    // Note: molt_is_truthy_* return i64 (0 or 1), not u64.
    for name in &["molt_is_truthy_int_nogil", "molt_is_truthy_bool_nogil"] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
    }

    // ── Comparison (u64, u64) -> u64 ──
    // Comparisons read heap objects but may invoke user-defined __eq__,
    // __lt__, etc. that allocate.  No memory attribute is safe here.
    for name in &[
        "molt_eq",
        "molt_ne",
        "molt_lt",
        "molt_le",
        "molt_gt",
        "molt_ge",
        "molt_contains",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Bitwise (u64, u64) -> u64 ──
    // Bitwise ops may invoke user numeric protocol methods.
    for name in &[
        "molt_bit_and",
        "molt_bit_or",
        "molt_bit_xor",
        "molt_lshift",
        "molt_rshift",
        // In-place bitwise: the Copy-carried `inplace_bit_*` arms in
        // lower_preserved_simpleir_op route through emit_bitwise, whose boxed
        // fallback now dispatches `|=`/`&=`/`^=` to molt_inplace_bit_* (trying
        // `__ior__`/etc. first — previously they wrongly used molt_bit_*).
        "molt_inplace_bit_and",
        "molt_inplace_bit_or",
        "molt_inplace_bit_xor",
    ] {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Refcount ──
    // molt_inc_ref_obj(bits: u64)  (void return)
    // molt_dec_ref_obj(bits: u64)  (void return)
    // These mutate refcount fields in heap objects.  No memory attribute.
    // dec_ref may trigger deallocation chains but always returns.
    {
        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        let inc = module.add_function(
            "molt_inc_ref_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, inc);
        add_willreturn(ctx, inc);
        let dec = module.add_function(
            "molt_dec_ref_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, dec);
        add_willreturn(ctx, dec);
    }

    // ── Allocation ──
    // molt_alloc(size_bits: u64) -> u64
    // Allocates heap memory — no memory restriction attribute.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_alloc",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Attribute access ──
    // molt_get_attr_name(obj_bits: u64, name_bits: u64) -> u64
    // molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64
    // molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64
    // These may invoke descriptors (__get__, __set__, __delete__) which
    // can have arbitrary side effects.  No memory attribute.
    {
        let get_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_get_attr_name",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let get_obj_ic_ty = i64_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        );
        let func = module.add_function(
            "molt_get_attr_object_ic",
            get_obj_ic_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_attr_name",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_del_attr_name",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Indexing ──
    // molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64
    // molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64
    // molt_delitem_method(obj_bits: u64, key_bits: u64) -> u64
    // May invoke __getitem__/__setitem__/__delitem__ with side effects.
    {
        let get_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_getitem_method",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        // molt_getitem_unchecked(obj_bits: u64, key_bits: u64) -> u64
        // Same signature as molt_getitem_method. The current runtime delegates to
        // the generic index path, so it cannot claim readonly/willreturn yet.
        let func = module.add_function(
            "molt_getitem_unchecked",
            get_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_setitem_method",
            set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let del_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_delitem_method",
            del_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Slice ──
    // molt_slice_new(start: u64, stop: u64, step: u64) -> u64
    // Allocates a new slice object.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_slice_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Iteration ──
    // molt_iter_next(iter_bits: u64) -> u64
    // Advances an iterator — mutates the iterator's internal state.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_iter_next",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Descriptor wrappers ──
    // Allocate new descriptor objects.
    {
        let unary_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_classmethod_new",
            unary_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_staticmethod_new",
            unary_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let property_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_property_new",
            property_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Exception handling ──
    // molt_raise(exc_bits: u64) -> u64
    // Sets the pending exception flag — mutates thread-local state.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_raise",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // exception handler stack helpers — all mutate thread-local exception state.
    {
        let noarg_ret_i64 = i64_ty.fn_type(&[], false);
        let unary_ret_i64 = i64_ty.fn_type(&[i64_ty.into()], false);
        for name in &[
            "molt_exception_clear",
            "molt_exception_last",
            "molt_exception_last_pending",
            "molt_exception_current",
            "molt_exception_push",
            "molt_exception_pop",
            "molt_exception_stack_enter",
            "molt_exception_stack_depth",
            "molt_exception_stack_clear",
        ] {
            let func = module.add_function(
                name,
                noarg_ret_i64,
                Some(inkwell::module::Linkage::External),
            );
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }
        for name in &[
            "molt_exception_enter_handler",
            "molt_exception_stack_exit",
            "molt_exception_stack_set_depth",
            "molt_exception_resolve_captured",
        ] {
            let func = module.add_function(
                name,
                unary_ret_i64,
                Some(inkwell::module::Linkage::External),
            );
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }
    }

    // ── Diagnostics ──
    // molt_warn_stderr(msg_bits: u64) -> void
    // Writes to stderr — side-effecting but always returns.
    {
        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_warn_stderr",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    {
        let fn_ty = void_ty.fn_type(&[], false);
        let func = module.add_function(
            "molt_print_newline",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);

        let fn_ty = void_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_print_obj",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Call infrastructure ──
    // Allocate and populate call-argument builders.  All allocate heap memory.
    // molt_callargs_new(pos_capacity: u64, kw_capacity: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_callargs_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_callargs_push_pos",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // molt_call_bind(call_bits: u64, builder_bits: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_bind",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        // No willreturn: dynamic callable dispatch may execute arbitrary user code.
    }
    // Fast dynamic-call entries execute the target callable directly.
    {
        for (name, arity) in &[
            ("molt_call_bind_ic", 3usize),
            ("molt_call_indirect_ic", 3),
            ("molt_call_func_fast0", 1),
            ("molt_call_func_fast1", 2),
            ("molt_call_func_fast2", 3),
            ("molt_call_func_fast3", 4),
        ] {
            let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                (0..*arity).map(|_| i64_ty.into()).collect();
            let fn_ty = i64_ty.fn_type(&params, false);
            let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
        }
    }

    // ── Function and code-object construction ──
    {
        for (name, arity) in &[
            ("molt_func_new", 3usize),
            ("molt_func_new_builtin_named", 4),
            ("molt_func_new_closure", 4),
            ("molt_code_new", 9),
        ] {
            let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                (0..*arity).map(|_| i64_ty.into()).collect();
            let fn_ty = i64_ty.fn_type(&params, false);
            let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let code_slot_set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_code_slot_set",
            code_slot_set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);

        let unary_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        for name in &[
            "molt_code_slots_init",
            "molt_trace_enter_slot",
            "molt_frame_locals_set",
            "molt_trace_set_line",
        ] {
            let func =
                module.add_function(name, unary_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let noarg_ty = i64_ty.fn_type(&[], false);
        let func = module.add_function(
            "molt_trace_exit",
            noarg_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);

        let fn_ptr_code_set_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_fn_ptr_code_set",
            fn_ptr_code_set_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_function_defaults_version",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Fused method-dispatch ICs ──
    // molt_call_method_icN(site, recv, name_ptr, name_len, a0..a{N-1}) -> u64
    //   N positional args, N ∈ 0..=4. All u64 — the name pointer is a real
    //   native pointer cast to i64 (every value NaN-boxed). These dispatch the
    //   resolved method, which executes arbitrary user code: NO `willreturn`
    //   (a method may loop forever or suspend), but `nounwind` holds (the
    //   `with_gil_entry_nopanic!` catch_unwind boundary converts every panic to
    //   a pending exception). 4 + N parameters.
    for (name, n) in &[
        ("molt_call_method_ic0", 0usize),
        ("molt_call_method_ic1", 1),
        ("molt_call_method_ic2", 2),
        ("molt_call_method_ic3", 3),
        ("molt_call_method_ic4", 4),
    ] {
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..(4 + *n)).map(|_| i64_ty.into()).collect();
        let fn_ty = i64_ty.fn_type(&params, false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }
    // molt_call_super_method_icN(site, class, self, name_ptr, name_len, a0..) -> u64
    //   5 + N parameters; same effect profile (nounwind, no willreturn).
    for (name, n) in &[
        ("molt_call_super_method_ic0", 0usize),
        ("molt_call_super_method_ic1", 1),
        ("molt_call_super_method_ic2", 2),
        ("molt_call_super_method_ic3", 3),
        ("molt_call_super_method_ic4", 4),
    ] {
        let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
            (0..(5 + *n)).map(|_| i64_ty.into()).collect();
        let fn_ty = i64_ty.fn_type(&params, false);
        let func = module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External));
        add_nounwind(ctx, func);
    }

    // ── Containers ──
    // molt_dict_new(capacity_bits: u64) -> u64
    // molt_set_new(capacity_bits: u64) -> u64
    // Allocate new container objects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_dict_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_set_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Import / module namespace helpers ──
    // molt_module_import(name_bits: u64) -> u64
    // May execute module-level code with arbitrary side effects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_module_import",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        // No willreturn: import may execute arbitrary module init code.
    }
    {
        let unary_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        for name in &[
            "molt_module_new",
            "molt_module_cache_get",
            "molt_module_cache_del",
        ] {
            let func =
                module.add_function(name, unary_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let binary_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        for name in &[
            "molt_module_cache_set",
            "molt_module_get_attr",
            "molt_module_import_from",
            "molt_module_get_global",
            "molt_module_get_name",
            "molt_module_del_global",
            "molt_module_del_global_if_present",
        ] {
            let func =
                module.add_function(name, binary_ty, Some(inkwell::module::Linkage::External));
            add_nounwind(ctx, func);
            add_willreturn(ctx, func);
        }

        let ternary_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_module_set_attr",
            ternary_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Method / Builtin call ──
    // molt_call_method(receiver: u64, name: u64, args_builder: u64) -> u64
    // Invokes arbitrary user code — no memory or willreturn attribute.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_method",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_call_builtin(name: u64, args_builder: u64) -> u64
    // Invokes a builtin function — may have arbitrary side effects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_builtin",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── String construction ──
    // molt_string_from_bytes(ptr: *const u8, len: u64, out: *mut u64) -> i32
    // Allocates a new string object from a byte buffer.
    {
        let ptr_ty = ctx.ptr_type(inkwell::AddressSpace::default());
        let fn_ty = i32_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
        let func = module.add_function(
            "molt_string_from_bytes",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    {
        let ptr_ty = ctx.ptr_type(inkwell::AddressSpace::default());
        let fn_ty = i64_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_bigint_from_str",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // molt_call_0(callable: u64) -> u64
    // Invoke a callable with zero arguments. Used by SCF desugaring.
    // Executes arbitrary user code — no memory or willreturn attribute.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_call_0",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Container builders ──
    // All builder functions allocate / mutate heap builder state.
    // list uses builder_new/append/finish
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_list_builder_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let fn_ty2 = void_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_list_builder_append",
            fn_ty2,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_list_builder_finish",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // tuple reuses the list-shaped builder payload, but finalizes to tuple
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_tuple_builder_finish",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // dict builder
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_dict_builder_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let fn_ty2 = void_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_dict_builder_append",
            fn_ty2,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_dict_builder_finish",
            i64_ty.fn_type(&[i64_ty.into()], false),
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }
    // set builder
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_builder_new",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let fn_ty2 = void_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_set_builder_append",
            fn_ty2,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        let func = module.add_function(
            "molt_set_builder_finish",
            i64_ty.fn_type(&[i64_ty.into()], false),
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
    }

    // ── Iteration (GetIter / ForIter) ──
    // molt_get_iter(obj: u64) -> u64
    // molt_for_iter(iter: u64) -> u64  (returns sentinel on exhaustion)
    // Both may invoke __iter__/__next__ with side effects.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_get_iter",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let func = module.add_function(
            "molt_for_iter",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Generator support ──
    // molt_yield(value: u64) -> u64
    // molt_yield_from(subiter: u64) -> u64
    // These suspend coroutine execution — NOT willreturn.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_yield",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        let func = module.add_function(
            "molt_yield_from",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }

    // ── Exception pending check ──
    // molt_exception_pending() -> u64  (returns nonzero if exception pending)
    // Reads thread-local exception flag — read-only, always returns.
    {
        let fn_ty = i64_ty.fn_type(&[], false);
        let func = module.add_function(
            "molt_exception_pending",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
        add_willreturn(ctx, func);
        add_memory_read(ctx, func);
    }

    // ── SCF dialect runtime helpers ──
    // These are called when SCF ops survive lowering (not yet fully desugared).
    // All execute user-provided closures with arbitrary side effects.
    // molt_scf_if(cond: u64, then_fn: u64, else_fn: u64) -> u64
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_scf_if",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_scf_for(lb: u64, ub: u64, step: u64, body_fn: u64) -> u64
    // Executes a loop — NOT willreturn (body may execute indefinitely
    // if step is zero, or the trip count may be very large).
    {
        let fn_ty = i64_ty.fn_type(
            &[i64_ty.into(), i64_ty.into(), i64_ty.into(), i64_ty.into()],
            false,
        );
        let func = module.add_function(
            "molt_scf_for",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_scf_while(cond_fn: u64, body_fn: u64) -> u64
    // Executes a while loop — NOT willreturn (may be infinite).
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
        let func = module.add_function(
            "molt_scf_while",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
    // molt_scf_yield(value: u64) -> u64
    // Suspends coroutine execution — NOT willreturn.
    {
        let fn_ty = i64_ty.fn_type(&[i64_ty.into()], false);
        let func = module.add_function(
            "molt_scf_yield",
            fn_ty,
            Some(inkwell::module::Linkage::External),
        );
        add_nounwind(ctx, func);
    }
}

#[cfg(all(test, feature = "llvm"))]
mod tests {
    use super::*;
    use inkwell::attributes::Attribute;
    use inkwell::context::Context;

    /// Check that a function has an enum attribute with the given name.
    fn has_fn_attr(func: FunctionValue<'_>, attr_name: &str) -> bool {
        let kind_id = Attribute::get_named_enum_kind_id(attr_name);
        if kind_id == 0 {
            // Unknown attribute name in this LLVM version — skip check.
            return true;
        }
        func.get_enum_attribute(AttributeLoc::Function, kind_id)
            .is_some()
    }

    #[test]
    fn runtime_functions_are_declared() {
        let ctx = Context::create();
        let module = ctx.create_module("test_rt");
        declare_runtime_functions(&ctx, &module);

        // Spot-check a few key functions exist
        assert!(module.get_function("molt_add").is_some());
        assert!(module.get_function("molt_sub").is_some());
        assert!(module.get_function("molt_eq").is_some());
        assert!(module.get_function("molt_inc_ref_obj").is_some());
        assert!(module.get_function("molt_dec_ref_obj").is_some());
        assert!(module.get_function("molt_alloc").is_some());
        assert!(module.get_function("molt_get_attr_name").is_some());
        assert!(module.get_function("molt_raise").is_some());
        assert!(module.get_function("molt_is_truthy").is_some());
        assert!(module.get_function("molt_code_new").is_some());
        assert!(module.get_function("molt_print_newline").is_some());
        assert!(module.get_function("molt_print_obj").is_some());
        assert!(module.get_function("molt_bigint_from_str").is_some());
        assert!(
            module
                .get_function("molt_function_defaults_version")
                .is_some()
        );
        // The augmented-assignment entries the boxed `emit_binary_arith` path
        // calls through `call_runtime_2` (which requires pre-declaration). A
        // TIR→LLVM-lowered function carrying `+=`/`-=`/`*=` (an inlined or
        // generator-fused caller) panics without these.
        assert!(module.get_function("molt_inplace_add").is_some());
        assert!(module.get_function("molt_inplace_sub").is_some());
        assert!(module.get_function("molt_inplace_mul").is_some());
    }

    #[test]
    fn fused_method_dispatch_ic_runtime_functions_are_declared() {
        let ctx = Context::create();
        let module = ctx.create_module("test_method_dispatch_ic");
        declare_runtime_functions(&ctx, &module);

        // call_method_icN: site + recv + name_ptr + name_len + N args = 4 + N.
        // call_super_method_icN: site + class + self + name_ptr + name_len + N
        // args = 5 + N. All u64 — the name pointer is a native pointer cast to
        // i64. These dispatch arbitrary user code, so they carry `nounwind`
        // (catch_unwind boundary) but must NOT carry `willreturn`.
        let willreturn_kind = Attribute::get_named_enum_kind_id("willreturn");
        for (name, arity) in &[
            ("molt_call_method_ic0", 4usize),
            ("molt_call_method_ic1", 5),
            ("molt_call_method_ic2", 6),
            ("molt_call_method_ic3", 7),
            ("molt_call_method_ic4", 8),
            ("molt_call_super_method_ic0", 5),
            ("molt_call_super_method_ic1", 6),
            ("molt_call_super_method_ic2", 7),
            ("molt_call_super_method_ic3", 8),
            ("molt_call_super_method_ic4", 9),
        ] {
            let func = module
                .get_function(name)
                .unwrap_or_else(|| panic!("{name} should be declared"));
            assert_eq!(
                func.count_params() as usize,
                *arity,
                "{name} should have {arity} i64 parameters"
            );
            assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
            // Method dispatch runs arbitrary user code (may loop/suspend): the
            // declaration must not promise termination.
            if willreturn_kind != 0 {
                assert!(
                    func.get_enum_attribute(AttributeLoc::Function, willreturn_kind)
                        .is_none(),
                    "{name} must NOT have willreturn (dispatches arbitrary user code)"
                );
            }
        }
    }

    #[test]
    fn function_and_code_runtime_functions_are_declared() {
        let ctx = Context::create();
        let module = ctx.create_module("test_function_code_runtime");
        declare_runtime_functions(&ctx, &module);

        let memory_kind = Attribute::get_named_enum_kind_id("memory");
        for (name, arity) in &[
            ("molt_func_new", 3usize),
            ("molt_func_new_builtin_named", 4),
            ("molt_func_new_closure", 4),
            ("molt_code_new", 9),
            ("molt_code_slot_set", 2),
            ("molt_code_slots_init", 1),
            ("molt_trace_enter_slot", 1),
            ("molt_trace_exit", 0),
            ("molt_frame_locals_set", 1),
            ("molt_trace_set_line", 1),
            ("molt_fn_ptr_code_set", 2),
            ("molt_function_defaults_version", 1),
        ] {
            let func = module
                .get_function(name)
                .unwrap_or_else(|| panic!("{name} should be declared"));
            assert_eq!(
                func.count_params() as usize,
                *arity,
                "{name} should have {arity} i64 parameters"
            );
            let ret_ty = func
                .get_type()
                .get_return_type()
                .unwrap_or_else(|| panic!("{name} should return i64"));
            assert!(ret_ty.is_int_type(), "{name} should return an integer");
            assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
            assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
            assert!(
                has_fn_attr(func, "willreturn"),
                "{name} should have willreturn"
            );
            if memory_kind != 0 {
                assert!(
                    func.get_enum_attribute(AttributeLoc::Function, memory_kind)
                        .is_none(),
                    "{name} should not claim a read-only/no-memory effect"
                );
            }
        }
    }

    #[test]
    fn diagnostics_and_constant_runtime_functions_are_declared() {
        let ctx = Context::create();
        let module = ctx.create_module("test_diagnostics_constants_runtime");
        declare_runtime_functions(&ctx, &module);

        let print_newline = module
            .get_function("molt_print_newline")
            .expect("molt_print_newline should be declared");
        assert_eq!(print_newline.count_params(), 0);
        assert!(
            print_newline.get_type().get_return_type().is_none(),
            "molt_print_newline should return void"
        );
        assert!(has_fn_attr(print_newline, "nounwind"));
        assert!(has_fn_attr(print_newline, "willreturn"));

        let print_obj = module
            .get_function("molt_print_obj")
            .expect("molt_print_obj should be declared");
        assert_eq!(print_obj.count_params(), 1);
        assert!(
            print_obj.get_type().get_return_type().is_none(),
            "molt_print_obj should return void"
        );
        assert!(has_fn_attr(print_obj, "nounwind"));
        assert!(has_fn_attr(print_obj, "willreturn"));

        let bigint_from_str = module
            .get_function("molt_bigint_from_str")
            .expect("molt_bigint_from_str should be declared");
        assert_eq!(bigint_from_str.count_params(), 2);
        let ret_ty = bigint_from_str
            .get_type()
            .get_return_type()
            .expect("molt_bigint_from_str should return i64");
        assert!(ret_ty.is_int_type());
        assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
        assert!(has_fn_attr(bigint_from_str, "nounwind"));
        assert!(has_fn_attr(bigint_from_str, "willreturn"));
    }

    #[test]
    fn dynamic_call_runtime_functions_are_declared_without_willreturn() {
        let ctx = Context::create();
        let module = ctx.create_module("test_dynamic_call_runtime");
        declare_runtime_functions(&ctx, &module);

        let willreturn_kind = Attribute::get_named_enum_kind_id("willreturn");
        for (name, arity) in &[
            ("molt_call_bind", 2usize),
            ("molt_call_bind_ic", 3),
            ("molt_call_indirect_ic", 3),
            ("molt_call_func_fast0", 1),
            ("molt_call_func_fast1", 2),
            ("molt_call_func_fast2", 3),
            ("molt_call_func_fast3", 4),
        ] {
            let func = module
                .get_function(name)
                .unwrap_or_else(|| panic!("{name} should be declared"));
            assert_eq!(
                func.count_params() as usize,
                *arity,
                "{name} should have {arity} i64 parameters"
            );
            assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
            if willreturn_kind != 0 {
                assert!(
                    func.get_enum_attribute(AttributeLoc::Function, willreturn_kind)
                        .is_none(),
                    "{name} must NOT have willreturn (dispatches arbitrary user code)"
                );
            }
        }
    }

    fn parse_literal_ensure_runtime_calls(source: &str) -> Vec<(String, usize, RuntimeReturnAbi)> {
        let production = source
            .split("#[cfg(all(test, feature = \"llvm\"))]")
            .next()
            .unwrap_or(source);
        let mut calls = Vec::new();
        for (needle, return_abi) in [
            ("ensure_runtime_i64_fn(\"", RuntimeReturnAbi::I64),
            ("ensure_runtime_void_fn(\"", RuntimeReturnAbi::Void),
        ] {
            let mut rest = production;
            while let Some(start) = rest.find(needle) {
                rest = &rest[start + needle.len()..];
                let Some(end_name) = rest.find('"') else {
                    break;
                };
                let name = &rest[..end_name];
                let after_name = &rest[end_name + 1..];
                let Some(comma) = after_name.find(',') else {
                    rest = after_name;
                    continue;
                };
                let mut digits = String::new();
                for ch in after_name[comma + 1..].chars() {
                    if ch.is_ascii_digit() {
                        digits.push(ch);
                    } else if !digits.is_empty() {
                        break;
                    } else if !ch.is_whitespace() {
                        break;
                    }
                }
                if let Ok(param_count) = digits.parse::<usize>() {
                    calls.push((name.to_string(), param_count, return_abi));
                }
                rest = after_name;
            }
        }
        calls
    }

    #[test]
    fn lowering_literal_runtime_imports_are_declared_or_classified() {
        let ctx = Context::create();
        let module = ctx.create_module("test_lowering_literal_runtime_imports");
        declare_runtime_functions(&ctx, &module);
        let source = include_str!("lowering.rs");

        let mut missing = Vec::new();
        for (name, param_count, return_abi) in parse_literal_ensure_runtime_calls(source) {
            if let Some(func) = module.get_function(&name) {
                assert_eq!(
                    func.count_params() as usize,
                    param_count,
                    "{name} central declaration arity must match lowering call"
                );
                match return_abi {
                    RuntimeReturnAbi::I64 => {
                        let ret_ty = func
                            .get_type()
                            .get_return_type()
                            .unwrap_or_else(|| panic!("{name} should return i64"));
                        assert!(ret_ty.is_int_type(), "{name} should return an integer");
                        assert_eq!(ret_ty.into_int_type().get_bit_width(), 64);
                    }
                    RuntimeReturnAbi::Void => {
                        assert!(
                            func.get_type().get_return_type().is_none(),
                            "{name} should return void"
                        );
                    }
                }
                continue;
            }
            if !is_classified_runtime_import(&name, param_count, return_abi) {
                missing.push(format!("{name}/{param_count}/{return_abi:?}"));
            }
        }

        assert!(
            missing.is_empty(),
            "lowering literal runtime imports must be centrally declared or classified: {}",
            missing.join(", ")
        );
    }

    #[test]
    fn module_namespace_runtime_functions_are_declared() {
        let ctx = Context::create();
        let module = ctx.create_module("test_module_runtime");
        declare_runtime_functions(&ctx, &module);

        for (name, arity) in &[
            ("molt_module_new", 1usize),
            ("molt_module_cache_get", 1),
            ("molt_module_cache_del", 1),
            ("molt_module_cache_set", 2),
            ("molt_module_get_attr", 2),
            ("molt_module_import_from", 2),
            ("molt_module_get_global", 2),
            ("molt_module_get_name", 2),
            ("molt_module_del_global", 2),
            ("molt_module_del_global_if_present", 2),
            ("molt_module_set_attr", 3),
        ] {
            let func = module
                .get_function(name)
                .unwrap_or_else(|| panic!("{name} should be declared"));
            assert_eq!(
                func.count_params() as usize,
                *arity,
                "{name} should have {arity} i64 parameters"
            );
            assert!(has_fn_attr(func, "nounwind"), "{name} should have nounwind");
            assert!(
                has_fn_attr(func, "willreturn"),
                "{name} should have willreturn"
            );
        }
    }

    #[test]
    fn all_functions_have_nounwind() {
        let ctx = Context::create();
        let module = ctx.create_module("test_nounwind");
        declare_runtime_functions(&ctx, &module);

        let mut func = module.get_first_function();
        while let Some(f) = func {
            assert!(
                has_fn_attr(f, "nounwind"),
                "Function {} is missing nounwind attribute",
                f.get_name().to_str().unwrap()
            );
            func = f.get_next_function();
        }
    }

    #[test]
    fn willreturn_on_simple_functions() {
        let ctx = Context::create();
        let module = ctx.create_module("test_willreturn");
        declare_runtime_functions(&ctx, &module);

        // These functions always terminate — must have willreturn.
        for name in &[
            "molt_alloc",
            "molt_inc_ref_obj",
            "molt_dec_ref_obj",
            "molt_slice_new",
            "molt_exception_pending",
        ] {
            let f = module.get_function(name).expect(name);
            assert!(
                has_fn_attr(f, "willreturn"),
                "Function {} is missing willreturn attribute",
                name
            );
        }
    }

    #[test]
    fn no_willreturn_on_control_flow_functions() {
        let ctx = Context::create();
        let module = ctx.create_module("test_no_willreturn");
        declare_runtime_functions(&ctx, &module);

        let willreturn_kind = Attribute::get_named_enum_kind_id("willreturn");
        if willreturn_kind == 0 {
            return; // LLVM version doesn't recognize this attribute.
        }

        // These functions may not return (coroutine suspension, loops,
        // deopt transfer, arbitrary user code execution).
        for name in &[
            "molt_add",
            "molt_str_concat",
            "molt_sub",
            "molt_mul",
            "molt_div",
            "molt_floordiv",
            "molt_mod",
            "molt_pow",
            "molt_inplace_add",
            "molt_inplace_sub",
            "molt_inplace_mul",
            "molt_inplace_div",
            "molt_inplace_floordiv",
            "molt_inplace_mod",
            "molt_inplace_pow",
            "molt_inplace_lshift",
            "molt_inplace_rshift",
            "molt_neg",
            "molt_not",
            "molt_invert",
            "molt_is_truthy",
            "molt_eq",
            "molt_ne",
            "molt_lt",
            "molt_le",
            "molt_gt",
            "molt_ge",
            "molt_contains",
            "molt_bit_and",
            "molt_bit_or",
            "molt_bit_xor",
            "molt_lshift",
            "molt_rshift",
            "molt_inplace_bit_and",
            "molt_inplace_bit_or",
            "molt_inplace_bit_xor",
            "molt_get_attr_name",
            "molt_get_attr_object_ic",
            "molt_set_attr_name",
            "molt_del_attr_name",
            "molt_getitem_method",
            "molt_getitem_unchecked",
            "molt_setitem_method",
            "molt_delitem_method",
            "molt_yield",
            "molt_yield_from",
            "molt_scf_for",
            "molt_scf_while",
            "molt_scf_yield",
            "molt_scf_if",
            "molt_get_iter",
            "molt_for_iter",
            "molt_call_method",
            "molt_call_builtin",
            "molt_call_bind",
            "molt_call_bind_ic",
            "molt_call_indirect_ic",
            "molt_call_func_fast0",
            "molt_call_func_fast1",
            "molt_call_func_fast2",
            "molt_call_func_fast3",
            "molt_call_0",
            "molt_module_import",
        ] {
            let f = module.get_function(name).expect(name);
            assert!(
                f.get_enum_attribute(AttributeLoc::Function, willreturn_kind)
                    .is_none(),
                "Function {} should NOT have willreturn (may not terminate)",
                name
            );
        }
    }

    #[test]
    fn memory_read_on_pure_readers() {
        let ctx = Context::create();
        let module = ctx.create_module("test_memory_read");
        declare_runtime_functions(&ctx, &module);

        let memory_kind = Attribute::get_named_enum_kind_id("memory");
        if memory_kind == 0 {
            return; // LLVM version doesn't support memory attribute.
        }

        // These functions only read memory.
        for name in &[
            "molt_is_truthy_int",
            "molt_is_truthy_bool",
            "molt_is_truthy_int_nogil",
            "molt_is_truthy_bool_nogil",
            "molt_exception_pending",
        ] {
            let f = module.get_function(name).expect(name);
            let attr = f
                .get_enum_attribute(AttributeLoc::Function, memory_kind)
                .unwrap_or_else(|| panic!("{} missing memory attribute", name));
            assert_eq!(
                attr.get_enum_value(),
                MEMORY_READ,
                "Function {} should have memory(read) = {}",
                name,
                MEMORY_READ
            );
        }
    }
}
