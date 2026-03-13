# TIR Intrinsic Inlining Design

**Status**: Design (not yet implemented)
**Priority**: P0 -- #1 optimization target
**Expected impact**: 20-40% improvement on call-heavy workloads
**Author**: Design research
**Date**: 2026-03-12

## 1. Problem Statement

Molt compiles Python to native code via Cranelift, but every runtime operation (refcount adjustment, type checks, truthiness tests, arithmetic fallbacks, container operations) crosses an FFI boundary into separately-compiled Rust code. The Cranelift-generated object code calls into `molt-runtime` functions compiled by rustc/LLVM through `Linkage::Import` declarations. This boundary prevents the compiler from:

1. **Inlining**: Each FFI call pays function prologue/epilogue, argument marshaling, and return value extraction overhead.
2. **Cross-boundary optimization**: Cranelift cannot perform CSE, DCE, constant propagation, or register allocation across the call boundary.
3. **Branch elimination**: Runtime functions re-check type tags that the compiler already verified (e.g., `molt_inc_ref_obj` re-extracts the pointer from a NaN-boxed value whose tag was already checked).
4. **GIL re-acquisition**: Every `extern "C"` runtime entry point acquires the GIL via `with_gil_entry!`, which on native targets does a TLS depth check + conditional mutex lock -- even though the compiled code already holds the GIL for the entire function body.

### Quantified FFI Surface

The backend (`runtime/molt-backend/src/lib.rs`, 14,199 lines) declares **373 unique** `molt_*` runtime functions via `Linkage::Import`, with **452 total** `Linkage::Import` declarations (many functions are declared at multiple call sites). A single compiled function body typically emits:

- `molt_dec_ref_obj` / `molt_inc_ref_obj`: Called at every variable death / ownership transfer (73+ emission sites in the backend)
- `molt_is_truthy`: Called at every `if`, `while`, `and`, `or`, `not` (7 declaration sites, used in every conditional path)
- `molt_add/sub/mul/...`: Called on the slow path of every arithmetic operation (already partially inlined for the int-int fast path)
- `molt_call_bind_ic`: Called at every dynamic dispatch site (8 declaration sites)
- `molt_callargs_new/push_pos`: Called for every non-direct call (9 declaration sites each)

A tight loop like `for i in range(N): total += x` pays at minimum 4 FFI calls per iteration (`molt_iter_next`, `molt_is_truthy` for the sentinel check, `molt_add` slow-path fallback, `molt_dec_ref_obj` for the previous `total`), plus the GIL re-entry overhead on each.

### Complete Categorized Inventory of Imported Runtime Functions

All 373 unique `molt_*` functions imported via `Linkage::Import`, organized by category. Declaration count indicates how many separate `declare_function` call sites exist in `lib.rs` (higher count = used in more codegen paths).

#### RC Operations (3 functions, ~73 emission sites per compiled function)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_inc_ref_obj` | 4 | **Yes** | Tag-checks bits, extracts pointer, checks null/immortal, atomic increment |
| `molt_dec_ref_obj` | 1 | **Yes** | Same as inc but with deallocation on zero. Most frequent call in compiled code |
| `molt_dec_ref` | 1 | **Yes** | Takes raw pointer (no tag check). Used for known-pointer paths |

#### Type Checks and Truthiness (10 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_is_truthy` | 7 | **Yes** | Dispatches on all types (none/bool/int/float/ptr). Every `if`/`while`/`and`/`or` |
| `molt_is` | 1 | Yes | Identity comparison. Trivially inlineable: `icmp_eq(a, b)` |
| `molt_not` | 1 | Yes | `!is_truthy(val)`. Composable from inlined truthiness |
| `molt_isinstance` | 1 | Moderate | Requires class hierarchy lookup |
| `molt_issubclass` | 1 | Low | |
| `molt_type_of` | 1 | Moderate | |
| `molt_builtin_type` | 1 | Low | |
| `molt_is_callable` | 1 | Low | |
| `molt_is_generator` | 1 | Low | |
| `molt_is_bound_method` | 2 | Low | |

#### Arithmetic (24 functions, partially inlined via `fast_int`)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_add` | 2 | **Yes** | Slow-path fallback for non-int or overflow |
| `molt_sub` | 1 | **Yes** | |
| `molt_mul` | 2 | **Yes** | |
| `molt_div` | 2 | Yes | |
| `molt_floordiv` | 2 | Yes | |
| `molt_mod` | 2 | Yes | |
| `molt_pow` | 1 | Moderate | |
| `molt_pow_mod` | 1 | Low | |
| `molt_matmul` | 1 | Low | |
| `molt_inplace_add` | 2 | **Yes** | |
| `molt_inplace_sub` | 1 | Yes | |
| `molt_inplace_mul` | 2 | Yes | |
| `molt_bit_or` | 2 | Moderate | |
| `molt_bit_and` | 2 | Moderate | |
| `molt_bit_xor` | 2 | Moderate | |
| `molt_lshift` | 2 | Moderate | |
| `molt_rshift` | 2 | Moderate | |
| `molt_inplace_bit_or` | 2 | Low | |
| `molt_inplace_bit_and` | 2 | Low | |
| `molt_inplace_bit_xor` | 2 | Low | |
| `molt_abs_builtin` | 1 | Low | |
| `molt_invert` | 1 | Low | Bitwise NOT |
| `molt_round` | 1 | Low | |
| `molt_trunc` | 1 | Low | |

#### Comparison (8 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_lt` | 1 | **Yes** | All comparisons have `fast_int` paths already |
| `molt_le` | 1 | **Yes** | |
| `molt_gt` | 1 | **Yes** | |
| `molt_ge` | 1 | **Yes** | |
| `molt_eq` | 1 | **Yes** | |
| `molt_ne` | 1 | Yes | |
| `molt_string_eq` | 1 | Yes | Specialized string equality |
| `molt_contains` | 1 | Yes | `in` operator |

#### Call Dispatch and Arguments (9 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_callargs_new` | 9 | **Yes** | Allocates CallArgs builder |
| `molt_callargs_push_pos` | 9 | **Yes** | Pushes positional arg |
| `molt_callargs_push_kw` | 1 | Moderate | Pushes keyword arg |
| `molt_callargs_expand_star` | 1 | Low | `*args` expansion |
| `molt_callargs_expand_kwstar` | 1 | Low | `**kwargs` expansion |
| `molt_call_bind_ic` | 8 | **Yes** | IC-based dynamic dispatch |
| `molt_call_indirect_ic` | 1 | Moderate | |
| `molt_invoke_ffi_ic` | 1 | Moderate | FFI invocation |
| `molt_handle_resolve` | 2 | Moderate | |

#### Function/Closure/Code Objects (15 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_func_new` | 1 | Low | |
| `molt_func_new_builtin` | 1 | Low | |
| `molt_func_new_closure` | 1 | Low | |
| `molt_code_new` | 1 | Low | |
| `molt_code_slot_set` | 1 | Low | |
| `molt_code_slots_init` | 1 | Low | |
| `molt_fn_ptr_code_set` | 1 | Low | |
| `molt_function_closure_bits` | 2 | Low | |
| `molt_function_default_kind` | 1 | Low | |
| `molt_function_is_generator` | 1 | Low | |
| `molt_function_is_coroutine` | 1 | Low | |
| `molt_is_function_obj` | 2 | Low | |
| `molt_bound_method_new` | 1 | Low | |
| `molt_closure_load` | 1 | Moderate | |
| `molt_closure_store` | 2 | Moderate | |

#### List Operations (16 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_list_builder_new` | 2 | Yes | List literal construction |
| `molt_list_builder_append` | 2 | Yes | |
| `molt_list_builder_finish` | 1 | Yes | |
| `molt_list_append` | 1 | **Yes** | Hot in list-building loops |
| `molt_list_pop` | 1 | Moderate | |
| `molt_list_extend` | 1 | Moderate | |
| `molt_list_insert` | 1 | Low | |
| `molt_list_remove` | 1 | Low | |
| `molt_list_clear` | 1 | Low | |
| `molt_list_copy` | 1 | Low | |
| `molt_list_reverse` | 1 | Low | |
| `molt_list_count` | 1 | Low | |
| `molt_list_index` | 1 | Low | |
| `molt_list_index_range` | 1 | Low | |
| `molt_list_contains` | 1 | Yes | `x in list` |
| `molt_list_from_range` | 1 | Low | |

#### Tuple Operations (4 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_tuple_builder_finish` | 1 | Yes | |
| `molt_tuple_from_list` | 1 | Low | |
| `molt_tuple_count` | 1 | Low | |
| `molt_tuple_index` | 1 | Low | |

#### Dict Operations (18 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_dict_new` | 1 | Yes | |
| `molt_dict_set` | 2 | **Yes** | |
| `molt_dict_get` | 1 | **Yes** | |
| `molt_dict_inc` | 1 | Yes | Counter pattern |
| `molt_dict_str_int_inc` | 1 | Yes | Specialized word count |
| `molt_dict_pop` | 1 | Moderate | |
| `molt_dict_setdefault` | 1 | Moderate | |
| `molt_dict_setdefault_empty_list` | 1 | Moderate | |
| `molt_dict_update` | 1 | Moderate | |
| `molt_dict_update_missing` | 1 | Moderate | |
| `molt_dict_update_kwstar` | 1 | Low | |
| `molt_dict_clear` | 1 | Low | |
| `molt_dict_copy` | 1 | Low | |
| `molt_dict_popitem` | 1 | Low | |
| `molt_dict_from_obj` | 1 | Low | |
| `molt_dict_keys` | 1 | Moderate | |
| `molt_dict_values` | 1 | Moderate | |
| `molt_dict_items` | 1 | Moderate | |
| `molt_dict_contains` | 1 | Yes | `key in dict` |

#### Set/Frozenset Operations (12 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_set_new` | 1 | Low | |
| `molt_set_add` | 2 | Moderate | |
| `molt_set_discard` | 1 | Low | |
| `molt_set_remove` | 1 | Low | |
| `molt_set_pop` | 1 | Low | |
| `molt_set_update` | 1 | Low | |
| `molt_set_intersection_update` | 1 | Low | |
| `molt_set_difference_update` | 1 | Low | |
| `molt_set_symdiff_update` | 1 | Low | |
| `molt_set_contains` | 1 | Moderate | |
| `molt_frozenset_new` | 1 | Low | |
| `molt_frozenset_add` | 2 | Low | |

#### Indexing and Slicing (6 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_index` | 1 | **Yes** | Generic subscript. Hot in loops |
| `molt_store_index` | 1 | **Yes** | `obj[key] = val` |
| `molt_del_index` | 1 | Low | |
| `molt_slice` | 1 | Moderate | `obj[a:b:c]` |
| `molt_slice_new` | 1 | Moderate | |
| `molt_len` | 1 | **Yes** | `len()` builtin |

#### Iterator Operations (6 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_iter_checked` | 1 | **Yes** | Creates iterator. Every `for` loop |
| `molt_iter_next` | 1 | **Yes** | Advances iterator. Every loop iteration |
| `molt_enumerate` | 1 | Moderate | |
| `molt_aiter` | 1 | Low | Async iteration |
| `molt_anext` | 1 | Low | |
| `molt_range_new` | 1 | Yes | |

#### String Operations (21 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_string_from_bytes` | 1 | Moderate | String literal construction |
| `molt_string_find` | 1 | Yes | |
| `molt_string_find_slice` | 1 | Low | |
| `molt_string_startswith` | 1 | Yes | |
| `molt_string_startswith_slice` | 1 | Low | |
| `molt_string_endswith` | 1 | Yes | |
| `molt_string_endswith_slice` | 1 | Low | |
| `molt_string_count` | 1 | Low | |
| `molt_string_count_slice` | 1 | Low | |
| `molt_string_join` | 1 | Moderate | |
| `molt_string_split` | 1 | Yes | |
| `molt_string_split_max` | 1 | Low | |
| `molt_string_lower` | 1 | Moderate | |
| `molt_string_upper` | 1 | Moderate | |
| `molt_string_capitalize` | 1 | Low | |
| `molt_string_strip` | 1 | Moderate | |
| `molt_string_lstrip` | 1 | Low | |
| `molt_string_rstrip` | 1 | Low | |
| `molt_string_replace` | 1 | Moderate | |
| `molt_string_split_ws_dict_inc` | 1 | Yes | Fused word-count |
| `molt_string_split_sep_dict_inc` | 1 | Yes | Fused split-count |

#### Bytes/Bytearray Operations (20 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_bytes_from_bytes` | 1 | Moderate | |
| `molt_bytes_from_obj` | 1 | Low | |
| `molt_bytes_from_str` | 1 | Low | |
| `molt_bytes_find` | 1 | Moderate | |
| `molt_bytes_find_slice` | 1 | Low | |
| `molt_bytes_startswith` | 1 | Low | |
| `molt_bytes_startswith_slice` | 1 | Low | |
| `molt_bytes_endswith` | 1 | Low | |
| `molt_bytes_endswith_slice` | 1 | Low | |
| `molt_bytes_count` | 1 | Low | |
| `molt_bytes_count_slice` | 1 | Low | |
| `molt_bytes_split` | 1 | Low | |
| `molt_bytes_split_max` | 1 | Low | |
| `molt_bytes_replace` | 1 | Low | |
| `molt_bytearray_from_obj` | 1 | Low | |
| `molt_bytearray_from_str` | 1 | Low | |
| `molt_bytearray_find` | 1 | Low | |
| `molt_bytearray_find_slice` | 1 | Low | |
| `molt_bytearray_startswith` | 1 | Low | |
| `molt_bytearray_startswith_slice` | 1 | Low | |
| `molt_bytearray_endswith` | 1 | Low | |
| `molt_bytearray_endswith_slice` | 1 | Low | |
| `molt_bytearray_count` | 1 | Low | |
| `molt_bytearray_count_slice` | 1 | Low | |
| `molt_bytearray_split` | 1 | Low | |
| `molt_bytearray_split_max` | 1 | Low | |
| `molt_bytearray_replace` | 1 | Low | |

#### Type Conversion (8 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_str_from_obj` | 1 | Yes | `str()` |
| `molt_repr_from_obj` | 1 | Yes | `repr()` |
| `molt_ascii_from_obj` | 1 | Low | |
| `molt_float_from_obj` | 1 | Moderate | `float()` |
| `molt_int_from_obj` | 1 | Moderate | `int()` |
| `molt_complex_from_obj` | 1 | Low | |
| `molt_format_builtin` | 1 | Low | `format()` |
| `molt_bigint_from_str` | 1 | Low | Large integer literal parsing |

#### Printing and I/O (7 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_print_obj` | 1 | Moderate | `print()` |
| `molt_print_newline` | 1 | Moderate | |
| `molt_output` | 1 | Low | |
| `molt_id` | 1 | Low | `id()` |
| `molt_ord` | 1 | Low | `ord()` |
| `molt_chr` | 1 | Low | `chr()` |
| `molt_env_get` | 1 | Low | |

#### File I/O (5 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_file_open` | 1 | Low | |
| `molt_file_read` | 1 | Low | |
| `molt_file_write` | 1 | Low | |
| `molt_file_close` | 1 | Low | |
| `molt_file_flush` | 1 | Low | |

#### Exception Handling (20 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_exception_pending_fast` | 1 | **Yes** | Checked after every fallible op |
| `molt_exception_push` | 1 | Moderate | |
| `molt_exception_pop` | 1 | Moderate | |
| `molt_exception_stack_clear` | 1 | Low | |
| `molt_exception_stack_depth` | 1 | Low | |
| `molt_exception_stack_enter` | 1 | Moderate | |
| `molt_exception_stack_exit` | 1 | Moderate | |
| `molt_exception_stack_set_depth` | 1 | Low | |
| `molt_exception_last` | 1 | Moderate | |
| `molt_exception_new` | 1 | Low | |
| `molt_exception_new_from_class` | 1 | Low | |
| `molt_exception_clear` | 1 | Low | |
| `molt_exception_kind` | 1 | Low | |
| `molt_exception_class` | 1 | Low | |
| `molt_exception_message` | 1 | Low | |
| `molt_exception_set_cause` | 1 | Low | |
| `molt_exception_set_last` | 1 | Low | |
| `molt_exception_set_value` | 1 | Low | |
| `molt_exception_context_set` | 1 | Low | |
| `molt_raise` | 1 | Low | |
| `molt_exceptiongroup_match` | 1 | Low | |
| `molt_exceptiongroup_combine` | 1 | Low | |

#### Object/Class System (19 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_alloc` | 1 | Moderate | Generic allocation |
| `molt_alloc_class` | 1 | Moderate | |
| `molt_alloc_class_trusted` | 1 | Moderate | |
| `molt_alloc_class_static` | 1 | Low | |
| `molt_object_new` | 1 | Moderate | |
| `molt_object_set_class` | 1 | Low | |
| `molt_object_field_set_ptr` | 1 | Yes | |
| `molt_object_field_init_ptr` | 1 | Yes | |
| `molt_class_new` | 1 | Low | |
| `molt_class_set_base` | 1 | Low | |
| `molt_class_apply_set_name` | 1 | Low | |
| `molt_class_layout_version` | 1 | Low | |
| `molt_class_set_layout_version` | 1 | Low | |
| `molt_super_new` | 1 | Low | |
| `molt_classmethod_new` | 1 | Low | |
| `molt_staticmethod_new` | 1 | Low | |
| `molt_property_new` | 1 | Low | |
| `molt_dataclass_new` | 1 | Low | |
| `molt_dataclass_get` | 1 | Moderate | |
| `molt_dataclass_set` | 1 | Moderate | |
| `molt_dataclass_set_class` | 1 | Low | |

#### Attribute Access (13 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_get_attr_ptr` | 1 | **Yes** | Field access by known offset |
| `molt_get_attr_object_ic` | 1 | **Yes** | IC-based attribute lookup |
| `molt_get_attr_special` | 1 | Moderate | `__xxx__` attributes |
| `molt_get_attr_name` | 1 | Yes | Dynamic attribute by name |
| `molt_get_attr_name_default` | 1 | Moderate | `getattr(obj, name, default)` |
| `molt_has_attr_name` | 1 | Low | `hasattr()` |
| `molt_set_attr_name` | 1 | Moderate | `setattr()` |
| `molt_set_attr_ptr` | 1 | Yes | Field store by known offset |
| `molt_set_attr_object` | 1 | Moderate | |
| `molt_del_attr_ptr` | 1 | Low | |
| `molt_del_attr_object` | 1 | Low | |
| `molt_del_attr_name` | 1 | Low | |
| `molt_guard_type` | 1 | Yes | Type guard for field access |
| `molt_guard_layout_ptr` | 1 | Yes | Layout guard for IC |
| `molt_guarded_field_get_ptr` | 1 | Yes | Guarded field load |
| `molt_guarded_field_set_ptr` | 1 | Moderate | |
| `molt_guarded_field_init_ptr` | 1 | Moderate | |

#### Module System (12 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_module_new` | 1 | Low | |
| `molt_module_cache_get` | 1 | Low | |
| `molt_module_cache_set` | 1 | Low | |
| `molt_module_cache_del` | 1 | Low | |
| `molt_module_import` | 1 | Low | |
| `molt_module_import_star` | 1 | Low | |
| `molt_module_get_attr` | 1 | Low | |
| `molt_module_get_global` | 1 | Moderate | |
| `molt_module_del_global` | 1 | Low | |
| `molt_module_get_name` | 1 | Low | |
| `molt_module_set_attr` | 1 | Low | |
| `molt_missing` | 2 | Low | Sentinel for missing values |

#### Context Managers (6 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_context_null` | 1 | Low | |
| `molt_context_enter` | 1 | Moderate | |
| `molt_context_exit` | 1 | Moderate | |
| `molt_context_closing` | 1 | Low | |
| `molt_context_unwind` | 1 | Low | |
| `molt_context_depth` | 1 | Low | |
| `molt_context_unwind_to` | 1 | Low | |

#### Async/Concurrency (28 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_task_new` | 5 | Moderate | |
| `molt_block_on` | 1 | Low | |
| `molt_future_poll` | 1 | Moderate | |
| `molt_sleep_register` | 1 | Low | |
| `molt_async_sleep_new` | 1 | Low | |
| `molt_chan_new` | 1 | Low | |
| `molt_chan_send` | 1 | Low | |
| `molt_chan_recv` | 1 | Low | |
| `molt_chan_drop` | 1 | Low | |
| `molt_spawn` | 1 | Low | |
| `molt_cancel_token_new` | 1 | Low | |
| `molt_cancel_token_clone` | 1 | Low | |
| `molt_cancel_token_drop` | 1 | Low | |
| `molt_cancel_token_cancel` | 1 | Low | |
| `molt_cancel_token_get_current` | 3 | Low | |
| `molt_cancel_token_set_current` | 1 | Low | |
| `molt_cancel_token_is_cancelled` | 1 | Low | |
| `molt_cancelled` | 1 | Low | |
| `molt_cancel_current` | 1 | Low | |
| `molt_future_cancel` | 1 | Low | |
| `molt_future_cancel_msg` | 1 | Low | |
| `molt_future_cancel_clear` | 1 | Low | |
| `molt_promise_new` | 1 | Low | |
| `molt_promise_set_result` | 1 | Low | |
| `molt_promise_set_exception` | 1 | Low | |
| `molt_thread_submit` | 1 | Low | |
| `molt_task_register_token_owned` | 3 | Low | |
| `molt_is_native_awaitable` | 1 | Low | |

#### Generator/Coroutine (7 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_asyncgen_new` | 2 | Low | |
| `molt_asyncgen_shutdown` | 1 | Low | |
| `molt_asyncgen_locals_register` | 1 | Low | |
| `molt_gen_locals_register` | 1 | Low | |
| `molt_generator_send` | 1 | Moderate | |
| `molt_generator_throw` | 1 | Low | |
| `molt_generator_close` | 1 | Low | |

#### Tracing/Debugging/Profiling (8 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_debug_trace` | 1 | Low | |
| `molt_trace_enter` | 2 | Low | |
| `molt_trace_enter_slot` | 2 | Low | |
| `molt_trace_exit` | 4 | Low | |
| `molt_trace_set_line` | 1 | Low | |
| `molt_frame_locals_set` | 1 | Low | |
| `molt_profile_enabled` | 1 | Low | |
| `molt_profile_struct_field_store` | 1 | Low | |
| `molt_recursion_guard_enter` | 3 | Moderate | |
| `molt_recursion_guard_exit` | 3 | Moderate | |

#### Vectorized Operations (20 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_vec_sum_int` | 1 | Moderate | Fused sum for int lists |
| `molt_vec_sum_int_trusted` | 1 | Moderate | |
| `molt_vec_sum_int_range` | 1 | Moderate | |
| `molt_vec_sum_int_range_trusted` | 1 | Moderate | |
| `molt_vec_sum_int_range_iter` | 1 | Moderate | |
| `molt_vec_sum_int_range_iter_trusted` | 1 | Moderate | |
| `molt_vec_sum_float` | 1 | Moderate | |
| `molt_vec_sum_float_trusted` | 1 | Moderate | |
| `molt_vec_sum_float_range` | 1 | Low | |
| `molt_vec_sum_float_range_trusted` | 1 | Low | |
| `molt_vec_sum_float_range_iter` | 1 | Low | |
| `molt_vec_sum_float_range_iter_trusted` | 1 | Low | |
| `molt_vec_prod_int` | 1 | Low | |
| `molt_vec_prod_int_trusted` | 1 | Low | |
| `molt_vec_prod_int_range` | 1 | Low | |
| `molt_vec_prod_int_range_trusted` | 1 | Low | |
| `molt_vec_min_int` | 1 | Low | |
| `molt_vec_min_int_trusted` | 1 | Low | |
| `molt_vec_min_int_range` | 1 | Low | |
| `molt_vec_min_int_range_trusted` | 1 | Low | |
| `molt_vec_max_int` | 1 | Low | |
| `molt_vec_max_int_trusted` | 1 | Low | |
| `molt_vec_max_int_range` | 1 | Low | |
| `molt_vec_max_int_range_trusted` | 1 | Low | |

#### Serialization (7 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_json_parse_scalar` | 1 | Low | |
| `molt_json_parse_scalar_obj` | 2 | Low | |
| `molt_msgpack_parse_scalar` | 1 | Low | |
| `molt_msgpack_parse_scalar_obj` | 2 | Low | |
| `molt_cbor_parse_scalar` | 1 | Low | |
| `molt_cbor_parse_scalar_obj` | 2 | Low | |
| `molt_taq_ingest_line` | 1 | Low | |

#### Miscellaneous (16 functions)

| Function | Decl count | Hot? | Notes |
|----------|-----------|------|-------|
| `molt_not_implemented` | 1 | Low | |
| `molt_ellipsis` | 1 | Low | |
| `molt_getargv` | 1 | Low | `sys.argv` |
| `molt_getframe` | 1 | Low | `sys._getframe()` |
| `molt_sys_executable` | 1 | Low | |
| `molt_bridge_unavailable` | 1 | Low | |
| `molt_memoryview_new` | 1 | Low | |
| `molt_memoryview_tobytes` | 1 | Low | |
| `molt_memoryview_cast` | 1 | Low | |
| `molt_intarray_from_seq` | 1 | Low | |
| `molt_buffer2d_new` | 1 | Low | |
| `molt_buffer2d_get` | 1 | Low | |
| `molt_buffer2d_set` | 1 | Low | |
| `molt_buffer2d_matmul` | 1 | Low | |
| `molt_statistics_mean_slice` | 1 | Low | |
| `molt_statistics_stdev_slice` | 1 | Low | |

### Top-20 Hottest Functions (Inlining Priority Order)

These are the functions called in tight loops, inside `fast_int` paths, or per-iteration in common patterns. Ordered by estimated per-iteration call frequency in a typical compute-heavy program:

| Rank | Function | Category | Calls/iteration | Why hot |
|------|----------|----------|----------------|---------|
| 1 | `molt_dec_ref_obj` | RC | 2-5 | Every variable death, scope exit, reassignment |
| 2 | `molt_inc_ref_obj` | RC | 2-5 | Every ownership transfer, function arg pass |
| 3 | `molt_is_truthy` | Type check | 1-2 | Every `if`/`while`/`and`/`or` condition |
| 4 | `molt_iter_next` | Iterator | 1 | Every `for` loop iteration |
| 5 | `molt_exception_pending_fast` | Exception | 1-3 | After every fallible op in try blocks |
| 6 | `molt_add` | Arithmetic | 0-1 | Slow path of accumulator patterns |
| 7 | `molt_index` | Container | 0-1 | Every `list[i]`, `dict[k]` |
| 8 | `molt_store_index` | Container | 0-1 | Every `list[i] = v`, `dict[k] = v` |
| 9 | `molt_len` | Container | 0-1 | `len()` in conditions and bounds |
| 10 | `molt_callargs_new` | Call dispatch | 0-1 | Every non-direct function call |
| 11 | `molt_callargs_push_pos` | Call dispatch | 0-1 | Per argument in non-direct calls |
| 12 | `molt_call_bind_ic` | Call dispatch | 0-1 | Every dynamic method dispatch |
| 13 | `molt_list_append` | Container | 0-1 | List-building loops |
| 14 | `molt_dict_get` | Container | 0-1 | Dict lookups in loops |
| 15 | `molt_dict_set` | Container | 0-1 | Dict stores in loops |
| 16 | `molt_lt`/`le`/`gt`/`ge`/`eq` | Comparison | 0-1 | Slow path of comparisons |
| 17 | `molt_get_attr_ptr` | Attribute | 0-1 | Field access on known types |
| 18 | `molt_get_attr_object_ic` | Attribute | 0-1 | IC-based attribute lookup |
| 19 | `molt_sub`/`mul` | Arithmetic | 0-1 | Slow path of arithmetic |
| 20 | `molt_contains` | Container | 0-1 | `x in collection` |

## 2. Current State: What Is Already Inlined

The backend already performs *selective* inline Cranelift IR emission for specific operations:

### 2.1 Fused Tag-Check-and-Unbox (Arithmetic Fast Path)

For typed arithmetic (`add`, `sub`, `mul`, `floordiv`, `mod`, bitwise ops), the backend emits:

```
    xor val, (QNAN | TAG_INT)       -- fused_tag_check_and_unbox_int
    bor lhs_xored, rhs_xored        -- fused_both_int_check
    ushr combined, 47
    icmp_eq upper, 0                 -- true iff both operands are NaN-boxed ints
    brif both_int, fast_block, slow_block

fast_block:
    iadd lhs_val, rhs_val            -- native integer arithmetic
    box_int_value result             -- re-tag as NaN-boxed int
    int_value_fits_inline check      -- guard against 47-bit overflow
    brif fits, merge_block, slow_block

slow_block:
    call molt_add(lhs, rhs)          -- FFI fallback for non-int types / overflow
```

This eliminates the FFI call for the common int+int case. The pattern is already proven and covers `add`, `sub`, `mul`, `floordiv`, `mod`, `bit_or`, `bit_and`, `bit_xor`, `lshift`, `rshift`, and their in-place variants.

### 2.2 NaN-Boxing Encode/Decode

Box/unbox operations are already emitted as inline Cranelift IR:
- `box_int_value`: `band(val, INT_MASK) | bor(QNAN | TAG_INT)`
- `unbox_int`: `ishl(val, 17) >> sshr(17)` (sign-extending 47-bit payload)
- `box_float_value`: `bitcast` (floats are stored as raw IEEE 754 bits)
- `box_bool_value`: `select(val, 1, 0) | bor(QNAN | TAG_BOOL)`
- `box_none`: constant `QNAN | TAG_NONE`
- `unbox_ptr_value`: `band(val, POINTER_MASK) | ishl(16)` (pointer recovery)

### 2.3 TIR-Level Function Inlining

The `inline_functions()` pass inlines small user-defined functions (up to `INLINE_OP_LIMIT=30` ops) at the IR level before Cranelift codegen. This operates on `OpIR` structures, not on runtime intrinsics.

### 2.4 What Is NOT Inlined (the Gap)

All runtime functions listed in section 1 cross the FFI boundary as opaque `call` instructions to Cranelift. The key insight is that many of these operations have trivial fast paths that could be emitted as Cranelift IR, with FFI fallback only for rare/complex cases.

## 3. Architecture: NaN-Boxing and Object Layout

Understanding the inlining strategy requires understanding the value representation.

### 3.1 NaN-Boxing Scheme

Every Molt value is a 64-bit `u64` using NaN-boxing:

```
Bits 63-52: IEEE 754 quiet NaN prefix (0x7ff8)
Bits 51-48: Tag field (TAG_MASK = 0x0007_0000_0000_0000)
Bits 47-0:  Payload (POINTER_MASK = 0x0000_FFFF_FFFF_FFFF)

Tags:
  TAG_INT     = 0x0001  -- 47-bit signed integer in payload
  TAG_BOOL    = 0x0002  -- 0 or 1 in payload
  TAG_NONE    = 0x0003  -- no payload
  TAG_PTR     = 0x0004  -- 48-bit pointer in payload
  TAG_PENDING = 0x0005  -- async pending sentinel

Floats: stored as raw f64 bits (no tag -- NaN-boxed values are distinguished
        by having the quiet NaN prefix; real floats never have the exact QNAN
        pattern because signaling NaNs are canonicalized)
```

**Key property for inlining**: Tag checks are 3 Cranelift instructions (`band`, `iconst`, `icmp`). For inline ints, the tag check *and* unbox can be fused into 2 instructions (`bxor`, `ishl+sshr`).

### 3.2 Heap Object Layout

Heap-allocated objects (strings, lists, dicts, user objects) are accessed via `TAG_PTR`:

```
                  MoltHeader (40 bytes, placed BEFORE the data pointer)
                  ┌────────────────────────────────────────┐
ptr - 40 bytes -> │ type_id:    u32  (offset -40)          │
                  │ ref_count:  AtomicU32  (offset -36)    │
                  │ poll_fn:    u64  (offset -32)          │
                  │ state:      i64  (offset -24)          │
                  │ size:       usize (offset -16)         │
                  │ flags:      u64  (offset -8)           │
                  ├────────────────────────────────────────┤
ptr ------------> │ payload data...                        │
                  └────────────────────────────────────────┘
```

Header constants (from backend):
- `HEADER_SIZE_BYTES = 40`
- `HEADER_STATE_OFFSET = -(40 - 16) = -24`
- `type_id` at `ptr - 40`
- `ref_count` at `ptr - 36`
- `flags` at `ptr - 8`
- `HEADER_FLAG_IMMORTAL = 1 << 15`

## 4. Design: Cranelift IR Templates for Runtime Operations

The core idea: for each runtime function, define the **fast path** as a sequence of Cranelift IR instructions emitted directly into the function body, with a **cold slow-path block** that falls back to the FFI call. This is exactly the pattern already used for arithmetic -- we generalize it.

### 4.1 Template Structure

Each inlineable operation follows this pattern:

```
  ; --- inline fast path ---
  <tag check on input(s)>
  brif tag_ok, fast_block, slow_block

fast_block:
  <inline implementation>
  jump merge_block(fast_result)

slow_block:     [cold]
  call molt_xxx(args...)         ; FFI fallback
  jump merge_block(slow_result)

merge_block:
  result = block_param
```

### 4.2 GIL Elimination

A critical optimization enabled by inlining: **GIL re-acquisition elimination**.

Every `extern "C" fn molt_xxx` acquires the GIL via `with_gil_entry!` which on native targets:
1. Reads TLS `GIL_DEPTH` (`RefCell<u32>`)
2. Increments depth
3. If depth was 0, acquires the mutex
4. On drop, decrements depth, releases mutex if depth reaches 0

Since the compiled function body already holds the GIL (acquired at function entry via the trampoline), every re-acquisition is pure overhead: a TLS read + branch + TLS write on the entry path, and the same on the exit path. For `molt_inc_ref_obj` / `molt_dec_ref_obj` which are called dozens of times per function, this dominates.

**When we inline the fast path, we eliminate GIL re-acquisition entirely** because the inlined code runs within the already-GIL-holding context. The FFI slow-path fallback still acquires the GIL, but it is taken rarely.

## 5. Phased Approach

### Phase 1: Reference Counting Operations (Highest Impact)

**Target functions**: `molt_inc_ref_obj`, `molt_dec_ref_obj`, `molt_inc_ref`, `molt_dec_ref`

**Why first**: These are the most frequently called runtime functions. Every variable assignment, scope exit, and ownership transfer emits at least one refcount call. A typical function body has 10-30 refcount FFI calls. Each costs ~15-25ns (GIL TLS check + pointer extraction + null check + header load + atomic increment + GIL TLS restore).

#### 5.1.1 `molt_inc_ref_obj(bits: u64)` Inline Template

The current Rust implementation:

```rust
pub extern "C" fn molt_inc_ref_obj(bits: u64) {
    with_gil_entry!(_py, {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            molt_inc_ref(ptr);
        }
    })
}

pub unsafe fn inc_ref_ptr(_py: &PyToken, ptr: *mut u8) {
    if ptr.is_null() { return; }
    let header_ptr = ptr.sub(sizeof(MoltHeader)) as *mut MoltHeader;
    if ((*header_ptr).flags & HEADER_FLAG_IMMORTAL) != 0 { return; }
    (*header_ptr).ref_count.fetch_add(1, Relaxed);
}
```

Cranelift IR inline template:

```
inc_ref_obj_inline(bits):
  ; Check if this is a pointer-tagged value (only pointers need refcounting)
  tag = band(bits, QNAN | TAG_MASK)
  is_ptr = icmp_eq(tag, QNAN | TAG_PTR)
  brif is_ptr, ptr_block, done_block

ptr_block:
  ; Extract pointer from NaN-boxed value
  ptr = unbox_ptr_value(bits)     ; band + ishl
  ; Check null (defensive)
  is_null = icmp_eq(ptr, 0)
  brif is_null, done_block, live_block

live_block:
  ; Load flags from header (ptr - 8 on the flags field)
  flags = load.i64(ptr - 8)       ; MoltHeader.flags offset
  immortal = band(flags, HEADER_FLAG_IMMORTAL)
  is_immortal = icmp_ne(immortal, 0)
  brif is_immortal, done_block, inc_block

inc_block:
  ; Atomic increment of ref_count (ptr - 36, AtomicU32)
  ref_count_addr = iadd(ptr, -36)
  atomic_rmw.i32 add ref_count_addr, 1   ; Cranelift atomic_rmw
  jump done_block

done_block:
  ; (no return value)
```

**Instruction count**: ~12 Cranelift instructions vs. 1 FFI call + ~20 instructions inside the runtime (plus GIL overhead).

**Code size trade-off**: Each inline site adds ~48-64 bytes of machine code (vs. ~8 bytes for a `call` instruction). With 30+ refcount sites per function, this adds ~1.5-2 KB per compiled function. Acceptable given the performance benefit, and cold blocks (immortal check, null check) can be outlined.

#### 5.1.2 `molt_dec_ref_obj(bits: u64)` Inline Template

Decrement is more complex because reaching zero triggers deallocation. The fast path handles the common case (decrement, count stays above 1); the slow path (count reaches 1) falls through to FFI.

```
dec_ref_obj_inline(bits):
  tag = band(bits, QNAN | TAG_MASK)
  is_ptr = icmp_eq(tag, QNAN | TAG_PTR)
  brif is_ptr, ptr_block, done_block

ptr_block:
  ptr = unbox_ptr_value(bits)
  is_null = icmp_eq(ptr, 0)
  brif is_null, done_block, live_block

live_block:
  flags = load.i64(ptr - 8)
  immortal = band(flags, HEADER_FLAG_IMMORTAL)
  is_immortal = icmp_ne(immortal, 0)
  brif is_immortal, done_block, dec_block

dec_block:
  ref_count_addr = iadd(ptr, -36)
  prev = atomic_rmw.i32 sub ref_count_addr, 1   ; returns previous value
  was_one = icmp_eq(prev, 1)
  brif was_one, dealloc_slow_block, done_block   [cold: dealloc_slow_block]

dealloc_slow_block:
  ; Undo the decrement (restore to 1) so the FFI path can handle deallocation
  ; correctly including __del__ / weak references / exception rooting
  atomic_rmw.i32 add ref_count_addr, 1
  call molt_dec_ref_obj(bits)     ; FFI handles full deallocation
  jump done_block

done_block:
```

**Key insight**: The common case (refcount > 1 after decrement) is fully inlined. Deallocation (refcount hits zero) is rare and stays in the FFI slow path. The undo-and-retry pattern ensures the FFI path sees the object in a consistent state.

**Alternative (simpler but equally correct)**: Instead of undo-and-retry, we can split the FFI path into `molt_dec_ref_obj_free(ptr)` that skips the tag-check and GIL entry, taking only the raw pointer. This avoids the extra atomic add and re-entry overhead. Requires adding a new, targeted runtime export.

#### 5.1.3 Optimization: Eliding Refcount Operations

Once refcount ops are inlined, the Cranelift optimizer can:

1. **Eliminate paired inc/dec**: `inc_ref_obj(x); ... dec_ref_obj(x)` where `x` is provably alive through the region can be eliminated entirely. This requires a custom Cranelift egraph rule or a pre-emission pass on OpIR.

2. **Batch refcount updates**: Multiple `inc_ref_obj(x)` on the same value can be coalesced into a single `atomic_rmw add N`.

3. **Skip refcount for inline values**: If the tag check proves the value is an int/bool/none/float (not a pointer), the entire refcount operation is dead code. With the inlined template, Cranelift's DCE can eliminate this automatically when the tag is known from prior type inference.

### Phase 2: Type Checks and Truthiness (Medium Impact)

**Target functions**: `molt_is_truthy`, `molt_is`, `molt_not`, `molt_isinstance`, `molt_type_of`

#### 5.2.1 `molt_is_truthy(val: u64) -> i64` Inline Template

The Rust implementation dispatches on type:

```rust
fn is_truthy(obj: MoltObject) -> bool {
    if obj.is_none()  { return false; }
    if let Some(b) = obj.as_bool()  { return b; }
    if let Some(i) = to_i64(obj)    { return i != 0; }
    if let Some(f) = obj.as_float() { return f != 0.0; }
    // ... pointer types: check string_len, list_len, dict_len, etc.
}
```

Cranelift IR inline template:

```
is_truthy_inline(val) -> i64:
  ; Fast path: handle inline types (none, bool, int, float) without FFI
  tag = band(val, QNAN | TAG_MASK)

  ; None check
  is_none = icmp_eq(tag, QNAN | TAG_NONE)
  brif is_none, false_block, check_bool_block

check_bool_block:
  is_bool = icmp_eq(tag, QNAN | TAG_BOOL)
  brif is_bool, bool_block, check_int_block

bool_block:
  bool_val = band(val, 1)         ; extract bool payload
  jump merge_block(bool_val)

check_int_block:
  is_int = icmp_eq(tag, QNAN | TAG_INT)
  brif is_int, int_block, check_float_block

int_block:
  int_val = unbox_int(val)
  nonzero = icmp_ne(int_val, 0)
  int_result = bint(nonzero)      ; convert flag to i64
  jump merge_block(int_result)

check_float_block:
  ; If not QNAN-tagged, it might be a float. Check by verifying it's NOT
  ; any QNAN-tagged value (the upper 13 bits must not be 0x7FF8..0x7FFF)
  upper = ushr(val, 51)
  is_qnan_prefix = icmp_eq(upper, 0x1FFF)  ; 0x7FF8 >> 3
  ; ... simplified: if no QNAN prefix, it's a float
  brif is_float, float_block, slow_block

float_block:
  fval = bitcast.f64(val)
  fzero = f64const(0.0)
  f_nonzero = fcmp(ne, fval, fzero)
  f_result = bint(f_nonzero)
  jump merge_block(f_result)

slow_block:    [cold]
  call_result = call molt_is_truthy(val)
  jump merge_block(call_result)

merge_block:
  result = block_param
```

**Impact**: `is_truthy` is called in every conditional branch. For a `while` loop with an integer condition, the inline path is ~6 instructions vs. ~30+ instructions through FFI (including GIL). For programs with many conditionals (sorting, searching, etc.), this alone can yield 5-10% improvement.

#### 5.2.2 `molt_is(a, b) -> u64` Inline Template

Identity comparison is trivial: compare the raw bits.

```
is_inline(a, b) -> u64:
  eq = icmp_eq(a, b)
  result = select(eq, BOX_TRUE, BOX_FALSE)
```

Two instructions. Currently an FFI call.

#### 5.2.3 `molt_not(val) -> u64` Inline Template

Compose `is_truthy_inline` + negate:

```
not_inline(val) -> u64:
  truthy = is_truthy_inline(val)
  is_zero = icmp_eq(truthy, 0)
  result = select(is_zero, BOX_TRUE, BOX_FALSE)
```

### Phase 3: Container Fast Paths (Targeted Impact)

**Target functions**: `molt_len`, `molt_index`, `molt_list_append`, `molt_dict_get`, `molt_iter_next`

These are more complex because they involve pointer dereferences into heap objects, requiring knowledge of the internal layout. The strategy is to inline only the common-case fast path for the most frequent container type.

#### 5.3.1 `molt_len(val) -> u64` Inline Template (List Fast Path)

```
len_inline(val) -> u64:
  tag = band(val, QNAN | TAG_MASK)
  is_ptr = icmp_eq(tag, QNAN | TAG_PTR)
  brif is_ptr, ptr_block, slow_block

ptr_block:
  ptr = unbox_ptr_value(val)
  type_id = load.u32(ptr - 40)    ; MoltHeader.type_id

  ; List fast path (most common for len())
  is_list = icmp_eq(type_id, TYPE_ID_LIST)     ; 201
  brif is_list, list_block, check_tuple_block

list_block:
  ; List layout: ptr -> [len: usize, capacity: usize, data_ptr: *mut u64]
  list_len = load.i64(ptr + LIST_LEN_OFFSET)
  result = box_int_value(list_len)
  jump merge_block(result)

check_tuple_block:
  is_tuple = icmp_eq(type_id, TYPE_ID_TUPLE)   ; 206
  brif is_tuple, tuple_block, slow_block

tuple_block:
  tuple_len = load.i64(ptr + TUPLE_LEN_OFFSET)
  result = box_int_value(tuple_len)
  jump merge_block(result)

slow_block:    [cold]
  call_result = call molt_len(val)
  jump merge_block(call_result)

merge_block:
  result = block_param
```

#### 5.3.2 `molt_index(obj, key) -> u64` Inline Template (List[int] Fast Path)

```
index_inline(obj_bits, key_bits) -> u64:
  ; Check: obj is a list pointer
  obj_tag = band(obj_bits, QNAN | TAG_MASK)
  is_ptr = icmp_eq(obj_tag, QNAN | TAG_PTR)
  brif is_ptr, check_type_block, slow_block

check_type_block:
  ptr = unbox_ptr_value(obj_bits)
  type_id = load.u32(ptr - 40)
  is_list = icmp_eq(type_id, TYPE_ID_LIST)
  brif is_list, check_key_block, slow_block

check_key_block:
  ; Check: key is an inline int
  (key_xored, key_val) = fused_tag_check_and_unbox_int(key_bits)
  key_upper = ushr(key_xored, 47)
  is_int_key = icmp_eq(key_upper, 0)
  brif is_int_key, bounds_check_block, slow_block

bounds_check_block:
  list_len = load.i64(ptr + LIST_LEN_OFFSET)
  ; Handle negative indices
  is_negative = icmp_slt(key_val, 0)
  adjusted_key = select(is_negative, iadd(key_val, list_len), key_val)
  ; Bounds check
  in_bounds = icmp_ult(adjusted_key, list_len)
  brif in_bounds, load_block, slow_block   ; slow_block handles IndexError

load_block:
  data_ptr = load.i64(ptr + LIST_DATA_PTR_OFFSET)
  elem_addr = iadd(data_ptr, imul(adjusted_key, 8))
  elem_bits = load.i64(elem_addr)
  ; inc_ref the loaded element (caller takes ownership)
  ; Use inline inc_ref from Phase 1
  inc_ref_obj_inline(elem_bits)
  jump merge_block(elem_bits)

slow_block:    [cold]
  call_result = call molt_index(obj_bits, key_bits)
  jump merge_block(call_result)

merge_block:
  result = block_param
```

#### 5.3.3 `molt_list_append(list, val) -> u64` Inline Template

```
list_append_inline(list_bits, val_bits):
  ptr = unbox_ptr_value(list_bits)        ; assume ptr-tagged (guarded by type info)
  type_id = load.u32(ptr - 40)
  is_list = icmp_eq(type_id, TYPE_ID_LIST)
  brif is_list, fast_block, slow_block

fast_block:
  len = load.i64(ptr + LIST_LEN_OFFSET)
  cap = load.i64(ptr + LIST_CAP_OFFSET)
  has_space = icmp_ult(len, cap)
  brif has_space, append_block, slow_block   ; realloc goes to slow path

append_block:
  data_ptr = load.i64(ptr + LIST_DATA_PTR_OFFSET)
  elem_addr = iadd(data_ptr, imul(len, 8))
  store.i64 val_bits, elem_addr
  new_len = iadd(len, 1)
  store.i64 new_len, (ptr + LIST_LEN_OFFSET)
  inc_ref_obj_inline(val_bits)
  result = box_none()
  jump done

slow_block:
  call molt_list_append(list_bits, val_bits)
  result = box_none()
  jump done
```

## 6. Interaction with Existing Optimizations

### 6.1 Fused Tag-Check-and-Unbox

The existing `fused_tag_check_and_unbox_int` pattern (used in arithmetic) generalizes cleanly. Phase 2's `is_truthy_inline` and Phase 3's index/len operations use the same XOR-based tag check. When the type system has already proven a value is `int`, the tag check is dead code and Cranelift eliminates it.

### 6.2 TIR Type Information

The TIR carries type annotations (`param_types`, `fast_int`, `fast_float`, `raw_int` flags on `OpIR`). These annotations should be extended to propagate container types:

```json
{
  "kind": "index",
  "args": ["list_var", "i"],
  "out": "elem",
  "container_type": "list",      // NEW: enables list-specific fast path
  "key_type": "int"              // NEW: enables int-key fast path
}
```

When `container_type` and `key_type` are known, the inliner can skip the type-check branches entirely, emitting only the fast path + a minimal guard.

### 6.3 Inline Cache (IC) Sites

The `molt_call_bind_ic` system uses call-site IDs (FNV hashes of `func_name + op_idx + lane`) for thread-local IC lookup. Inlined operations do not conflict with IC sites because they replace the call entirely rather than caching dispatch targets. However, the IC system provides a useful signal: high-miss-rate IC sites indicate polymorphic call targets where inlining the fast path (monomorphic type check + inline body + cold polymorphic fallback) would be most beneficial.

### 6.4 Egraph Simplification

The `egraph_simplify` module (feature-gated) can optimize Cranelift IR after emission. Inlined refcount operations create opportunities for the egraph:
- Adjacent `inc_ref_obj(x) ... dec_ref_obj(x)` can be recognized as identity.
- Tag checks on the same value across operations can be CSE'd.
- Branch conditions that are provably true/false from prior checks can be folded.

## 7. Code Size Analysis

### Per-Operation Code Size Estimates

| Operation | Inline bytes | FFI call bytes | Ratio | Sites/function (typical) |
|-----------|-------------|----------------|-------|--------------------------|
| `inc_ref_obj` | ~48 | ~8 | 6x | 10-20 |
| `dec_ref_obj` | ~64 | ~8 | 8x | 10-20 |
| `is_truthy` | ~56 | ~8 | 7x | 2-8 |
| `is` | ~16 | ~8 | 2x | 0-2 |
| `not` | ~64 | ~8 | 8x | 0-4 |
| `len` (list+tuple) | ~56 | ~8 | 7x | 0-2 |
| `index` (list[int]) | ~96 | ~8 | 12x | 0-4 |
| `list_append` | ~80 | ~8 | 10x | 0-2 |

### Function-Level Impact

A typical compiled function with 15 RC ops, 4 truthiness checks, and 2 index operations:

- **Before**: ~21 FFI calls * 8 bytes = 168 bytes of call-site code
- **After**: ~15 * 56 + 4 * 56 + 2 * 96 = 1,256 bytes of inline code
- **Net increase**: ~1,088 bytes per function (~1 KB)

For a compiled program with 100 functions, this adds ~100 KB of code. Modern L1 instruction caches are 32-64 KB, so the impact depends on working set. However, the hot-path instructions (the common-case branches) are compact and sequential, while the cold slow-path blocks are outlined and rarely touched. Net effect on I-cache is likely positive for hot loops.

### Mitigation Strategies

1. **Selective inlining**: Only inline in functions marked as hot (via PGO profile, `hot_functions` field in `PgoProfileIR`). Cold functions keep FFI calls.

2. **Shared slow-path blocks**: Multiple inline sites in the same function can share a single slow-path block per runtime function (requires block parameter threading but saves code).

3. **Outlined fast-path helpers**: For Phase 3 operations, emit the inline fast path as a local function within the Cranelift module (`Linkage::Local`) and call it, letting Cranelift's own inliner decide whether to inline based on its size heuristics.

4. **Threshold gating**: Environment variable `MOLT_INLINE_INTRINSICS=0|1|2|3` to control phase level. `0` = off (current behavior), `1` = RC only, `2` = RC + type checks, `3` = all.

## 8. Implementation Plan

### Phase 1: RC Inlining (Weeks 1-2)

1. **Add layout constants to backend**: Define `HEADER_TYPE_ID_OFFSET`, `HEADER_REFCOUNT_OFFSET`, `HEADER_FLAGS_OFFSET` as constants derived from `MoltHeader` layout.

2. **Implement `emit_inc_ref_obj_inline()`**: New function in `lib.rs` that emits the Cranelift IR template from section 5.1.1. Takes `FunctionBuilder`, `Value` (the bits), returns nothing.

3. **Implement `emit_dec_ref_obj_inline()`**: New function with the template from section 5.1.2. The slow-path block calls `molt_dec_ref_obj` via FFI.

4. **Replace `emit_maybe_ref_adjust()`**: The current function unconditionally calls `local_inc_ref_obj`. Replace with a conditional: if `MOLT_INLINE_INTRINSICS >= 1`, use the inline template; otherwise, keep the FFI call.

5. **Replace all `local_dec_ref_obj` and `local_inc_ref_obj` call sites**: There are 73+ sites in lib.rs. Replace each `builder.ins().call(local_dec_ref_obj, &[val])` with `emit_dec_ref_obj_inline(&mut builder, val, local_dec_ref_obj)`.

6. **Add `molt_dec_ref_obj_dealloc(ptr: *mut u8)` to runtime**: New export that skips tag extraction and GIL acquisition (assumes caller already holds GIL and provides raw pointer). This is the optimal slow-path target for the inline dec_ref template.

7. **Testing**: Run full differential test suite. Verify no RC correctness regressions. Add a specific test for immortal objects, null pointers, and refcount-reaching-zero paths.

8. **Benchmarking**: Measure on `bench/` suite with `MOLT_INLINE_INTRINSICS=1` vs. `=0`. Target: 8-15% improvement on call-heavy benchmarks (fibonacci, string processing, list manipulation).

### Phase 2: Type Check Inlining (Weeks 3-4)

1. **Implement `emit_is_truthy_inline()`**: Template from section 5.2.1.

2. **Replace `is_truthy` call sites**: In `"and"`, `"or"`, `"if"`, `"while"`, `"not"`, `"is_truthy"` op handlers.

3. **Implement `emit_is_inline()`**: Trivial 2-instruction template.

4. **Implement `emit_not_inline()`**: Compose `is_truthy_inline` + negate.

5. **Testing + benchmarking**: Target: additional 5-10% on conditional-heavy code.

### Phase 3: Container Fast Paths (Weeks 5-8)

1. **Define container layout constants**: List, tuple, dict internal offsets. These must be kept in sync with `molt-runtime` layout (use `build.rs` to generate shared constants or add compile-time assertions).

2. **Implement `emit_len_inline()`**: List + tuple fast paths.

3. **Implement `emit_index_inline()`**: List[int] fast path.

4. **Implement `emit_list_append_inline()`**: In-capacity fast path.

5. **Extend OpIR type annotations**: Add `container_type`, `key_type` fields.

6. **Layout synchronization**: Add a `runtime/molt-runtime/src/object/layout_constants.rs` that exports all struct offsets as `pub const`. Import in `molt-backend` via a shared crate or build-time code generation.

7. **Testing + benchmarking**: Target: additional 5-10% on container-heavy code.

## 9. Risks and Mitigations

### 9.1 Layout Coupling

Inlining assumes specific memory layouts for `MoltHeader`, list, tuple, etc. If the runtime changes these layouts, the inlined code silently corrupts memory.

**Mitigation**: Compile-time layout assertions. Add a `layout_check` function exported from the runtime that returns a hash of all struct layouts. The backend calls this at initialization and panics if the hash doesn't match the one it was compiled against. Additionally, share layout constants via a common crate or `build.rs` code generation rather than duplicating magic numbers.

### 9.2 Atomic Ordering Correctness

The inlined `atomic_rmw` uses Cranelift's `AtomicRmwOp::Add` with appropriate memory ordering. The runtime uses `Ordering::Relaxed` for increments and `Ordering::AcqRel` for decrements. The Cranelift IR must match these orderings exactly.

**Mitigation**: Use `MemFlags::new().with_atomic()` for atomic loads/stores in Cranelift. Verify ordering by inspection against the Rust source. Add a test that increments/decrements across threads and checks for count consistency.

### 9.3 WASM Target Differences

On `wasm32`, refcounts use `Cell<u32>` (non-atomic) and the GIL is a no-op. The inlined templates must be target-aware:
- On native: emit `atomic_rmw`
- On wasm32: emit plain `load` + `iadd` + `store`

**Mitigation**: Conditional emission based on `self.module.isa().triple().architecture`. The backend already has target-specific paths for WASM codegen.

### 9.4 Debug Tracing

The runtime has debug-mode tracing (`debug_rc_object()`, `debug_file_rc()`) that logs refcount operations. Inlined operations bypass this.

**Mitigation**: When `MOLT_DEBUG_RC=1`, disable inline RC and fall back to FFI calls. Check the env var once at codegen time (not at runtime) and emit FFI calls if debug mode is requested.

### 9.5 Cranelift Compile Time

More IR per function means slower Cranelift compilation.

**Mitigation**: The `--profile dev` path uses `dev-fast` (opt-level 1, 256 codegen units). Cranelift's compilation cost scales roughly linearly with IR size. Adding ~1 KB of IR per function (a ~20-30% increase in typical function IR size) should add ~20-30% to Cranelift compilation time. For `--profile release`, the compilation cost is dominated by LLVM-style optimization passes and LTO, so the additional IR has minimal impact. Gate with `MOLT_INLINE_INTRINSICS` for profiles where compile time matters.

## 10. Success Criteria

| Metric | Target | Measurement |
|--------|--------|-------------|
| Fibonacci(35) | 20% faster | `bench/` suite |
| String word-count | 15% faster | `bench/` suite |
| List comprehension (1M elements) | 25% faster | `bench/` suite |
| Dict iteration | 15% faster | `bench/` suite |
| Compile time (dev profile) | < 30% slower | `time molt build --profile dev` |
| Code size | < 2x per function | `size` on compiled binaries |
| Differential test pass rate | 100% (no regressions) | `molt_diff.py` full sweep |

## 11. Future Work (Beyond Phase 3)

- **Escape analysis**: If a heap object is provably local (never escapes the function), eliminate all refcount operations entirely. This requires TIR-level analysis but the inlined RC operations make the optimization trivially recognizable.

- **Deferred refcount batching**: Accumulate RC adjustments in a local counter and flush with a single atomic operation at scope boundaries. Reduces atomic operation count from N to 1 per scope.

- **Profile-guided inline expansion**: Use PGO data to identify which container types are actually used at each call site and emit only those fast paths, reducing code size.

- **Cranelift `FuncRef` sharing**: Currently each operation re-declares the FFI fallback function. Share `FuncRef` declarations across operations within the same function body to reduce module symbol table bloat.

- **GIL-free runtime exports**: For operations where the fast path is fully inlined and the slow path is rare, create `_nogil` variants of runtime functions that skip GIL acquisition (caller guarantees GIL is held). This reduces slow-path overhead from ~30ns to ~10ns.
