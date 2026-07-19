# Luau Backend OpIR Support Matrix

**Status:** Generated
**Source:** `runtime/molt-backend-luau/src/luau`
**Target:** current/future Luau surface; Molt does not add legacy Lua compatibility shims.

## Summary

- `compile-error`: `0`
- `implemented-exact`: `22`
- `implemented-target-limited`: `8`
- `not-admitted`: `376`
- `total`: `406`

## Matrix

| OpIR kind | Status | Note |
| --- | --- | --- |
| `*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `-` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `abs` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `add` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `alloc` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `alloc_class` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `alloc_task` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `and` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `ascii_from_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `async_work_poll` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `asyncgen_locals_register` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `band` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `binding_alias` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `binop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bit_and` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bit_or` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bit_xor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `block_on` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bool_const` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bound_method_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `box_from_raw_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `br_if` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `branch` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `branch_false` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bridge_unavailable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `build_dict` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `build_list` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `builtin_func` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `builtin_type` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bxor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `bytearray_fill_range` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytearray_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytearray_from_str` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytes_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `bytes_from_str` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `call` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_async` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `call_bind` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_func` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_function` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_guarded` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_indirect` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_internal` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `call_method` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
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
| `cbor_parse` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `chan_drop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `chan_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `chan_recv_yield` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `chan_send_yield` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `check_exception` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `checked_add` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `checked_mul` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `chr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `class_apply_set_name` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `class_layout_version` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `class_merge_layout` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `class_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `class_set_base` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `class_set_layout_version` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `classmethod_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `closure_load` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `closure_store` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `code_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `code_slot_set` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `code_slots_init` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `compare` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `complex_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `const` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `const_bigint` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `const_bool` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `const_bytes` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `const_ellipsis` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `const_float` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `const_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `const_none` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `const_not_implemented` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `const_str` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `contains` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
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
| `del_attr_generic_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `del_attr_generic_ptr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `del_attr_name` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `del_index` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `del_item` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_clear` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_copy` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_from_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_get` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_inc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_items` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_keys` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_pop` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_popitem` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_set` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_setdefault` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_setdefault_empty_list` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_str_int_inc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `dict_update` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_update_kwstar` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_update_missing` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `dict_values` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `div` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `drop_inserted` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `else` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `end_for` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `end_if` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `enumerate` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `eq` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_class` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_clear` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_context_set` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_finally_pending_observer` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_kind` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_last` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_last_pending` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_match_builtin` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_message` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_new_builtin` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_new_builtin_empty` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_new_builtin_one` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_new_from_class` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_pop` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_push` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_region_drops_inserted` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_set_cause` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_set_last` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_set_value` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_stack_clear` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_stack_depth` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_stack_enter` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_stack_exit` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exception_stack_set_depth` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exceptiongroup_combine` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `exceptiongroup_match` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_close` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_flush` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_open` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_read` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `file_write` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `float_from_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `floordiv` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `fn_ptr_code_set` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `for_iter` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `for_range` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `frame_locals_set` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `frozenset_add` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `frozenset_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `func_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `func_new_closure` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `function_closure_bits` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `future_cancel` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `future_cancel_clear` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `future_cancel_msg` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `ge` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `gen_locals_register` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `get_attr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `get_attr_generic_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `get_attr_generic_ptr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `get_attr_name` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `get_attr_name_default` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `get_attr_special_obj` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `get_item` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `getargv` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `getframe` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `goto` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `gt` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `guard_tag` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `guard_type` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `guarded_field_get` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `guarded_field_init` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `guarded_field_set` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `guarded_load` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `has_attr_name` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `id` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `identity_alias` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `if` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inc_ref` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `index` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_add` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `inplace_bit_and` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_bit_or` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_bit_xor` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `inplace_matmul` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `inplace_mul` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `inplace_sub` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `int_from_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `int_from_str_of_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `intarray_from_seq` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `invert` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `invoke_ffi` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `is` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `is_callable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `is_native_awaitable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `is_not` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `isinstance` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `issubclass` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `iter` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `iter_next` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `iter_next_unboxed` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `json_parse` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `jump` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `label` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `le` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `len` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `line` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `list_append` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_clear` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_copy` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_count` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_extend` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_fill_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_from_range` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `list_index` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_index_range` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_insert` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_pop` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_remove` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_repeat_range` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `list_reverse` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `load` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `load_local` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `load_var` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_break` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_break_if_exception` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_break_if_false` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_break_if_true` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_carry_init` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `loop_carry_update` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `loop_continue` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_end` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `loop_index_next` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_index_start` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `loop_start` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `lshift` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `lt` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `matmul` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `memoryview_cast` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `memoryview_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `memoryview_tobytes` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `missing` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `mod` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_cache_del` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_cache_get` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_cache_set` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_del_global` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_del_global_if_present` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_get_attr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_get_global` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_get_name` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_import` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_import_from` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_import_star` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `module_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `module_set_attr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `msgpack_parse` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `mul` | `implemented-target-limited` | Shared target contract admits only representation-proven non-integer scalar domains. |
| `ne` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `none_const` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `nop` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `not` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `object_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `object_set_class` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `or` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `ord` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `ord_at` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `pcall_failure_jump` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `pcall_handler_end` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `pcall_wrap_begin` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `pcall_wrap_end` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `phi` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `pow` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `pow_mod` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `print` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `print_newline` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `promise_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `promise_set_exception` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `promise_set_result` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `property_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `raise` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `range_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `release` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `repr_from_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `ret` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `ret_void` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `return` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `return_value` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `round` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `rshift` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_add` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_add_probe` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_attr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_attr_generic_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_attr_generic_ptr` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_attr_name` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_clear` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_discard` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_item` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_pop` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_remove` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `set_update` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
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
| `store_index` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `store_init` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `store_local` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `store_subscript` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `store_var` | `implemented-exact` | Lowered and outside every generated target-contract limitation. |
| `str_from_obj` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
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
| `string_join` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
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
| `subscript` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `super_new` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `sys_executable` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `taq_ingest_line` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `task_register_token_owned` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `thread_submit` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `trace_enter_slot` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `trace_exit` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `trunc` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `try_end` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `try_start` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `tuple_from_list` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `tuple_new` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `type_of` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `unary_op` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `unbox_to_raw_int` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `unpack_sequence` | `not-admitted` | Shared generated target contract rejects this semantic family before source generation. |
| `vec_max_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `vec_min_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `vec_prod_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |
| `vec_sum_*` | `not-admitted` | Operation is unclassified in the generated target-contract authority. |

## Status Definitions

- `implemented-exact`: emitted without known Luau target limitation or checked-output stub marker.
- `implemented-target-limited`: emitted for an admitted subset with an explicit Luau/Python semantic limit.
- `compile-error`: checked Luau emission rejects this unsupported operation.
- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.
