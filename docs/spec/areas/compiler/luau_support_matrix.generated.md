# Luau Backend OpIR Support Matrix

**Status:** Generated
**Source:** `runtime/molt-backend-luau/src/luau`
**Target:** current/future Luau surface; Molt does not add legacy Lua compatibility shims.

## Summary

- `compile-error`: `2`
- `implemented-exact`: `186`
- `implemented-target-limited`: `12`
- `not-admitted`: `223`
- `total`: `423`

## Matrix

| OpIR kind | Status | Note |
| --- | --- | --- |
| `*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `-` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `abs` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `add` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `alloc` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `alloc_class` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `alloc_task` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `and` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `ascii_from_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `async_work_poll` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `asyncgen_locals_register` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `band` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `binding_alias` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `binop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `binop_floor_div` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `binop_mod` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `binop_pow` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bit_and` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bit_not` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bit_or` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bit_xor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `block_on` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bool_const` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `borrow` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bound_method_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `box_from_raw_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `br_if` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `branch` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `branch_false` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `bridge_unavailable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `build_dict` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `build_list` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `builtin_func` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `builtin_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `builtin_range` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `builtin_sum` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `builtin_type` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `bxor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bytearray_fill_range` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytearray_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytearray_from_str` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytes_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytes_from_str` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `call` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_async` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `call_bind` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_func` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_function` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_guarded` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_indirect` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_internal` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `call_method` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `callargs_expand_kwstar` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `callargs_expand_star` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `callargs_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `callargs_push_kw` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `callargs_push_pos` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_current` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_cancel` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_clone` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_drop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_get_current` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_is_cancelled` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancel_token_set_current` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cancelled` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `cast_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `cbor_parse` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `chan_drop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `chan_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `chan_recv_yield` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `chan_send_yield` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `check_exception` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `checked_add` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `checked_mul` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `chr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `class_apply_set_name` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `class_layout_version` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `class_merge_layout` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `class_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `class_set_base` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `class_set_layout_version` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `classmethod_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `closure_load` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `closure_store` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `code_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `code_slot_set` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `code_slots_init` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `compare` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `complex_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `const` | `implemented-target-limited` | Shared target contract admits only concrete integer literals exactly representable by Luau's numeric carrier. |
| `const_bigint` | `implemented-target-limited` | Shared target contract admits only concrete integer literals exactly representable by Luau's numeric carrier. |
| `const_bool` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `const_bytes` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `const_ellipsis` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `const_float` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `const_int` | `implemented-target-limited` | Shared target contract admits only concrete integer literals exactly representable by Luau's numeric carrier. |
| `const_none` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `const_not_implemented` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `const_str` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `contains` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `context_closing` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `context_depth` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `context_enter` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `context_exit` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `context_null` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `context_unwind` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `context_unwind_to` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `copy_var` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dataclass_get` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dataclass_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dataclass_new_values` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dataclass_set` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dataclass_set_class` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dec_ref` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `del_attr_generic_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `del_attr_generic_ptr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `del_attr_name` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `del_index` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `del_item` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_clear` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_copy` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_get` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_inc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_items` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_keys` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_pop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_popitem` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_set` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_setdefault` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_setdefault_empty_list` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_str_int_inc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_update` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_update_kwstar` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_update_missing` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `dict_values` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `div` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `drop_inserted` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `else` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `end_for` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `end_if` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `enumerate` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `eq` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_class` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_clear` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_context_set` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_finally_pending_observer` | `implemented-target-limited` | Unmarked observer is lowered; the generated target contract rejects the pending-call/eval-breaker-marked variant. |
| `exception_kind` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_last` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_last_pending` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_match_builtin` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_message` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_new_builtin` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_new_builtin_empty` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_new_builtin_one` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_new_from_class` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_pop` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_push` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_region_drops_inserted` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_set_cause` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_set_last` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_set_value` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_stack_clear` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_stack_depth` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_stack_enter` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_stack_exit` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exception_stack_set_depth` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exceptiongroup_combine` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `exceptiongroup_match` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `file_close` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_flush` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_open` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_read` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_write` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `float_from_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `floor_div` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `floordiv` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `fn_ptr_code_set` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `for_iter` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `for_range` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `format_string` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `frame_locals_set` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `frozenset_add` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `frozenset_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `func_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `func_new_closure` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `function_closure_bits` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `future_cancel` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `future_cancel_clear` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `future_cancel_msg` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `ge` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `gen_locals_register` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `get_attr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `get_attr_generic_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `get_attr_generic_ptr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `get_attr_name` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `get_attr_name_default` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `get_attr_special_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `get_item` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `getargv` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `getframe` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `goto` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `gt` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `guard_tag` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `guard_type` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `guarded_field_get` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `guarded_field_init` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `guarded_field_set` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `guarded_load` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `has_attr_name` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `id` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `identity_alias` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `if` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `inc_ref` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `index` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `inplace_add` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `inplace_bit_and` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_bit_or` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_bit_xor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_floordiv` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_lshift` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_matmul` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `inplace_mod` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_mul` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `inplace_rshift` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_sub` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `int_from_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `int_from_str_of_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `intarray_from_seq` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `invert` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `invoke_ffi` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `is` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `is_callable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `is_native_awaitable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `is_not` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `isinstance` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `issubclass` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `iter` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `iter_next` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `iter_next_unboxed` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `json_parse` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `jump` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `label` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `le` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `len` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `line` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_append` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_clear` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_copy` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_count` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_extend` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_fill_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_from_range` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `list_index` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_index_range` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_insert` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_pop` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_remove` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_repeat_range` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `list_reverse` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `load` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `load_local` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `load_var` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_break` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_break_if_exception` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_break_if_false` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_break_if_true` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_carry_init` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `loop_carry_update` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `loop_continue` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_end` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_index_next` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_index_start` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_start` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `lshift` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `lt` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `matmul` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `memoryview_cast` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `memoryview_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `memoryview_tobytes` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `missing` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `mod` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `mod_` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_cache_del` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_cache_get` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_cache_set` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_del_global` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_del_global_if_present` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_get_attr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_get_global` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_get_name` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_import` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_import_from` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_import_star` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `module_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `module_set_attr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `modulo` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `msgpack_parse` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `mul` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `ne` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `none_const` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `nop` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `not` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `object_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `object_set_class` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `or` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `ord` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `ord_at` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `pcall_failure_jump` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `pcall_handler_end` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `pcall_wrap_begin` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `pcall_wrap_end` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `phi` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `pow` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `pow_mod` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `print` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `print_newline` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `promise_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `promise_set_exception` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `promise_set_result` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `property_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `raise` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `range` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `range_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `release` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `repr_from_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `round` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `rshift` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_add` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_add_probe` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_attr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_attr_generic_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_attr_generic_ptr` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_attr_name` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_clear` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_discard` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_item` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_pop` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_remove` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `set_update` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `shl` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `shr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `spawn` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `state_label` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `state_switch` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `state_transition` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `state_yield` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `staticmethod_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `store` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `store_index` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `store_init` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `store_local` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `store_subscript` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `store_var` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `str_from_obj` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `string_concat` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_const` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_count` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_count_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_endswith` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_endswith_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_eq` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `string_find` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_find_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_format` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `string_index` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_index_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_join` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `string_lower` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_lstrip` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_partition` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_repeat` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_replace` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_rfind` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_rfind_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_rindex` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_rindex_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_rpartition` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_rstrip` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split_field` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split_field_eq` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split_field_len` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split_sep_dict_inc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split_validate` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_split_ws_dict_inc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_splitlines` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_startswith` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_startswith_slice` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_strip` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `string_upper` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `sub` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `subscript` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `sum` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `super_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `sys_executable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `taq_ingest_line` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `task_register_token_owned` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `thread_submit` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `trace_enter_slot` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `trace_exit` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `trunc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `try_end` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `try_start` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `tuple_from_list` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `tuple_new` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `type_of` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `unary_invert` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `unary_op` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `unbox_to_raw_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `unpack_sequence` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `vec_max_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `vec_min_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `vec_prod_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `vec_sum_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |

## Status Definitions

- `implemented-exact`: emitted without known Luau target limitation or checked-output stub marker.
- `implemented-target-limited`: emitted for an admitted subset with an explicit Luau/Python semantic limit.
- `compile-error`: checked Luau emission rejects this unsupported operation.
- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.
