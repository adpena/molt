//! Static import registry and op→import dependency table for WASM backend.
//!
//! Adding a new runtime import: add to IMPORT_REGISTRY + OP_IMPORT_DEPS.
//! The codegen declares its own dependencies through OP_IMPORT_DEPS.

use crate::SimpleIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_plan::wasm_specialized_container_import;
use std::collections::{BTreeMap, BTreeSet};

/// (import_name, wasm_type_idx) for every host import.
///
/// Categories sourced from categories.toml:
///   INTERNAL — Runtime machinery, not exposed to Python
///   BUILTIN  — Always available without import (CPython builtins)
///   STDLIB   — Standard library modules, loaded on demand
pub(crate) const IMPORT_REGISTRY: &[(&str, u32)] = &[
    // ── INTERNAL: Memory management ──
    ("alloc", 2),
    ("alloc_class", 3),
    ("alloc_class_static", 3),
    ("alloc_class_trusted", 3),
    // Scope arena (MLKit-style region allocator). NoEscape allocations bypass
    // the global allocator; the entire arena is freed in O(1) at scope exit.
    ("arena_new", 0),          // () -> i64 (arena handle)
    ("arena_alloc_object", 3), // (arena, size_bits) -> nan-boxed bits
    ("arena_free", 1),         // (arena) -> ()
    ("dec_ref_obj", 1),
    ("inc_ref_obj", 1),
    ("obj_get_state", 14), // (i32) -> i64
    ("obj_set_state", 39), // (i32, i64) -> ()
    // ── INTERNAL: Resource tracking ──
    ("resource_on_allocate", 1), // (i32 size) -> i32 (0=ok, 1=denied)
    ("resource_on_free", 1),     // (i32 size) -> void
    // ── INTERNAL: Module system ──
    ("module_cache_get", 2),
    ("module_cache_del", 2),
    ("module_cache_set", 3),
    ("module_del_global", 3),
    ("module_del_global_if_present", 3),
    ("module_get_attr", 3),
    ("module_import_from", 3),
    ("module_get_global", 3),
    ("module_get_name", 3),
    ("module_import", 2),
    ("module_import_star", 3),
    ("module_new", 2),
    ("module_set_attr", 5),
    // ── INTERNAL: Exceptions ──
    ("exception_active", 0),
    ("exception_current", 0),
    ("exception_enter_handler", 1),
    ("exception_class", 2),
    ("exception_clear", 0),
    ("exception_context_set", 2),
    ("exception_kind", 2),
    ("exception_last", 0),
    ("exception_last_pending", 0),
    ("exception_match_builtin", 3),
    ("exception_resolve_captured", 1),
    ("exception_message", 2),
    ("exception_new", 3),
    ("exception_new_builtin", 3),
    ("exception_new_builtin_empty", 2),
    ("exception_new_builtin_one", 3),
    ("exception_new_from_class", 3),
    ("exception_pending", 0),
    ("exception_pop", 0),
    ("exception_push", 0),
    ("exception_set_cause", 3),
    ("exception_set_last", 2),
    ("exception_set_value", 3),
    ("exception_stack_clear", 0),
    ("exceptiongroup_combine", 2),
    ("exceptiongroup_match", 3),
    ("raise", 2),
    // ── INTERNAL: Classes and types ──
    ("builtin_type", 2),
    ("class_apply_set_name", 2),
    ("class_layout_version", 2),
    ("class_merge_layout", 5),
    ("class_new", 2),
    ("class_set_base", 3),
    ("class_set_layout_version", 3),
    ("classmethod_new", 2),
    ("isinstance", 3),
    ("issubclass", 3),
    ("object_field_get", 3),
    ("object_field_get_ptr", 16),
    ("object_field_init", 5),
    ("object_field_init_ptr", 17),
    ("object_field_set", 5),
    ("object_field_set_ptr", 17),
    ("object_new", 0),
    ("object_new_bound", 2),
    ("object_new_bound_sized", 3),
    ("object_set_class", 16),
    ("property_new", 5),
    ("staticmethod_new", 2),
    ("super_builtin", 3),
    ("super_new", 3),
    ("type_of", 2),
    // ── INTERNAL: Functions and closures ──
    ("bound_method_new", 3),
    ("closure_load", 16),
    ("closure_store", 17),
    ("fn_ptr_code_set", 3),
    ("func_new", 5),
    ("func_new_builtin", 5),
    ("func_new_closure", 7),
    ("function_closure_bits", 2),
    ("function_default_kind", 2),
    ("function_defaults_version", 2),
    ("function_is_coroutine", 2),
    ("function_is_generator", 2),
    ("function_init_metadata_packed", 7),
    ("function_set_builtin", 2),
    ("function_set_defaults", 5),
    ("is_bound_method", 2),
    ("is_function_obj", 2),
    // ── INTERNAL: Call dispatch ──
    ("bridge_unavailable", 2),
    ("capabilities_has", 2),
    ("capabilities_require", 2),
    ("capabilities_trusted", 0),
    ("call_arity_error", 3),
    ("call_bind", 3),
    ("call_bind_ic", 5),
    // Fused instance-method dispatch ICs (no bound-method / callargs alloc on
    // the fast path). Type indices declared in wasm.rs (41-45); `name_ptr` is
    // an i32 linear-memory address, every other arg is a NaN-boxed i64.
    ("call_method_ic0", 41), // (site, recv, name_ptr:i32, name_len) -> i64
    ("call_method_ic1", 42), // + a0
    ("call_method_ic2", 43), // + a0, a1
    ("call_method_ic3", 44), // + a0, a1, a2
    ("call_method_ic4", 45), // + a0, a1, a2, a3
    // Fused super().method() dispatch ICs (no super / bound-method / callargs
    // alloc on the fast path). Type indices declared in wasm.rs (46-50).
    ("call_super_method_ic0", 46), // (site, class, self, name_ptr:i32, name_len) -> i64
    ("call_super_method_ic1", 47), // + a0
    ("call_super_method_ic2", 48), // + a0, a1
    ("call_super_method_ic3", 49), // + a0, a1, a2
    ("call_super_method_ic4", 50), // + a0, a1, a2, a3
    ("call_func_dispatch", 7),
    ("call_indirect_ic", 5),
    ("callargs_expand_kwstar", 3),
    ("callargs_expand_star", 3),
    ("callargs_new", 3),
    ("callargs_push_kw", 5),
    ("callargs_push_pos", 3),
    ("handle_resolve", 13),
    ("invoke_ffi_ic", 7),
    // ── INTERNAL: Fast-path method dispatch ──
    ("fast_dict_get", 5),       // (method, key, default) -> i64
    ("fast_list_append", 3),    // (method, elem) -> i64
    ("fast_str_join", 3),       // (method, iterable) -> i64
    ("fast_str_startswith", 3), // (method, prefix) -> i64
    ("fast_str_upper", 1),      // (method) -> i64
    ("fast_str_lower", 1),      // (method) -> i64
    ("fast_str_strip", 1),      // (method) -> i64
    // ── INTERNAL: Guards and inline caches ──
    ("guard_layout_ptr", 17),
    ("guard_type", 3),
    ("guarded_field_get_ptr", 21),
    ("guarded_field_init_ptr", 22),
    ("guarded_field_set_ptr", 22),
    ("guarded_class_def", 40),
    // ── INTERNAL: Arithmetic ──
    ("abs_builtin", 2),
    ("add", 3),
    // Full-range i64 → boxed int (heap BigInt outside the 47-bit inline
    // window). The LIR fast lane's overflow-safe box cold path
    // (emit_box_i64_overflow_safe) reaches this via a NAMED runtime call.
    ("int_from_i64", 2),
    ("str_concat", 3),
    ("str_contains", 3),
    ("bit_and", 3),
    ("bit_or", 3),
    ("bit_xor", 3),
    ("div", 3),
    ("divmod_builtin", 3),
    ("floordiv", 3),
    ("inplace_add", 3),
    ("inplace_bit_and", 3),
    ("inplace_bit_or", 3),
    ("inplace_bit_xor", 3),
    ("inplace_div", 3),
    ("inplace_floordiv", 3),
    ("inplace_lshift", 3),
    ("inplace_matmul", 3),
    ("inplace_mod", 3),
    ("inplace_mul", 3),
    ("inplace_pow", 3),
    ("inplace_rshift", 3),
    ("inplace_sub", 3),
    ("invert", 2),
    ("lshift", 3),
    ("matmul", 3),
    ("mod", 3),
    ("mul", 3),
    ("pow", 3),
    ("pow_mod", 5),
    ("neg", 2),
    ("pos", 2),
    ("round", 5),
    ("rshift", 3),
    ("sub", 3),
    ("trunc", 2),
    // ── INTERNAL: Comparisons and identity ──
    ("eq", 3),
    ("ge", 3),
    ("gt", 3),
    ("is", 3),
    ("le", 3),
    ("lt", 3),
    ("ne", 3),
    // ── INTERNAL: Singletons ──
    ("ellipsis", 0),
    ("missing", 0),
    ("not_implemented", 0),
    ("pending", 0),
    // ── INTERNAL: Truthiness ──
    ("is_callable", 2),
    ("is_generator", 2),
    ("is_truthy", 2),
    ("is_truthy_bool", 2),
    ("is_truthy_bool_nogil", 2),
    ("is_truthy_int", 2),
    ("is_truthy_int_nogil", 2),
    ("not", 2),
    // ── INTERNAL: Context managers ──
    ("context_closing", 2),
    ("context_depth", 0),
    ("context_enter", 2),
    ("context_exit", 3),
    ("context_null", 2),
    ("context_unwind", 2),
    ("context_unwind_to", 3),
    // ── INTERNAL: Code objects and tracing ──
    ("code_new", 35),
    ("code_slot_set", 3),
    ("code_slots_init", 2),
    ("frame_locals_set", 2),
    ("trace_enter_slot", 2),
    ("trace_exit", 0),
    ("trace_set_line", 2),
    // ── INTERNAL: Runtime lifecycle ──
    ("runtime_init", 0),
    ("runtime_shutdown", 0),
    ("sys_set_version_info", 29),
    // ── INTERNAL: Output ──
    ("print_builtin", 12),
    ("print_newline", 8),
    ("print_obj", 1),
    // ── INTERNAL: Generator/coroutine locals ──
    ("gen_locals", 2),
    ("gen_locals_register", 5),
    // ── INTERNAL: Generators and coroutines ──
    ("future_cancel", 2),
    ("future_cancel_clear", 2),
    ("future_cancel_msg", 3),
    ("future_poll", 2),
    ("generator_close", 2),
    ("generator_send", 3),
    ("generator_throw", 3),
    ("promise_new", 0),
    ("promise_poll", 2),
    ("promise_set_exception", 3),
    ("promise_set_result", 3),
    // ── INTERNAL: Tasks and scheduling ──
    ("block_on", 2),
    ("sleep_register", 27),
    ("spawn", 1),
    ("task_new", 5),
    ("task_register_token_owned", 3),
    // ── INTERNAL: Recursion guard ──
    ("recursion_guard_enter", 0),
    ("recursion_guard_exit", 8),
    // ── INTERNAL: Bootstrap ──
    ("set_intrinsic_manifest", 3),
    // ── INTERNAL: Attributes ──
    ("del_attr_generic", 23),
    ("del_attr_name", 3),
    ("del_attr_object", 18),
    ("del_attr_ptr", 23),
    ("get_attr_generic", 23),
    ("get_attr_name", 3),
    ("get_attr_name_default", 5),
    ("get_attr_object", 18),
    ("get_attr_object_ic", 25),
    ("get_attr_ptr", 23),
    ("get_attr_special", 18),
    ("getattr_builtin", 5),
    ("has_attr_name", 3),
    ("set_attr_generic", 24),
    ("set_attr_name", 5),
    ("set_attr_object", 25),
    ("set_attr_ptr", 24),
    // ── INTERNAL: Sequences and collections ──
    ("contains", 3),
    ("del_index", 3),
    ("dict_clear", 2),
    ("dict_contains", 3),
    ("dict_copy", 2),
    ("dict_from_obj", 2),
    ("dict_get", 5),
    ("dict_getitem", 3),
    ("dict_inc", 5),
    ("dict_items", 2),
    ("dict_keys", 2),
    ("dict_new", 2),
    ("dict_pop", 7),
    ("dict_popitem", 2),
    ("dict_set", 5),
    ("dict_setdefault", 5),
    ("dict_setitem", 5),
    ("dict_setdefault_empty_list", 3),
    ("dict_str_int_inc", 5),
    ("dict_update", 3),
    ("dict_update_kwstar", 3),
    ("dict_values", 2),
    ("enumerate", 5),
    ("enumerate_builtin", 3),
    ("filter_builtin", 3),
    ("frozenset_add", 3),
    ("frozenset_new", 2),
    ("index", 3),
    ("intarray_from_seq", 2),
    ("iter", 2),
    ("iter_next", 2),
    ("iter_sentinel", 3),
    ("len", 2),
    ("len_dict", 2),
    ("len_list", 2),
    ("len_set", 2),
    ("len_str", 2),
    ("len_tuple", 2),
    ("list_append", 3),
    ("list_builder_append", 6),
    ("list_contains", 3),
    ("list_builder_finish", 2),
    ("list_builder_new", 2),
    ("list_clear", 2),
    // Specialized flat i64 list (Codon-style type-specialized container)
    ("list_int_getitem", 3),
    ("list_int_getitem_nogil", 3),
    ("list_int_getitem_raw", 3),
    ("list_int_getitem_raw_checked", 3),
    ("list_int_getitem_truthy", 3),
    ("list_int_len", 2),
    ("list_int_new", 3),
    ("list_fill_new", 3),
    ("list_int_setitem", 5),
    ("list_int_setitem_nogil", 5),
    ("list_int_setitem_raw", 5),
    ("list_copy", 2),
    ("list_count", 3),
    ("list_extend", 3),
    ("list_from_range", 5),
    ("list_index", 3),
    ("list_index_range", 7),
    ("list_insert", 5),
    ("list_pop", 3),
    ("list_remove", 3),
    ("list_reverse", 2),
    ("list_sort", 5),
    ("map_builtin", 3),
    ("range_new", 5),
    ("reversed_builtin", 2),
    ("set_add", 3),
    ("set_add_probe", 3),
    ("set_contains", 3),
    ("set_difference_update", 3),
    ("set_discard", 3),
    ("set_intersection_update", 3),
    ("set_new", 2),
    ("set_pop", 2),
    ("set_remove", 3),
    ("set_symdiff_update", 3),
    ("set_update", 3),
    ("slice", 5),
    ("slice_new", 5),
    ("sorted_builtin", 5),
    ("store_index", 5),
    ("tuple_builder_finish", 2),
    ("tuple_count", 3),
    ("tuple_from_list", 2),
    ("tuple_getitem", 3),
    ("tuple_index", 3),
    ("zip_builtin", 3),
    // ── INTERNAL: Type constructors ──
    ("ascii_from_obj", 2),
    ("bigint_from_str", 16),
    ("bin_builtin", 2),
    ("callable_builtin", 2),
    ("chr", 2),
    ("complex_from_obj", 5),
    ("float_from_obj", 2),
    ("format_builtin", 3),
    ("hex_builtin", 2),
    ("int_from_obj", 5),
    ("int_from_str_of_obj", 5),
    ("oct_builtin", 2),
    ("ord", 2),
    ("ord_at", 3),
    ("str_from_obj", 2),
    // ── INTERNAL: Builtins (misc) ──
    ("aiter", 2),
    ("all_builtin", 2),
    ("anext", 2),
    ("anext_builtin", 3),
    ("anext_default_poll", 2),
    ("any_builtin", 2),
    ("compile_builtin", 9),
    ("dir_builtin", 2),
    ("env_get", 3),
    ("env_snapshot", 0),
    ("errno_constants", 0),
    ("getargv", 0),
    ("getcwd", 0),
    ("getframe", 2),
    ("getpid", 0),
    ("getrecursionlimit", 0),
    ("hash_builtin", 2),
    ("id", 2),
    ("max_builtin", 5),
    ("min_builtin", 5),
    ("next_builtin", 3),
    ("open_builtin", 28),
    ("repr_builtin", 2),
    ("repr_from_obj", 2),
    ("round_builtin", 3),
    ("setrecursionlimit", 2),
    ("sum_builtin", 3),
    ("vars_builtin", 2),
    // ── INTERNAL: Memoryview ──
    ("memoryview_cast", 7),
    ("memoryview_new", 2),
    ("memoryview_tobytes", 2),
    // ── INTERNAL: Sys module ──
    ("sys_abiflags", 0),
    ("sys_api_version", 0),
    ("sys_executable", 0),
    ("sys_hexversion", 0),
    ("sys_implementation_payload", 0),
    ("sys_platform", 0),
    ("sys_stderr", 0),
    ("sys_stdin", 0),
    ("sys_stdout", 0),
    ("sys_version", 0),
    ("sys_version_info", 0),
    // ── INTERNAL: Dataclass ──
    ("dataclass_get", 3),
    ("dataclass_new", 7),
    ("dataclass_set", 5),
    ("dataclass_set_class", 3),
    // ── INTERNAL: Channels ──
    ("chan_drop", 2),
    ("chan_new", 2),
    ("chan_recv", 2),
    ("chan_recv_blocking", 2),
    ("chan_send", 3),
    ("chan_send_blocking", 3),
    ("chan_try_recv", 2),
    ("chan_try_send", 3),
    // ── INTERNAL: Serialization (msgpack/cbor) ──
    ("cbor_parse_scalar", 19),
    ("cbor_parse_scalar_obj", 2),
    ("msgpack_parse_scalar", 19),
    ("msgpack_parse_scalar_obj", 2),
    // ── INTERNAL: Vectorized ops ──
    ("vec_max_int", 3),
    ("vec_max_int_range", 5),
    ("vec_max_int_range_trusted", 5),
    ("vec_max_int_trusted", 3),
    ("vec_min_int", 3),
    ("vec_min_int_range", 5),
    ("vec_min_int_range_trusted", 5),
    ("vec_min_int_trusted", 3),
    ("vec_prod_int", 3),
    ("vec_prod_int_range", 5),
    ("vec_prod_int_range_trusted", 5),
    ("vec_prod_int_trusted", 3),
    ("vec_sum_float", 3),
    ("vec_sum_float_range", 5),
    ("vec_sum_float_range_iter", 3),
    ("vec_sum_float_range_iter_trusted", 3),
    ("vec_sum_float_range_trusted", 5),
    ("vec_sum_float_trusted", 3),
    ("vec_sum_int", 3),
    ("vec_sum_int_range", 5),
    ("vec_sum_int_range_iter", 3),
    ("vec_sum_int_range_iter_trusted", 3),
    ("vec_sum_int_range_trusted", 5),
    ("vec_sum_int_trusted", 3),
    // ── INTERNAL: Heapq ──
    ("heapq_heapify", 2),
    ("heapq_heappop", 2),
    ("heapq_heappush", 3),
    ("heapq_heappushpop", 3),
    ("heapq_heapreplace", 3),
    // ── INTERNAL: Statistics ──
    ("statistics_mean_slice", 12),
    ("statistics_stdev_slice", 12),
    // ── INTERNAL: TAQ ──
    ("taq_ingest_line", 5),
    // ── INTERNAL: IO wait (internal) ──
    ("io_wait_new", 5),
    // ── STDLIB: asyncio ──
    ("async_sleep", 3),
    ("async_sleep_poll", 2),
    ("asyncgen_hooks_get", 0),
    ("asyncgen_hooks_set", 3),
    ("asyncgen_locals", 2),
    ("asyncgen_locals_register", 5),
    ("asyncgen_new", 2),
    ("asyncgen_poll", 2),
    ("asyncgen_shutdown", 0),
    ("asyncio_fd_watcher_poll", 2),
    ("asyncio_gather_poll", 2),
    ("asyncio_ready_runner_poll", 2),
    ("asyncio_server_accept_loop_poll", 2),
    ("asyncio_sock_accept_poll", 2),
    ("asyncio_sock_connect_poll", 2),
    ("asyncio_sock_recv_into_poll", 2),
    ("asyncio_sock_recv_poll", 2),
    ("asyncio_sock_recvfrom_into_poll", 2),
    ("asyncio_sock_recvfrom_poll", 2),
    ("asyncio_sock_sendall_poll", 2),
    ("asyncio_sock_sendto_poll", 2),
    ("asyncio_socket_reader_read_poll", 2),
    ("asyncio_socket_reader_readline_poll", 2),
    ("asyncio_stream_reader_read_poll", 2),
    ("asyncio_stream_reader_readline_poll", 2),
    ("asyncio_stream_send_all_poll", 2),
    ("asyncio_timer_handle_poll", 2),
    ("asyncio_wait_for_poll", 2),
    ("asyncio_wait_poll", 2),
    // ── STDLIB: buffer ──
    ("buffer2d_get", 5),
    ("buffer2d_matmul", 3),
    ("buffer2d_new", 5),
    ("buffer2d_set", 7),
    // ── STDLIB: bytes ──
    ("bytearray_count", 3),
    ("bytearray_count_slice", 9),
    ("bytearray_endswith", 3),
    ("bytearray_endswith_slice", 9),
    ("bytearray_find", 3),
    ("bytearray_find_slice", 9),
    ("bytearray_fill_range", 7),
    ("bytearray_from_obj", 2),
    ("bytearray_from_str", 5),
    ("bytearray_replace", 7),
    ("bytearray_split", 3),
    ("bytearray_split_max", 5),
    ("bytearray_startswith", 3),
    ("bytearray_startswith_slice", 9),
    ("bytes_count", 3),
    ("bytes_count_slice", 9),
    ("bytes_endswith", 3),
    ("bytes_endswith_slice", 9),
    ("bytes_find", 3),
    ("bytes_find_slice", 9),
    ("bytes_from_bytes", 19),
    ("bytes_from_obj", 2),
    ("bytes_from_str", 5),
    ("bytes_replace", 7),
    ("bytes_split", 3),
    ("bytes_split_max", 5),
    ("bytes_startswith", 3),
    ("bytes_startswith_slice", 9),
    // ── STDLIB: cancel ──
    ("cancel_current", 0),
    ("cancel_token_cancel", 2),
    ("cancel_token_clone", 2),
    ("cancel_token_drop", 2),
    ("cancel_token_get_current", 0),
    ("cancel_token_is_cancelled", 2),
    ("cancel_token_new", 2),
    ("cancel_token_set_current", 2),
    ("cancelled", 0),
    // ── STDLIB: contextlib ──
    ("contextlib_async_exitstack_enter_context_poll", 2),
    ("contextlib_async_exitstack_exit_poll", 2),
    ("contextlib_asyncgen_enter_poll", 2),
    ("contextlib_asyncgen_exit_poll", 2),
    // ── STDLIB: importlib ──
    ("importlib_bootstrap_payload", 3),
    ("importlib_cache_from_source", 2),
    ("importlib_coerce_module_name", 5),
    ("importlib_decode_source", 2),
    ("importlib_ensure_default_meta_path", 2),
    ("importlib_exec_extension", 5),
    ("importlib_exec_restricted_source", 5),
    ("importlib_exec_sourceless", 5),
    ("importlib_extension_loader_payload", 5),
    ("importlib_filefinder_find_spec", 5),
    ("importlib_filefinder_invalidate", 2),
    ("importlib_find_in_path", 3),
    ("importlib_find_in_path_package_context", 3),
    ("importlib_find_spec", 28),
    ("importlib_find_spec_orchestrate", 12),
    ("importlib_frozen_external_payload", 3),
    ("importlib_frozen_payload", 3),
    ("importlib_import_transaction", 7),
    ("importlib_import_optional", 2),
    ("importlib_import_or_fallback", 3),
    ("importlib_import_required", 2),
    ("importlib_invalidate_caches", 0),
    ("importlib_known_absent_missing_name", 2),
    ("importlib_load_module_shim", 5),
    ("importlib_metadata_dist_paths", 3),
    ("importlib_metadata_distributions_payload", 3),
    ("importlib_metadata_entry_points_filter_payload", 12),
    ("importlib_metadata_entry_points_select_payload", 7),
    ("importlib_metadata_normalize_name", 2),
    ("importlib_metadata_packages_distributions_payload", 3),
    ("importlib_metadata_payload", 2),
    ("importlib_metadata_record_payload", 2),
    ("importlib_metadata_types_payload", 7),
    ("importlib_module_from_spec", 2),
    ("importlib_module_spec_is_package", 2),
    ("importlib_package_root_from_origin", 2),
    ("importlib_path_is_archive_member", 2),
    ("importlib_pathfinder_find_spec", 5),
    ("importlib_read_file", 2),
    ("importlib_reload", 7),
    ("importlib_resolve_name", 3),
    ("importlib_resources_as_file_enter", 3),
    ("importlib_resources_as_file_exit", 5),
    ("importlib_resources_contents_from_package", 5),
    ("importlib_resources_contents_from_package_parts", 7),
    ("importlib_resources_files_payload", 7),
    ("importlib_resources_is_resource_from_package", 7),
    ("importlib_resources_is_resource_from_package_parts", 7),
    ("importlib_resources_joinpath", 3),
    ("importlib_resources_loader_reader", 3),
    ("importlib_resources_module_name", 3),
    ("importlib_resources_normalize_path", 2),
    ("importlib_resources_only", 5),
    ("importlib_resources_open_mode_is_text", 2),
    ("importlib_resources_open_resource_bytes_from_package", 7),
    (
        "importlib_resources_open_resource_bytes_from_package_parts",
        7,
    ),
    ("importlib_resources_package_info", 5),
    ("importlib_resources_package_leaf_name", 2),
    ("importlib_resources_path_payload", 2),
    ("importlib_resources_read_text_from_package", 9),
    ("importlib_resources_read_text_from_package_parts", 9),
    ("importlib_resources_reader_child_names", 3),
    ("importlib_resources_reader_contents", 2),
    ("importlib_resources_reader_contents_from_roots", 2),
    ("importlib_resources_reader_exists", 3),
    ("importlib_resources_reader_files_traversable", 2),
    ("importlib_resources_reader_is_dir", 3),
    ("importlib_resources_reader_is_resource", 3),
    ("importlib_resources_reader_is_resource_from_roots", 3),
    ("importlib_resources_reader_open_resource_bytes", 3),
    (
        "importlib_resources_reader_open_resource_bytes_from_roots",
        3,
    ),
    ("importlib_resources_reader_resource_path", 3),
    ("importlib_resources_reader_resource_path_from_roots", 3),
    ("importlib_resources_reader_roots", 2),
    ("importlib_resources_resource_path_from_package", 7),
    ("importlib_resources_resource_path_from_package_parts", 7),
    ("importlib_runtime_modules", 0),
    ("importlib_set_module_state", 28),
    ("importlib_source_exec_payload", 5),
    ("importlib_source_from_cache", 2),
    ("importlib_source_hash", 2),
    ("importlib_sourceless_loader_payload", 5),
    ("importlib_spec_from_file_location", 12),
    ("importlib_spec_from_loader", 12),
    ("importlib_stabilize_module_state", 9),
    ("importlib_validate_resource_name", 2),
    ("importlib_zip_read_entry", 3),
    ("importlib_zip_source_exec_payload", 7),
    // ── STDLIB: io ──
    ("file_close", 2),
    ("file_detach", 2),
    ("file_fileno", 2),
    ("file_flush", 2),
    ("file_isatty", 2),
    ("file_open", 3),
    ("file_read", 3),
    ("file_readable", 2),
    ("file_readinto", 3),
    ("file_readinto1", 3),
    ("file_readline", 3),
    ("file_readlines", 3),
    ("file_reconfigure", 9),
    ("file_seek", 5),
    ("file_seekable", 2),
    ("file_tell", 2),
    ("file_truncate", 3),
    ("file_writable", 2),
    ("file_write", 3),
    ("file_writelines", 3),
    ("io_wait", 2),
    ("stream_clone", 2),
    ("stream_close", 1),
    ("stream_drop", 1),
    ("stream_new", 2),
    ("stream_recv", 2),
    ("stream_send", 18),
    ("stream_send_obj", 3),
    // ── STDLIB: json ──
    ("json_parse_scalar", 19),
    ("json_parse_scalar_obj", 2),
    // ── STDLIB: lock ──
    ("lock_acquire", 5),
    ("lock_drop", 2),
    ("lock_locked", 2),
    ("lock_new", 0),
    ("lock_release", 2),
    ("rlock_acquire", 5),
    ("rlock_drop", 2),
    ("rlock_locked", 2),
    ("rlock_new", 0),
    ("rlock_release", 2),
    // ── STDLIB: math ──
    ("math_acos", 2),
    ("math_cos", 2),
    ("math_exp", 2),
    ("math_lgamma", 2),
    ("math_log", 2),
    ("math_log2", 2),
    ("math_sin", 2),
    // ── STDLIB: os ──
    ("os_access", 3),
    ("os_altsep", 0),
    ("os_chdir", 2),
    ("os_chmod", 3),
    ("os_close", 2),
    ("os_cpu_count", 0),
    ("os_curdir", 0),
    ("os_devnull", 0),
    ("os_dup", 2),
    ("os_dup2", 3),
    ("os_extsep", 0),
    ("os_fdopen", 5),
    ("os_fsencode", 2),
    ("os_fspath", 2),
    ("os_fstat", 2),
    ("os_ftruncate", 3),
    ("os_get_inheritable", 2),
    ("os_get_terminal_size", 2),
    ("os_getcwd", 0),
    ("os_getegid", 0),
    ("os_geteuid", 0),
    ("os_getgid", 0),
    ("os_getloadavg", 0),
    ("os_getlogin", 0),
    ("os_getpgrp", 0),
    ("os_getpid", 0),
    ("os_getppid", 0),
    ("os_getuid", 0),
    ("os_isatty", 2),
    ("os_kill", 3),
    ("os_linesep", 0),
    ("os_link", 3),
    ("os_listdir", 2),
    ("os_lseek", 5),
    ("os_lstat", 2),
    ("os_mkdir", 3),
    ("os_name", 0),
    ("os_open", 5),
    ("os_open_flags", 0),
    ("os_pardir", 0),
    ("os_path_commonpath", 2),
    ("os_path_commonprefix", 2),
    ("os_path_getatime", 2),
    ("os_path_getctime", 2),
    ("os_path_getmtime", 2),
    ("os_path_getsize", 2),
    ("os_path_realpath", 2),
    ("os_path_samefile", 3),
    ("os_pathsep", 0),
    ("os_pipe", 0),
    ("os_read", 3),
    ("os_readlink", 2),
    ("os_removedirs", 2),
    ("os_rename", 3),
    ("os_replace", 3),
    ("os_rmdir", 2),
    ("os_scandir", 2),
    ("os_sendfile", 7),
    ("os_sep", 0),
    ("os_set_inheritable", 3),
    ("os_setpgrp", 0),
    ("os_setsid", 0),
    ("os_stat", 2),
    ("os_symlink", 3),
    ("os_sysconf", 2),
    ("os_sysconf_names", 0),
    ("os_truncate", 3),
    ("os_umask", 2),
    ("os_uname", 0),
    ("os_urandom", 2),
    ("os_utime", 5),
    ("os_waitpid", 3),
    ("os_walk", 5),
    ("os_wexitstatus", 2),
    ("os_wifexited", 2),
    ("os_wifsignaled", 2),
    ("os_wifstopped", 2),
    ("os_write", 3),
    ("os_wstopsig", 2),
    ("os_wtermsig", 2),
    ("path_chmod", 3),
    ("path_exists", 2),
    ("path_listdir", 2),
    ("path_mkdir", 3),
    ("path_rmdir", 2),
    ("path_unlink", 2),
    // ── STDLIB: socket ──
    ("socket_accept", 2),
    ("socket_bind", 3),
    ("socket_clone", 2),
    ("socket_close", 2),
    ("socket_connect", 3),
    ("socket_connect_ex", 3),
    ("socket_constants", 0),
    ("socket_detach", 2),
    ("socket_drop", 1),
    ("socket_fileno", 2),
    ("socket_getaddrinfo", 9),
    ("socket_getblocking", 2),
    ("socket_gethostname", 0),
    ("socket_getnameinfo", 3),
    ("socket_getpeername", 2),
    ("socket_getservbyname", 3),
    ("socket_getservbyport", 3),
    ("socket_getsockname", 2),
    ("socket_getsockopt", 7),
    ("socket_gettimeout", 2),
    ("socket_has_ipv6", 0),
    ("socket_inet_ntop", 3),
    ("socket_inet_pton", 3),
    ("socket_listen", 3),
    ("socket_new", 7),
    ("socket_recv", 5),
    ("socket_recv_into", 7),
    ("socket_recvfrom", 5),
    ("socket_send", 5),
    ("socket_sendall", 5),
    ("socket_sendto", 7),
    ("socket_setblocking", 3),
    ("socket_setsockopt", 7),
    ("socket_settimeout", 3),
    ("socket_shutdown", 3),
    ("socketpair", 5),
    // ── STDLIB: sqlite ──
    ("db_exec", 26),
    ("db_exec_obj", 3),
    ("db_query", 26),
    ("db_query_obj", 3),
    // ── STDLIB: string ──
    ("string_capitalize", 2),
    ("string_count", 3),
    ("string_count_slice", 9),
    ("string_endswith", 3),
    ("string_endswith_slice", 9),
    ("string_eq", 3),
    ("string_find", 3),
    ("string_find_slice", 9),
    ("string_format", 3),
    ("string_from_bytes", 19),
    ("string_join", 3),
    ("string_lower", 2),
    ("string_lstrip", 3),
    ("string_replace", 7),
    ("string_rstrip", 3),
    ("string_split", 3),
    ("string_split_field_eq", 7),
    ("string_split_field", 5),
    ("string_split_field_end", 5),
    ("string_split_field_is_ascii", 5),
    ("string_split_field_len", 5),
    ("string_split_field_len_from_bounds", 7),
    ("string_split_field_ord_at_bounds", 12),
    ("string_split_field_start", 5),
    ("string_split_field_to_int", 5),
    ("string_split_max", 5),
    ("string_split_sep_dict_inc", 7),
    ("string_split_validate", 3),
    ("string_split_ws_dict_inc", 5),
    ("string_startswith", 3),
    ("string_startswith_slice", 9),
    ("string_strip", 3),
    ("string_upper", 2),
    // ── STDLIB: struct ──
    ("struct_calcsize", 2),
    ("struct_iter_unpack", 3),
    ("struct_pack", 3),
    ("struct_pack_into", 5),
    ("struct_unpack", 3),
    ("struct_unpack_from", 5),
    // ── STDLIB: subprocess ──
    ("process_drop", 1),
    ("process_kill", 2),
    ("process_pid", 2),
    ("process_poll", 2),
    ("process_returncode", 2),
    ("process_spawn", 9),
    ("process_stderr", 2),
    ("process_stdin", 2),
    ("process_stdout", 2),
    ("process_terminate", 2),
    ("process_wait_future", 2),
    // ── STDLIB: threading ──
    ("thread_current_ident", 0),
    ("thread_current_native_id", 0),
    ("thread_drop", 2),
    ("thread_ident", 2),
    ("thread_is_alive", 2),
    ("thread_join", 3),
    ("thread_native_id", 2),
    ("thread_poll", 2),
    ("thread_spawn", 2),
    ("thread_submit", 5),
    // ── STDLIB: time ──
    ("time_gmtime", 2),
    ("time_localtime", 2),
    ("time_monotonic", 0),
    ("time_monotonic_ns", 0),
    ("time_perf_counter", 0),
    ("time_perf_counter_ns", 0),
    ("time_process_time", 0),
    ("time_process_time_ns", 0),
    ("time_strftime", 3),
    ("time_time", 0),
    ("time_time_ns", 0),
    ("time_timezone", 0),
    ("time_tzname", 0),
    // ── STDLIB: weakref ──
    ("weakref_drop", 2),
    ("weakref_get", 2),
    ("weakref_register", 5),
    // ── STDLIB: websocket ──
    ("ws_close", 1),
    ("ws_connect", 19),
    ("ws_connect_obj", 2),
    ("ws_drop", 1),
    ("ws_pair", 20),
    ("ws_pair_obj", 2),
    ("ws_recv", 2),
    ("ws_send", 18),
    ("ws_send_obj", 3),
    ("ws_wait", 2),
    ("ws_wait_new", 5),
];

pub(crate) const OP_IMPORT_DEPS: &[(&str, &[&str])] = &[
    // ── Core structural imports ──
    // Absolute minimum needed by ANY program regardless of IR content.
    // Everything else is discovered by IR-scanning below.
    // Target: <80 for hello-world. Current: ~50.
    (
        "__structural__",
        &[
            // Memory management
            "alloc",
            "dec_ref_obj",
            "inc_ref_obj",
            // Call dispatch (call_func uses call_func_dispatch)
            "call_func_dispatch",
            // Exceptions — any program can raise
            "exception_active",
            "exception_current",
            "exception_enter_handler",
            "exception_class",
            "exception_clear",
            "exception_context_set",
            "exception_kind",
            "exception_last",
            "exception_last_pending",
            "exception_resolve_captured",
            "exception_message",
            "exception_new",
            "exception_new_from_class",
            "exception_pending",
            "exception_pop",
            "exception_push",
            "exception_set_cause",
            "exception_set_last",
            "exception_set_value",
            "exception_stack_clear",
            // Function creation (func_new is used by every module)
            "func_new",
            // Pointer resolution
            "handle_resolve",
            // Identity and truthiness
            "is",
            "is_truthy",
            "not",
            // Module system
            "module_cache_get",
            "module_cache_set",
            "module_get_attr",
            "module_new",
            "module_set_attr",
            // Output
            "print_newline",
            "print_obj",
            // Exception raising
            "raise",
            // Runtime lifecycle
            "runtime_init",
            "runtime_shutdown",
            // Bootstrap
            "set_intrinsic_manifest",
            "sys_set_version_info",
        ],
    ),
    // ── On-demand: fast-path truthiness ──
    // Pulled in when if/while_test ops exist; codegen may select the integer
    // variant only from explicit representation flags.
    ("if", &["is_truthy_int", "is_truthy_bool"]),
    ("while_test", &["is_truthy_int", "is_truthy_bool"]),
    // ── On-demand: comparison ops ──
    // Pulled in when comparison ops appear in IR.
    ("eq", &["eq"]),
    ("ne", &["ne"]),
    ("lt", &["lt"]),
    ("le", &["le"]),
    ("gt", &["gt"]),
    ("ge", &["ge"]),
    // ── On-demand: context managers ──
    // Pulled in when with-statement ops appear in IR.
    (
        "context_enter",
        &[
            "context_enter",
            "context_exit",
            "context_depth",
            "context_closing",
            "context_null",
            "context_unwind",
            "context_unwind_to",
        ],
    ),
    (
        "context_exit",
        &["context_exit", "context_depth", "context_unwind_to"],
    ),
    ("context_closing", &["context_closing"]),
    ("context_null", &["context_null"]),
    (
        "context_unwind",
        &["context_unwind", "context_unwind_to", "context_depth"],
    ),
    // ── On-demand: class infrastructure ──
    // Pulled in when class-definition ops appear in IR.
    (
        "alloc_class",
        &[
            "alloc_class",
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "alloc_class_static",
        &[
            "alloc_class_static",
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "alloc_class_trusted",
        &[
            "alloc_class_trusted",
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "class_new",
        &[
            "class_new",
            "class_set_base",
            "class_apply_set_name",
            "class_layout_version",
            "class_merge_layout",
            "class_set_layout_version",
        ],
    ),
    (
        "class_def",
        &[
            "guarded_class_def",
            "class_layout_version",
            "class_set_layout_version",
        ],
    ),
    // ── On-demand: object field access ──
    // Pulled in when load/store/object ops appear in IR.
    ("object_new", &["object_new", "object_set_class"]),
    ("object_new_bound", &[]),
    ("object_new_bound_stack", &["object_new_bound_sized"]),
    ("object_field_get", &["object_field_get"]),
    ("object_field_get_ptr", &["object_field_get_ptr"]),
    ("object_field_init", &["object_field_init"]),
    ("object_field_init_ptr", &["object_field_init_ptr"]),
    ("object_field_set", &["object_field_set"]),
    ("object_field_set_ptr", &["object_field_set_ptr"]),
    // ── On-demand: guards and inline caches ──
    // Pulled in when guard/guarded ops appear in IR. (Also via guarded_field_* entries below.)
    ("guard_type", &["guard_type"]),
    ("guard_layout_ptr", &["guard_layout_ptr"]),
    // ── On-demand: closures ──
    // Already have entries below; removed from structural.
    // ── On-demand: exception groups ──
    ("exceptiongroup_combine", &["exceptiongroup_combine"]),
    ("exceptiongroup_match", &["exceptiongroup_match"]),
    // ── On-demand: singletons ──
    ("ellipsis", &["ellipsis"]),
    ("missing", &["missing"]),
    ("not_implemented", &["not_implemented"]),
    ("pending", &["pending"]),
    // ── On-demand: function variants ──
    ("func_new_builtin", &["func_new_builtin"]),
    ("func_new_closure", &["func_new_closure"]),
    ("fn_ptr_code_set", &["fn_ptr_code_set"]),
    ("builtin_func", &["func_new_builtin"]),
    // ── On-demand: call variants ──
    ("call_bind", &["call_bind"]),
    ("call_bind_ic", &["call_bind_ic"]),
    // Fused method-dispatch ICs: a single op kind may lower to any of the five
    // arity variants (the codegen selects by positional-arg count), so pull in
    // the whole family when the fused op is present. Mirrors the context_enter
    // family-expansion convention.
    (
        "call_method_ic",
        &[
            "call_method_ic0",
            "call_method_ic1",
            "call_method_ic2",
            "call_method_ic3",
            "call_method_ic4",
        ],
    ),
    (
        "call_super_method_ic",
        &[
            "call_super_method_ic0",
            "call_super_method_ic1",
            "call_super_method_ic2",
            "call_super_method_ic3",
            "call_super_method_ic4",
        ],
    ),
    ("call_arity_error", &["call_arity_error"]),
    ("call_indirect", &["call_indirect_ic"]),
    ("call_indirect_ic", &["call_indirect_ic"]),
    ("invoke_ffi_ic", &["invoke_ffi_ic"]),
    // ── On-demand: module system extras ──
    ("module_cache_del", &["module_cache_del"]),
    ("module_del_global", &["module_del_global"]),
    (
        "module_del_global_if_present",
        &["module_del_global_if_present"],
    ),
    ("module_get_global", &["module_get_global"]),
    ("module_get_name", &["module_get_name"]),
    ("module_import_from", &["module_import_from"]),
    ("module_import", &["module_import"]),
    ("module_import_star", &["module_import_star"]),
    // ── On-demand: code objects and tracing ──
    ("code_new", &["code_new"]),
    ("code_slot_set", &["code_slot_set"]),
    ("code_slots_init", &["code_slots_init"]),
    ("trace_enter_slot", &["trace_enter_slot"]),
    ("trace_exit", &["trace_exit"]),
    ("line", &["trace_set_line"]),
    ("trace_set_line", &["trace_set_line"]),
    // ── On-demand: check_exception ──
    ("check_exception", &["exception_pending"]),
    // ── Defaults-devirt deopt guard: reads the function defaults version ──
    ("function_defaults_version", &["function_defaults_version"]),
    // ── Arithmetic ops ──
    // Auto-discovered by kind match, declared here for completeness
    // so the scanner has explicit dep table hits.
    ("add", &["add", "str_concat"]),
    ("str_concat", &["str_concat"]),
    ("sub", &["sub"]),
    ("mul", &["mul"]),
    ("div", &["div"]),
    ("floordiv", &["floordiv"]),
    ("mod", &["mod"]),
    ("pow", &["pow"]),
    ("pow_mod", &["pow_mod"]),
    ("lshift", &["lshift"]),
    ("rshift", &["rshift"]),
    ("bit_and", &["bit_and"]),
    ("bit_or", &["bit_or"]),
    ("bit_xor", &["bit_xor"]),
    ("invert", &["invert"]),
    ("neg", &["neg"]),
    ("pos", &["pos"]),
    ("matmul", &["matmul"]),
    ("inplace_add", &["inplace_add", "str_concat"]),
    ("inplace_sub", &["inplace_sub"]),
    ("inplace_mul", &["inplace_mul"]),
    ("inplace_div", &["inplace_div"]),
    ("inplace_floordiv", &["inplace_floordiv"]),
    ("inplace_mod", &["inplace_mod"]),
    ("inplace_pow", &["inplace_pow"]),
    ("inplace_lshift", &["inplace_lshift"]),
    ("inplace_rshift", &["inplace_rshift"]),
    ("inplace_matmul", &["inplace_matmul"]),
    ("inplace_bit_and", &["inplace_bit_and"]),
    ("inplace_bit_or", &["inplace_bit_or"]),
    ("inplace_bit_xor", &["inplace_bit_xor"]),
    ("abs_builtin", &["abs_builtin"]),
    ("round", &["round"]),
    ("trunc", &["trunc"]),
    // Length ops
    ("len", &["len"]),
    ("len_dict", &["len_dict"]),
    ("len_list", &["len_list"]),
    ("len_set", &["len_set"]),
    ("len_str", &["len_str"]),
    ("len_tuple", &["len_tuple"]),
    // Attribute access ops
    ("get_attr_generic", &["get_attr_generic"]),
    ("get_attr_name", &["get_attr_name"]),
    ("get_attr_name_default", &["get_attr_name_default"]),
    ("get_attr_object", &["get_attr_object"]),
    ("get_attr_special", &["get_attr_special"]),
    ("set_attr_generic", &["set_attr_generic"]),
    ("set_attr_name", &["set_attr_name"]),
    ("del_attr_generic", &["del_attr_generic"]),
    ("del_attr_name", &["del_attr_name"]),
    ("del_attr_object", &["del_attr_object"]),
    ("has_attr_name", &["has_attr_name"]),
    // Iterator ops
    ("iter", &["iter"]),
    ("iter_next", &["iter_next"]),
    ("iter_next_unboxed", &["index", "iter_next"]),
    ("iter_sentinel", &["iter_sentinel"]),
    ("contains", &["contains"]),
    // Comparison/identity ops
    ("guard_tag", &["guard_type"]),
    ("isinstance", &["isinstance"]),
    ("exception_match_builtin", &["exception_match_builtin"]),
    ("issubclass", &["issubclass"]),
    ("is_bound_method", &["is_bound_method"]),
    ("is_callable", &["is_callable"]),
    ("is_function_obj", &["is_function_obj"]),
    ("is_generator", &["is_generator"]),
    ("exception_new_builtin", &["exception_new_builtin"]),
    (
        "exception_new_builtin_empty",
        &["exception_new_builtin_empty"],
    ),
    ("exception_new_builtin_one", &["exception_new_builtin_one"]),
    // Generator ops
    ("generator_close", &["generator_close"]),
    ("generator_send", &["generator_send"]),
    ("generator_throw", &["generator_throw"]),
    // Builtins and type ops
    ("builtin_type", &["builtin_type"]),
    ("type_of", &["type_of"]),
    ("bound_method_new", &["bound_method_new"]),
    ("bridge_unavailable", &["bridge_unavailable"]),
    ("classmethod_new", &["classmethod_new"]),
    ("staticmethod_new", &["staticmethod_new"]),
    ("property_new", &["property_new"]),
    ("super_new", &["super_new"]),
    ("repr_builtin", &["repr_builtin"]),
    ("repr_from_obj", &["repr_from_obj"]),
    ("int_from_str_of_obj", &["int_from_str_of_obj"]),
    ("ord_at", &["ord_at"]),
    ("str_from_obj", &["str_from_obj"]),
    ("format_builtin", &["format_builtin"]),
    ("string_eq", &["string_eq"]),
    ("frame_locals_set", &["frame_locals_set"]),
    (
        "recursion_guard_enter",
        &["recursion_guard_enter", "recursion_guard_exit"],
    ),
    ("recursion_guard_exit", &["recursion_guard_exit"]),
    // Call support ops
    ("callargs_new", &["callargs_new"]),
    ("callargs_push_pos", &["callargs_push_pos"]),
    ("callargs_push_kw", &["callargs_push_kw"]),
    ("callargs_expand_star", &["callargs_expand_star"]),
    ("callargs_expand_kwstar", &["callargs_expand_kwstar"]),
    // Function metadata ops
    ("function_default_kind", &["function_default_kind"]),
    ("function_is_coroutine", &["function_is_coroutine"]),
    ("function_is_generator", &["function_is_generator"]),
    (
        "function_init_metadata_packed",
        &["function_init_metadata_packed"],
    ),
    ("function_set_builtin", &["function_set_builtin"]),
    ("function_set_defaults", &["function_set_defaults"]),
    (
        "async_generator_yield",
        &[
            "asyncgen_hooks_get",
            "asyncgen_hooks_set",
            "asyncgen_locals",
            "asyncgen_locals_register",
            "asyncgen_new",
            "asyncgen_shutdown",
        ],
    ),
    (
        "asyncgen_new",
        &[
            "asyncgen_hooks_get",
            "asyncgen_hooks_set",
            "asyncgen_locals",
            "asyncgen_locals_register",
            "asyncgen_new",
            "asyncgen_shutdown",
        ],
    ),
    ("call_async", &["handle_resolve", "inc_ref_obj", "task_new"]),
    (
        "call_guarded",
        &[
            "call_bind_ic",
            "call_func_dispatch",
            "callargs_new",
            "callargs_push_pos",
            "handle_resolve",
            "is_function_obj",
            "is_truthy",
            "recursion_guard_enter",
            "recursion_guard_exit",
            "trace_enter_slot",
            "trace_exit",
        ],
    ),
    (
        "call_method",
        &[
            "call_bind_ic",
            "callargs_new",
            "callargs_push_pos",
            "fast_dict_get",
            "fast_list_append",
            "fast_str_join",
            "fast_str_startswith",
            "fast_str_upper",
            "fast_str_lower",
            "fast_str_strip",
        ],
    ),
    (
        "cbor_parse",
        &[
            "alloc",
            "cbor_parse_scalar",
            "cbor_parse_scalar_obj",
            "handle_resolve",
        ],
    ),
    ("closure_load", &["closure_load", "handle_resolve"]),
    ("closure_store", &["closure_store", "handle_resolve"]),
    ("const_bigint", &["bigint_from_str"]),
    ("const_bytes", &["bytes_from_bytes"]),
    ("const_ellipsis", &["ellipsis"]),
    ("const_not_implemented", &["not_implemented"]),
    ("const_str", &["string_from_bytes"]),
    (
        "coroutine",
        &[
            "cancel_token_get_current",
            "handle_resolve",
            "inc_ref_obj",
            "task_new",
            "task_register_token_owned",
        ],
    ),
    (
        "alloc_task",
        &[
            "cancel_token_get_current",
            "handle_resolve",
            "inc_ref_obj",
            "task_new",
            "task_register_token_owned",
        ],
    ),
    ("del_attr_generic_ptr", &["del_attr_ptr", "handle_resolve"]),
    ("dict_new", &["dict_new", "dict_set"]),
    ("frozenset_new", &["frozenset_add", "frozenset_new"]),
    (
        "function_closure_bits",
        &["function_closure_bits", "inc_ref_obj"],
    ),
    (
        "gen_locals_register",
        &["gen_locals", "gen_locals_register"],
    ),
    ("get_attr_generic_obj", &["get_attr_object_ic"]),
    ("get_attr_generic_ptr", &["get_attr_ptr", "handle_resolve"]),
    ("get_attr_special_obj", &["get_attr_special"]),
    ("guard_layout", &["guard_layout_ptr", "handle_resolve"]),
    (
        "guarded_field_get",
        &[
            "guard_layout_ptr",
            "guarded_field_get_ptr",
            "handle_resolve",
            "inc_ref_obj",
        ],
    ),
    (
        "guarded_field_init",
        &[
            "guard_layout_ptr",
            "guarded_field_init_ptr",
            "handle_resolve",
            "object_field_init_ptr",
        ],
    ),
    (
        "guarded_field_set",
        &[
            "guard_layout_ptr",
            "guarded_field_set_ptr",
            "handle_resolve",
            "object_field_set_ptr",
        ],
    ),
    ("guarded_load", &["inc_ref_obj", "object_field_get"]),
    (
        "invoke_ffi",
        &["callargs_new", "callargs_push_pos", "invoke_ffi_ic"],
    ),
    ("io_wait", &["io_wait", "io_wait_new"]),
    ("io_wait_new", &["io_wait", "io_wait_new"]),
    (
        "json_parse",
        &[
            "alloc",
            "handle_resolve",
            "json_parse_scalar",
            "json_parse_scalar_obj",
        ],
    ),
    ("list_int_new", &["list_int_new"]),
    ("list_fill_new", &["list_fill_new"]),
    (
        "build_list",
        &[
            "list_builder_append",
            "list_builder_finish",
            "list_builder_new",
        ],
    ),
    (
        "list_new",
        &[
            "list_builder_append",
            "list_builder_finish",
            "list_builder_new",
        ],
    ),
    ("load", &["inc_ref_obj", "object_field_get"]),
    (
        "msgpack_parse",
        &[
            "alloc",
            "handle_resolve",
            "msgpack_parse_scalar",
            "msgpack_parse_scalar_obj",
        ],
    ),
    ("object_set_class", &["handle_resolve", "object_set_class"]),
    ("process_spawn", &["process_poll", "process_spawn"]),
    ("set_attr_generic_obj", &["set_attr_object"]),
    ("set_attr_generic_ptr", &["handle_resolve", "set_attr_ptr"]),
    ("set_new", &["set_add", "set_new"]),
    (
        "state_transition",
        &[
            "closure_store",
            "future_poll",
            "handle_resolve",
            "obj_get_state",
            "obj_set_state",
            "sleep_register",
        ],
    ),
    ("state_yield", &["inc_ref_obj", "obj_set_state"]),
    ("chan_send_yield", &["chan_send", "obj_set_state"]),
    ("chan_recv_yield", &["chan_recv", "obj_set_state"]),
    ("store", &["object_field_set"]),
    ("store_init", &["object_field_init"]),
    ("string_format", &["format_builtin"]),
    ("task_new", &["task_new", "task_register_token_owned"]),
    ("thread_spawn", &["thread_poll", "thread_spawn"]),
    ("thread_submit", &["thread_poll", "thread_submit"]),
    (
        "tuple_new",
        &[
            "list_builder_append",
            "list_builder_new",
            "tuple_builder_finish",
        ],
    ),
    ("unpack_sequence", &["index"]),
    ("yield_await", &["future_poll"]),
    ("yield_send", &["future_poll"]),
    // Websocket group: any ws_ op requires the full websocket import set.
    // Each individual ws_ op maps to the same complete group because the
    // runtime's ws implementation is tightly coupled (connect needs wait,
    // send needs wait, etc.).
    (
        "ws_connect",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_connect_obj",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_pair",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_pair_obj",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_send",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_send_obj",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_recv",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_close",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_drop",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_wait",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
    (
        "ws_wait_new",
        &[
            "ws_close",
            "ws_connect",
            "ws_connect_obj",
            "ws_drop",
            "ws_pair",
            "ws_pair_obj",
            "ws_recv",
            "ws_send",
            "ws_send_obj",
            "ws_wait",
            "ws_wait_new",
        ],
    ),
];

/// Scan the IR for imports that must be declared before relocatable linking.
/// Non-relocatable Auto registers the canonical import registry and strips
/// from TrackedImportIds after codegen, because the emitted-use ledger is
/// the only exact authority for final import retention. Relocatable Auto
/// cannot use that post-serialization strip phase, so it still needs this
/// conservative pre-emission set for declarations handed to the linker.
pub(crate) fn collect_reloc_required_imports(ir: &SimpleIR) -> BTreeSet<String> {
    let mut required: BTreeSet<String> = BTreeSet::new();

    // Build a lookup from the deps table.
    let deps_map: BTreeMap<&str, &[&str]> = OP_IMPORT_DEPS.iter().map(|&(k, v)| (k, v)).collect();

    // Structural imports: always needed regardless of IR content.
    if let Some(structural) = deps_map.get("__structural__") {
        for &name in *structural {
            required.insert(name.to_string());
        }
    }

    if let Ok(extra_required) = std::env::var("MOLT_WASM_EXTRA_REQUIRED_IMPORTS") {
        for name in extra_required
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            required.insert(name.to_string());
        }
    }

    // Build the set of all defined function/import names so Auto mode can
    // distinguish internal direct calls from imported runtime direct calls.
    let defined_function_names: BTreeSet<&str> =
        ir.functions.iter().map(|func| func.name.as_str()).collect();

    // Build the set of all known import names for auto-discovery.
    let known_imports: BTreeSet<&str> = IMPORT_REGISTRY.iter().map(|&(name, _)| name).collect();

    // Scan IR ops: each op declares its own import dependencies.
    for func_ir in &ir.functions {
        let scalar_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        for (op_index, op) in func_ir.ops.iter().enumerate() {
            let kind = op.kind.as_str();
            if matches!(
                std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                Some("1")
            ) && op.s_value.as_deref() == Some("__main_____f_poll")
            {
                eprintln!(
                    "WASM_IMPORTS saw_op kind={} s_value={:?} task_kind={:?} args={:?} func={}",
                    kind, op.s_value, op.task_kind, op.args, func_ir.name
                );
            }

            // `object_new_bound` chooses its runtime entry from the
            // static payload-size metadata attached to the op. Keep the
            // auto import set as precise as the codegen path: typed class
            // allocation sites with a positive payload size use the sized
            // constructor, while hand-built legacy SimpleIR without the
            // metadata keeps the unsized verifier/allocator path.
            if kind == "object_new_bound" {
                let import_name = if op.value.is_some_and(|size| size > 0) {
                    "object_new_bound_sized"
                } else {
                    "object_new_bound"
                };
                required.insert(import_name.to_string());
                continue;
            }

            // Check explicit dependency table first.
            if let Some(deps) = deps_map.get(kind) {
                if matches!(
                    std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                    Some("1")
                ) && kind == "alloc_task"
                {
                    eprintln!(
                        "WASM_IMPORTS alloc_task deps={deps:?} func={}",
                        func_ir.name
                    );
                }
                for &dep in *deps {
                    required.insert(dep.to_string());
                }
            }
            // Auto-discovery: if op kind matches a known import name, include it.
            else if known_imports.contains(kind) {
                required.insert(kind.to_string());
            }
            if crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(kind)
                && op.out.is_some()
            {
                required.insert("inc_ref_obj".to_string());
            }

            // builtin_func ops reference imports by s_value (with molt_ prefix).
            if kind == "builtin_func"
                && let Some(name) = op.s_value.as_ref()
            {
                let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                required.insert(import_name.to_string());
            }
            if kind == "call"
                && let Some(name) = op.s_value.as_ref()
                && !defined_function_names.contains(name.as_str())
            {
                let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                if name.starts_with("molt_") || known_imports.contains(import_name) {
                    required.insert(import_name.to_string());
                }
            }
            if kind == "call_async"
                && let Some(name) = op.s_value.as_ref()
            {
                let import_name = name.strip_prefix("molt_").unwrap_or(name.as_str());
                if known_imports.contains(import_name) {
                    required.insert(import_name.to_string());
                }
            }

            // Task allocation semantics are keyed off task_kind metadata in
            // practice; keep the required runtime imports even if an
            // intermediate op kind rewrite obscures the original alloc_task
            // key before import collection runs.
            if let Some(task_kind) = op.task_kind.as_deref() {
                if matches!(
                    std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                    Some("1")
                ) {
                    eprintln!(
                        "WASM_IMPORTS task_meta kind={} task_kind={} args={} func={}",
                        kind,
                        task_kind,
                        op.args.as_ref().map(|a| a.len()).unwrap_or(0),
                        func_ir.name
                    );
                }
                required.insert("task_new".to_string());
                let has_args = op.args.as_ref().is_some_and(|args| !args.is_empty());
                if has_args {
                    required.insert("handle_resolve".to_string());
                    required.insert("inc_ref_obj".to_string());
                }
                if matches!(task_kind, "future" | "coroutine") {
                    required.insert("cancel_token_get_current".to_string());
                    required.insert("task_register_token_owned".to_string());
                }
            }

            // Some ops lower to specialized runtime imports selected from
            // final TIR/LIR container facts at codegen time. Mirror that
            // logic here so Auto mode keeps the same import lane that
            // compile_func will actually emit.
            let specialized_import =
                wasm_specialized_container_import(&scalar_plan, op_index, kind, op);
            if let Some(import_name) = specialized_import {
                required.insert(import_name.to_string());
            }

            // Prefix-based discovery for stdlib groups.
            // If the op kind starts with a known stdlib prefix, include it.
            // Group expansions (e.g., ws_ -> all websocket imports) are handled
            // by OP_IMPORT_DEPS entries above, not here.
            for prefix in [
                "os_",
                "path_",
                "time_",
                "struct_",
                "importlib_",
                "asyncio_",
                "contextlib_async",
                "socket_",
                "file_",
                "stream_",
                "lock_",
                "rlock_",
                "thread_",
                "process_",
                "db_",
                "ws_",
                "cancel_token_",
                "chan_",
                "string_",
                "bytes_",
                "bytearray_",
                "math_",
                "json_",
                "msgpack_",
                "cbor_",
                "vec_",
                "heapq_",
                "buffer2d_",
                "statistics_",
                "weakref_",
                "memoryview_",
                "taq_",
                "sys_",
                "dataclass_",
            ] {
                if kind.starts_with(prefix) {
                    required.insert(kind.to_string());
                    break;
                }
            }

            // Special singleton matches.
            match kind {
                "socketpair" | "cancelled" | "cancel_current" | "spawn" | "block_on"
                | "sleep_register" | "intarray_from_seq" | "enumerate" | "aiter" | "anext"
                | "open_builtin" | "compile_builtin" | "getargv" | "getpid" | "getframe"
                | "getcwd" | "getrecursionlimit" | "setrecursionlimit" | "env_get"
                | "env_snapshot" | "os_name" | "errno_constants" => {
                    required.insert(kind.to_string());
                }
                _ => {}
            }
        }

        // Scan poll functions for poll import references.
        if func_ir.name.ends_with("_poll") {
            for op in &func_ir.ops {
                if (op.kind == "call_func" || op.kind == "invoke_ffi")
                    && let Some(s) = op.s_value.as_ref()
                    && s.ends_with("_poll")
                {
                    let import_name = s.strip_prefix("molt_").unwrap_or(s.as_str());
                    required.insert(import_name.to_string());
                }
            }
        }
    }

    required
}

#[cfg(test)]
mod tests {
    use super::{IMPORT_REGISTRY, OP_IMPORT_DEPS};

    #[test]
    fn module_cache_del_is_registered_as_on_demand_wasm_import() {
        let import_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "module_cache_del").then_some(type_idx));
        assert_eq!(
            import_type,
            Some(2),
            "module_cache_del must use the unary i64 -> i64 host import ABI"
        );

        let structural = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "__structural__").then_some(deps))
            .expect("structural WASM import deps must exist");
        assert!(
            !structural.contains(&"module_cache_del"),
            "module_cache_del is cleanup-only and must not inflate every Auto-profile WASM binary"
        );

        let op_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "module_cache_del").then_some(deps))
            .expect("module_cache_del op must declare its WASM import dependency");
        assert_eq!(
            op_deps,
            ["module_cache_del"],
            "module_cache_del codegen must request its runtime import explicitly"
        );
    }

    #[test]
    fn object_new_bound_declares_wasm_imports() {
        let bound_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "object_new_bound").then_some(type_idx));
        assert_eq!(
            bound_type,
            Some(2),
            "object_new_bound must use the unary i64 -> i64 host import ABI"
        );

        let sized_type = IMPORT_REGISTRY
            .iter()
            .find_map(|&(name, type_idx)| (name == "object_new_bound_sized").then_some(type_idx));
        assert_eq!(
            sized_type,
            Some(3),
            "object_new_bound_sized must use the binary i64,i64 -> i64 host import ABI"
        );

        let op_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "object_new_bound").then_some(deps))
            .expect("object_new_bound op must declare its WASM import dependencies");
        assert!(
            op_deps.is_empty(),
            "object_new_bound dependencies are selected from payload-size metadata during import collection"
        );

        let stack_deps = OP_IMPORT_DEPS
            .iter()
            .find_map(|&(kind, deps)| (kind == "object_new_bound_stack").then_some(deps))
            .expect("object_new_bound_stack op must declare its WASM import dependencies");
        assert_eq!(
            stack_deps,
            ["object_new_bound_sized"],
            "WASM has no native stack object representation; stack-eligible class allocation lowers to the sized heap constructor"
        );
    }
}
