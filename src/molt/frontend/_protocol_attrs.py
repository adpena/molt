"""Generated Protocol attribute base for the SimpleTIRGenerator surface."""

from __future__ import annotations

import ast
from typing import Any, Protocol, TYPE_CHECKING

from molt.frontend._types import (
    ActiveException,
    CFGGraph,
    CanonicalizationState,
    ClassInfo,
    ControlMaps,
    FallbackPolicy,
    FormatParseState,
    FormatToken,
    FuncInfo,
    IntrinsicHandleClassConstructorSpec,
    LoopBoundFact,
    MethodInfo,
    MidendEnvConfig,
    MidendFunctionPolicy,
    MidendProfile,
    MidendTier,
    MidendTierClassification,
    MoltOp,
    MoltValue,
    SCCPResult,
    TryScope,
    _ClassNsScope,
    _TrackedOpsList,
)

if TYPE_CHECKING:
    from molt.frontend.sema import SemaResult
    from molt.type_facts import TypeFacts


class _GeneratorProtocolAttrs(Protocol):
    _IMPORT_TRANSACTION_BOOTSTRAP_MODULES: frozenset[str]
    _STUB_IMPORT_MODULES: frozenset[str]
    _active_classcell_cell: MoltValue | None
    _active_midend_function_name: Any
    _class_body_depth: int
    _class_ns_stack: list[_ClassNsScope]
    _deferred_runtime_warnings: list[str]
    _emitted_syntax_warnings: set[tuple[str, int, str]]
    _expr_col: tuple[int, int] | None
    _inline_super_must_fold: bool
    _midend_env_snapshot: Any
    _midend_stats_reported: Any
    _module_attr_type_hints: dict[str, str]
    _module_cache_values: dict[str, MoltValue]
    _module_pressure_funcs_map_ref: Any
    _module_pressure_function_count: Any
    _module_pressure_total_ops: Any
    _op_by_result: dict[str, MoltOp]
    _sema: "SemaResult | None"
    _source_is_stdlib_module: Any
    _typing_import_aliases: set[str]
    active_exceptions: list[ActiveException]
    annotation_name_counter: Any
    annotation_type_params: dict[str, MoltValue]
    async_closure_offset: int | None
    async_context: Any
    async_index_loop_stack: list[int]
    async_internal_locals: set[str]
    async_local_hints: dict[str, str]
    async_locals: dict[str, int]
    async_locals_base: int
    async_public_locals: set[str]
    block_terminated: Any
    boxed_local_hints: dict[str, str]
    boxed_locals: dict[str, MoltValue]
    bytearray_len_hints: dict[str, int]
    class_annotation_exec_counter: Any
    class_annotation_exec_map: MoltValue | None
    class_annotation_exec_name: str | None
    class_annotation_items: list[tuple[str, ast.expr, int]]
    class_definition_pending: set[str]
    classes: dict[str, ClassInfo]
    closure_locals: set[str]
    code_id_counter: Any
    code_slots_emitted: Any
    comp_shadow_locals: set[str]
    compat: Any
    const_ints: dict[str, int]
    container_elem_hints: dict[str, str]
    context_depth: Any
    control_flow_depth: Any
    current_class: str | None
    current_func_name: str
    current_gpu_kernel_context: bool
    current_line: int | None
    current_method_first_param: str | None
    current_ops: list[MoltOp]
    defer_module_attrs: Any
    deferred_module_attrs: set[str]
    del_targets: set[str]
    dict_key_hints: dict[str, str]
    dict_value_hints: dict[str, str]
    eager_annotations: Any
    enable_phi: Any
    entry_module: Any
    exact_builtin_locals: dict[str, str]
    exact_locals: dict[str, str]
    exception_stack_depth_baseline: MoltValue | None
    exception_stack_prev_baseline: MoltValue | None
    explicit_type_hints: dict[str, str]
    fallback_policy: Any
    finally_depth: Any
    format_token_cache: dict[tuple[str, int, tuple[str, ...]], list[FormatToken]]
    free_var_hints: dict[str, str]
    free_vars: dict[str, int]
    func_aliases: dict[str, str]
    func_code_ids: dict[str, int]
    func_default_specs: dict[str, dict[str, Any]]
    func_symbol_names: dict[str, str]
    funcs_map: dict[str, FuncInfo]
    function_exception_label: int | None
    future_annotations: Any
    genexpr_counter: Any
    global_decls: set[str]
    global_dict_key_hints: dict[str, str]
    global_dict_value_hints: dict[str, str]
    global_elem_hints: dict[str, str]
    global_imported_attr_names: dict[str, str]
    global_imported_module_attr_mutations: set[tuple[str, str]]
    global_imported_modules: dict[str, str]
    global_imported_names: dict[str, str]
    globals: dict[str, MoltValue]
    globals_builtin_emitted: Any
    globals_builtin_val: MoltValue | None
    gpu_kernel_symbols_by_name: dict[str, str]
    imported_attr_names: dict[str, str]
    imported_module_attr_mutations: set[tuple[str, str]]
    imported_modules: dict[str, str]
    imported_names: dict[str, str]
    module_attr_overrides: set[tuple[str, str]]
    in_annotation: Any
    in_generator: Any
    instance_attr_mutations: dict[str, set[str]]
    known_func_defaults: dict[str, dict[str, dict[str, Any]]]
    known_func_kinds: dict[str, dict[str, str]]
    known_modules: Any
    lambda_counter: Any
    local_class_names: set[str]
    local_imported_modules: set[str]
    local_imported_names: set[str]
    local_intrinsic_wrappers: set[str]
    locals: dict[str, MoltValue]
    locals_cache_val: MoltValue | None
    loop_break_counter: Any
    loop_break_flags: list[int | str | None]
    loop_guard_assumptions: list[dict[str, tuple[str, bool]]]
    loop_layout_guards: list[dict[str, tuple[str, MoltValue]]]
    loop_static_class_counter: Any
    loop_static_class_eager_refs: list[set[str]]
    loop_static_class_refs: list[dict[str, MoltValue]]
    loop_try_depths: list[int]
    midend_env: Any
    midend_hot_functions: set[str]
    midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]]
    midend_policy_outcomes_by_function: dict[str, dict[str, Any]]
    midend_stats: dict[str, int]
    midend_stats_by_function: dict[str, dict[str, int]]
