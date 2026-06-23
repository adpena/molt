# Luau Backend OpIR Support Matrix

**Status:** Generated
**Source:** `runtime/molt-backend/src/luau.rs`
**Target:** current/future Luau surface; Molt does not add legacy Lua compatibility shims.

## Summary

- `compile-error`: `1`
- `implemented-exact`: `294`
- `implemented-target-limited`: `49`
- `not-admitted`: `52`
- `runtime-capability-error`: `6`
- `total`: `402`

## Matrix

| OpIR kind | Status | Note |
| --- | --- | --- |
| `*` | `implemented-exact` | Lowered without checked-output stub markers. |
| `-` | `implemented-exact` | Lowered without checked-output stub markers. |
| `abs` | `implemented-exact` | Lowered without checked-output stub markers. |
| `add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc` | `implemented-target-limited` | Modeled as Luau table allocation for the admitted subset. |
| `alloc_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc_class_static` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc_class_trusted` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc_task` | `implemented-target-limited` | Generator/listcomp tasks use coroutine collection paths. |
| `and` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ascii_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `asyncgen_locals_register` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `band` | `implemented-exact` | Lowered without checked-output stub markers. |
| `binop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bit_and` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bit_or` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bit_xor` | `implemented-exact` | Lowered without checked-output stub markers. |
| `block_on` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `bool_const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bor` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bound_method_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `box_from_raw_int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `br_if` | `implemented-exact` | Valid labeled conditional branch lowers to Luau goto; missing target labels fail closed. |
| `branch` | `implemented-exact` | Valid labeled conditional branch lowers to Luau goto; missing target labels fail closed. |
| `branch_false` | `implemented-exact` | Valid labeled false-branch lowers to Luau goto; missing target labels fail closed. |
| `bridge_unavailable` | `implemented-exact` | Lowered without checked-output stub markers. |
| `build_dict` | `implemented-exact` | Lowered without checked-output stub markers. |
| `build_list` | `implemented-exact` | Lowered without checked-output stub markers. |
| `builtin_func` | `implemented-exact` | Lowered without checked-output stub markers. |
| `builtin_type` | `implemented-target-limited` | Modeled as named Luau type metadata for the admitted subset. |
| `bxor` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bytearray_fill_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bytearray_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bytearray_from_str` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bytes_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bytes_from_str` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call_async` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `call_bind` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call_func` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call_guarded` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call_indirect` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call_internal` | `implemented-exact` | Lowered without checked-output stub markers. |
| `call_method` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable dispatch for the admitted subset. |
| `callargs_expand_kwstar` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_expand_star` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_push_kw` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_push_pos` | `implemented-exact` | Lowered without checked-output stub markers. |
| `cancel_current` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_cancel` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_clone` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_drop` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_get_current` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_is_cancelled` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancel_token_set_current` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cancelled` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `cbor_parse` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `chan_drop` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `chan_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `chan_recv_yield` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `chan_send_yield` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `check_exception` | `implemented-exact` | Lowered without checked-output stub markers. |
| `checked_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `chr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `class_apply_set_name` | `implemented-target-limited` | __set_name__ hooks dispatch over Luau class-table snapshots for the admitted subset. |
| `class_layout_version` | `implemented-target-limited` | Modeled as Luau class-table layout metadata for the admitted subset. |
| `class_merge_layout` | `implemented-target-limited` | Maintains Luau class-table layout metadata for the admitted subset. |
| `class_new` | `implemented-target-limited` | Modeled as Luau table/metatable object for the admitted subset. |
| `class_set_base` | `implemented-exact` | Lowered without checked-output stub markers. |
| `class_set_layout_version` | `implemented-target-limited` | Modeled as Luau class-table layout metadata for the admitted subset. |
| `classmethod_new` | `implemented-target-limited` | Modeled as Luau descriptor metadata for the admitted subset. |
| `closure_load` | `implemented-exact` | Lowered without checked-output stub markers. |
| `closure_store` | `implemented-exact` | Lowered without checked-output stub markers. |
| `code_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `code_slot_set` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `code_slots_init` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `compare` | `implemented-exact` | Lowered without checked-output stub markers. |
| `complex_from_obj` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_bigint` | `implemented-target-limited` | Luau numbers are IEEE-754 doubles; arbitrary precision is not represented. |
| `const_bool` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_bytes` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_ellipsis` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_float` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_none` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_not_implemented` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_str` | `implemented-exact` | Lowered without checked-output stub markers. |
| `contains` | `implemented-exact` | Lowered without checked-output stub markers. |
| `context_closing` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `context_depth` | `implemented-exact` | Lowered without checked-output stub markers. |
| `context_enter` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `context_exit` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `context_null` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `context_unwind` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `context_unwind_to` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `copy_var` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dataclass_get` | `implemented-target-limited` | Modeled as Luau field/index access for the admitted subset. |
| `dataclass_new` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
| `dataclass_new_values` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dataclass_set` | `implemented-target-limited` | Modeled as Luau field assignment for the admitted subset. |
| `dataclass_set_class` | `implemented-target-limited` | Modeled as Luau field assignment for the admitted subset. |
| `dec_ref` | `implemented-exact` | Lowered without checked-output stub markers. |
| `del_attr_generic_obj` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable deletion for the admitted subset. |
| `del_attr_generic_ptr` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable deletion for the admitted subset. |
| `del_attr_name` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable deletion for the admitted subset. |
| `del_index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `del_item` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_copy` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_get` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_inc` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_items` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_keys` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_pop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_popitem` | `implemented-target-limited` | Luau table iteration order is not CPython insertion order. |
| `dict_set` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_setdefault` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_setdefault_empty_list` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_str_int_inc` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_update` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_update_kwstar` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_update_missing` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dict_values` | `implemented-exact` | Lowered without checked-output stub markers. |
| `div` | `implemented-exact` | Lowered without checked-output stub markers. |
| `drop_inserted` | `implemented-exact` | Lowered without checked-output stub markers. |
| `else` | `implemented-exact` | Lowered without checked-output stub markers. |
| `end_for` | `implemented-exact` | Lowered without checked-output stub markers. |
| `end_if` | `implemented-exact` | Lowered without checked-output stub markers. |
| `enumerate` | `implemented-exact` | Lowered without checked-output stub markers. |
| `eq` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_context_set` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_finally_pending_observer` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_kind` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_last` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_last_pending` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_match_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_message` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new_builtin_empty` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new_builtin_one` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new_from_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_pop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_push` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_region_drops_inserted` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_set_cause` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_set_last` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_set_value` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_stack_clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_stack_depth` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_stack_enter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_stack_exit` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_stack_set_depth` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exceptiongroup_combine` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `exceptiongroup_match` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `file_close` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_flush` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_open` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_read` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_write` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `float_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `floordiv` | `implemented-exact` | Lowered without checked-output stub markers. |
| `fn_ptr_code_set` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `for_iter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `for_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `frame_locals_set` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `frozenset_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `frozenset_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `func_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `func_new_closure` | `implemented-exact` | Lowered without checked-output stub markers. |
| `function_closure_bits` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `future_cancel` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `future_cancel_clear` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `future_cancel_msg` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `ge` | `implemented-exact` | Lowered without checked-output stub markers. |
| `gen_locals_register` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `get_attr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `get_attr_generic_obj` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable lookup for the admitted subset. |
| `get_attr_generic_ptr` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable lookup for the admitted subset. |
| `get_attr_name` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable lookup for the admitted subset. |
| `get_attr_name_default` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable lookup for the admitted subset. |
| `get_attr_special_obj` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable lookup for the admitted subset. |
| `get_item` | `implemented-exact` | Lowered without checked-output stub markers. |
| `getargv` | `implemented-target-limited` | Luau has no process argv surface; materializes an empty argv list. |
| `getframe` | `implemented-target-limited` | Luau has no Python frame-object introspection surface; materializes None for fallback-aware stdlib paths. |
| `goto` | `implemented-exact` | Lowered without checked-output stub markers. |
| `gt` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guard_tag` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guard_type` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_field_get` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_field_init` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_field_set` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_load` | `implemented-exact` | Lowered without checked-output stub markers. |
| `has_attr_name` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable lookup for the admitted subset. |
| `id` | `implemented-target-limited` | Uses string identity representation, not CPython object address identity. |
| `identity_alias` | `implemented-exact` | Lowered without checked-output stub markers. |
| `if` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inc_ref` | `implemented-exact` | Lowered without checked-output stub markers. |
| `index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_bit_and` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_bit_or` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_bit_xor` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_mul` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_sub` | `implemented-exact` | Lowered without checked-output stub markers. |
| `int_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `int_from_str_of_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `intarray_from_seq` | `implemented-target-limited` | Modeled as a copied dense Luau integer table for vector consumers. |
| `invert` | `implemented-exact` | Lowered without checked-output stub markers. |
| `invoke_ffi` | `runtime-capability-error` | Roblox/Luau FFI capability is unavailable. |
| `is` | `implemented-target-limited` | Non-None identity currently lowers through equality on Luau values. |
| `is_callable` | `implemented-exact` | Lowered without checked-output stub markers. |
| `is_native_awaitable` | `implemented-target-limited` | Luau has no Molt native poll-function object representation; target values are non-native awaitables. |
| `isinstance` | `implemented-target-limited` | Uses Luau type metadata and metatable inheritance for the admitted subset. |
| `issubclass` | `implemented-target-limited` | Uses Luau type metadata and metatable inheritance for the admitted subset. |
| `iter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `iter_next` | `implemented-exact` | Lowered without checked-output stub markers. |
| `iter_next_unboxed` | `implemented-exact` | Lowered without checked-output stub markers. |
| `json_parse` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `jump` | `implemented-exact` | Lowered without checked-output stub markers. |
| `label` | `implemented-exact` | Lowered without checked-output stub markers. |
| `le` | `implemented-exact` | Lowered without checked-output stub markers. |
| `len` | `implemented-exact` | Lowered without checked-output stub markers. |
| `line` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_append` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_copy` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_count` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_extend` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_fill_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_from_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_index_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_insert` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_pop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_remove` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_repeat_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `list_reverse` | `implemented-exact` | Lowered without checked-output stub markers. |
| `load` | `implemented-exact` | Lowered without checked-output stub markers. |
| `load_local` | `implemented-exact` | Lowered without checked-output stub markers. |
| `load_var` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_break` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_break_if_exception` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `loop_break_if_false` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_break_if_true` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_carry_init` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_carry_update` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_continue` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_end` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_index_next` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_index_start` | `implemented-exact` | Lowered without checked-output stub markers. |
| `loop_start` | `implemented-exact` | Lowered without checked-output stub markers. |
| `lshift` | `implemented-exact` | Lowered without checked-output stub markers. |
| `lt` | `implemented-exact` | Lowered without checked-output stub markers. |
| `matmul` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `memoryview_cast` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `memoryview_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `memoryview_tobytes` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `missing` | `implemented-exact` | Lowered without checked-output stub markers. |
| `mod` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_cache_del` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_cache_get` | `implemented-target-limited` | Only known module bridges are materialized in Luau. |
| `module_cache_set` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_del_global` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_del_global_if_present` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_get_attr` | `implemented-target-limited` | Known module bridges are direct; unknown attrs return nil unless rejected by checked output. |
| `module_get_global` | `implemented-target-limited` | Dynamic module lookup depends on Luau module cache entries. |
| `module_get_name` | `implemented-target-limited` | Dynamic module lookup depends on Luau module cache entries. |
| `module_import` | `implemented-target-limited` | Only known module bridges are materialized in Luau. |
| `module_import_from` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_import_star` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_new` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
| `module_set_attr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `msgpack_parse` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `mul` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ne` | `implemented-exact` | Lowered without checked-output stub markers. |
| `none_const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `nop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `not` | `implemented-exact` | Lowered without checked-output stub markers. |
| `object_new` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
| `object_set_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `or` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ord` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ord_at` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_failure_jump` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_handler_end` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_wrap_begin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_wrap_end` | `implemented-exact` | Lowered without checked-output stub markers. |
| `phi` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pow` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pow_mod` | `implemented-exact` | Lowered without checked-output stub markers. |
| `print` | `implemented-exact` | Lowered without checked-output stub markers. |
| `print_newline` | `implemented-exact` | Lowered without checked-output stub markers. |
| `promise_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `promise_set_exception` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `promise_set_result` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `property_new` | `implemented-target-limited` | Modeled as Luau descriptor metadata for the admitted subset. |
| `raise` | `implemented-exact` | Lowered without checked-output stub markers. |
| `range_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `release` | `implemented-exact` | Lowered without checked-output stub markers. |
| `repr_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ret` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ret_void` | `implemented-exact` | Lowered without checked-output stub markers. |
| `return` | `implemented-exact` | Lowered without checked-output stub markers. |
| `return_value` | `implemented-exact` | Lowered without checked-output stub markers. |
| `round` | `implemented-exact` | Lowered without checked-output stub markers. |
| `rshift` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_add_probe` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_attr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_attr_generic_obj` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable assignment for the admitted subset. |
| `set_attr_generic_ptr` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable assignment for the admitted subset. |
| `set_attr_name` | `implemented-target-limited` | Uses descriptor-aware Luau table/metatable assignment for the admitted subset. |
| `set_clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_discard` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_item` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_pop` | `implemented-target-limited` | Luau table iteration order is not CPython set pop order. |
| `set_remove` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_update` | `implemented-exact` | Lowered without checked-output stub markers. |
| `shl` | `implemented-exact` | Lowered without checked-output stub markers. |
| `shr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `spawn` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `state_label` | `implemented-exact` | Lowered without checked-output stub markers. |
| `state_switch` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `state_transition` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `state_yield` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `staticmethod_new` | `implemented-target-limited` | Modeled as Luau descriptor metadata for the admitted subset. |
| `store` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_init` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_local` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_subscript` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_var` | `implemented-exact` | Lowered without checked-output stub markers. |
| `str_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_concat` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_count` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_count_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_endswith` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_endswith_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_eq` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_find` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_find_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_format` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_index_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_join` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_lower` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_lstrip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_partition` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_repeat` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_replace` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rfind` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rfind_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rindex` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rindex_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rpartition` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rstrip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_field` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_field_eq` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_field_len` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_sep_dict_inc` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_validate` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_ws_dict_inc` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_splitlines` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_startswith` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_startswith_slice` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_strip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_upper` | `implemented-exact` | Lowered without checked-output stub markers. |
| `sub` | `implemented-exact` | Lowered without checked-output stub markers. |
| `subscript` | `implemented-exact` | Lowered without checked-output stub markers. |
| `super_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `sys_executable` | `implemented-target-limited` | Luau has no executable path surface; materializes an empty string. |
| `taq_ingest_line` | `implemented-exact` | Lowered without checked-output stub markers. |
| `task_register_token_owned` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `thread_submit` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `trace_enter_slot` | `implemented-target-limited` | Luau checked output does not materialize Molt tracing frame-stack state. |
| `trace_exit` | `implemented-target-limited` | Luau checked output does not materialize Molt tracing frame-stack state. |
| `trunc` | `implemented-exact` | Lowered without checked-output stub markers. |
| `try_end` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `try_start` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `tuple_from_list` | `implemented-exact` | Lowered without checked-output stub markers. |
| `tuple_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `type_of` | `implemented-exact` | Lowered without checked-output stub markers. |
| `unary_op` | `implemented-exact` | Lowered without checked-output stub markers. |
| `unbox_to_raw_int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `unpack_sequence` | `implemented-exact` | Lowered without checked-output stub markers. |
| `vec_max_*` | `implemented-exact` | Lowered without checked-output stub markers. |
| `vec_min_*` | `implemented-exact` | Lowered without checked-output stub markers. |
| `vec_prod_*` | `implemented-exact` | Lowered without checked-output stub markers. |
| `vec_sum_*` | `implemented-exact` | Lowered without checked-output stub markers. |

## Status Definitions

- `implemented-exact`: emitted without known Luau target limitation or checked-output stub marker.
- `implemented-target-limited`: emitted for an admitted subset with an explicit Luau/Python semantic limit.
- `compile-error`: checked Luau emission rejects this unsupported operation.
- `runtime-capability-error`: operation requires a target capability unavailable in Roblox/Luau.
- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.
