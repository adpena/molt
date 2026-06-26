"""Frontend TIR generator: SimpleTIRGenerator assembly + public API.

The generator is composed from visitor/lowering mixins (F1 decomposition,
move-only). Shared data types, constants, and lookup tables live in the
``molt.frontend._types`` leaf and are re-exported here so the historical public
import surface (``from molt.frontend import MoltValue, MoltOp, ...``) is
preserved exactly. Per-family method bodies live under ``molt.frontend.visitors``
and ``molt.frontend.lowering`` and are mixed into the class below via MRO.
"""

from __future__ import annotations

import ast
from typing import (
    TYPE_CHECKING,
    Any,
    Literal,
)

# Shared frontend data types / constants / tables (the import-graph leaf).
# Imported explicitly (no star) and re-exported via __all__ so the historical
# ``from molt.frontend import <name>`` surface is preserved exactly. This single
# source also covers the compat / cfg_analysis / type_facts names the assembly
# class uses (they are re-exported from _types).
from molt.frontend._types import (
    _IC_TABLE_CAPACITY,
    _ic_counter,
    _STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF,
    _next_ic_index,
    _InlineSuperFoldRequired,
    _ClassNsScope,
    MoltValue,
    MoltOp,
    SCCPResult,
    LoopBoundFact,
    _INLINE_INT_MIN,
    _INLINE_INT_MAX,
    _FAST_ARITH_OPS,
    _SCCP_OVERDEFINED,
    _SCCP_UNKNOWN,
    _SCCP_MISSING,
    MidendProfile,
    MidendTier,
    _MIDEND_ENV_KEYS,
    MidendTierClassification,
    MidendFunctionPolicy,
    MidendEnvConfig,
    ActiveException,
    BuiltinFuncSpec,
    FormatLiteral,
    FormatField,
    FormatToken,
    FormatParseState,
    GEN_SEND_OFFSET,
    GEN_THROW_OFFSET,
    GEN_CLOSED_OFFSET,
    GEN_YIELD_FROM_OFFSET,
    GEN_CONTROL_SIZE,
    BUILTIN_TYPE_TAGS,
    BUILTIN_LAYOUT_MIN,
    IMPLICIT_CLASSMETHOD_NAMES,
    IMPLICIT_STATICMETHOD_NAMES,
    _function_is_instance_method,
    _BUILTIN_FAST_METHODS,
    BUILTIN_EXCEPTION_NAMES,
    BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS,
    _MOLT_MISSING,
    _MOLT_CLOSURE_PARAM,
    _MOLT_LOCALS_CACHE,
    _MOLT_GLOBALS_BUILTIN,
    _MOLT_MODULE_CHUNK_PARAM,
    _MOLT_MODULE_CHUNK_PREFIX,
    _BOOTSTRAP_TRACE_EXEMPT_MODULES,
    MOLT_BIND_KIND_OPEN,
    BUILTIN_FUNC_SPECS,
    _INTRINSIC_ARITY_CACHE,
    _INTRINSIC_SYMBOL_CACHE,
    _INTRINSIC_DEFAULTS_CACHE,
    _ensure_intrinsic_arity_cache,
    _ensure_intrinsic_symbol_cache,
    _ensure_intrinsic_defaults_cache,
    _canonical_intrinsic_runtime_name,
    _intrinsic_arity_exact,
    _intrinsic_defaults_exact,
    _intrinsic_arity,
    MOLT_REEXPORT_FUNCTIONS,
    MOLT_DIRECT_CALLS,
    MOLT_DIRECT_CALL_BIND_ALWAYS,
    IntrinsicHandleClassConstructorSpec,
    INTRINSIC_HANDLE_CLASS_CONSTRUCTORS,
    INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE,
    STDLIB_DIRECT_CALL_MODULES,
    TryScope,
    MethodInfo,
    ClassInfo,
    FuncInfo,
    _TrackedOpsList,
    CanonicalizationState,
    _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY,
    CompatibilityError,
    CompatibilityReporter,
    FallbackPolicy,
    CFGGraph,
    ControlMaps,
    build_cfg,
    normalize_type_hint,
)

# Bind/Sema phase (doc 44 §F2b): free functions over the immutable ast.Module
# computing the SemaResult — immutable semantic facts (static class graph, const
# environment, top-level function metadata) — once, pre-walk, in the cfg_analysis.py
# house shape. F2b is additive: visit_Module populates the existing pre-walk state
# dicts FROM SemaResult (the _populate_sema_state shim) so the walk is byte-identical;
# F2c rewires the read-sites onto SemaResult and deletes the mutable god-object dicts.
from molt.frontend.sema import (
    FunctionKind,
    SemaResult,
    analyze_module,
)

# Visitor / lowering mixins composed into SimpleTIRGenerator (F1 decomposition).
from molt.frontend.lowering.analysis_collect_static import AnalysisCollectStaticMixin
from molt.frontend.lowering.analysis_patterns import AnalysisPatternMixin
from molt.frontend.lowering.attribute_access import AttributeAccessMixin
from molt.frontend.lowering.class_resolution import ClassResolutionMixin
from molt.frontend.lowering.compile_warnings import CompileWarningMixin
from molt.frontend.lowering.emission_core import EmissionCoreMixin
from molt.frontend.lowering.exception_lowering import ExceptionLoweringMixin
from molt.frontend.lowering.expression_primitives import ExpressionPrimitivesMixin
from molt.frontend.lowering.function_lifecycle import FunctionLifecycleMixin
from molt.frontend.lowering.function_metadata import FunctionMetadataMixin
from molt.frontend.lowering.import_lowering import ImportLoweringMixin
from molt.frontend.lowering.local_bindings import LocalBindingMixin
from molt.frontend.lowering.loop_lowering import LoopLoweringMixin
from molt.frontend.lowering.midend_optimization import MidendOptimizationMixin
from molt.frontend.lowering.module_globals import ModuleGlobalsMixin
from molt.frontend.lowering.module_lifecycle import ModuleLifecycleMixin
from molt.frontend.lowering.ownership_lowering import OwnershipLoweringMixin
from molt.frontend.lowering.runtime_references import RuntimeReferenceMixin
from molt.frontend.lowering.serialization import SerializationMixin
from molt.frontend.lowering.sema_state import SemaStateMixin
from molt.frontend.lowering.string_formatting import StringFormattingMixin
from molt.frontend.lowering.symbol_naming import SymbolNamingMixin
from molt.frontend.lowering.type_annotations import TypeAnnotationMixin
from molt.frontend.visitors.async_gen import AsyncGenVisitorMixin
from molt.frontend.visitors.pattern_match import PatternMatchMixin
from molt.frontend.visitors.calls import CallVisitorMixin
from molt.frontend.visitors.classes import ClassDefVisitorMixin
from molt.frontend.visitors.comprehensions import ComprehensionMixin
from molt.frontend.visitors.expressions import ExpressionVisitorMixin
from molt.frontend.visitors.functions import FunctionVisitorMixin
from molt.frontend.visitors.statement_assignments import (
    AssignmentStatementVisitorMixin,
)
from molt.frontend.visitors.statement_control_flow import (
    ControlFlowStatementVisitorMixin,
)
from molt.frontend.visitors.statement_scope import StatementScopeVisitorMixin

if TYPE_CHECKING:
    from molt.type_facts import TypeFacts

__all__ = [
    "_IC_TABLE_CAPACITY",
    "_ic_counter",
    "_STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF",
    "_next_ic_index",
    "_InlineSuperFoldRequired",
    "MoltValue",
    "MoltOp",
    "SCCPResult",
    "LoopBoundFact",
    "_INLINE_INT_MIN",
    "_INLINE_INT_MAX",
    "_FAST_ARITH_OPS",
    "_SCCP_OVERDEFINED",
    "_SCCP_UNKNOWN",
    "_SCCP_MISSING",
    "MidendProfile",
    "MidendTier",
    "_MIDEND_ENV_KEYS",
    "MidendTierClassification",
    "MidendFunctionPolicy",
    "MidendEnvConfig",
    "ActiveException",
    "BuiltinFuncSpec",
    "FormatLiteral",
    "FormatField",
    "FormatToken",
    "FormatParseState",
    "GEN_SEND_OFFSET",
    "GEN_THROW_OFFSET",
    "GEN_CLOSED_OFFSET",
    "GEN_YIELD_FROM_OFFSET",
    "GEN_CONTROL_SIZE",
    "BUILTIN_TYPE_TAGS",
    "BUILTIN_LAYOUT_MIN",
    "IMPLICIT_CLASSMETHOD_NAMES",
    "IMPLICIT_STATICMETHOD_NAMES",
    "_function_is_instance_method",
    "_BUILTIN_FAST_METHODS",
    "BUILTIN_EXCEPTION_NAMES",
    "BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS",
    "_MOLT_MISSING",
    "_MOLT_CLOSURE_PARAM",
    "_MOLT_LOCALS_CACHE",
    "_MOLT_GLOBALS_BUILTIN",
    "_MOLT_MODULE_CHUNK_PARAM",
    "_MOLT_MODULE_CHUNK_PREFIX",
    "_BOOTSTRAP_TRACE_EXEMPT_MODULES",
    "MOLT_BIND_KIND_OPEN",
    "BUILTIN_FUNC_SPECS",
    "_INTRINSIC_ARITY_CACHE",
    "_INTRINSIC_SYMBOL_CACHE",
    "_INTRINSIC_DEFAULTS_CACHE",
    "_ensure_intrinsic_arity_cache",
    "_ensure_intrinsic_symbol_cache",
    "_ensure_intrinsic_defaults_cache",
    "_canonical_intrinsic_runtime_name",
    "_intrinsic_arity_exact",
    "_intrinsic_defaults_exact",
    "_intrinsic_arity",
    "MOLT_REEXPORT_FUNCTIONS",
    "MOLT_DIRECT_CALLS",
    "MOLT_DIRECT_CALL_BIND_ALWAYS",
    "IntrinsicHandleClassConstructorSpec",
    "INTRINSIC_HANDLE_CLASS_CONSTRUCTORS",
    "INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE",
    "STDLIB_DIRECT_CALL_MODULES",
    "TryScope",
    "MethodInfo",
    "ClassInfo",
    "FuncInfo",
    "_TrackedOpsList",
    "CanonicalizationState",
    "_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY",
    "CompatibilityError",
    "CompatibilityReporter",
    "FallbackPolicy",
    "CFGGraph",
    "ControlMaps",
    "build_cfg",
    "normalize_type_hint",
    "SimpleTIRGenerator",
    "compile_to_tir",
    "SemaResult",
    "analyze_module",
]


class SimpleTIRGenerator(
    LocalBindingMixin,
    MidendOptimizationMixin,
    SerializationMixin,
    OwnershipLoweringMixin,
    StringFormattingMixin,
    RuntimeReferenceMixin,
    CompileWarningMixin,
    EmissionCoreMixin,
    ExpressionPrimitivesMixin,
    ExceptionLoweringMixin,
    FunctionLifecycleMixin,
    FunctionMetadataMixin,
    ModuleGlobalsMixin,
    ModuleLifecycleMixin,
    SemaStateMixin,
    ImportLoweringMixin,
    SymbolNamingMixin,
    AttributeAccessMixin,
    ClassResolutionMixin,
    TypeAnnotationMixin,
    LoopLoweringMixin,
    AnalysisCollectStaticMixin,
    AnalysisPatternMixin,
    AsyncGenVisitorMixin,
    PatternMatchMixin,
    CallVisitorMixin,
    ClassDefVisitorMixin,
    ComprehensionMixin,
    ExpressionVisitorMixin,
    FunctionVisitorMixin,
    AssignmentStatementVisitorMixin,
    ControlFlowStatementVisitorMixin,
    StatementScopeVisitorMixin,
    ast.NodeVisitor,
):
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
        entry_module: str | None = None,
        enable_phi: bool = True,
        known_modules: set[str] | None = None,
        known_classes: dict[str, ClassInfo] | None = None,
        stdlib_allowlist: set[str] | None = None,
        known_func_defaults: dict[str, dict[str, dict[str, Any]]] | None = None,
        known_func_kinds: dict[str, dict[str, str]] | None = None,
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
        self.state_count: int = 0
        self.classes: dict[str, ClassInfo] = dict(known_classes or {})
        self.local_class_names: set[str] = set()
        # Depth of class-statement bodies currently being lowered.  A nested
        # ``class`` statement (a class defined inside another class body) must
        # bind into the *enclosing class namespace* — exactly like a method or
        # a class-attribute assignment — never into module globals, even when
        # the outermost enclosing class is at module scope (where
        # ``current_func_name == "molt_main"``).  ``visit_ClassDef`` increments
        # this around its body-statement loop and consults it when publishing
        # the finished class so the nested class is bound as a class-body local
        # that the enclosing loop harvests into ``class_attr_values``.
        self._class_body_depth: int = 0
        # Class-body block-execution scope stack (P0 #50).  When the body of a
        # ``class`` statement is lowered as a NORMAL block (so arbitrary control
        # flow / ``del`` "just work", exactly like CPython's class-body code
        # object), the innermost entry on this stack describes the active class
        # namespace.  ``_store_local_value`` / ``_load_local_value`` /
        # ``_emit_delete_name`` consult it so a class-body name binds into the
        # namespace mapping (STORE_INDEX) and reads back from it (INDEX) — the
        # heap-backed dict IS the mutable store, so loop-carried mutation is
        # correct without SSA phi participation (the same mechanism the module
        # dict provides at module scope).  ``ns`` is the namespace MoltValue when
        # the class is built dynamically (``dynamic_build``); ``attr_values`` is
        # the name→MoltValue snapshot the static ``CLASS_DEF`` path consumes.
        # ``names`` is the set of names that are class-body-scoped (a Name not in
        # this set falls through to enclosing/global/builtin resolution, matching
        # CPython LOAD_NAME).
        self._class_ns_stack: list[_ClassNsScope] = []
        self.locals: dict[str, MoltValue] = {}
        # Backing store for the current frame's `locals()` snapshot semantics.
        # Stored outside `self.locals` to avoid accidental shadowing/rewrites by lowering passes.
        self.locals_cache_val: MoltValue | None = None
        self.boxed_locals: dict[str, MoltValue] = {}
        self.closure_locals: set[str] = set()
        self.comp_shadow_locals: set[str] = set()
        # Cell list (1-element list MoltValue) backing the implicit
        # ``__class__`` closure variable of the class currently having its
        # methods compiled.  Set by visit_ClassDef before the method
        # compilation loop when any method references ``super()``/``__class__``
        # (see `_function_needs_classcell`), and cleared afterwards.  Zero-arg
        # ``super()`` and bare ``__class__`` loads read the class object from
        # this cell — exactly mirroring CPython's ``__class__`` closure cell —
        # rather than re-deriving the class by module-attribute name (which is
        # wrong for function-local / nested classes that are not module
        # globals).  The same cell is stored as ``__classcell__`` in the class
        # namespace and filled with the finished class object after the
        # metaclass call.
        self._active_classcell_cell: MoltValue | None = None
        self._expr_col: tuple[int, int] | None = (
            None  # expression-level col_offset for traceback carets
        )
        self.boxed_local_hints: dict[str, str] = {}
        self.free_vars: dict[str, int] = {}
        self.free_var_hints: dict[str, str] = {}
        self.global_decls: set[str] = set()
        self.nonlocal_decls: set[str] = set()
        self.scope_assigned: set[str] = set()
        self.del_targets: set[str] = set()
        self.unbound_check_names: set[str] = set()
        # Set while inlining a method that closes over the implicit ``__class__``
        # super cell: any ``super()`` in the inlined body that cannot fold
        # statically raises ``_InlineSuperFoldRequired`` to abort the inline,
        # because the caller's spliced scope has no ``__class__`` cell.
        self._inline_super_must_fold: bool = False
        self.exact_locals: dict[str, str] = {}
        self.exact_builtin_locals: dict[str, str] = {}
        self.globals: dict[str, MoltValue] = {}
        self.module_chunk_globals: set[str] = set()
        self.func_symbol_names: dict[str, str] = {}
        self.func_default_specs: dict[str, dict[str, Any]] = {}
        self.stable_module_funcs: set[str] = set()
        self.module_declared_funcs: dict[str, FunctionKind] = {}
        self.module_declared_classes: set[str] = set()
        # Static class graph for this module — the soundness substrate for the
        # zero-arg ``super()`` fold (see ``_collect_module_class_graph``).
        # ``module_subclassed_names``: names referenced as a base anywhere.
        # ``module_class_bases``: class name -> list of base-name lists across
        # all class statements defining that name (``["<opaque>"]`` for a class
        # whose bases are not all simple names).
        self.module_subclassed_names: set[str] = set()
        self.module_class_bases: dict[str, list[list[str]]] = {}
        self.stable_module_classes: set[str] = set()
        self.module_defined_funcs: set[str] = set()
        self.class_definition_pending: set[str] = set()
        self.module_global_mutations: set[str] = set()
        self.module_globals_dict_escaped = False
        self.module_intrinsic_globals: dict[str, str] = {}
        self.reserved_external_func_symbols: set[str] = set()
        # Track the last-known type hint for module-scope attributes.
        # Populated by _emit_module_attr_set_on and read by _emit_module_attr_get.
        # Enables fast_int/fast_float paths for module-scope loop variables.
        self._module_attr_type_hints: dict[str, str] = {}
        self.mutated_classes: set[str] = set()
        self.instance_attr_mutations: dict[str, set[str]] = {}
        self.imported_names: dict[str, str] = {}
        self.global_imported_names: dict[str, str] = {}
        # Maps bind_name -> original attr_name for `from X import Y as Z`
        # (bind_name="Z", attr_name="Y").  Used to resolve cross-module call
        # targets to the original function name rather than the alias.
        self.imported_attr_names: dict[str, str] = {}
        self.global_imported_attr_names: dict[str, str] = {}
        self.imported_modules: dict[str, str] = {}
        self.global_imported_modules: dict[str, str] = {}
        self.local_imported_names: set[str] = set()
        self.local_imported_modules: set[str] = set()
        self.imported_module_attr_mutations: set[tuple[str, str]] = set()
        self.global_imported_module_attr_mutations: set[tuple[str, str]] = set()
        self.local_intrinsic_wrappers: set[str] = set()
        self.gpu_kernel_symbols_by_name: dict[str, str] = {}
        self.current_gpu_kernel_context: bool = False
        # Track aliases for ``import typing as <alias>`` so that
        # ``@<alias>.overload`` is recognised as a typing overload stub.
        self._typing_import_aliases: set[str] = set()
        self.async_locals: dict[str, int] = {}
        self.async_internal_locals: set[str] = set()
        self.async_public_locals: set[str] = set()
        self.async_locals_base: int = 0
        self.async_closure_offset: int | None = None
        self.async_local_hints: dict[str, str] = {}
        # Always eagerly emit __annotations__ dicts.  Our runtime does not
        # implement the deferred __annotate__ protocol (PEP 749), so we must
        # materialise annotations at definition time regardless of the host
        # Python version.
        self.eager_annotations = True
        self.parse_codec = parse_codec
        self.type_hint_policy = type_hint_policy
        self.explicit_type_hints: dict[str, str] = {}
        self.annotation_type_params: dict[str, MoltValue] = {}
        self.in_annotation = False
        self.container_elem_hints: dict[str, str] = {}
        self.global_elem_hints: dict[str, str] = {}
        self.dict_key_hints: dict[str, str] = {}
        self.dict_value_hints: dict[str, str] = {}
        self.bytearray_len_hints: dict[str, int] = {}
        self.stdlib_hint_trust = False
        if source_path:
            normalized_path = source_path.replace("\\", "/")
            if "/src/molt/stdlib/" in normalized_path or normalized_path.startswith(
                "src/molt/stdlib/"
            ):
                self.stdlib_hint_trust = True
        self._source_is_stdlib_module = self.stdlib_hint_trust
        self.global_dict_key_hints: dict[str, str] = {}
        self.global_dict_value_hints: dict[str, str] = {}
        self.type_facts = type_facts
        self.module_name = module_name or "__main__"
        self.type_facts_module = type_facts_module or self.module_name
        self.module_spec_name = module_spec_name or self.module_name
        self.entry_module = entry_module
        self.enable_phi = enable_phi
        self.module_prefix = f"{self._sanitize_module_name(self.module_name)}__"
        self.known_modules = set(known_modules or [])
        self.stdlib_allowlist = set(stdlib_allowlist or [])
        self.known_func_defaults: dict[str, dict[str, dict[str, Any]]] = (
            known_func_defaults or {}
        )
        self.known_func_kinds: dict[str, dict[str, str]] = known_func_kinds or {}
        self.module_func_defaults: dict[str, dict[str, Any]] = {}
        self.module_annotations: MoltValue | None = None
        self.module_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.module_annotation_ids: dict[int, int] = {}
        self.module_annotation_exec_map: MoltValue | None = None
        self.module_annotation_exec_name: str | None = None
        self.module_annotation_exec_counter = 0
        self.module_annotation_emitted = False
        self.globals_builtin_val: MoltValue | None = None
        self.globals_builtin_emitted = False
        self.module_annotations_conditional = False
        self.module_frame_emitted = False
        self.module_chunking = module_chunking
        self.module_chunk_max_ops = module_chunk_max_ops
        self.module_stmt_offsets: list[int] = []
        self.module_chunk_counter = 0
        self.module_chunk_symbols: list[str] = []
        if optimization_profile not in {"dev", "release"}:
            optimization_profile = "release"
        self.optimization_profile: MidendProfile = optimization_profile
        self.midend_hot_functions: set[str] = {
            symbol.strip()
            for symbol in (pgo_hot_functions or set())
            if isinstance(symbol, str) and symbol.strip()
        }
        self.midend_env = self._resolve_midend_env_config()
        self._midend_env_snapshot = self._capture_midend_env_snapshot()
        self.module_frame_entered = False
        self.module_frame_exited = False
        self.module_frame_code_id: int | None = None
        self.class_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.class_annotation_exec_map: MoltValue | None = None
        self.class_annotation_exec_name: str | None = None
        self.class_annotation_exec_counter = 0
        self.annotation_name_counter = 0
        self.module_obj: MoltValue | None = None
        self.future_annotations = False
        self.defer_module_attrs = False
        self.deferred_module_attrs: set[str] = set()
        self.fallback_policy = fallback_policy
        self.compat = CompatibilityReporter(fallback_policy, source_path)
        self.source_path = source_path
        self.module_is_package = False
        self.module_is_namespace = module_is_namespace
        self._emitted_syntax_warnings: set[tuple[str, int, str]] = set()
        self._deferred_runtime_warnings: list[str] = []
        if source_path:
            normalized_path = source_path.replace("\\", "/")
            if normalized_path.endswith("/__init__.py") or normalized_path.endswith(
                "/__init__.pyi"
            ):
                self.module_is_package = True
        if self.module_is_namespace:
            self.module_is_package = True
        self.module_package_override: str | None = None
        self.module_package_override_set = False
        self.module_spec_override: str | None = None
        self.module_spec_override_set = False
        self.module_spec_override_is_package: bool | None = None
        self.context_depth = 0
        self.control_flow_depth = 0
        self.try_end_labels: list[int] = []
        self.try_scopes: list[TryScope] = []
        self.try_suppress_depth: int | None = None
        self.try_handler_scopes: list[TryScope] = []
        self.function_exception_label: int | None = self.next_label()
        self.exception_stack_depth_baseline: MoltValue | None = None
        self.exception_stack_prev_baseline: MoltValue | None = None
        self.return_unwind_depth = 0
        self.return_unwind_popped_scopes = []
        self.finally_depth = 0
        self.return_label: int | None = None
        self.return_slot: MoltValue | None = None
        self.return_slot_index: MoltValue | None = None
        self.return_slot_offset: int | None = None
        self.block_terminated = False
        self.range_loop_stack: list[tuple[MoltValue, MoltValue]] = []
        self.async_index_loop_stack: list[int] = []
        self.loop_break_flags: list[int | str | None] = []
        self.loop_try_depths: list[int] = []
        self.loop_break_counter = 0
        self.loop_layout_guards: list[dict[str, tuple[str, MoltValue]]] = []
        self.loop_guard_assumptions: list[dict[str, tuple[str, bool]]] = []
        self.loop_static_class_refs: list[dict[str, MoltValue]] = []
        self.loop_static_class_eager_refs: list[set[str]] = []
        self.loop_static_class_counter = 0
        self.active_exceptions: list[ActiveException] = []
        self.func_aliases: dict[str, str] = {}
        self.reserved_func_symbols: dict[str, str] = {}
        self.const_ints: dict[str, int] = {}
        # Producing-op index (result SSA name -> MoltOp), maintained by emit().
        # The named-local binding stamp (#58 `bound_local`) uses it to mark the
        # op whose result a plain function local binds to. Value names are
        # globally unique (next_var), so no per-function reset is needed.
        self._op_by_result: dict[str, MoltOp] = {}
        self.format_token_cache: dict[
            tuple[str, int, tuple[str, ...]], list[FormatToken]
        ] = {}
        self.in_generator = False
        self.async_context = False
        self.lambda_counter = 0
        self.genexpr_counter = 0
        self.midend_stats: dict[str, int] = {
            "expanded_attempts": 0,
            "expanded_accepted": 0,
            "expanded_fallbacks": 0,
            "midend_module_skips": 0,
            "midend_oversized_function_skips": 0,
            "invalid_unbound_rollback": 0,
            "invalid_unbound_uses": 0,
            "fixed_point_fail_fast": 0,
            "cfg_structural_failures": 0,
            "cfg_structural_canonicalizations": 0,
            "sccp_iteration_cap_hits": 0,
            "cse_dce_fp_cap_hits": 0,
            "sccp_branch_prunes": 0,
            "loop_edge_thread_prunes": 0,
            "try_edge_thread_prunes": 0,
            "unreachable_blocks_removed": 0,
            "cfg_region_prunes": 0,
            "label_prunes": 0,
            "jump_noop_elisions": 0,
            "licm_hoists": 0,
            "guard_hoist_attempts": 0,
            "guard_hoist_accepted": 0,
            "guard_hoist_rejected": 0,
            "fused_dict_guard_prunes": 0,
            "phi_edge_trims": 0,
            "gvn_hits": 0,
            "dce_removed_total": 0,
        }
        self.midend_stats_by_function: dict[str, dict[str, int]] = {}
        self.midend_pass_stats_by_function: dict[str, dict[str, dict[str, Any]]] = {}
        self.midend_policy_outcomes_by_function: dict[str, dict[str, Any]] = {}
        self._active_midend_function_name = "<direct>"
        self._midend_stats_reported = False
        self.qualname_stack: list[tuple[str, bool]] = []
        self.current_class: str | None = None
        self.current_method_first_param: str | None = None
        self.current_line: int | None = None
        # Per-function cache: module name → cached MoltValue from MODULE_CACHE_GET.
        # Avoids emitting redundant MODULE_CACHE_GET ops for the same module within
        # a single function scope.  Reset at start_function().
        self._module_cache_values: dict[str, MoltValue] = {}
        # Module-level constant dicts: name → {str_key: constant_value}
        # Populated during visit_Module to support compile-time **kwargs resolution
        # (e.g. @dataclass(**SLOTS) where SLOTS = {"slots": True}).
        self.module_const_dicts: dict[str, dict[str, Any]] = {}
        # The immutable SemaResult for the module currently being lowered (doc 44
        # §F2b). Computed once, pre-walk, by _populate_sema_state in visit_Module;
        # None until then (and for direct non-module lowering entry points).
        self._sema: SemaResult | None = None
        self._register_code_symbol("molt_main")
        if self.module_name:
            is_real_entry_module = (
                self.entry_module
                and self.module_name == self.entry_module
                and self.module_name != "__main__"
            )
            entry_spawn_override_enabled = (
                is_real_entry_module and "multiprocessing.spawn" in self.known_modules
            )
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=[self.module_name], result=name_val)
            )
            module_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(MoltOp(kind="MODULE_NEW", args=[name_val], result=module_val))
            self.emit(
                MoltOp(
                    kind="MODULE_CACHE_SET",
                    args=[name_val, module_val],
                    result=MoltValue("none"),
                )
            )
            if entry_spawn_override_enabled:
                spawn_key = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=["MOLT_MP_SPAWN"], result=spawn_key)
                )
                spawn_default = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[""], result=spawn_default))
                spawn_value = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="ENV_GET",
                        args=[spawn_key, spawn_default],
                        result=spawn_value,
                    )
                )
                spawn_expected = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["1"], result=spawn_expected))
                spawn_eq = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="STRING_EQ",
                        args=[spawn_value, spawn_expected],
                        result=spawn_eq,
                    )
                )
                spawn_not = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[spawn_eq], result=spawn_not))
                self.emit(MoltOp(kind="IF", args=[spawn_not], result=MoltValue("none")))
            if is_real_entry_module:
                main_name = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__main__"], result=main_name))
                self.emit(
                    MoltOp(
                        kind="MODULE_CACHE_SET",
                        args=[main_name, module_val],
                        result=MoltValue("none"),
                    )
                )
                name_attr = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__name__"], result=name_attr))
                self.emit(
                    MoltOp(
                        kind="MODULE_SET_ATTR",
                        args=[module_val, name_attr, main_name],
                        result=MoltValue("none"),
                    )
                )
                if entry_spawn_override_enabled:
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.module_obj = module_val
        self._emit_module_metadata()
        self._apply_type_facts("molt_main")


def compile_to_tir(
    source: str,
    parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
    type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
    fallback_policy: FallbackPolicy = "error",
) -> dict[str, Any]:
    # Reset the IC counter so that repeated compilations of the same source
    # produce identical IR (determinism guarantee).
    _ic_counter[0] = 0
    tree = ast.parse(source)
    gen = SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
    )
    gen.visit(tree)
    return gen.to_json()
