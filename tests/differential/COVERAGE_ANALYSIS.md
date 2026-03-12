# Differential Test Coverage Analysis

## Summary

- **Total tracked features:** 221
- **Covered by tests:** 212
- **Overall coverage:** 95.9%
- **Total test files scanned:** 2570

## Coverage by Category

| Category | Covered | Total | Coverage |
| --- | --- | --- | --- |
| async | 6 | 6 | 100.0% |
| builtin_functions | 60 | 64 | 93.8% |
| classes | 38 | 41 | 92.7% |
| collection_operations | 12 | 12 | 100.0% |
| comprehensions | 6 | 6 | 100.0% |
| context_managers | 3 | 3 | 100.0% |
| control_flow | 16 | 16 | 100.0% |
| exception_handling | 8 | 8 | 100.0% |
| functions | 13 | 13 | 100.0% |
| imports | 5 | 5 | 100.0% |
| operators | 30 | 31 | 96.8% |
| string_operations | 12 | 12 | 100.0% |
| type_hints | 3 | 4 | 75.0% |

## async

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| async_def | `tests/differential/basic/async_anext_default_future.py`, `tests/differential/basic/async_anext_future.py`, `tests/differential/basic/async_cancellation_token.py` (+168 more) |
| await_expr | `tests/differential/basic/async_anext_default_future.py`, `tests/differential/basic/async_anext_future.py`, `tests/differential/basic/async_cancellation_token.py` (+146 more) |
| async_for | `tests/differential/basic/async_for_else.py`, `tests/differential/basic/async_for_iter.py`, `tests/differential/basic/async_for_temporary_iterable_return_self.py` (+2 more) |
| async_with | `tests/differential/basic/async_for_with_exception_propagation.py`, `tests/differential/basic/async_with_basic.py`, `tests/differential/basic/async_with_instance_callable.py` (+20 more) |
| async_comprehension | `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_await_filters.py`, `tests/differential/basic/async_comprehensions_nested.py` (+1 more) |
| async_generator | `tests/differential/basic/async_comprehensions_nested.py`, `tests/differential/basic/async_generator_asend_after_close.py`, `tests/differential/basic/async_generator_asend_none_edges.py` (+19 more) |

## builtin_functions

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| print | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/arith.py` (+2549 more) |
| len | `tests/differential/basic/async_long_running.py`, `tests/differential/basic/builtins_api_surface_312_plus.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+1029 more) |
| range | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/async_comprehensions_nested.py`, `tests/differential/basic/async_hang_probe.py` (+77 more) |
| type | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_starred_nested.py`, `tests/differential/basic/assignment_starred_targets.py` (+1358 more) |
| isinstance | `tests/differential/basic/assignment_equality_side_effects.py`, `tests/differential/basic/builtins_basic.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+273 more) |
| issubclass | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+163 more) |
| int_constructor | `tests/differential/basic/bigint_ops.py`, `tests/differential/basic/builtin_keyword_args.py`, `tests/differential/basic/int_float_conversions.py` (+14 more) |
| float_constructor | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+165 more) |
| str_constructor | `tests/differential/basic/async_closure_decorators.py`, `tests/differential/basic/async_generator_reentrancy.py`, `tests/differential/basic/asyncgen_hooks_api.py` (+1009 more) |
| bool_constructor | `tests/differential/basic/bool_bool_len_precedence.py`, `tests/differential/basic/bool_len_exceptions.py`, `tests/differential/basic/bool_len_fallback.py` (+899 more) |
| list_constructor | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/builtin_constructor_dynamic.py`, `tests/differential/basic/builtin_iterators.py` (+167 more) |
| tuple_constructor | `tests/differential/basic/guard_dict_shape_mutation.py`, `tests/differential/basic/metaclass_execution.py`, `tests/differential/pyperformance/pyperformance_subset_smoke.py` (+20 more) |
| dict_constructor | `tests/differential/basic/composite_interactions.py`, `tests/differential/basic/dict_constructor_errors.py`, `tests/differential/basic/list_dict.py` (+16 more) |
| set_constructor | `tests/differential/basic/boolean_edges.py`, `tests/differential/basic/hashability.py`, `tests/differential/basic/set_basic.py` (+9 more) |
| frozenset_constructor | `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/frozenset_basic.py`, `tests/differential/basic/set_method_attrs.py` (+7 more) |
| bytes_constructor | `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py` (+176 more) |
| bytearray_constructor | `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/builtin_chr_ord.py`, `tests/differential/basic/builtin_formatting.py` (+48 more) |
| enumerate | `tests/differential/basic/composite_interactions.py`, `tests/differential/basic/enumerate_basic.py`, `tests/differential/basic/websocket_frame_control_ping_pong.py` (+3 more) |
| zip | `tests/differential/basic/builtin_iterators.py`, `tests/differential/basic/pep618_zip_strict.py` |
| map | `tests/differential/basic/builtin_iterators.py`, `tests/differential/basic/stress_structures_pass.py`, `tests/differential/basic/sum_map_function_defaults.py` (+1 more) |
| filter | `tests/differential/basic/builtin_iterators.py` |
| sorted | `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_await_filters.py`, `tests/differential/basic/async_generator_introspection.py` (+949 more) |
| reversed | `tests/differential/basic/builtin_iterators.py` |
| iter_builtin | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_unpack_error_order.py`, `tests/differential/basic/assignment_unpack_side_effects.py` (+20 more) |
| next_builtin | `tests/differential/basic/call_task_trampoline.py`, `tests/differential/basic/generator_close_multiple_yields.py`, `tests/differential/basic/generator_close_return_semantics.py` (+41 more) |
| abs | `tests/differential/basic/builtin_numeric_ops.py`, `tests/differential/basic/datamodel_dunder_ops.py`, `tests/differential/stdlib/cmath_basic.py` (+2 more) |
| min | `tests/differential/basic/builtin_reductions.py`, `tests/differential/stdlib/tkinter_phase0_core_semantics.py` |
| max | `tests/differential/basic/builtin_reductions.py` |
| sum | `tests/differential/basic/builtin_reductions.py`, `tests/differential/basic/sum_map_function_defaults.py`, `tests/differential/stdlib/glob_api_surface_312plus.py` |
| round | `tests/differential/basic/bigint_ops.py`, `tests/differential/basic/builtin_keyword_args.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+162 more) |
| pow_builtin | `tests/differential/basic/float_protocol.py`, `tests/differential/stdlib/math_edges.py`, `tests/differential/stdlib/math_log_zero_pow_overflow.py` |
| divmod | `tests/differential/basic/builtin_numeric_ops.py` |
| hash_builtin | `tests/differential/basic/dataclass_full.py`, `tests/differential/basic/object_hash_builtin.py`, `tests/differential/basic/slice_indices.py` (+4 more) |
| id_builtin | `tests/differential/stdlib/asyncio_runner_get_loop.py`, `tests/differential/stdlib/httpclient_connection_pool_basic.py` |
| repr_builtin | `tests/differential/basic/attr_security.py`, `tests/differential/basic/boolean_edges.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+183 more) |
| chr_builtin | `tests/differential/basic/builtin_chr_ord.py` |
| ord_builtin | `tests/differential/basic/builtin_chr_ord.py`, `tests/differential/basic/bytes_bytearray_ops.py`, `tests/differential/basic/negative_indexing.py` (+6 more) |
| hex_builtin | `tests/differential/basic/builtin_formatting.py` |
| oct_builtin | `tests/differential/basic/builtin_formatting.py` |
| bin_builtin | `tests/differential/basic/builtin_formatting.py` |
| all_builtin | `tests/differential/stdlib/asyncio_start_tls_signature.py`, `tests/differential/stdlib/bisect_api_surface.py`, `tests/differential/stdlib/platform_basic.py` (+5 more) |
| any_builtin | `tests/differential/basic/core_stdlib_intrinsics_smoke.py`, `tests/differential/stdlib/abc_basic.py`, `tests/differential/stdlib/http_upgrade_headers_httpclient.py` (+16 more) |
| callable_builtin | `tests/differential/basic/builtins_api_surface_312_plus.py`, `tests/differential/basic/builtins_basic.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+858 more) |
| getattr_builtin | `tests/differential/basic/assignment_equality_side_effects.py`, `tests/differential/basic/attr_hooks.py`, `tests/differential/basic/attr_security.py` (+929 more) |
| setattr_builtin | `tests/differential/basic/attr_security.py`, `tests/differential/basic/class_descriptors.py`, `tests/differential/basic/dict_subclass_slots.py` (+4 more) |
| delattr_builtin | `tests/differential/basic/attr_dunder_access.py`, `tests/differential/basic/attr_security.py`, `tests/differential/basic/class_layout_size_fallback.py` (+2 more) |
| hasattr_builtin | `tests/differential/basic/attr_dunder_access.py`, `tests/differential/basic/attribute_property_delete.py`, `tests/differential/basic/builtins_basic.py` (+64 more) |
| format_builtin | `tests/differential/basic/complex_format.py`, `tests/differential/basic/complex_format_errors.py`, `tests/differential/basic/complex_formatting.py` (+2 more) |
| ascii_builtin | `tests/differential/basic/builtin_chr_ord.py`, `tests/differential/basic/builtin_formatting.py` |
| open_builtin | `tests/differential/basic/capability_error_messages_io_net.py`, `tests/differential/basic/context_return_unwind_scope.py`, `tests/differential/basic/file_buffering_text.py` (+86 more) |
| super_builtin | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_unpack_error_order.py`, `tests/differential/basic/attribute_getattribute_fallback.py` (+34 more) |
| vars_builtin | `tests/differential/basic/dir_globals_locals_vars.py` |
| dir_builtin | `tests/differential/basic/attribute_dir_behavior.py`, `tests/differential/basic/builtins_api_surface_312_plus.py`, `tests/differential/basic/dir_globals_locals_vars.py` (+1 more) |
| globals_builtin | `tests/differential/basic/dir_globals_locals_vars.py`, `tests/differential/basic/globals_callable.py`, `tests/differential/basic/import_star.py` (+1 more) |
| locals_builtin | `tests/differential/basic/assignment_unpack_error_propagation.py`, `tests/differential/basic/builtins_name_resolution_locals_import.py`, `tests/differential/basic/dir_globals_locals_vars.py` (+4 more) |
| memoryview_constructor | `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/find_attr.py`, `tests/differential/basic/index_dunder.py` (+20 more) |
| complex_constructor | `tests/differential/basic/builtin_conversion_edges.py`, `tests/differential/stdlib/cmath_basic.py`, `tests/differential/stdlib/multiprocessing_codec_fastpath_transport.py` |
| slice_constructor | `tests/differential/basic/slice_indices.py`, `tests/differential/basic/slice_objects.py`, `tests/differential/stdlib/operator_basic.py` (+2 more) |
| object_constructor | `tests/differential/basic/builtin_chr_ord.py`, `tests/differential/basic/kwonly_method_return.py`, `tests/differential/basic/print_keywords.py` (+15 more) |
| property_builtin | `tests/differential/basic/descriptor_data_nondata_precedence.py`, `tests/differential/basic/descriptor_delete.py`, `tests/differential/basic/descriptor_precedence.py` (+1 more) |

### Gaps (Untested Features)

- `input_builtin`
- `classmethod_builtin`
- `staticmethod_builtin`
- `breakpoint_builtin`

## classes

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| class_def | `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/arith_dunder_floordiv_mod.py`, `tests/differential/basic/arith_dunder_mul_truediv.py` (+359 more) |
| inheritance | `tests/differential/basic/arith_dunder_subclass_precedence.py`, `tests/differential/basic/arith_reflected_subclass_priority.py`, `tests/differential/basic/attribute_lookup_order.py` (+110 more) |
| multiple_inheritance | `tests/differential/basic/class_mro_entries_multiple.py`, `tests/differential/basic/class_mro_entries_prepare_order.py`, `tests/differential/basic/class_mro_entries_with_bases.py` (+6 more) |
| class_decorator | `tests/differential/basic/attribute_lookup_order.py`, `tests/differential/basic/attribute_property_delete.py`, `tests/differential/basic/builtins_symbol_classmethod_c2ce1533.py` (+37 more) |
| staticmethod | `tests/differential/basic/class_descriptors.py`, `tests/differential/basic/getattr_calls.py` |
| classmethod | `tests/differential/basic/class_descriptors.py`, `tests/differential/basic/class_getattr_basic.py`, `tests/differential/basic/class_mro_entries_prepare_order.py` (+14 more) |
| property | `tests/differential/basic/attribute_lookup_order.py`, `tests/differential/basic/attribute_property_delete.py`, `tests/differential/basic/class_descriptors.py` (+4 more) |
| dunder_init | `tests/differential/basic/assignment_chain_order.py`, `tests/differential/basic/assignment_equality_side_effects.py`, `tests/differential/basic/assignment_target_eval_order.py` (+135 more) |
| dunder_repr | `tests/differential/basic/builtin_formatting.py`, `tests/differential/basic/compare_rich_result_passthrough.py`, `tests/differential/basic/datamodel_dunder_ops.py` (+2 more) |
| dunder_str | `tests/differential/basic/datamodel_dunder_ops.py`, `tests/differential/stdlib/csv_writer_custom_str.py`, `tests/differential/stdlib/string_format_method.py` |
| dunder_eq | `tests/differential/basic/assignment_equality_side_effects.py`, `tests/differential/basic/compare_eq_notimplemented.py`, `tests/differential/basic/compare_ne_dunder.py` (+11 more) |
| dunder_hash | `tests/differential/basic/hash_eq_interplay.py`, `tests/differential/stdlib/atexit_core_behaviors.py`, `tests/differential/stdlib/weakref_extended.py` (+1 more) |
| dunder_lt | `tests/differential/basic/compare_not_implemented.py`, `tests/differential/basic/compare_notimplemented_fallback.py`, `tests/differential/basic/compare_rich_result_passthrough.py` (+4 more) |
| dunder_le | `tests/differential/basic/compare_rich_result_passthrough.py`, `tests/differential/basic/list_sort.py`, `tests/differential/stdlib/functools_total_ordering_roots.py` |
| dunder_gt | `tests/differential/basic/compare_not_implemented.py`, `tests/differential/basic/compare_notimplemented_fallback.py`, `tests/differential/basic/list_sort.py` (+1 more) |
| dunder_ge | `tests/differential/basic/list_sort.py`, `tests/differential/stdlib/functools_total_ordering_roots.py` |
| dunder_add | `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/arith_dunder_precedence.py`, `tests/differential/basic/arith_dunder_subclass_precedence.py` (+1 more) |
| dunder_mul | `tests/differential/basic/arith_dunder_mul_truediv.py`, `tests/differential/basic/arith_reflected_ops.py` |
| dunder_truediv | `tests/differential/basic/arith_dunder_mul_truediv.py` |
| dunder_floordiv | `tests/differential/basic/arith_dunder_floordiv_mod.py` |
| dunder_mod | `tests/differential/basic/arith_dunder_floordiv_mod.py` |
| dunder_len | `tests/differential/basic/bool_bool_len_precedence.py`, `tests/differential/basic/bool_len_exceptions.py`, `tests/differential/basic/bool_len_fallback.py` (+10 more) |
| dunder_getitem | `tests/differential/basic/container_dunders.py`, `tests/differential/basic/iter_getitem_fallback.py`, `tests/differential/basic/iter_len_getitem_priority.py` (+6 more) |
| dunder_setitem | `tests/differential/basic/assignment_target_eval_order.py`, `tests/differential/basic/class_prepare_decorator_order.py`, `tests/differential/basic/class_prepare_decorator_order_extended.py` (+3 more) |
| dunder_delitem | `tests/differential/basic/del_semantics.py` |
| dunder_contains | `tests/differential/basic/container_dunders.py`, `tests/differential/basic/datamodel_dunder_ops.py`, `tests/differential/stdlib/collections_abc_basic.py` |
| dunder_iter | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_unpack_custom_iter.py`, `tests/differential/basic/assignment_unpack_error_custom_iter.py` (+17 more) |
| dunder_next | `tests/differential/basic/assignment_unpack_custom_iter.py`, `tests/differential/basic/assignment_unpack_error_custom_iter.py`, `tests/differential/basic/assignment_unpack_error_propagation.py` (+8 more) |
| dunder_call | `tests/differential/basic/attr_hooks.py`, `tests/differential/basic/call_indirect_dynamic_callable.py`, `tests/differential/basic/class_decorators.py` (+9 more) |
| dunder_enter | `tests/differential/basic/with_exception_chaining.py`, `tests/differential/basic/with_exit_raises.py`, `tests/differential/basic/with_exit_suppression.py` (+6 more) |
| dunder_exit | `tests/differential/basic/with_exception_chaining.py`, `tests/differential/basic/with_exit_raises.py`, `tests/differential/basic/with_exit_suppression.py` (+6 more) |
| dunder_del | `tests/differential/basic/finalizer_exit_semantics.py`, `tests/differential/basic/finalizer_resurrection_once.py` |
| dunder_bool | `tests/differential/basic/bool_bool_len_precedence.py`, `tests/differential/basic/bool_len_exceptions.py`, `tests/differential/basic/builtin_conversion_edges.py` (+2 more) |
| dunder_getattr | `tests/differential/basic/attr_dunder_access.py`, `tests/differential/basic/attr_hooks.py`, `tests/differential/basic/attr_security.py` (+8 more) |
| dunder_setattr | `tests/differential/basic/assignment_chain_order.py`, `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_target_eval_order.py` (+5 more) |
| dunder_delattr | `tests/differential/basic/attr_dunder_access.py`, `tests/differential/basic/attribute_set_delete_order.py`, `tests/differential/basic/del_semantics.py` (+1 more) |
| slots | `tests/differential/basic/builtins_symbol_copyright_521307dd.py`, `tests/differential/basic/builtins_symbol_credits_66c22fad.py`, `tests/differential/basic/builtins_symbol_help_92005ecf.py` (+9 more) |
| super_call | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_unpack_error_order.py`, `tests/differential/basic/attribute_getattribute_fallback.py` (+34 more) |

### Gaps (Untested Features)

- `dunder_sub`
- `dunder_pow`
- `dunder_neg`

## collection_operations

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| list_literal | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/assignment_aliasing_mutability.py` (+1478 more) |
| tuple_literal | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/assignment_aliasing_mutability.py` (+1259 more) |
| dict_literal | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/attr_hooks.py` (+1007 more) |
| set_literal | `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/boolean_edges.py`, `tests/differential/basic/builtins_symbol_globals_061e4a93.py` (+53 more) |
| subscript_access | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/assignment_aliasing_mutability.py`, `tests/differential/basic/assignment_target_eval_order.py` (+1328 more) |
| slice_access | `tests/differential/basic/assignment_aliasing_mutability.py`, `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/builtins_api_surface_312_plus.py` (+953 more) |
| list_method_call | `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/assignment_aliasing_mutability.py`, `tests/differential/basic/assignment_chain_descriptors.py` (+1167 more) |
| dict_method_call | `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_await_filters.py`, `tests/differential/basic/async_generator_introspection.py` (+1007 more) |
| set_method_call | `tests/differential/basic/class_descriptors.py`, `tests/differential/basic/composite_interactions.py`, `tests/differential/basic/hashability.py` (+19 more) |
| unpacking_assign | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_starred_nested.py`, `tests/differential/basic/assignment_starred_targets.py` (+865 more) |
| starred_unpack | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_starred_nested.py`, `tests/differential/basic/assignment_starred_targets.py` (+2 more) |
| dict_unpack | `tests/differential/basic/pep448_unpacking_generalizations.py`, `tests/differential/stdlib/json_encoder_hooks.py` |

## comprehensions

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| list_comprehension | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_await_filters.py` (+966 more) |
| dict_comprehension | `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_await_filters.py`, `tests/differential/basic/builtins_symbol_globals_061e4a93.py` (+14 more) |
| set_comprehension | `tests/differential/basic/comprehension_exception_propagation.py`, `tests/differential/basic/comprehensions.py`, `tests/differential/stdlib/ast_basic.py` (+4 more) |
| generator_expression | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/builtins_api_surface_312_plus.py` (+228 more) |
| nested_comprehension | `tests/differential/basic/async_comprehensions_nested.py`, `tests/differential/basic/comprehension_eval_order.py`, `tests/differential/basic/comprehension_nested_walrus.py` (+4 more) |
| comprehension_filter | `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_await_filters.py`, `tests/differential/basic/async_comprehensions_nested.py` (+35 more) |

## context_managers

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| with_statement | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+1093 more) |
| with_as | `tests/differential/basic/builtins_symbol_open_5fc7e38b.py`, `tests/differential/basic/capability_error_messages_io_net.py`, `tests/differential/basic/context_closing.py` (+222 more) |
| nested_with | `tests/differential/basic/with_multi_target_order.py`, `tests/differential/stdlib/cpython312plus_api_gap_submodule_asyncio_base_events_8f5b7366.py`, `tests/differential/stdlib/cpython312plus_api_gap_submodule_asyncio_base_futures_592aaacf.py` (+699 more) |

## control_flow

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| if_statement | `tests/differential/basic/arith.py`, `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/arith_dunder_floordiv_mod.py` (+1385 more) |
| if_else | `tests/differential/basic/arith.py`, `tests/differential/basic/boolean_edges.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+204 more) |
| elif | `tests/differential/basic/stress_structures_fail.py`, `tests/differential/basic/stress_structures_pass.py`, `tests/differential/basic/websocket_frame_control_ping_pong.py` (+6 more) |
| for_loop | `tests/differential/basic/async_comprehensions_nested.py`, `tests/differential/basic/async_hang_probe.py`, `tests/differential/basic/async_long_running.py` (+1022 more) |
| for_else | `tests/differential/basic/control_flow_complex.py`, `tests/differential/basic/control_flow_nested_break_regression.py`, `tests/differential/basic/for_else.py` (+1 more) |
| while_loop | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/exception_traceback_chain.py`, `tests/differential/basic/for_else.py` (+33 more) |
| while_else | `tests/differential/basic/for_else.py`, `tests/differential/basic/while_else_basic.py`, `tests/differential/pyperformance/fixtures/pyperformance_smoke/pyperformance/data-files/benchmarks/bm_fannkuch/run_benchmark.py` |
| break | `tests/differential/basic/async_for_else.py`, `tests/differential/basic/control_flow_complex.py`, `tests/differential/basic/control_flow_nested_break_regression.py` (+41 more) |
| continue | `tests/differential/basic/composite_interactions.py`, `tests/differential/basic/control_flow_complex.py`, `tests/differential/basic/control_flow_nested_break_regression.py` (+714 more) |
| pass | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/async_generator_athrow_after_stop.py`, `tests/differential/basic/async_generator_ge_after_stop.py` (+92 more) |
| match_statement | `tests/differential/basic/match_class_attr_access_errors.py`, `tests/differential/basic/match_class_attr_lookup_order.py`, `tests/differential/basic/match_class_getattribute_vs_getattr.py` (+21 more) |
| return | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/arith_builtin_reflected.py` (+1347 more) |
| yield | `tests/differential/basic/async_comprehensions_nested.py`, `tests/differential/basic/async_generator_asend_after_close.py`, `tests/differential/basic/async_generator_asend_none_edges.py` (+60 more) |
| yield_from | `tests/differential/basic/generator_introspection_attrs.py`, `tests/differential/basic/generator_methods.py`, `tests/differential/basic/generator_protocol.py` (+3 more) |
| raise | `tests/differential/basic/assignment_unpack_custom_iter.py`, `tests/differential/basic/assignment_unpack_error_propagation.py`, `tests/differential/basic/async_anext_default_future.py` (+330 more) |
| assert | `tests/differential/basic/pep701_fstring_comment_backslash_edges.py`, `tests/differential/basic/ws_pair_basic.py`, `tests/differential/stdlib/abc_subclasshook_return_contract.py` (+32 more) |

## exception_handling

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| try_except | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_starred_nested.py` (+1479 more) |
| try_finally | `tests/differential/basic/async_generator_close_semantics.py`, `tests/differential/basic/async_generator_finalization.py`, `tests/differential/basic/async_generator_protocol.py` (+143 more) |
| try_else | `tests/differential/basic/bytes_translate_maketrans.py`, `tests/differential/basic/codec_parity.py`, `tests/differential/basic/exception_chain_in_finally_else.py` (+80 more) |
| except_as | `tests/differential/basic/assignment_starred_error_order.py`, `tests/differential/basic/assignment_starred_nested.py`, `tests/differential/basic/assignment_starred_targets.py` (+1381 more) |
| except_star | `tests/differential/basic/exceptiongroup_basic.py`, `tests/differential/basic/exceptiongroup_except_star.py`, `tests/differential/basic/exceptiongroup_multiple_except_star.py` (+8 more) |
| bare_except | `tests/differential/basic/generator_reraise.py` |
| raise_from | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+166 more) |
| raise_bare | `tests/differential/basic/async_cancellation_token.py`, `tests/differential/basic/exception_reraise_clear_context.py`, `tests/differential/basic/exceptiongroup_multiple_except_star.py` (+15 more) |

## functions

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| function_def | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/arith_builtin_reflected.py` (+1830 more) |
| lambda | `tests/differential/basic/attr_security.py`, `tests/differential/basic/builtin_conversion_edges.py`, `tests/differential/basic/builtin_iterators.py` (+243 more) |
| default_args | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/attr_hooks.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+208 more) |
| star_args | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/async_closure_decorators.py` (+46 more) |
| star_kwargs | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/args_kwargs_eval_order.py`, `tests/differential/basic/async_closure_decorators.py` (+40 more) |
| keyword_only_args | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/class_decorators.py`, `tests/differential/basic/composite_interactions.py` (+13 more) |
| positional_only_args | `tests/differential/basic/args_kwargs.py`, `tests/differential/basic/function_call_posonly_kwonly_errors.py`, `tests/differential/basic/pep570_posonly_args.py` (+2 more) |
| decorator | `tests/differential/basic/async_closure_decorators.py`, `tests/differential/basic/attr_hooks.py`, `tests/differential/basic/attribute_lookup_order.py` (+63 more) |
| nested_function | `tests/differential/basic/async_cancellation_token.py`, `tests/differential/basic/async_closure_decorators.py`, `tests/differential/basic/async_with_instance_callable.py` (+72 more) |
| recursive_function | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+154 more) |
| closure | `tests/differential/basic/async_closure_decorators.py`, `tests/differential/basic/async_method_trampoline.py`, `tests/differential/basic/attr_hooks.py` (+45 more) |
| global_keyword | `tests/differential/basic/assignment_unboundlocal_global.py`, `tests/differential/basic/async_yield_spill.py`, `tests/differential/basic/del_global_basic.py` (+4 more) |
| nonlocal_keyword | `tests/differential/basic/comprehension_nested_nonlocal_defs.py`, `tests/differential/basic/comprehension_nonlocal_capture.py`, `tests/differential/basic/del_name.py` (+2 more) |

## imports

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| import_module | `tests/differential/basic/async_anext_default_future.py`, `tests/differential/basic/async_anext_future.py`, `tests/differential/basic/async_cancellation_token.py` (+2041 more) |
| import_from | `tests/differential/basic/assignment_annotations.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py` (+341 more) |
| import_alias | `tests/differential/basic/import_graph_cache.py`, `tests/differential/basic/pkg_basic/__init__.py`, `tests/differential/basic/pkg_graph/__init__.py` (+63 more) |
| import_star | `tests/differential/basic/import_star.py` |
| relative_import | `tests/differential/basic/rel_main_pkg/entry.py`, `tests/differential/basic/rel_pkg/mod_b.py`, `tests/differential/basic/rel_pkg/sub/mod_c.py` |

## operators

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| add | `tests/differential/basic/arith.py`, `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/arith_dunder_precedence.py` (+1012 more) |
| sub | `tests/differential/basic/call_indirect_dynamic_callable.py`, `tests/differential/basic/class_inheritance.py`, `tests/differential/basic/complex_format.py` (+30 more) |
| mult | `tests/differential/basic/arith.py`, `tests/differential/basic/arith_dunder_mul_truediv.py`, `tests/differential/basic/arith_reflected_ops.py` (+207 more) |
| div | `tests/differential/basic/arith_dunder_mul_truediv.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py` (+201 more) |
| floordiv | `tests/differential/basic/arith_dunder_floordiv_mod.py`, `tests/differential/basic/bigint_ops.py`, `tests/differential/basic/float_ops.py` (+2 more) |
| mod | `tests/differential/basic/arith_dunder_floordiv_mod.py`, `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_nested.py` (+14 more) |
| pow_op | `tests/differential/basic/builtin_formatting.py`, `tests/differential/basic/builtin_numeric_ops.py`, `tests/differential/basic/float_ops.py` (+9 more) |
| matmul | `tests/differential/basic/shift_matmul.py` |
| lshift | `tests/differential/basic/bigint_ops.py`, `tests/differential/basic/bitwise_int.py`, `tests/differential/basic/default_literals.py` (+6 more) |
| rshift | `tests/differential/basic/bigint_ops.py`, `tests/differential/basic/bitwise_int.py`, `tests/differential/basic/shift_matmul.py` |
| bitor | `tests/differential/basic/async_for_else.py`, `tests/differential/basic/bigint_ops.py`, `tests/differential/basic/bitwise_int.py` (+190 more) |
| bitxor | `tests/differential/basic/bitwise_int.py`, `tests/differential/basic/frozenset_basic.py`, `tests/differential/basic/set_algebra.py` (+6 more) |
| bitand | `tests/differential/basic/bitwise_int.py`, `tests/differential/basic/dict_view_set_ops.py`, `tests/differential/basic/frozenset_basic.py` (+23 more) |
| invert | `tests/differential/basic/bitwise_int.py` |
| unary_neg | `tests/differential/basic/async_closure_decorators.py`, `tests/differential/basic/bitwise_int.py`, `tests/differential/basic/boolean_edges.py` (+249 more) |
| not_op | `tests/differential/basic/boolean_edges.py`, `tests/differential/basic/builtins_api_surface_312_plus.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+938 more) |
| and_op | `tests/differential/basic/bool_short_circuit_order.py`, `tests/differential/basic/boolean_edges.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+241 more) |
| or_op | `tests/differential/basic/async_yield_spill.py`, `tests/differential/basic/bool_short_circuit_order.py`, `tests/differential/basic/boolean_edges.py` (+898 more) |
| eq | `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/arith_dunder_floordiv_mod.py`, `tests/differential/basic/arith_dunder_mul_truediv.py` (+1376 more) |
| noteq | `tests/differential/basic/compare_ne_dunder.py`, `tests/differential/basic/compare_ops.py`, `tests/differential/basic/compare_rich_result_passthrough.py` (+27 more) |
| lt | `tests/differential/basic/arith.py`, `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/async_yield_spill.py` (+189 more) |
| lte | `tests/differential/basic/async_for_iter.py`, `tests/differential/basic/compare_ops.py`, `tests/differential/basic/compare_rich_result_passthrough.py` (+13 more) |
| gt | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/async_comprehensions_await_filters.py`, `tests/differential/basic/builtins_symbol_abs_82451b41.py` (+194 more) |
| gte | `tests/differential/basic/async_anext_default_future.py`, `tests/differential/basic/async_anext_future.py`, `tests/differential/basic/async_comprehensions.py` (+47 more) |
| is_op | `tests/differential/basic/assignment_aliasing_mutability.py`, `tests/differential/basic/assignment_equality_side_effects.py`, `tests/differential/basic/async_generator_introspection.py` (+1012 more) |
| is_not | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+938 more) |
| in_op | `tests/differential/basic/assignment_unpack_error_propagation.py`, `tests/differential/basic/attr_dunder_access.py`, `tests/differential/basic/builtins_name_resolution_locals_import.py` (+865 more) |
| not_in | `tests/differential/basic/compare_ops.py`, `tests/differential/basic/pep701_fstring_comment_backslash_edges.py`, `tests/differential/basic/websocket_frame_control_ping_pong.py` (+12 more) |
| augmented_assign | `tests/differential/basic/assignment_unpack_custom_iter.py`, `tests/differential/basic/assignment_unpack_error_propagation.py`, `tests/differential/basic/async_anext_default_future.py` (+67 more) |
| walrus | `tests/differential/basic/assignment_walrus_scope.py`, `tests/differential/basic/comprehension_nested_walrus.py`, `tests/differential/basic/comprehension_walrus_and_or_filters.py` (+7 more) |

### Gaps (Untested Features)

- `unary_pos`

## string_operations

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| fstring | `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/assignment_chain_order.py`, `tests/differential/basic/assignment_target_eval_order.py` (+343 more) |
| fstring_expression | `tests/differential/basic/arith_builtin_reflected.py`, `tests/differential/basic/assignment_chain_order.py`, `tests/differential/basic/assignment_target_eval_order.py` (+343 more) |
| fstring_format_spec | `tests/differential/basic/complex_format.py`, `tests/differential/basic/format_numeric.py`, `tests/differential/basic/format_protocol.py` (+5 more) |
| fstring_debug | `tests/differential/basic/fstring_debug_format_spec.py`, `tests/differential/basic/fstring_format_specifiers.py`, `tests/differential/basic/pep701_fstring_comment_backslash_edges.py` (+1 more) |
| fstring_conversion | `tests/differential/basic/format_numeric.py`, `tests/differential/basic/fstring_debug_format_spec.py`, `tests/differential/basic/fstring_format_specifiers.py` (+4 more) |
| string_concat | `tests/differential/basic/async_closure_decorators.py`, `tests/differential/basic/builtins_symbol_super_8451ba8a.py`, `tests/differential/basic/class_mro_super.py` (+721 more) |
| string_repeat | `tests/differential/stdlib/csv_field_size_limit_error.py`, `tests/differential/stdlib/csv_large_field_size.py`, `tests/differential/stdlib/email_header_folding.py` (+1 more) |
| string_slice | `tests/differential/basic/assignment_aliasing_mutability.py`, `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/builtins_api_surface_312_plus.py` (+953 more) |
| string_method_call | `tests/differential/basic/async_with_basic.py`, `tests/differential/basic/builtin_keyword_args.py`, `tests/differential/basic/builtins_api_surface_312_plus.py` (+1242 more) |
| bytes_literal | `tests/differential/basic/assignment_annotations.py`, `tests/differential/basic/augassign_inplace.py`, `tests/differential/basic/boolean_edges.py` (+263 more) |
| raw_string | `tests/differential/stdlib/importlib_util_cache_from_source_intrinsic.py`, `tests/differential/stdlib/re_named_backref_lookaround.py`, `tests/differential/stdlib/re_verbose_flag.py` |
| multiline_string | `tests/differential/basic/builtins_symbol_abs_82451b41.py`, `tests/differential/basic/builtins_symbol_aiter_9064845b.py`, `tests/differential/basic/builtins_symbol_all_d87c4480.py` (+304 more) |

## type_hints

### Covered Features

| Feature | Test Files (sample) |
| --- | --- |
| function_annotation | `tests/differential/basic/assignment_annotations.py`, `tests/differential/basic/async_anext_default_future.py`, `tests/differential/basic/async_anext_future.py` (+1294 more) |
| variable_annotation | `tests/differential/basic/assignment_annotations.py`, `tests/differential/basic/async_for_else.py`, `tests/differential/basic/async_for_temporary_iterable_return_self.py` (+1025 more) |
| type_alias | `tests/differential/basic/pep695_type_params_syntax.py` |

### Gaps (Untested Features)

- `generic_class`
