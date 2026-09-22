"""GeneratorStateMixin: generator construction and reset-state authority.

Owns the assembled generator's constructor plus shared reset primitives for
per-function and module-chunk transient state. The constructor body is a
move-only extraction from frontend/__init__.py; reset helpers centralize the
state lists that were previously duplicated across constructor, start_function,
and module chunk reset paths.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Any, Literal

from molt.frontend._types import (
    _ClassNsScope,
    AsyncFrameSlot,
    ClassInfo,
    CompatibilityReporter,
    ComprehensionBinding,
    ExactClassFact,
    FallbackPolicy,
    FormatToken,
    FuncInfo,
    LoopScope,
    MidendProfile,
    MoltOp,
    MoltValue,
    ScratchCell,
)
from molt.frontend.sema import FunctionKind, SemaResult
from molt.native_callable_exports import (
    NativeCallableExportError,
    normalize_native_callable_export,
)

if TYPE_CHECKING:
    from molt.compiler_analysis.python_imports import ModuleExecutionKind
    from molt.frontend._protocol import _GeneratorProtocol
    from molt.type_facts import TypeFacts

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


FUNCTION_LOCAL_BINDING_STATE_ATTRS = (
    "locals",
    "locals_cache_cell",
    "compiler_bindings",
    "parameter_bindings",
    "boxed_locals",
    "closure_locals",
    "comp_shadow_locals",
    "comprehension_bindings",
    "current_python_first_arg",
    "python_frame_context_active",
    "python_class_body_context",
    "_class_ns_stack",
    "_class_body_depth",
    "boxed_local_hints",
    "free_vars",
    "free_var_hints",
    "global_decls",
    "nonlocal_decls",
    "scope_assigned",
    "del_targets",
    "unbound_check_names",
    "exact_locals",
    "exact_class_token",
)

FUNCTION_IMPORT_RESOLUTION_STATE_ATTRS = (
    "imported_names",
    "imported_attr_names",
    "imported_modules",
    "imported_module_provenance",
    "_module_provenance_flow_stack",
    "local_imported_names",
    "local_imported_modules",
    "imported_module_attr_mutations",
)

FUNCTION_ASYNC_SCOPE_STATE_ATTRS = (
    "async_locals",
    "async_frame_slots",
    "async_internal_bindings",
    "async_spill_slots",
    "async_locals_base",
    "async_closure_offset",
    "async_public_hints",
    "async_internal_hints",
    "async_spill_hints",
)

FUNCTION_TYPE_HINT_SCOPE_STATE_ATTRS = (
    "explicit_type_hints",
    "container_elem_hints",
    "dict_key_hints",
    "dict_value_hints",
    "bytearray_len_hints",
)

FUNCTION_CACHE_STATE_ATTRS = (
    "const_ints",
    "_op_by_result",
    "_module_cache_values",
    "in_generator",
    "async_context",
    "current_line",
)

FUNCTION_CONTROL_FLOW_STATE_ATTRS = (
    "control_flow_depth",
    "try_end_labels",
    "try_scopes",
    "try_suppress_depth",
    "try_handler_scopes",
    "function_exception_label",
    "exception_stack_depth_baseline",
    "exception_stack_prev_baseline",
    "return_unwind_depth",
    "finally_depth",
    "active_exceptions",
    "loop_scopes",
    "loop_layout_guards",
    "loop_guard_assumptions",
    "return_label",
    "return_slot",
    "return_slot_offset",
    "block_terminated",
)

FUNCTION_CONTEXT_STATE_ATTRS = (
    "current_class",
    "current_method_first_param",
    "defer_module_attrs",
    "deferred_module_attrs",
    "class_definition_pending",
)

FUNCTION_STATE_SNAPSHOT_ATTRS = (
    FUNCTION_CONTEXT_STATE_ATTRS
    + FUNCTION_LOCAL_BINDING_STATE_ATTRS
    + FUNCTION_ASYNC_SCOPE_STATE_ATTRS
    + FUNCTION_TYPE_HINT_SCOPE_STATE_ATTRS
    + FUNCTION_CONTROL_FLOW_STATE_ATTRS
    + FUNCTION_IMPORT_RESOLUTION_STATE_ATTRS
    + FUNCTION_CACHE_STATE_ATTRS
)


class GeneratorStateMixin(_MixinBase):
    comprehension_bindings: dict[str, ComprehensionBinding]
    current_python_first_arg: str | MoltValue | None
    _next_exact_class_token: int
    exact_class_token: int
    exact_locals: dict[str, ExactClassFact]
    loop_layout_guards: list[dict[str, tuple[str, MoltValue, int]]]
    loop_guard_assumptions: list[dict[str, tuple[str, bool, int]]]

    def _capture_state_attrs(self, attrs: tuple[str, ...]) -> dict[str, Any]:
        missing = [name for name in attrs if not hasattr(self, name)]
        if missing:
            raise AssertionError(
                f"state capture requested uninitialized attrs: {missing}"
            )
        return {name: getattr(self, name) for name in attrs}

    def _restore_state_attrs(
        self, attrs: tuple[str, ...], state: dict[str, Any]
    ) -> None:
        expected = set(attrs)
        actual = set(state)
        if actual != expected:
            missing = sorted(expected - actual)
            extra = sorted(actual - expected)
            raise AssertionError(
                f"state restore payload drift: missing={missing} extra={extra}"
            )
        for name in attrs:
            setattr(self, name, state[name])

    def _capture_function_scope_state(self) -> dict[str, Any]:
        return self._capture_state_attrs(FUNCTION_STATE_SNAPSHOT_ATTRS)

    def _restore_function_scope_state(self, state: dict[str, Any]) -> None:
        self._restore_state_attrs(FUNCTION_STATE_SNAPSHOT_ATTRS, state)

    def _reset_local_binding_state(
        self,
        *,
        reset_locals_cache: bool,
        reset_del_targets: bool,
    ) -> None:
        self.locals = {}
        if reset_locals_cache:
            # Backing store for the current frame's `locals()` snapshot semantics.
            # Stored outside `self.locals` to avoid accidental shadowing/rewrites.
            self.locals_cache_cell: ScratchCell | None = None
        self.compiler_bindings: dict[str, MoltValue] = {}
        self.parameter_bindings: dict[str, str] = {}
        self.boxed_locals = {}
        self.closure_locals = set()
        self.comp_shadow_locals = set()
        self.comprehension_bindings: dict[str, ComprehensionBinding] = {}
        self.current_python_first_arg: str | MoltValue | None = None
        self.python_frame_context_active = False
        self.python_class_body_context = False
        # A real function/code-object boundary does not inherit class LOAD_NAME
        # storage. Annotation evaluators capture their required owners through
        # explicit closure inputs rather than retaining outer-function SSA.
        self._class_ns_stack: list[_ClassNsScope] = []
        self._class_body_depth: int = 0
        self.boxed_local_hints = {}
        self.free_vars = {}
        self.free_var_hints = {}
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        if reset_del_targets:
            self.del_targets = set()
        self.unbound_check_names = set()
        self.exact_class_token = self._advance_exact_class_token()
        self.exact_locals = {}

    def _reset_import_resolution_state(
        self,
        *,
        reset_module_attr_mutations: bool,
    ) -> None:
        self.imported_names = dict(self.global_imported_names)
        self.imported_attr_names = dict(self.global_imported_attr_names)
        self.imported_modules = dict(self.global_imported_modules)
        self.imported_module_provenance = dict(self.global_imported_module_provenance)
        self._module_provenance_flow_stack = []
        self.local_imported_names = set()
        self.local_imported_modules = set()
        if reset_module_attr_mutations:
            self.imported_module_attr_mutations = set(
                self.global_imported_module_attr_mutations
            )

    def _reset_async_scope_state(self) -> None:
        self.async_locals: dict[str, AsyncFrameSlot] = {}
        self.async_frame_slots: list[AsyncFrameSlot] = []
        self.async_internal_bindings: dict[str, AsyncFrameSlot] = {}
        self.async_spill_slots: dict[str, AsyncFrameSlot] = {}
        self.async_locals_base = 0
        self.async_closure_offset = None
        self.async_public_hints: dict[str, str] = {}
        self.async_internal_hints: dict[str, str] = {}
        self.async_spill_hints: dict[str, str] = {}

    def _reset_type_hint_scope_state(self, *, reset_bytearray_len: bool) -> None:
        self.explicit_type_hints = {}
        self.container_elem_hints = {}
        self.dict_key_hints = {}
        self.dict_value_hints = {}
        if reset_bytearray_len:
            self.bytearray_len_hints = {}

    def _reset_function_cache_state(self) -> None:
        self.const_ints = {}
        # Producing-op index (result SSA name -> MoltOp), maintained by emit().
        # Value names are globally unique (next_var), so no per-function reset is
        # needed beyond clearing the current function/chunk view.
        self._op_by_result = {}
        # Per-function cache: module name -> cached MoltValue from MODULE_CACHE_GET.
        self._module_cache_values = {}
        self.in_generator = False
        self.async_context = False
        self.current_line = None

    def _reset_control_flow_state(
        self,
        *,
        reset_function_exception_label: bool,
    ) -> None:
        self.control_flow_depth = 0
        self.try_end_labels = []
        self.try_scopes = []
        self.try_suppress_depth = None
        self.try_handler_scopes = []
        if reset_function_exception_label:
            self.function_exception_label = self.next_label()
        self.exception_stack_depth_baseline = None
        self.exception_stack_prev_baseline = None
        self.return_unwind_depth = 0
        self.finally_depth = 0
        self.return_label = None
        self.return_slot = None
        self.return_slot_offset = None
        self.block_terminated = False
        self.loop_scopes: list[LoopScope] = []
        self.loop_layout_guards = []
        self.loop_guard_assumptions = []
        self.active_exceptions = []

    def __init__(
        self,
        parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
        type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
        fallback_policy: FallbackPolicy = "error",
        source_path: str | None = None,
        type_facts: "TypeFacts | None" = None,
        type_facts_module: str | None = None,
        module_name: str | None = None,
        module_spec_name: str | None = None,
        module_is_namespace: bool = False,
        module_execution_kind: ModuleExecutionKind | None = None,
        entry_module: str | None = None,
        enable_phi: bool = True,
        known_modules: set[str] | None = None,
        direct_call_modules: set[str] | None = None,
        known_classes: dict[str, ClassInfo] | None = None,
        stdlib_allowlist: set[str] | None = None,
        known_func_defaults: dict[str, dict[str, dict[str, Any]]] | None = None,
        known_func_kinds: dict[str, dict[str, str]] | None = None,
        native_callable_exports: dict[str, dict[str, Any]] | None = None,
        native_python_exports: set[str] | None = None,
        native_support_function_roots: set[str] | None = None,
        target_python: tuple[int, int] = (3, 12),
        target_sys_platform: str | None = None,
        module_chunking: bool = False,
        module_chunk_max_ops: int = 0,
        optimization_profile: MidendProfile = "release",
        pgo_hot_functions: set[str] | None = None,
    ) -> None:
        self._module_pressure_function_count = 0
        self._module_pressure_total_ops = 0
        self.funcs_map: dict[str, FuncInfo] = {
            "molt_main": {
                "params": [],
                "param_types": [],
                "return_abi": "void",
                "return_hint": None,
                "ops": self._new_tracked_ops(),
            }
        }
        self._module_pressure_funcs_map_ref = self.funcs_map
        self.current_func_name: str = "molt_main"
        self.current_ops: list[MoltOp] = self.funcs_map["molt_main"]["ops"]
        self.func_code_ids: dict[str, int] = {}
        self.code_id_counter = 0
        self.code_slots_emitted = False
        self.var_count: int = 0
        self.reserved_python_identifiers: set[str] = set()
        self.state_count: int = 0
        self.known_classes: dict[str, ClassInfo] = dict(known_classes or {})
        self.classes: dict[str, ClassInfo] = dict(self.known_classes)
        self.local_class_names: set[str] = set()
        # Token allocation belongs to the generator, not to a restorable
        # function scope. Restoring an outer scope must never recycle a token
        # that was already used while lowering a nested function.
        self._next_exact_class_token = 0
        self._reset_local_binding_state(
            reset_locals_cache=True,
            reset_del_targets=True,
        )
        self._expr_col: tuple[int, int] | None = (
            None  # expression-level col_offset for traceback carets
        )
        self.globals: dict[str, MoltValue] = {}
        # Final entry-module callable binding state.  Publication/import/delete
        # lowering updates this table in execution order; serialization carries
        # it on the entry module initializer for linker contract projection.
        self.app_callable_bindings: dict[str, dict[str, Any]] = {}
        self.module_chunk_globals: set[str] = set()
        self.func_symbol_names: dict[str, str] = {}
        self.func_default_specs: dict[str, dict[str, Any]] = {}
        self.stable_module_funcs: set[str] = set()
        self.module_declared_funcs: dict[str, FunctionKind] = {}
        self.module_declared_classes: set[str] = set()
        self.stable_module_classes: set[str] = set()
        self.module_defined_funcs: set[str] = set()
        self.module_elided_deleted_funcs: set[str] = set()
        self.class_definition_pending: set[str] = set()
        self.module_global_mutations: set[str] = set()
        self.module_globals_dict_escaped = False
        # Track the last-known type hint for module-scope attributes.
        # Populated by _emit_module_attr_set_on and read by _emit_module_attr_get.
        # Enables fast_int/fast_float paths for module-scope loop variables.
        self._module_attr_type_hints: dict[str, str] = {}
        self.mutated_classes: set[str] = set()
        self.instance_attr_mutations: dict[str, set[str]] = {}
        self.global_imported_names: dict[str, str] = {}
        # Maps bind_name -> original attr_name for `from X import Y as Z`
        # (bind_name="Z", attr_name="Y").  Used to resolve cross-module call
        # targets to the original function name rather than the alias.
        self.global_imported_attr_names: dict[str, str] = {}
        self.global_imported_modules: dict[str, str] = {}
        self.global_imported_module_provenance: dict[str, frozenset[str]] = {}
        self.global_imported_module_attr_mutations: set[tuple[str, str]] = set()
        self._reset_import_resolution_state(reset_module_attr_mutations=True)
        self.local_intrinsic_wrappers: set[str] = set()
        self.gpu_kernel_symbols_by_name: dict[str, str] = {}
        self.current_gpu_kernel_context: bool = False
        # Track aliases for ``import typing as <alias>`` so that
        # ``@<alias>.overload`` is recognised as a typing overload stub.
        self._typing_import_aliases: set[str] = set()
        self._reset_async_scope_state()
        if target_python not in {(3, 12), (3, 13), (3, 14)}:
            raise ValueError(
                "target_python must be one of the supported feature versions: "
                "(3, 12), (3, 13), or (3, 14)"
            )
        self.target_python: tuple[int, int] = target_python
        # Python 3.14 owns deferred evaluation through __annotate__; 3.12/3.13
        # retain eager evaluation unless future annotations applies.
        self.eager_annotations = target_python < (3, 14)
        self.parse_codec = parse_codec
        self.type_hint_policy = type_hint_policy
        self.annotation_type_params: dict[str, MoltValue] = {}
        self.in_annotation = False
        self._reset_type_hint_scope_state(reset_bytearray_len=True)
        self.global_elem_hints: dict[str, str] = {}
        self.stdlib_hint_trust = False
        if source_path:
            normalized_path = source_path.replace("\\", "/")
            if "/src/molt/stdlib/" in normalized_path or normalized_path.startswith(
                "src/molt/stdlib/"
            ):
                self.stdlib_hint_trust = True
        self._source_is_stdlib_module = self.stdlib_hint_trust
        self.target_sys_platform = target_sys_platform
        self.global_dict_key_hints: dict[str, str] = {}
        self.global_dict_value_hints: dict[str, str] = {}
        self.type_facts = type_facts
        self._init_module_lifecycle_state(
            source_path=source_path,
            module_name=module_name,
            module_spec_name=module_spec_name,
            module_is_namespace=module_is_namespace,
            module_execution_kind=module_execution_kind,
            entry_module=entry_module,
            module_chunking=module_chunking,
            module_chunk_max_ops=module_chunk_max_ops,
            known_modules=known_modules,
            direct_call_modules=direct_call_modules,
            stdlib_allowlist=stdlib_allowlist,
            target_python=target_python,
        )
        self.type_facts_module = type_facts_module or self.module_name
        self.enable_phi = enable_phi
        self.known_func_defaults: dict[str, dict[str, dict[str, Any]]] = (
            known_func_defaults or {}
        )
        self.known_func_kinds: dict[str, dict[str, str]] = known_func_kinds or {}
        self.native_callable_exports: dict[str, dict[str, Any]] = {}
        for qualified_name, spec in (native_callable_exports or {}).items():
            if not isinstance(qualified_name, str) or not isinstance(spec, dict):
                raise ValueError(
                    "native_callable_exports must map qualified names to objects"
                )
            try:
                normalized = normalize_native_callable_export(
                    spec, qualified_name=qualified_name
                )
            except NativeCallableExportError as exc:
                raise ValueError(
                    f"native callable export {qualified_name!r} is invalid: {exc}"
                ) from exc
            self.native_callable_exports[qualified_name] = normalized.digest_payload()
        self.native_python_exports: set[str] = {
            qualified_name
            for qualified_name in (native_python_exports or set())
            if isinstance(qualified_name, str) and qualified_name
        }
        self.native_support_function_roots: set[str] = {
            name
            for name in (native_support_function_roots or set())
            if isinstance(name, str) and name
        }
        self.module_func_defaults: dict[str, dict[str, Any]] = {}
        self.module_annotations: MoltValue | None = None
        self.module_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.module_annotation_ids: dict[int, int] = {}
        self.module_annotation_exec_map: MoltValue | None = None
        self.module_annotation_exec_name: str | None = None
        self.module_annotation_exec_counter = 0
        self.module_annotation_emitted = False
        self.module_annotations_conditional = False
        self._init_midend_state(optimization_profile, pgo_hot_functions)
        self.class_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.class_annotation_exec_map: MoltValue | None = None
        self.class_annotation_exec_counter = 0
        self.annotation_name_counter = 0
        self.future_annotations = False
        self.defer_module_attrs = False
        self.deferred_module_attrs: set[str] = set()
        self.fallback_policy = fallback_policy
        self.compat = CompatibilityReporter(fallback_policy, source_path)
        self._emitted_syntax_warnings: set[tuple[str, int, str]] = set()
        self._deferred_runtime_warnings: list[str] = []
        self._reset_control_flow_state(reset_function_exception_label=True)
        self.func_aliases: dict[str, str] = {}
        self.reserved_func_symbols: dict[str, str] = {}
        self._reset_function_cache_state()
        self.format_token_cache: dict[
            tuple[str, int, tuple[str, ...]], list[FormatToken]
        ] = {}
        self.lambda_counter = 0
        self.genexpr_counter = 0
        self.qualname_stack: list[tuple[str, bool]] = []
        self.current_class: str | None = None
        self.current_method_first_param: str | None = None
        # Module-level constant dicts: name → {str_key: constant_value}
        # Populated during visit_Module to support compile-time **kwargs resolution
        # (e.g. @dataclass(**SLOTS) where SLOTS = {"slots": True}).
        self.module_const_dicts: dict[str, dict[str, Any]] = {}
        # The immutable SemaResult for the module currently being lowered (doc 44
        # §F2b). Computed once, pre-walk, by _populate_sema_state in visit_Module;
        # None until then (and for direct non-module lowering entry points).
        self._sema: SemaResult | None = None
        self._register_code_symbol("molt_main")
        self._emit_initial_module_object()
        self._apply_type_facts("molt_main")
