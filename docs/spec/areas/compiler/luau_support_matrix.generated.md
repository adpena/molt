# Luau Backend OpIR Support Matrix

**Status:** Generated
**Source:** `runtime/molt-backend/src/luau.rs`
**Target:** current/future Luau surface; Molt does not add legacy Lua compatibility shims.

## Summary

- `compile-error`: `7`
- `implemented-exact`: `314`
- `implemented-target-limited`: `18`
- `not-admitted`: `24`
- `runtime-capability-error`: `5`
- `total`: `368`

## Matrix

| OpIR kind | Status | Note |
| --- | --- | --- |
| `!=` | `implemented-exact` | Lowered without checked-output stub markers. |
| `%` | `implemented-exact` | Lowered without checked-output stub markers. |
| `&` | `implemented-exact` | Lowered without checked-output stub markers. |
| `*` | `implemented-exact` | Lowered without checked-output stub markers. |
| `**` | `implemented-exact` | Lowered without checked-output stub markers. |
| `+` | `implemented-exact` | Lowered without checked-output stub markers. |
| `-` | `implemented-exact` | Lowered without checked-output stub markers. |
| `/` | `implemented-exact` | Lowered without checked-output stub markers. |
| `//` | `implemented-exact` | Lowered without checked-output stub markers. |
| `<<` | `implemented-exact` | Lowered without checked-output stub markers. |
| `<>` | `implemented-exact` | Lowered without checked-output stub markers. |
| `>>` | `implemented-exact` | Lowered without checked-output stub markers. |
| `^` | `implemented-exact` | Lowered without checked-output stub markers. |
| `abs` | `implemented-exact` | Lowered without checked-output stub markers. |
| `add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `all` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc` | `implemented-target-limited` | Modeled as Luau table allocation for the admitted subset. |
| `alloc_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc_class_static` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc_class_trusted` | `implemented-exact` | Lowered without checked-output stub markers. |
| `alloc_task` | `implemented-target-limited` | Generator/listcomp tasks use coroutine collection paths. |
| `and` | `implemented-exact` | Lowered without checked-output stub markers. |
| `any` | `implemented-exact` | Lowered without checked-output stub markers. |
| `append` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ascii_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `binop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bit_and` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bit_or` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bit_xor` | `implemented-exact` | Lowered without checked-output stub markers. |
| `block_on` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `bool` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bool_const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `bound_method_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `box_from_raw_int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `br_if` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `branch` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `branch_false` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `bridge_unavailable` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `build_dict` | `implemented-exact` | Lowered without checked-output stub markers. |
| `build_list` | `implemented-exact` | Lowered without checked-output stub markers. |
| `builtin_func` | `implemented-exact` | Lowered without checked-output stub markers. |
| `builtin_type` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
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
| `call_method` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_expand_star` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_push_kw` | `implemented-exact` | Lowered without checked-output stub markers. |
| `callargs_push_pos` | `implemented-exact` | Lowered without checked-output stub markers. |
| `cbor_parse` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `check_exception` | `implemented-exact` | Lowered without checked-output stub markers. |
| `chr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `class_new` | `implemented-target-limited` | Modeled as Luau table/metatable object for the admitted subset. |
| `class_set_base` | `implemented-exact` | Lowered without checked-output stub markers. |
| `classmethod_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `closure_load` | `implemented-exact` | Lowered without checked-output stub markers. |
| `closure_store` | `implemented-exact` | Lowered without checked-output stub markers. |
| `code_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `compare` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_bigint` | `implemented-target-limited` | Luau numbers are IEEE-754 doubles; arbitrary precision is not represented. |
| `const_bool` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_bytes` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_ellipsis` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_float` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_none` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_not_implemented` | `implemented-exact` | Lowered without checked-output stub markers. |
| `const_str` | `implemented-exact` | Lowered without checked-output stub markers. |
| `contains` | `implemented-exact` | Lowered without checked-output stub markers. |
| `context_depth` | `implemented-exact` | Lowered without checked-output stub markers. |
| `copy` | `implemented-exact` | Lowered without checked-output stub markers. |
| `dataclass_get` | `implemented-target-limited` | Modeled as Luau field/index access for the admitted subset. |
| `dataclass_new` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
| `dataclass_set` | `implemented-target-limited` | Modeled as Luau field assignment for the admitted subset. |
| `dataclass_set_class` | `implemented-target-limited` | Modeled as Luau field assignment for the admitted subset. |
| `del_attr_generic_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `del_attr_generic_ptr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `del_attr_name` | `implemented-exact` | Lowered without checked-output stub markers. |
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
| `dict_values` | `implemented-exact` | Lowered without checked-output stub markers. |
| `div` | `implemented-exact` | Lowered without checked-output stub markers. |
| `else` | `implemented-exact` | Lowered without checked-output stub markers. |
| `end_for` | `implemented-exact` | Lowered without checked-output stub markers. |
| `end_if` | `implemented-exact` | Lowered without checked-output stub markers. |
| `enumerate` | `implemented-exact` | Lowered without checked-output stub markers. |
| `eq` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_clear` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_kind` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_last` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_message` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_new_from_class` | `implemented-exact` | Lowered without checked-output stub markers. |
| `exception_stack_depth` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `exceptiongroup_combine` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `exceptiongroup_match` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `extend` | `implemented-exact` | Lowered without checked-output stub markers. |
| `file_close` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_flush` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_open` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_read` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `file_write` | `runtime-capability-error` | Roblox/Luau filesystem capability is unavailable. |
| `filter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `float` | `implemented-exact` | Lowered without checked-output stub markers. |
| `float_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `floordiv` | `implemented-exact` | Lowered without checked-output stub markers. |
| `for_iter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `for_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `frozenset_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `frozenset_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `func_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `func_new_closure` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ge` | `implemented-exact` | Lowered without checked-output stub markers. |
| `get_attr_name` | `implemented-exact` | Lowered without checked-output stub markers. |
| `get_attr_name_default` | `implemented-exact` | Lowered without checked-output stub markers. |
| `get_item` | `implemented-exact` | Lowered without checked-output stub markers. |
| `getargv` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `getframe` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `goto` | `implemented-exact` | Lowered without checked-output stub markers. |
| `gt` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_field_get` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_field_init` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_field_set` | `implemented-exact` | Lowered without checked-output stub markers. |
| `guarded_load` | `implemented-exact` | Lowered without checked-output stub markers. |
| `has_attr_name` | `implemented-exact` | Lowered without checked-output stub markers. |
| `id` | `implemented-target-limited` | Uses string identity representation, not CPython object address identity. |
| `identity_alias` | `implemented-exact` | Lowered without checked-output stub markers. |
| `if` | `implemented-exact` | Lowered without checked-output stub markers. |
| `index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_bit_and` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_bit_or` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_bit_xor` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_mul` | `implemented-exact` | Lowered without checked-output stub markers. |
| `inplace_sub` | `implemented-exact` | Lowered without checked-output stub markers. |
| `insert` | `implemented-exact` | Lowered without checked-output stub markers. |
| `int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `int_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `invert` | `implemented-exact` | Lowered without checked-output stub markers. |
| `invoke_ffi` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `is` | `implemented-target-limited` | Non-None identity currently lowers through equality on Luau values. |
| `is_callable` | `implemented-exact` | Lowered without checked-output stub markers. |
| `isinstance` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `issubclass` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `iter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `iter_next` | `implemented-exact` | Lowered without checked-output stub markers. |
| `json` | `implemented-exact` | Lowered without checked-output stub markers. |
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
| `loop_break` | `implemented-exact` | Lowered without checked-output stub markers. |
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
| `map` | `implemented-exact` | Lowered without checked-output stub markers. |
| `math` | `implemented-exact` | Lowered without checked-output stub markers. |
| `matmul` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `max` | `implemented-exact` | Lowered without checked-output stub markers. |
| `min` | `implemented-exact` | Lowered without checked-output stub markers. |
| `missing` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `mod` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_del_global` | `implemented-exact` | Lowered without checked-output stub markers. |
| `module_get_attr` | `implemented-target-limited` | Known module bridges are direct; unknown attrs return nil unless rejected by checked output. |
| `module_get_global` | `implemented-target-limited` | Dynamic module lookup depends on Luau module cache entries. |
| `module_get_name` | `implemented-target-limited` | Dynamic module lookup depends on Luau module cache entries. |
| `module_new` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
| `module_set_attr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_abs_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_all` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_all_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_any` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_any_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_ascii_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_bin_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_bool` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_bool_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_callable_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_chr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_dir_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_divmod_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_enumerate` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_enumerate_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_filter` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_float` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_float_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_format_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_getattr_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_hash_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_hex_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_id` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_int_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_isinstance` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_issubclass` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_iter_checked` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_len` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_map` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_max_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_min_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_next_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_oct_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_ord` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_print_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_repr_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_reversed` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_reversed_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_round_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_sorted` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_sorted_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_str` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_str_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_sum` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_sum_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_vars_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_zip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `molt_zip_builtin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `msgpack_parse` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `mul` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ne` | `implemented-exact` | Lowered without checked-output stub markers. |
| `none_const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `nop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `not` | `implemented-exact` | Lowered without checked-output stub markers. |
| `object_new` | `implemented-target-limited` | Modeled as Luau table object for the admitted subset. |
| `object_set_class` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `or` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ord` | `implemented-exact` | Lowered without checked-output stub markers. |
| `os` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_handler_end` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_wrap_begin` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pcall_wrap_end` | `implemented-exact` | Lowered without checked-output stub markers. |
| `phi` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pop` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pow` | `implemented-exact` | Lowered without checked-output stub markers. |
| `pow_mod` | `implemented-exact` | Lowered without checked-output stub markers. |
| `print` | `implemented-exact` | Lowered without checked-output stub markers. |
| `print_newline` | `implemented-exact` | Lowered without checked-output stub markers. |
| `property_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `raise` | `implemented-exact` | Lowered without checked-output stub markers. |
| `range` | `implemented-exact` | Lowered without checked-output stub markers. |
| `range_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `remove` | `implemented-exact` | Lowered without checked-output stub markers. |
| `repr_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ret` | `implemented-exact` | Lowered without checked-output stub markers. |
| `ret_void` | `implemented-exact` | Lowered without checked-output stub markers. |
| `return` | `implemented-exact` | Lowered without checked-output stub markers. |
| `return_value` | `implemented-exact` | Lowered without checked-output stub markers. |
| `reverse` | `implemented-exact` | Lowered without checked-output stub markers. |
| `reversed` | `implemented-exact` | Lowered without checked-output stub markers. |
| `round` | `implemented-exact` | Lowered without checked-output stub markers. |
| `rshift` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_add` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_attr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_attr_generic_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_attr_generic_ptr` | `implemented-exact` | Lowered without checked-output stub markers. |
| `set_attr_name` | `implemented-exact` | Lowered without checked-output stub markers. |
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
| `sort` | `implemented-exact` | Lowered without checked-output stub markers. |
| `sorted` | `implemented-exact` | Lowered without checked-output stub markers. |
| `spawn` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `state_label` | `implemented-exact` | Lowered without checked-output stub markers. |
| `state_yield` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `staticmethod_new` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `store` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_index` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_init` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_local` | `implemented-exact` | Lowered without checked-output stub markers. |
| `store_subscript` | `implemented-exact` | Lowered without checked-output stub markers. |
| `str` | `implemented-exact` | Lowered without checked-output stub markers. |
| `str_from_obj` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_concat` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_const` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_endswith` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_eq` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_find` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_format` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_join` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_lower` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_lstrip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_repeat` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_replace` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_rstrip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_split_sep_dict_inc` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `string_split_ws_dict_inc` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `string_startswith` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_strip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `string_upper` | `implemented-exact` | Lowered without checked-output stub markers. |
| `sub` | `implemented-exact` | Lowered without checked-output stub markers. |
| `subscript` | `implemented-exact` | Lowered without checked-output stub markers. |
| `sum` | `implemented-exact` | Lowered without checked-output stub markers. |
| `super_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `sys_executable` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `taq_ingest_line` | `compile-error` | Checked Luau emission rejects unsupported markers. |
| `time` | `implemented-exact` | Lowered without checked-output stub markers. |
| `trunc` | `implemented-exact` | Lowered without checked-output stub markers. |
| `try_end` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `try_start` | `not-admitted` | Checked Luau emission rejects semantic stub markers. |
| `tuple_from_list` | `implemented-exact` | Lowered without checked-output stub markers. |
| `tuple_new` | `implemented-exact` | Lowered without checked-output stub markers. |
| `type_of` | `implemented-exact` | Lowered without checked-output stub markers. |
| `unary_op` | `implemented-exact` | Lowered without checked-output stub markers. |
| `unbox_to_raw_int` | `implemented-exact` | Lowered without checked-output stub markers. |
| `unpack_sequence` | `implemented-exact` | Lowered without checked-output stub markers. |
| `zip` | `implemented-exact` | Lowered without checked-output stub markers. |
| `|` | `implemented-exact` | Lowered without checked-output stub markers. |
| `~` | `implemented-exact` | Lowered without checked-output stub markers. |

## Status Definitions

- `implemented-exact`: emitted without known Luau target limitation or checked-output stub marker.
- `implemented-target-limited`: emitted for an admitted subset with an explicit Luau/Python semantic limit.
- `compile-error`: checked Luau emission rejects this unsupported operation.
- `runtime-capability-error`: operation requires a target capability unavailable in Roblox/Luau.
- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.
