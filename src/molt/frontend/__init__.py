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
import json
from contextlib import contextmanager
import string as _py_string
from typing import (
    TYPE_CHECKING,
    Any,
    Literal,
    Sequence,
    cast,
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

# Frontend op.kind tables generated from runtime/molt-backend/src/tir/op_kinds.toml
# (the cross-component single source of truth; tools/gen_op_kinds.py renders this,
# tests/test_gen_op_kinds.py pins it in sync). These REPLACE the formerly hand-kept
# _RAISING_OP_KINDS / CHECK_EXCEPTION skip set / _augassign_op_kind tables (task #44
# F2a), killing the frontend⇄backend dual raising-oracle drift.
from molt.frontend.lowering.op_kinds_generated import (
    AUGASSIGN_OP_KIND,
    CHECK_EXCEPTION_SKIP_KINDS,
    RAISING_KIND_NAMES,
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
    expression_contains_yield,
    normalize_function_kind,
    stateful_function_frame_plan,
)

# Visitor / lowering mixins composed into SimpleTIRGenerator (F1 decomposition).
from molt.frontend.lowering.analysis_collect_static import AnalysisCollectStaticMixin
from molt.frontend.lowering.analysis_patterns import AnalysisPatternMixin
from molt.frontend.lowering.class_resolution import ClassResolutionMixin
from molt.frontend.lowering.compile_warnings import CompileWarningMixin
from molt.frontend.lowering.local_bindings import LocalBindingMixin
from molt.frontend.lowering.midend_optimization import MidendOptimizationMixin
from molt.frontend.lowering.module_lifecycle import ModuleLifecycleMixin
from molt.frontend.lowering.serialization import SerializationMixin
from molt.frontend.lowering.symbol_naming import SymbolNamingMixin
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
    CompileWarningMixin,
    ModuleLifecycleMixin,
    SymbolNamingMixin,
    ClassResolutionMixin,
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

    def visit(self, node: ast.AST) -> Any:
        try:
            if isinstance(node, (ast.stmt, ast.ExceptHandler)):
                lineno = getattr(node, "lineno", None)
                if lineno:
                    col = getattr(node, "col_offset", None)
                    end_col = getattr(node, "end_col_offset", None)
                    self._emit_line_marker(int(lineno), col, end_col)
            # Track expression-level column offsets for traceback carets.
            # When an expression node is visited, record its position so
            # that ops emitted during this visit carry the expression's
            # col_offset (not the statement's).
            if isinstance(node, ast.expr):
                col = getattr(node, "col_offset", None)
                end_col = getattr(node, "end_col_offset", None)
                if col is not None and end_col is not None:
                    prev = getattr(self, "_expr_col", None)
                    self._expr_col = (col, end_col)
                    result = super().visit(node)
                    self._expr_col = prev
                    return result
            return super().visit(node)
        except CompatibilityError:
            raise
        except NotImplementedError as exc:
            raise self.compat.unsupported(
                node,
                feature=str(exc),
                tier="bridge",
                impact="high",
            ) from exc

    def next_var(self) -> str:
        name = f"v{self.var_count}"
        self.var_count += 1
        return name

    def next_label(self) -> int:
        self.state_count += 1
        return self.state_count

    @contextmanager
    def _suppress_check_exception(self, *, emit_on_exit: bool = True) -> Any:
        prior = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        try:
            yield
        finally:
            self.try_suppress_depth = prior
            if emit_on_exit:
                if (
                    self.try_suppress_depth is None
                    or len(self.try_end_labels) > self.try_suppress_depth
                ):
                    if self.try_end_labels:
                        handler_label = self.try_end_labels[-1]
                    else:
                        handler_label = self.function_exception_label
                    if handler_label is not None:
                        self.emit(
                            MoltOp(
                                kind="CHECK_EXCEPTION",
                                args=[handler_label],
                                result=MoltValue("none"),
                            )
                        )

    def emit(self, op: MoltOp) -> None:
        # Auto-attach expression column offsets to raising ops. RAISING_KIND_NAMES
        # is generated from runtime/molt-backend/src/tir/op_kinds.toml (the
        # [[frontend_raising_kind]] table cross-checked against the [[opcode]]
        # may_throw oracle) — see the module-level import.
        if (
            op.col_offset is None
            and op.kind in RAISING_KIND_NAMES
            and getattr(self, "_expr_col", None) is not None
        ):
            op.col_offset, op.end_col_offset = self._expr_col
        if (
            op.kind == "CONST"
            and op.result
            and isinstance(op.args[0], int)
            and not isinstance(op.args[0], bool)
        ):
            self.const_ints[op.result.name] = op.args[0]
        if op.result is not None and op.result.name not in ("none", ""):
            self._op_by_result[op.result.name] = op
        self.current_ops.append(op)
        if (
            self.try_suppress_depth is not None
            and len(self.try_end_labels) <= self.try_suppress_depth
        ):
            return
        if self.try_end_labels:
            handler_label = self.try_end_labels[-1]
        else:
            if self.function_exception_label is None:
                return
            handler_label = self.function_exception_label
        # CHECK_EXCEPTION_SKIP_KINDS is generated from op_kinds.toml's
        # [[frontend_check_exception_skip]] table (control-flow / structural
        # kinds, plus RAISE / STATE_TRANSITION whose exceptional edge is handled
        # structurally). Opcode-backed members are cross-checked against the
        # may_throw oracle at generation. See the module-level import.
        if op.kind in CHECK_EXCEPTION_SKIP_KINDS:
            return
        self.current_ops.append(
            MoltOp(
                kind="CHECK_EXCEPTION",
                args=[handler_label],
                result=MoltValue("none"),
            )
        )

    def _emit_line_marker(
        self,
        lineno: int,
        col_offset: int | None = None,
        end_col_offset: int | None = None,
    ) -> None:
        if lineno <= 0:
            return
        if self.current_line == lineno:
            return
        self.current_line = lineno
        op = MoltOp(
            kind="LINE",
            args=[lineno],
            result=MoltValue("none"),
            source_line=lineno,
        )
        # Attach column offsets for traceback caret annotations.
        if col_offset is not None:
            op.col_offset = col_offset
        if end_col_offset is not None:
            op.end_col_offset = end_col_offset
        self.emit(op)

    def _emit_line_marker_force(self) -> None:
        if not self.current_line or self.current_line <= 0:
            return
        self.emit(
            MoltOp(
                kind="LINE",
                args=[self.current_line],
                result=MoltValue("none"),
            )
        )

    def _fast_int_enabled(self) -> bool:
        return self._hints_enabled()

    def _hints_enabled(self) -> bool:
        return self.type_hint_policy in {"trust", "check"} or self.stdlib_hint_trust

    def _should_fast_int(self, op: MoltOp) -> bool:
        if op.kind not in _FAST_ARITH_OPS:
            return False
        if op.kind in {"NEG", "POS"}:
            return all(
                isinstance(arg, MoltValue) and arg.type_hint == "int" for arg in op.args
            )
        # Bitwise ops on bools must NOT use the fast_int path because the
        # backend's inline band/bor/bxor + box_int_value always returns an
        # int, losing the bool type.  CPython preserves bool: True & False
        # returns False (bool), not 0 (int).  The slow path (runtime call)
        # handles bool operands correctly via from_bool.
        if op.kind in {"BIT_AND", "BIT_OR", "BIT_XOR"} and any(
            isinstance(arg, MoltValue) and arg.type_hint == "bool" for arg in op.args
        ):
            return False
        return all(
            isinstance(arg, MoltValue) and arg.type_hint in {"int", "bool"}
            for arg in op.args
        )

    def _should_fast_float(self, op: MoltOp) -> bool:
        if op.kind not in _FAST_ARITH_OPS:
            return False
        return all(
            isinstance(arg, MoltValue) and arg.type_hint == "float" for arg in op.args
        )

    def _emit_bridge_unavailable(self, message: str) -> MoltValue:
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="BRIDGE_UNAVAILABLE", args=[msg_val], result=res))
        return res

    def _bridge_fallback(
        self,
        node: ast.AST,
        feature: str,
        *,
        impact: Literal["low", "medium", "high"] = "high",
        alternative: str | None = None,
        detail: str | None = None,
    ) -> MoltValue:
        issue = self.compat.bridge_unavailable(
            node, feature, impact=impact, alternative=alternative, detail=detail
        )
        if self.fallback_policy != "bridge":
            raise self.compat.error(issue)
        return self._emit_bridge_unavailable(issue.runtime_message())

    def _is_contextmanager_decorator(self, deco: ast.expr) -> bool:
        if isinstance(deco, ast.Name) and deco.id == "contextmanager":
            return True
        if (
            isinstance(deco, ast.Attribute)
            and isinstance(deco.value, ast.Name)
            and deco.value.id == "contextlib"
            and deco.attr == "contextmanager"
        ):
            return True
        return False

    @staticmethod
    def _is_gpu_kernel_decorator(deco: ast.expr) -> bool:
        """Return True if the decorator is @gpu.kernel."""
        # @gpu.kernel  (attribute form: gpu.kernel)
        if (
            isinstance(deco, ast.Attribute)
            and isinstance(deco.value, ast.Name)
            and deco.value.id == "gpu"
            and deco.attr == "kernel"
        ):
            return True
        # @kernel  (bare name after `from molt.gpu import kernel`)
        if isinstance(deco, ast.Name) and deco.id == "kernel":
            return True
        return False

    def _has_gpu_kernel_decorator(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        """Return True if any decorator on *node* is @gpu.kernel."""
        return any(self._is_gpu_kernel_decorator(d) for d in node.decorator_list)

    @staticmethod
    def _sanitize_module_name(name: str) -> str:
        out: list[str] = []
        for ch in name:
            if ch.isalnum() or ch == "_":
                out.append(ch)
            else:
                out.append("_")
        if not out:
            return "module"
        return "".join(out)

    @classmethod
    def module_init_symbol(cls, name: str) -> str:
        return f"molt_init_{cls._sanitize_module_name(name)}"

    @staticmethod
    def _function_contains_locals_call(
        node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if (
                isinstance(current, ast.Call)
                and isinstance(current.func, ast.Name)
                and current.func.id == "locals"
            ):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _expr_contains_locals_call(node: ast.AST) -> bool:
        stack: list[ast.AST] = [node]
        while stack:
            current = stack.pop()
            if (
                isinstance(current, ast.Call)
                and isinstance(current.func, ast.Name)
                and current.func.id == "locals"
            ):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _function_contains_return(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.Return):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _body_has_exception_handlers(body: list[ast.stmt]) -> bool:
        """Return True if the body contains try/with handler constructs.

        This gates ONLY the exception-STACK depth bookkeeping
        (EXCEPTION_STACK_ENTER/DEPTH/SET_DEPTH/EXIT), which is needed solely
        when the function pushes/pops the runtime exception-handler stack — i.e.
        it contains ``try``/``with`` (and their async/star variants).

        It does NOT gate exception OBSERVATION: every function unconditionally
        carries a function-level exception label and the per-may-raise-op
        CHECK_EXCEPTION routing, so a raising callee's pending exception is
        always observed.  Decoupling these two concerns is the C2 fix — the old
        ``_function_needs_exception_stack`` conflated them and (by opting a
        function out of *observation*) caused silent-wrong exception
        propagation.  A bare ``raise`` does NOT require depth bookkeeping: it
        sets the pending flag and jumps to the function label, whose handler's
        depth-restore is a no-op when no handler stack was ever pushed.
        """
        stack: list[ast.AST] = list(body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.Try, ast.TryStar, ast.With, ast.AsyncWith)):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _block_needs_context_unwind(body: list[ast.stmt]) -> bool:
        stack: list[ast.AST] = list(body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.With, ast.AsyncWith)):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    def _has_typing_overload_decorator(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        """Return True if the function has a @typing.overload or @overload decorator.

        Handles ``typing.overload``, bare ``overload``, and aliased forms
        like ``t.overload`` (from ``import typing as t``).
        """
        for deco in node.decorator_list:
            if isinstance(deco, ast.Attribute):
                if isinstance(deco.value, ast.Name) and deco.attr == "overload":
                    # Accept any <name>.overload where <name> resolves to
                    # the typing module — covers ``typing.overload``,
                    # ``t.overload``, etc.  We check a known set of names
                    # to avoid false positives with unrelated ``overload``
                    # attributes.
                    alias = deco.value.id
                    if alias == "typing" or alias in self._typing_import_aliases:
                        return True
            elif isinstance(deco, ast.Name) and deco.id == "overload":
                return True
        return False

    def start_function(
        self,
        name: str,
        params: list[str] | None = None,
        param_types: list[str] | None = None,
        type_facts_name: str | None = None,
        needs_return_slot: bool = False,
        has_exception_handlers: bool = True,
    ) -> None:
        if name not in self.funcs_map:
            self.funcs_map[name] = FuncInfo(
                params=params or [],
                param_types=param_types or [],
                return_hint=None,
                ops=self._new_tracked_ops(count_function=True),
            )
        else:
            self.funcs_map[name]["params"] = params or []
            self.funcs_map[name]["param_types"] = param_types or []
            self.funcs_map[name].setdefault("return_hint", None)
            self.funcs_map[name]["ops"].clear()
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]
        self.locals = {}
        self.locals_cache_val = None
        self.boxed_locals = {}
        self.closure_locals = set()
        self.comp_shadow_locals = set()
        self.boxed_local_hints = {}
        self.free_vars = {}
        self.free_var_hints = {}
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.del_targets = set()
        self.unbound_check_names = set()
        self.exact_locals = {}
        self.exact_builtin_locals = {}
        self.imported_names = dict(self.global_imported_names)
        self.imported_attr_names = dict(self.global_imported_attr_names)
        self.imported_modules = dict(self.global_imported_modules)
        self.local_imported_names = set()
        self.local_imported_modules = set()
        self.imported_module_attr_mutations = set(
            self.global_imported_module_attr_mutations
        )
        self.async_locals = {}
        self.async_internal_locals = set()
        self.async_public_locals = set()
        self.async_locals_base = 0
        self.async_closure_offset = None
        self.async_local_hints = {}
        self.explicit_type_hints = {}
        self.container_elem_hints = {}
        self.dict_key_hints = {}
        self.dict_value_hints = {}
        self.context_depth = 0
        self.control_flow_depth = 0
        self.const_ints = {}
        self._op_by_result = {}
        self._module_cache_values = {}
        self.in_generator = False
        self.async_context = False
        self.current_line = None
        self.try_end_labels = []
        self.try_scopes = []
        self.try_suppress_depth = None
        self.try_handler_scopes = []
        # ── Exception model (C2): two decoupled concerns ──────────────────
        # 1. OBSERVATION — every function unconditionally carries a
        #    function-level exception label.  `emit()` auto-routes a pending
        #    exception to this label after every may-raise op (the redundant
        #    checks are removed later by the oracle-driven `check_exception_elim`
        #    TIR pass).  A raising callee sets the runtime exception-pending flag
        #    regardless of the caller's syntactic shape, so there is NO sound way
        #    to opt a function out of observation without re-opening the
        #    silent-wrong-propagation bug class (a lambda that calls
        #    `int("x")` returning None instead of raising).  Hence the label is
        #    ALWAYS created.
        #
        # 2. STACK-DEPTH bookkeeping (EXCEPTION_STACK_ENTER/DEPTH and the
        #    matching SET_DEPTH/EXIT at returns) — needed ONLY when the function
        #    pushes/pops the runtime exception-handler stack, i.e. it contains a
        #    `try`/`with` handler.  A function without handlers never changes the
        #    depth, so the ENTER/DEPTH baselines (and their per-return restore)
        #    are pure overhead.  Gating them on `has_exception_handlers` keeps a
        #    trivial leaf like `lambda x: x + 1` cheap (label + post-op check
        #    only) — the same cost CPython pays — while preserving full
        #    correctness for handler-bearing functions.
        self.function_exception_label = self.next_label()
        if has_exception_handlers:
            self.exception_stack_prev_baseline = MoltValue(
                self.next_var(), type_hint="int"
            )
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_ENTER",
                    args=[],
                    result=self.exception_stack_prev_baseline,
                )
            )
            self.exception_stack_depth_baseline = MoltValue(
                self.next_var(), type_hint="int"
            )
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_DEPTH",
                    args=[],
                    result=self.exception_stack_depth_baseline,
                )
            )
        else:
            self.exception_stack_prev_baseline = None
            self.exception_stack_depth_baseline = None
        self.return_unwind_depth = 0
        self.return_unwind_popped_scopes = []
        self.finally_depth = 0
        self.active_exceptions = []
        self.return_label = None
        self.return_slot = None
        self.return_slot_index = None
        self.return_slot_offset = None
        self.block_terminated = False
        self.loop_static_class_refs = []
        self.loop_static_class_eager_refs = []
        if needs_return_slot:
            self._init_return_slot()
        self._apply_type_facts(type_facts_name or name)
        # NOTE: locals-dict allocation moved to per-visitor conditional paths
        # (visit_FunctionDef, visit_AsyncFunctionDef, lambda visitors) which
        # gate on _function_contains_locals_call().  The previous unconditional
        # _init_locals_cache() here caused a heap dict allocation on every
        # function call, even for functions that never use locals().
        # See bench/results/fib_regression_analysis.txt for details.

    def _module_can_defer_attrs(self, node: ast.Module) -> bool:
        for current in ast.walk(node):
            if isinstance(
                current,
                (
                    ast.FunctionDef,
                    ast.AsyncFunctionDef,
                    ast.ClassDef,
                    ast.Lambda,
                    ast.ListComp,
                    ast.SetComp,
                    ast.DictComp,
                    ast.GeneratorExp,
                ),
            ):
                return False
            if isinstance(current, ast.Call) and isinstance(current.func, ast.Name):
                if current.func.id in {"globals", "locals", "vars"}:
                    return False
        return True

    def _module_has_future_annotations(self, node: ast.Module) -> bool:
        found = False

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                return

            def visit_ListComp(self, node: ast.ListComp) -> None:
                return

            def visit_SetComp(self, node: ast.SetComp) -> None:
                return

            def visit_DictComp(self, node: ast.DictComp) -> None:
                return

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                nonlocal found
                if node.module != "__future__":
                    return
                for alias in node.names:
                    if alias.name == "annotations":
                        found = True
                        return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
            if found:
                break
        return found

    # _collect_module_func_defaults moved to frontend/sema/funcmeta.py (F2b,
    # doc 44): the top-level function default/param-shape table is computed
    # pre-walk by sema.collect_module_func_defaults and populated into
    # self.module_func_defaults by _populate_sema_state.

    @staticmethod
    def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
        if isinstance(expr, ast.Constant):
            return {"const": True, "value": expr.value}
        return {"const": False}

    @classmethod
    def _default_specs_from_args(cls, args: ast.arguments) -> list[dict[str, Any]]:
        default_specs = [cls._default_spec_for_expr(expr) for expr in args.defaults]
        if not args.kwonlyargs or not args.kw_defaults:
            return default_specs
        kwonly_names = [arg.arg for arg in args.kwonlyargs]
        kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
        suffix: list[tuple[str, ast.expr]] = []
        for name, expr in reversed(kwonly_pairs):
            if expr is None:
                break
            suffix.append((name, expr))
        for name, expr in reversed(suffix):
            spec = cls._default_spec_for_expr(expr)
            spec["kwonly"] = True
            spec["name"] = name
            default_specs.append(spec)
        return default_specs

    def _record_func_default_specs(self, func_symbol: str, args: ast.arguments) -> None:
        if args.vararg or args.kwarg:
            # Mark as having vararg/kwarg so the direct-call path knows to
            # fall back to CALL_BIND for proper varargs packing.
            self.func_default_specs[func_symbol] = {"has_vararg": True}
            return
        params = self._function_param_names(args)
        default_specs = self._default_specs_from_args(args)
        self.func_default_specs[func_symbol] = {
            "params": len(params),
            "defaults": default_specs,
            "posonly": len(args.posonlyargs),
            "kwonly": len(args.kwonlyargs),
            "kind": "sync",
            "has_decorators": False,
        }

    def _normalized_return_hint(self, returns: ast.expr | None) -> str | None:
        hint = self._annotation_to_hint(returns)
        if hint and hint[:1] in {"'", '"'} and hint[-1:] == hint[:1]:
            hint = hint[1:-1]
        return hint

    def _module_stable_funcs(self, node: ast.Module) -> set[str]:
        counts, funcs, dynamic = self._collect_module_assignments(node)
        if dynamic:
            return set()
        global_rebinds = self._collect_global_rebinds(node)
        return {
            name
            for name in funcs
            if counts.get(name, 0) == 1 and name not in global_rebinds
        }

    # _collect_module_func_kinds / _collect_module_class_names /
    # _collect_module_class_graph moved to frontend/sema/ (F2b, doc 44):
    #   collect_module_func_kinds  -> sema/funcmeta.py
    #   collect_module_class_names -> sema/funcmeta.py
    #   build_class_graph          -> sema/classgraph.py
    # They are computed pre-walk by analyze_module() and populated into
    # self.module_declared_funcs / self.module_declared_classes /
    # self.module_class_bases / self.module_subclassed_names by
    # _populate_sema_state. AST suspension-shape facts are owned by
    # _function_param_names / _split_function_args / _default_specs_from_args /
    # _default_spec_for_expr STAY on the class — the lowering walk still calls them
    # at ~30 sites; sema/funcmeta.py carries its own private copies.)

    # _collect_module_const_dicts moved to frontend/sema/constenv.py (F2b,
    # doc 44): the module-level const-dict table is computed pre-walk by
    # sema.collect_module_const_dicts and populated into self.module_const_dicts
    # by _populate_sema_state.

    def _record_instance_attr_mutation(self, class_name: str, attr: str) -> None:
        if class_name not in self.classes:
            return
        self.instance_attr_mutations.setdefault(class_name, set()).add(attr)

    def _instance_attr_mutated(self, class_name: str, attr: str) -> bool:
        return attr in self.instance_attr_mutations.get(class_name, set())

    def _flush_deferred_module_attrs(self) -> None:
        if not self.deferred_module_attrs or self.module_obj is None:
            return
        for name in sorted(self.deferred_module_attrs):
            # Skip variables that are live in the module dict via
            # module_global_mutations (loop-carried variables).
            # Their current value is in the module dict, not in a
            # local SSA variable.  Writing back the stale SSA value
            # would overwrite the accumulated loop result.
            if name in self.module_global_mutations:
                continue
            val = self._load_local_value(name)
            if val is None:
                val = self.globals.get(name)
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self._emit_module_attr_set_on(self.module_obj, name, val)

    def _capture_function_state(self) -> dict[str, Any]:
        return {
            "current_class": self.current_class,
            "current_method_first_param": self.current_method_first_param,
            "locals": self.locals,
            "locals_cache_val": self.locals_cache_val,
            "boxed_locals": self.boxed_locals,
            "closure_locals": self.closure_locals,
            "comp_shadow_locals": self.comp_shadow_locals,
            "boxed_local_hints": self.boxed_local_hints,
            "free_vars": self.free_vars,
            "free_var_hints": self.free_var_hints,
            "global_decls": self.global_decls,
            "nonlocal_decls": self.nonlocal_decls,
            "scope_assigned": self.scope_assigned,
            "del_targets": self.del_targets,
            "unbound_check_names": self.unbound_check_names,
            "exact_locals": self.exact_locals,
            "exact_builtin_locals": self.exact_builtin_locals,
            "async_locals": self.async_locals,
            "async_internal_locals": self.async_internal_locals,
            "async_public_locals": self.async_public_locals,
            "async_locals_base": self.async_locals_base,
            "async_closure_offset": self.async_closure_offset,
            "async_local_hints": self.async_local_hints,
            "explicit_type_hints": self.explicit_type_hints,
            "container_elem_hints": self.container_elem_hints,
            "dict_key_hints": self.dict_key_hints,
            "dict_value_hints": self.dict_value_hints,
            "bytearray_len_hints": self.bytearray_len_hints,
            "context_depth": self.context_depth,
            "control_flow_depth": self.control_flow_depth,
            "const_ints": self.const_ints,
            "in_generator": self.in_generator,
            "async_context": self.async_context,
            "try_end_labels": self.try_end_labels,
            "try_scopes": self.try_scopes,
            "try_suppress_depth": self.try_suppress_depth,
            "try_handler_scopes": self.try_handler_scopes,
            "function_exception_label": self.function_exception_label,
            "exception_stack_depth_baseline": self.exception_stack_depth_baseline,
            "exception_stack_prev_baseline": self.exception_stack_prev_baseline,
            "return_unwind_depth": self.return_unwind_depth,
            "return_unwind_popped_scopes": self.return_unwind_popped_scopes,
            "active_exceptions": self.active_exceptions,
            "loop_guard_assumptions": self.loop_guard_assumptions,
            "return_label": self.return_label,
            "return_slot": self.return_slot,
            "return_slot_index": self.return_slot_index,
            "return_slot_offset": self.return_slot_offset,
            "defer_module_attrs": self.defer_module_attrs,
            "deferred_module_attrs": self.deferred_module_attrs,
            "imported_names": self.imported_names,
            "imported_attr_names": self.imported_attr_names,
            "imported_modules": self.imported_modules,
            "local_imported_names": self.local_imported_names,
            "local_imported_modules": self.local_imported_modules,
            "imported_module_attr_mutations": self.imported_module_attr_mutations,
            "class_definition_pending": self.class_definition_pending,
            "block_terminated": self.block_terminated,
            "_module_cache_values": self._module_cache_values,
        }

    def _restore_function_state(self, state: dict[str, Any]) -> None:
        self.current_class = state["current_class"]
        self.current_method_first_param = state["current_method_first_param"]
        self.locals = state["locals"]
        self.locals_cache_val = state["locals_cache_val"]
        self.boxed_locals = state["boxed_locals"]
        self.closure_locals = state["closure_locals"]
        self.comp_shadow_locals = state["comp_shadow_locals"]
        self.boxed_local_hints = state["boxed_local_hints"]
        self.free_vars = state["free_vars"]
        self.free_var_hints = state["free_var_hints"]
        self.global_decls = state["global_decls"]
        self.nonlocal_decls = state["nonlocal_decls"]
        self.scope_assigned = state["scope_assigned"]
        self.del_targets = state["del_targets"]
        self.unbound_check_names = state["unbound_check_names"]
        self.exact_locals = state["exact_locals"]
        self.exact_builtin_locals = state["exact_builtin_locals"]
        self.async_locals = state["async_locals"]
        self.async_internal_locals = state["async_internal_locals"]
        self.async_public_locals = state["async_public_locals"]
        self.async_locals_base = state["async_locals_base"]
        self.async_closure_offset = state["async_closure_offset"]
        self.async_local_hints = state["async_local_hints"]
        self.explicit_type_hints = state["explicit_type_hints"]
        self.container_elem_hints = state["container_elem_hints"]
        self.dict_key_hints = state["dict_key_hints"]
        self.dict_value_hints = state["dict_value_hints"]
        self.bytearray_len_hints = state["bytearray_len_hints"]
        self.context_depth = state["context_depth"]
        self.control_flow_depth = state["control_flow_depth"]
        self.const_ints = state["const_ints"]
        self.in_generator = state["in_generator"]
        self.async_context = state["async_context"]
        self.try_end_labels = state["try_end_labels"]
        self.try_scopes = state["try_scopes"]
        self.try_suppress_depth = state["try_suppress_depth"]
        self.try_handler_scopes = state["try_handler_scopes"]
        self.function_exception_label = state["function_exception_label"]
        self.exception_stack_depth_baseline = state["exception_stack_depth_baseline"]
        self.exception_stack_prev_baseline = state["exception_stack_prev_baseline"]
        self.return_unwind_depth = state["return_unwind_depth"]
        self.return_unwind_popped_scopes = state["return_unwind_popped_scopes"]
        self.active_exceptions = state["active_exceptions"]
        self.loop_guard_assumptions = state["loop_guard_assumptions"]
        self.return_label = state["return_label"]
        self.return_slot = state["return_slot"]
        self.return_slot_index = state["return_slot_index"]
        self.return_slot_offset = state["return_slot_offset"]
        self.defer_module_attrs = state["defer_module_attrs"]
        self.deferred_module_attrs = state["deferred_module_attrs"]
        self.imported_names = state["imported_names"]
        self.imported_attr_names = state["imported_attr_names"]
        self.imported_modules = state["imported_modules"]
        self.local_imported_names = state["local_imported_names"]
        self.local_imported_modules = state["local_imported_modules"]
        self.imported_module_attr_mutations = state["imported_module_attr_mutations"]
        self.class_definition_pending = state["class_definition_pending"]
        self.block_terminated = state["block_terminated"]
        self._module_cache_values = state["_module_cache_values"]

    def _populate_sema_state(self, node: ast.Module) -> SemaResult:
        """Compute the module's :class:`SemaResult` once, pre-walk, and populate
        the existing god-object pre-walk state dicts from it (doc 44 §F2b).

        This is the additive shim: the lowering walk continues to read the same
        ``self.module_*`` dicts, so the emitted IR is byte-identical to the prior
        inline-computation path.  Each dict is filled from exactly one
        ``SemaResult`` field — this assignment table IS the F2c worklist (the
        read-sites that F2c rewires onto ``self._sema`` and then deletes the dict).

        Shim inventory — god-object dict  ←  SemaResult source:

          self.module_const_dicts         ← sema.const_dicts
                                            (sema/constenv.collect_module_const_dicts)
          self.module_declared_funcs      ← sema.function_meta.declared_funcs
                                            (sema/funcmeta.collect_module_func_kinds)
          self.module_declared_classes    ← sema.function_meta.declared_classes
                                            (sema/funcmeta.collect_module_class_names)
          self.module_class_bases         ← sema.class_graph.bases_by_class
                                            (sema/classgraph.build_class_graph)
          self.module_subclassed_names    ← sema.class_graph.subclassed_names
                                            (sema/classgraph.build_class_graph)
          self.module_func_defaults       ← known_func_defaults override, else
                                            sema.function_meta.defaults
                                            (sema/funcmeta.collect_module_func_defaults)

        These six dicts are each written exactly once (here) and only *read* during
        the walk — verified against HEAD: no ``.add``/``.pop``/``[k]=`` mutation of
        any of them occurs in the visit/emit methods.  Walk-mutated cursors
        (``const_ints`` written in ``emit()``; ``exact_locals`` mutated across
        visit methods; the per-function scope dicts populated lazily in
        ``start_function``) are deliberately NOT Sema facts and stay where they are
        (doc 44 risk #1: mis-classifying a cursor as a fact is a miscompile).
        """
        sema = analyze_module(node)
        self._sema = sema
        self.module_const_dicts = sema.const_dicts
        self.module_declared_funcs = sema.function_meta.declared_funcs
        self.module_declared_classes = sema.function_meta.declared_classes
        self.module_class_bases = sema.class_graph.bases_by_class
        self.module_subclassed_names = sema.class_graph.subclassed_names
        self.module_func_defaults = self.known_func_defaults.get(
            self.module_name, sema.function_meta.defaults
        )
        return sema

    def _init_return_slot(self) -> None:
        if self.return_label is not None:
            return
        if not self.is_async():
            return
        self.return_label = self.next_label()
        self.return_slot_index = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=self.return_slot_index))
        init = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        self.return_slot = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=self.return_slot))

    def _store_return_slot_for_stateful(self) -> None:
        if not self.is_async() or self.return_slot is None:
            return
        if self.return_slot_offset is None:
            self.return_slot_offset = self._async_local_offset("__molt_return_slot")
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", self.return_slot_offset, self.return_slot],
                result=MoltValue("none"),
            )
        )

    def _load_return_slot(self) -> MoltValue | None:
        if self.return_slot is None:
            return None
        if self.is_async() and self.return_slot_offset is not None:
            slot_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.return_slot_offset],
                    result=slot_val,
                )
            )
            return slot_val
        return self.return_slot

    def _load_return_slot_index(self) -> MoltValue:
        if self.is_async():
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            return idx
        idx = self.return_slot_index
        if idx is None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.return_slot_index = idx
        return idx

    def _emit_return_value(self, value: MoltValue) -> None:
        exit_baseline_now = self.return_slot is None or self.return_label is None
        if exit_baseline_now:
            self._emit_plain_local_scope_exit_boundaries(preserve=value)
            if self.current_func_name != "molt_main":
                self._emit_boxed_locals_cleanup()
            self._emit_restore_exception_stack_depth(exit_baseline=True)
            if self._function_needs_frame_trace():
                self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ret", args=[value], result=MoltValue("none")))
            return
        self._emit_restore_exception_stack_depth(exit_baseline=False)
        slot = self._load_return_slot()
        if slot is None:
            self._emit_plain_local_scope_exit_boundaries(preserve=value)
            if self.current_func_name != "molt_main":
                self._emit_boxed_locals_cleanup()
            if self._function_needs_frame_trace():
                self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ret", args=[value], result=MoltValue("none")))
            return
        idx = self._load_return_slot_index()
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[slot, idx, value],
                result=MoltValue("none"),
            )
        )
        self._emit_plain_local_scope_exit_boundaries()
        if self.current_func_name != "molt_main":
            self._emit_boxed_locals_cleanup()
        self.emit(
            MoltOp(kind="JUMP", args=[self.return_label], result=MoltValue("none"))
        )

    def _emit_return_label(self) -> None:
        if self.return_label is None or self.return_slot is None:
            return
        self.emit(
            MoltOp(kind="LABEL", args=[self.return_label], result=MoltValue("none"))
        )
        self._emit_restore_exception_stack_depth()
        slot = self._load_return_slot()
        if slot is None:
            return
        res = MoltValue(self.next_var())
        idx = self._load_return_slot_index()
        self.emit(MoltOp(kind="INDEX", args=[slot, idx], result=res))
        if self._function_needs_frame_trace():
            self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))

    def _emit_boxed_locals_cleanup(self) -> None:
        if not self.boxed_locals:
            return
        skip = set(self.free_vars) | self.closure_locals
        for name, cell in self.boxed_locals.items():
            if name in skip:
                continue
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            missing = self._emit_missing_value()
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, missing],
                    result=MoltValue("none"),
                )
            )

    def _emit_restore_exception_stack_depth(
        self, *, exit_baseline: bool = True
    ) -> None:
        baseline = self.exception_stack_depth_baseline
        if baseline is not None:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_SET_DEPTH",
                    args=[baseline],
                    result=MoltValue("none"),
                )
            )
        if not exit_baseline:
            return
        prev_baseline = self.exception_stack_prev_baseline
        if prev_baseline is None:
            return
        self.emit(
            MoltOp(
                kind="EXCEPTION_STACK_EXIT",
                args=[prev_baseline],
                result=MoltValue("none"),
            )
        )

    def _emit_function_exception_handler(self, *, clear_handlers: bool = False) -> None:
        label = self.function_exception_label
        if label is None:
            return
        module_failure_cleanup = bool(
            self.module_name
            and (
                self.current_func_name == "molt_main"
                or self.current_func_name.startswith("molt_init_")
            )
        )
        if module_failure_cleanup and not self._ends_with_return_jump():
            self.emit(MoltOp(kind="ret_void", args=[], result=MoltValue("none")))
        prev_label = self.function_exception_label
        self.function_exception_label = None
        with self._suppress_check_exception(emit_on_exit=False):
            self.emit(MoltOp(kind="LABEL", args=[label], result=MoltValue("none")))
            if module_failure_cleanup:
                module_name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR",
                        args=[self.module_name],
                        result=module_name_val,
                    )
                )
                self.emit(
                    MoltOp(
                        kind="MODULE_CACHE_DEL",
                        args=[module_name_val],
                        result=MoltValue("none"),
                    )
                )
                if (
                    self.entry_module
                    and self.module_name == self.entry_module
                    and self.module_name != "__main__"
                ):
                    main_name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(
                            kind="CONST_STR", args=["__main__"], result=main_name_val
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="MODULE_CACHE_DEL",
                            args=[main_name_val],
                            result=MoltValue("none"),
                        )
                    )
        self._emit_restore_exception_stack_depth()
        if self._function_needs_frame_trace():
            self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True, clear_handlers=clear_handlers)
        self.function_exception_label = prev_label

    def _ends_with_return_jump(self) -> bool:
        if not self.current_ops:
            return False
        last = self.current_ops[-1]
        if last.kind in {"ret", "ret_void"}:
            return True
        if (
            last.kind == "JUMP"
            and self.return_label is not None
            and last.args
            and last.args[0] == self.return_label
        ):
            return True
        return False

    def resume_function(self, name: str) -> None:
        if self.current_func_name != name:
            self._emit_function_exception_handler()
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]

    def _parse_container_hint(self, hint: str) -> tuple[str, str | None]:
        if hint.endswith("]") and "[" in hint:
            base, inner = hint.split("[", 1)
            base = base.strip()
            inner = inner[:-1].strip()
            if base in {"list", "tuple"} and inner:
                if "," in inner:
                    parts = [part.strip() for part in inner.split(",") if part.strip()]
                    if parts:
                        inner = parts[0]
                return base, inner
            if base == "dict":
                return base, None
        return hint, None

    def _parse_dict_hint(self, hint: str) -> tuple[str | None, str | None]:
        if not hint.startswith("dict[") or not hint.endswith("]"):
            return None, None
        inner = hint[len("dict[") : -1]
        parts = [part.strip() for part in inner.split(",") if part.strip()]
        if len(parts) != 2:
            return None, None
        return parts[0], parts[1]

    def _expr_is_data_descriptor(self, expr: ast.expr) -> bool:
        if isinstance(expr, ast.Call) and isinstance(expr.func, ast.Name):
            if expr.func.id == "property":
                return True
            class_info = self.classes.get(expr.func.id)
            if class_info:
                methods = class_info.get("methods", {})
                return "__set__" in methods or "__delete__" in methods
        return False

    def _class_attr_is_data_descriptor(self, class_name: str, attr: str) -> bool:
        class_info = self.classes.get(class_name)
        if not class_info:
            return False
        for mro_name in class_info.get("mro", [class_name]):
            mro_info = self.classes.get(mro_name)
            if not mro_info:
                continue
            class_attrs = mro_info.get("class_attrs", {})
            expr = class_attrs.get(attr)
            if expr is not None and self._expr_is_data_descriptor(expr):
                return True
            method_info = mro_info.get("methods", {}).get(attr)
            if method_info and method_info["descriptor"] == "property":
                return True
        return False

    def _class_layout_stable(self, class_name: str) -> bool:
        class_info = self.classes.get(class_name)
        if not class_info:
            return False
        if class_info.get("dynamic") or class_info.get("dataclass"):
            return False
        if class_name in self.mutated_classes:
            return False
        return True

    def _task_closure_size(
        self, payload_slots: int, *, include_gen_control: bool
    ) -> int:
        base = self.async_locals_base + len(self.async_locals) * 8
        required = payload_slots * 8
        if include_gen_control:
            required += GEN_CONTROL_SIZE
        if base < required:
            return required
        return base

    def _apply_hint_to_value(
        self, _name: str | None, value: MoltValue, hint: str
    ) -> None:
        base, elem = self._parse_container_hint(hint)
        value.type_hint = base
        if self.current_func_name == "molt_main":
            elem_target = self.global_elem_hints
            key_target = self.global_dict_key_hints
            val_target = self.global_dict_value_hints
        else:
            elem_target = self.container_elem_hints
            key_target = self.dict_key_hints
            val_target = self.dict_value_hints
        key = value.name
        if base == "dict":
            dict_key, dict_val = self._parse_dict_hint(hint)
            if dict_key and dict_val:
                key_target[key] = dict_key
                val_target[key] = dict_val
            else:
                key_target.pop(key, None)
                val_target.pop(key, None)
            elem_target.pop(key, None)
        else:
            if elem:
                elem_target[key] = elem
            else:
                elem_target.pop(key, None)
            key_target.pop(key, None)
            val_target.pop(key, None)

    def _propagate_container_hints(self, dest: str, src: MoltValue) -> None:
        if self.current_func_name == "molt_main":
            elem_map = self.global_elem_hints
            key_map = self.global_dict_key_hints
            val_map = self.global_dict_value_hints
        else:
            elem_map = self.container_elem_hints
            key_map = self.dict_key_hints
            val_map = self.dict_value_hints
        if src.name in elem_map:
            elem_map[dest] = elem_map[src.name]
        else:
            elem_map.pop(dest, None)
        if src.name in key_map and src.name in val_map:
            key_map[dest] = key_map[src.name]
            val_map[dest] = val_map[src.name]
        else:
            key_map.pop(dest, None)
            val_map.pop(dest, None)
        # Propagate list_int container tracking across assignments
        li_set = getattr(self, "_list_int_containers", set())
        if src.name in li_set:
            li_set.add(dest)
        else:
            li_set.discard(dest)
        if src.name in self.bytearray_len_hints:
            self.bytearray_len_hints[dest] = self.bytearray_len_hints[src.name]
        else:
            self.bytearray_len_hints.pop(dest, None)

    def _record_list_element_write(
        self,
        target: MoltValue,
        target_name: str | None,
        elem_hint: str | None,
    ) -> None:
        elem_map = (
            self.global_elem_hints
            if self.current_func_name == "molt_main"
            else self.container_elem_hints
        )
        keys = [target.name]
        if target_name is not None:
            keys.append(target_name)
        current = next((elem_map[key] for key in keys if key in elem_map), None)
        if elem_hint in {None, "Any", "Unknown", "missing"}:
            for key in keys:
                elem_map.pop(key, None)
            return
        if current is not None and current != elem_hint:
            for key in keys:
                elem_map.pop(key, None)
            return
        for key in keys:
            elem_map[key] = elem_hint

    def _bytearray_len_hint_for(
        self, name: str | None, value: MoltValue | None
    ) -> int | None:
        if value is not None and value.name in self.bytearray_len_hints:
            return self.bytearray_len_hints[value.name]
        if name is not None:
            return self.bytearray_len_hints.get(name)
        return None

    def _invalidate_bytearray_len_hint(
        self, name: str | None, value: MoltValue | None = None
    ) -> None:
        if value is not None:
            self.bytearray_len_hints.pop(value.name, None)
        if name is not None:
            self.bytearray_len_hints.pop(name, None)

    def _copy_container_hints_for_name_load(self, var_name: str, ssa_name: str) -> None:
        """Copy container element/dict hints from a Python variable binding to
        a fresh SSA name produced by a load."""
        if self.current_func_name == "molt_main":
            elem_map = self.global_elem_hints
            key_map = self.global_dict_key_hints
            val_map = self.global_dict_value_hints
        else:
            elem_map = self.container_elem_hints
            key_map = self.dict_key_hints
            val_map = self.dict_value_hints
        if var_name in elem_map:
            elem_map[ssa_name] = elem_map[var_name]
        if var_name in key_map:
            key_map[ssa_name] = key_map[var_name]
        if var_name in val_map:
            val_map[ssa_name] = val_map[var_name]
        # Propagate list_int container tracking to boxed reload
        li_set = getattr(self, "_list_int_containers", set())
        if var_name in li_set:
            li_set.add(ssa_name)

    def _container_elem_hint(self, value: MoltValue) -> str | None:
        if value.name in self.container_elem_hints:
            return self.container_elem_hints[value.name]
        return self.global_elem_hints.get(value.name)

    def _dict_key_hint(self, value: MoltValue) -> str | None:
        if value.name in self.dict_key_hints:
            return self.dict_key_hints[value.name]
        return self.global_dict_key_hints.get(value.name)

    def _iterable_element_hint(self, iterable: MoltValue) -> str | None:
        hint = iterable.type_hint
        if hint in {"range", "intarray"}:
            return "int"
        if hint == "str":
            return "str"
        if hint in {"bytes", "bytearray"}:
            return "int"
        if hint == "file_text":
            return "str"
        if hint == "file_bytes":
            return "bytes"
        if hint == "dict":
            return self._dict_key_hint(iterable)
        return self._container_elem_hint(iterable)

    def _reduction_acc_numeric_hint(self, name: str, value: MoltValue) -> str | None:
        hint = self.boxed_local_hints.get(name) or value.type_hint
        if hint in {"int", "float"}:
            return hint
        return None

    def _dict_value_hint(self, value: MoltValue) -> str | None:
        if value.name in self.dict_value_hints:
            return self.dict_value_hints[value.name]
        return self.global_dict_value_hints.get(value.name)

    def _apply_type_facts(self, func_name: str) -> None:
        if self.type_facts is None:
            return
        if func_name == "molt_main":
            hints = self.type_facts.hints_for_globals(
                self.type_facts_module, self.type_hint_policy
            )
        else:
            hints = self.type_facts.hints_for_function(
                self.type_facts_module, func_name, self.type_hint_policy
            )
        self.explicit_type_hints.update(hints)

    def _annotation_to_hint(self, node: ast.expr | None) -> str | None:
        if node is None:
            return None
        try:
            text = ast.unparse(node)
        except Exception:
            return None
        stripped = text.strip()
        if stripped[:1] in {"'", '"'} and stripped[-1:] == stripped[:1]:
            stripped = stripped[1:-1]
        return normalize_type_hint(stripped)

    def _annotation_source(self, node: ast.expr) -> str:
        try:
            return ast.unparse(node)
        except Exception as exc:
            raise NotImplementedError("Unsupported annotation expression") from exc

    def _emit_annotation_value(
        self, node: ast.expr, *, stringize: bool | None = None
    ) -> MoltValue:
        use_string = self.future_annotations if stringize is None else stringize
        if use_string:
            text = self._annotation_source(node)
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[text], result=res))
            return res
        prev_in_annotation = self.in_annotation
        self.in_annotation = True
        try:
            val = self.visit(node)
        finally:
            self.in_annotation = prev_in_annotation
        if val is None:
            raise NotImplementedError("Unsupported annotation expression")
        return val

    def _annotation_exec_name(self, owner: str) -> str:
        name = f"__molt_annotations_exec_{owner}_{self.annotation_name_counter}"
        self.annotation_name_counter += 1
        return name

    def _annotation_exec_id(self, *, is_module: bool) -> int:
        if is_module:
            ident = self.module_annotation_exec_counter
            self.module_annotation_exec_counter += 1
            return ident
        ident = self.class_annotation_exec_counter
        self.class_annotation_exec_counter += 1
        return ident

    def _annotate_qualname(self) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return "__annotate__"
        return f"{prefix}.__annotate__"

    def _ensure_module_annotation_exec_map(self) -> MoltValue:
        if self.module_annotation_exec_map is not None:
            return self.module_annotation_exec_map
        if self.module_chunking and self.module_annotation_exec_name:
            existing = self._emit_module_attr_get(self.module_annotation_exec_name)
            self.module_annotation_exec_map = existing
            return existing
        owner = self._sanitize_module_name(self.module_name)
        name = self._annotation_exec_name(owner)
        self.module_annotation_exec_name = name
        exec_map = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=exec_map))
        self.module_annotation_exec_map = exec_map
        self._store_local_value(name, exec_map)
        if self.current_func_name.startswith("molt_init_"):
            self.globals[name] = exec_map
            self._emit_module_attr_set(name, exec_map)
        return exec_map

    def _emit_annotation_exec_mark(self, exec_map: MoltValue, exec_id: int) -> None:
        key_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[exec_id], result=key_val))
        val_val = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=val_val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[exec_map, key_val, val_val],
                result=MoltValue("none"),
            )
        )

    def _emit_module_annotations_dict(self) -> MoltValue:
        if self.control_flow_depth > 0:
            self.module_annotations_conditional = True
        if not self.module_annotations_conditional:
            if self.module_annotations is not None:
                return self.module_annotations
            existing = self.locals.get("__annotations__")
            if existing is not None and existing.type_hint == "dict":
                self.module_annotations = existing
                return existing
            ann = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
            self._emit_module_attr_set("__annotations__", ann)
            if self.current_func_name == "molt_main":
                self.globals["__annotations__"] = ann
            self.locals["__annotations__"] = ann
            self.module_annotations = ann
            return ann
        return self._emit_module_annotations_dict_dynamic()

    def _emit_module_annotations_dict_dynamic(self) -> MoltValue:
        module_dict = self._emit_globals_dict()
        key_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__annotations__"], result=key_val))
        default_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        existing = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="DICT_GET",
                args=[module_dict, key_val, default_val],
                result=existing,
            )
        )
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[existing, default_val], result=is_none))
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            ann = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[module_dict, key_val, ann],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            merged = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="PHI", args=[ann, existing], result=merged))
            return merged

        placeholder = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[placeholder], result=cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        ann = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[module_dict, key_val, ann],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, ann],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, existing],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        merged = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=merged))
        return merged

    def _annotation_items_for_function(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[tuple[str, ast.expr]]:
        items: list[tuple[str, ast.expr]] = []
        for arg in node.args.posonlyargs + node.args.args:
            if arg.annotation is not None:
                items.append((arg.arg, arg.annotation))
        if node.args.vararg is not None and node.args.vararg.annotation is not None:
            items.append((node.args.vararg.arg, node.args.vararg.annotation))
        for arg in node.args.kwonlyargs:
            if arg.annotation is not None:
                items.append((arg.arg, arg.annotation))
        if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
            items.append((node.args.kwarg.arg, node.args.kwarg.annotation))
        if node.returns is not None:
            items.append(("return", node.returns))
        return items

    def _emit_type_params_values(
        self, type_params: Sequence[ast.AST | ast.type_param] | None
    ) -> tuple[list[MoltValue], dict[str, MoltValue]]:
        if not type_params:
            return [], {}
        type_param_func = self._emit_module_attr_get_on("typing", "_molt_type_param")
        values: list[MoltValue] = []
        mapping: dict[str, MoltValue] = {}
        for param in type_params:
            if isinstance(param, (ast.TypeVar, ast.ParamSpec, ast.TypeVarTuple)):
                name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[param.name], result=name_val))
                call_args: list[MoltValue] = [type_param_func, name_val]
                default_expr = getattr(param, "default_value", None)
                if default_expr is not None:
                    default_val = self._emit_annotation_value(
                        default_expr, stringize=self.future_annotations
                    )
                    call_args.append(default_val)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="CALL_FUNC", args=call_args, result=res))
                values.append(res)
                mapping[param.name] = res
                continue
            raise NotImplementedError(
                f"Unsupported type parameter: {type(param).__name__}"
            )
        return values, mapping

    def _emit_attach_type_params(
        self, owner: MoltValue, type_params: list[MoltValue]
    ) -> None:
        if not type_params:
            return
        tuple_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=type_params, result=tuple_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[owner, "__type_params__", tuple_val],
                result=MoltValue("none"),
            )
        )

    def _emit_annotate_function_obj(
        self,
        *,
        items: list[tuple[str, ast.expr, int]],
        exec_map_name: str | None,
        stringize: bool,
        module_override: str | None = None,
    ) -> MoltValue:
        func_symbol = self._function_symbol("__annotate__")
        free_vars: set[str] = set()
        for _name, expr, _exec_id in items:
            free_vars.update(self._collect_annotation_free_vars(expr))
        if exec_map_name and self.current_func_name != "molt_main":
            free_vars.add(exec_map_name)
        free_vars_list = sorted(free_vars)
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if free_vars_list and self.current_func_name != "molt_main":
            self.unbound_check_names.update(free_vars_list)
            for name in free_vars_list:
                self._box_local(name)
                self.closure_locals.add(name)
            for name in free_vars_list:
                hint = self.boxed_local_hints.get(name)
                if hint is None:
                    value = self.locals.get(name)
                    if value is not None and value.type_hint:
                        hint = value.type_hint
                free_var_hints[name] = hint or "Any"
            closure_items = self._closure_cells_for(free_vars_list)
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val))
            has_closure = True
        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, 1, closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(MoltOp(kind="FUNC_NEW", args=[func_symbol, 1], result=func_val))
        self._emit_function_metadata(
            func_val,
            name="__annotate__",
            qualname=self._annotate_qualname(),
            trace_lineno=None,
            posonly_params=["format"],
            pos_or_kw_params=[],
            kwonly_params=[],
            vararg=None,
            varkw=None,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=None,
            module_override=module_override,
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        params = ["format"]
        if has_closure:
            params = [_MOLT_CLOSURE_PARAM] + params
        self.start_function(func_symbol, params=params, type_facts_name="__annotate__")
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars_list)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.del_targets = set()
        self.unbound_check_names = set()
        format_val = MoltValue("format", type_hint="int")
        self.locals["format"] = format_val

        one_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one_val))
        is_one = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[format_val, one_val], result=is_one))
        two_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[2], result=two_val))
        is_two = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[format_val, two_val], result=is_two))
        exec_map_val: MoltValue | None = None
        if exec_map_name is not None:
            exec_map_val = self.visit(ast.Name(id=exec_map_name, ctx=ast.Load()))
        missing_val = MoltValue(self.next_var(), type_hint="missing")
        self.emit(MoltOp(kind="MISSING", args=[], result=missing_val))

        def emit_annotation_body(use_stringize: bool) -> None:
            res_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res_dict))
            for name, expr, exec_id in items:
                if exec_map_val is not None:
                    key_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[exec_id], result=key_val))
                    exec_flag = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="DICT_GET",
                            args=[exec_map_val, key_val, missing_val],
                            result=exec_flag,
                        )
                    )
                    is_missing = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(
                            kind="IS",
                            args=[exec_flag, missing_val],
                            result=is_missing,
                        )
                    )
                    self.emit(
                        MoltOp(kind="IF", args=[is_missing], result=MoltValue("none"))
                    )
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                value_val = self._emit_annotation_value(expr, stringize=use_stringize)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res_dict, key_val, value_val],
                        result=MoltValue("none"),
                    )
                )
                if exec_map_val is not None:
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ret", args=[res_dict], result=MoltValue("none")))

        self.emit(MoltOp(kind="IF", args=[is_one], result=MoltValue("none")))
        emit_annotation_body(stringize)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_two], result=MoltValue("none")))
        emit_annotation_body(stringize)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=msg_val))
        err_val = self._emit_exception_new("NotImplementedError", msg_val)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        return func_val

    def _emit_function_annotate(
        self, func_val: MoltValue, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> None:
        items = self._annotation_items_for_function(node)
        type_params = getattr(node, "type_params", None)
        type_param_vals, type_param_map = self._emit_type_params_values(type_params)
        if not items:
            self._emit_attach_type_params(func_val, type_param_vals)
            return
        annotated_items = [(name, expr, idx) for idx, (name, expr) in enumerate(items)]
        prev_type_params = self.annotation_type_params
        if type_param_map:
            merged = dict(prev_type_params)
            merged.update(type_param_map)
            self.annotation_type_params = merged
        try:
            if not self.future_annotations and not self.eager_annotations:
                annotate_val = self._emit_annotate_function_obj(
                    items=annotated_items,
                    exec_map_name=None,
                    stringize=self.future_annotations,
                )
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[func_val, "__annotate__", annotate_val],
                        result=MoltValue("none"),
                    )
                )
            else:
                # Build __annotations__ dict directly from the annotation
                # items.  For future_annotations, all values are strings.
                # For eager_annotations, they're evaluated types.
                # Calling __annotate__(1) through CALL_FUNC has been unreliable
                # for TIR-compiled functions, so we build the dict inline.
                ann_items: list[MoltValue] = []
                for name, expr in items:
                    key_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                    val = self._emit_annotation_value(
                        expr, stringize=self.future_annotations
                    )
                    ann_items.extend([key_val, val])
                ann_dict = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=ann_items, result=ann_dict))
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[func_val, "__annotations__", ann_dict],
                        result=MoltValue("none"),
                    )
                )
        finally:
            self.annotation_type_params = prev_type_params
        self._emit_attach_type_params(func_val, type_param_vals)

    def _guard_tag_for_hint(self, hint: str) -> int | None:
        mapping = {
            "Any": 0,
            "Unknown": 0,
            "int": 1,
            "float": 2,
            "bool": 3,
            "None": 4,
            "str": 5,
            "bytes": 6,
            "bytearray": 7,
            "complex": 19,
            "list": 8,
            "tuple": 9,
            "dict": 10,
            "range": 11,
            "slice": 12,
            "dataclass": 13,
            "buffer2d": 14,
            "memoryview": 15,
            "intarray": 16,
            "set": 17,
            "frozenset": 18,
        }
        return mapping.get(hint)

    def _emit_guard_type(self, value: MoltValue, hint: str) -> None:
        base = hint.split("[", 1)[0] if "[" in hint else hint
        tag = self._guard_tag_for_hint(base)
        if tag is None or tag == 0:
            return
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[tag], result=tag_val))
        self.emit(
            MoltOp(kind="GUARD_TAG", args=[value, tag_val], result=MoltValue("none"))
        )

    def _get_or_emit_module_cache(
        self, module_name: str, *, effect_proof: str | None = None
    ) -> MoltValue:
        """Return a MoltValue for *module_name* from MODULE_CACHE_GET.

        Emits a fresh CONST_STR + MODULE_CACHE_GET pair on every call.  Earlier
        versions cached the MoltValue across the function scope, but state-machine
        lowering (used for module init functions with jumps/labels) can place the
        first MODULE_CACHE_GET in a branch that is skipped when a preceding
        exception redirects the state machine.  Re-emitting the lookup each time
        ensures the local is populated in the state that actually uses it.

        Note: this helper is only appropriate for simple, unconditional MODULE_CACHE_GET
        calls (i.e. for the *current* module or other modules that are guaranteed already
        loaded).  Use ``_emit_module_load`` for modules that may need lazy-initialisation.
        """
        module_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_name_val))
        module_val = MoltValue(self.next_var(), type_hint="module")
        metadata = {"effect_proof": effect_proof} if effect_proof else None
        self.emit(
            MoltOp(
                kind="MODULE_CACHE_GET",
                args=[module_name_val],
                result=module_val,
                metadata=metadata,
            )
        )
        return module_val

    def _emit_module_attr_set(
        self, name: str, value: MoltValue, *, defer: bool = True
    ) -> None:
        if self.current_func_name != "molt_main" or self.module_obj is None:
            return
        if defer and self.defer_module_attrs:
            self.deferred_module_attrs.add(name)
            return
        if not defer and self.defer_module_attrs:
            self.deferred_module_attrs.discard(name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[self.module_obj, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _emit_module_attr_set_on(
        self, module_val: MoltValue, name: str, value: MoltValue
    ) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )
        # Track the value's type hint so _emit_module_attr_get can propagate
        # it to downstream consumers (enabling fast_int/fast_float paths).
        if (
            isinstance(value, MoltValue)
            and value.type_hint
            and value.type_hint != "Any"
        ):
            self._module_attr_type_hints[name] = value.type_hint

    def _emit_module_global_del(self, name: str) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self.module_obj
        if self.current_func_name != "molt_main" or module_val is None:
            module_val = self._get_or_emit_module_cache(self.module_name)
        self.emit(
            MoltOp(
                kind="MODULE_DEL_GLOBAL",
                args=[module_val, name_val],
                result=MoltValue("none"),
            )
        )

    def _emit_module_global_del_safe(self, name: str) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self.module_obj
        if self.current_func_name != "molt_main" or module_val is None:
            module_val = self._get_or_emit_module_cache(self.module_name)
        self.emit(
            MoltOp(
                kind="MODULE_DEL_GLOBAL_IF_PRESENT",
                args=[module_val, name_val],
                result=MoltValue("none"),
            )
        )

    def _emit_function_default_values(
        self,
        func_val: MoltValue,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        kwonly_params: list[str],
    ) -> tuple[MoltValue, MoltValue, MoltValue]:
        yield_in_defaults = False
        yield_in_kwdefaults = False
        func_spill: int | None = None
        if self.in_generator:
            yield_in_defaults = any(
                expression_contains_yield(expr) for expr in default_exprs
            )
            yield_in_kwdefaults = any(
                expression_contains_yield(expr)
                for expr in kw_default_exprs
                if expr is not None
            )
            if yield_in_defaults or yield_in_kwdefaults:
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )

        if default_exprs:
            default_vals: list[MoltValue] = []
            for expr in default_exprs:
                val = self.visit(expr)
                if val is None:
                    val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                default_vals.append(val)
            defaults_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(kind="TUPLE_NEW", args=default_vals, result=defaults_tuple)
            )
            if func_spill is not None and yield_in_defaults:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            defaults_val = defaults_tuple
        else:
            defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=defaults_none))
            defaults_val = defaults_none

        if kw_default_exprs and kwonly_params:
            kw_pairs: list[MoltValue] = []
            for name, expr in zip(kwonly_params, kw_default_exprs):
                if expr is None:
                    continue
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                val = self.visit(expr)
                if val is None:
                    val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                kw_pairs.extend([key_val, val])
            if kw_pairs:
                kw_defaults = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=kw_pairs, result=kw_defaults))
                if func_spill is not None and yield_in_kwdefaults:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                kwdefaults_val = kw_defaults
            else:
                kw_defaults_none = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=kw_defaults_none))
                if func_spill is not None and yield_in_kwdefaults:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                kwdefaults_val = kw_defaults_none
        else:
            kw_defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=kw_defaults_none))
            kwdefaults_val = kw_defaults_none
        if func_spill is not None and (yield_in_defaults or yield_in_kwdefaults):
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        return func_val, defaults_val, kwdefaults_val

    def _emit_function_metadata(
        self,
        func_val: MoltValue,
        *,
        name: str,
        qualname: str,
        trace_filename: str | None = None,
        trace_lineno: int | None = None,
        trace_name: str | None = None,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        docstring: str | None,
        module_override: str | None = None,
        is_coroutine: bool = False,
        is_generator: bool = False,
        is_async_generator: bool = False,
        bind_kind: int | None = None,
        poll_fn_symbol: str | None = None,
        emit_code: bool = True,
        varnames: list[str] | None = None,
        code_names: list[str] | None = None,
    ) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))

        qual_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[qualname], result=qual_val))

        module_name = module_override or self.module_name
        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_val))

        arg_name_vals: list[MoltValue] = []
        for param in posonly_params + pos_or_kw_params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            arg_name_vals.append(param_val)
        arg_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=arg_name_vals, result=arg_names_tuple))

        posonly_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[len(posonly_params)], result=posonly_val))

        kwonly_name_vals: list[MoltValue] = []
        for param in kwonly_params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            kwonly_name_vals.append(param_val)
        kwonly_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=kwonly_name_vals, result=kwonly_tuple))

        if vararg is None:
            vararg_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=vararg_val))
        else:
            vararg_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[vararg], result=vararg_val))

        if varkw is None:
            varkw_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=varkw_val))
        else:
            varkw_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[varkw], result=varkw_val))
        func_val, defaults_val, kwdefaults_val = self._emit_function_default_values(
            func_val, default_exprs, kw_default_exprs, kwonly_params
        )

        if bind_kind is not None:
            bind_kind_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[bind_kind], result=bind_kind_val))
        else:
            bind_kind_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=bind_kind_val))

        if docstring is None:
            doc_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=doc_val))
        else:
            doc_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[docstring], result=doc_val))

        code_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=code_val))
        if emit_code:
            filename = trace_filename or self.source_path or "<unknown>"
            trace_label = trace_name or qualname or name
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[filename], result=file_val))
            line_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[int(trace_lineno or 0)],
                    result=line_val,
                )
            )
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[trace_label], result=name_val))
            linetable_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=linetable_val))
            varnames_list = varnames
            if varnames_list is None:
                varnames_list = self._varnames_from_params(
                    posonly_params=posonly_params,
                    pos_or_kw_params=pos_or_kw_params,
                    kwonly_params=kwonly_params,
                    vararg=vararg,
                    varkw=varkw,
                )
            varname_vals: list[MoltValue] = []
            for varname in varnames_list:
                var_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[varname], result=var_val))
                varname_vals.append(var_val)
            varnames_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(kind="TUPLE_NEW", args=varname_vals, result=varnames_tuple)
            )
            code_name_vals: list[MoltValue] = []
            if code_names is not None:
                for code_name in code_names:
                    code_name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[code_name], result=code_name_val)
                    )
                    code_name_vals.append(code_name_val)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=code_name_vals, result=names_tuple))
            argcount_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[len(posonly_params) + len(pos_or_kw_params)],
                    result=argcount_val,
                )
            )
            posonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[len(posonly_params)], result=posonly_val)
            )
            kwonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[len(kwonly_params)], result=kwonly_val)
            )
            code_val = MoltValue(self.next_var(), type_hint="code")
            self.emit(
                MoltOp(
                    kind="CODE_NEW",
                    args=[
                        file_val,
                        name_val,
                        line_val,
                        linetable_val,
                        varnames_tuple,
                        names_tuple,
                        argcount_val,
                        posonly_val,
                        kwonly_val,
                    ],
                    result=code_val,
                )
            )
            code_symbol = self._code_symbol_for_value(func_val)
            if code_symbol is not None:
                code_id = self._register_code_symbol(code_symbol)
                self.emit(
                    MoltOp(
                        kind="CODE_SLOT_SET",
                        args=[code_val],
                        result=MoltValue("none"),
                        metadata={"code_id": code_id},
                    )
                )
            if poll_fn_symbol is not None:
                self.emit(
                    MoltOp(
                        kind="FN_PTR_CODE_SET",
                        args=[poll_fn_symbol, code_val],
                        result=MoltValue("none"),
                    )
                )

        metadata_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(
            MoltOp(
                kind="TUPLE_NEW",
                args=[
                    name_val,
                    qual_val,
                    module_val,
                    arg_names_tuple,
                    posonly_val,
                    kwonly_tuple,
                    vararg_val,
                    varkw_val,
                    defaults_val,
                    kwdefaults_val,
                    doc_val,
                ],
                result=metadata_tuple,
            )
        )
        init_metadata = self._emit_runtime_function(
            "molt_function_init_metadata_packed", 4
        )
        init_res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[init_metadata, func_val, metadata_tuple, code_val, bind_kind_val],
                result=init_res,
            )
        )

        def set_attr(attr: str, value: MoltValue) -> None:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, attr, value],
                    result=MoltValue("none"),
                )
            )

        if is_coroutine:
            coro_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=coro_val))
            set_attr("__molt_is_coroutine__", coro_val)
        if is_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_generator__", gen_val)
        if is_async_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_async_generator__", gen_val)

    def _build_gpu_kernel_descriptor_json(
        self, *, func_symbol: str, func_name: str
    ) -> str:
        func_info = self.funcs_map[func_symbol]
        payload = {
            "schema_version": 1,
            "kind": "molt_gpu_kernel",
            "symbol": func_symbol,
            "name": func_name,
            "params": list(func_info["params"]),
            "ops": self.map_ops_to_json(func_info["ops"], function_name=func_name),
        }
        return json.dumps(payload, sort_keys=True, separators=(",", ":"))

    @staticmethod
    def _split_function_args(
        args: ast.arguments,
    ) -> tuple[list[ast.arg], list[ast.arg], list[ast.arg], str | None, str | None]:
        posonly = list(args.posonlyargs)
        pos_or_kw = list(args.args)
        kwonly = list(args.kwonlyargs)
        vararg = args.vararg.arg if args.vararg else None
        varkw = args.kwarg.arg if args.kwarg else None
        return posonly, pos_or_kw, kwonly, vararg, varkw

    @classmethod
    def _function_param_names(cls, args: ast.arguments) -> list[str]:
        posonly, pos_or_kw, kwonly, vararg, varkw = cls._split_function_args(args)
        names = [arg.arg for arg in posonly + pos_or_kw]
        if vararg is not None:
            names.append(vararg)
        names.extend(arg.arg for arg in kwonly)
        if varkw is not None:
            names.append(varkw)
        return names

    def _emit_module_attr_get(
        self, name: str, *, effect_proof: str | None = None
    ) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(
                self.module_name, effect_proof=effect_proof
            )
        # Propagate the last-known type hint for this module attribute.
        # When a module-scope variable was assigned from a typed expression
        # (e.g., count = 0 → int), the MODULE_GET_ATTR result inherits
        # that type so downstream _should_fast_int checks can fire.
        attr_hint = self._module_attr_type_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=attr_hint)
        metadata = {"effect_proof": effect_proof} if effect_proof else None
        self.emit(
            MoltOp(
                kind="MODULE_GET_ATTR",
                args=[module_val, name_val],
                result=res,
                metadata=metadata,
            )
        )
        return res

    def _emit_class_ref(self, class_name: str) -> MoltValue:
        static_ref = self._current_module_static_class_ref(class_name)
        if static_ref is not None:
            return static_ref
        class_info = self.classes.get(class_name)
        module_name = class_info.get("module") if class_info else None
        if module_name and module_name != self.module_name:
            return self._emit_module_attr_get_on(module_name, class_name)
        return self._emit_module_attr_get(class_name)

    def _current_module_static_class_ref(self, class_name: str) -> MoltValue | None:
        if self.current_func_name != "molt_main":
            return None
        if self.module_globals_dict_escaped:
            return None
        if class_name in self.module_global_mutations:
            return None
        if class_name in self.class_definition_pending:
            return None
        class_info = self.classes.get(class_name)
        if class_info is None:
            return None
        if class_info.get("module") != self.module_name:
            return None
        if class_info.get("decorated"):
            return None
        if not self._class_layout_stable(class_name):
            return None
        static_name = class_info.get("class_value_name")
        if not static_name:
            return None
        current = self.globals.get(class_name)
        if current is None or current.name != static_name:
            return None
        return current

    def _emit_global_get(self, name: str) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="MODULE_GET_GLOBAL", args=[module_val, name_val], result=res)
        )
        return res

    def _name_resolves_to_builtin(self, name: str) -> bool:
        """True if `name` names a builtin type/function/exception.

        Used to keep `del`/`except`-target read routing CPython-faithful for
        names that shadow a builtin: once the module binding is removed, a bare
        read must fall back to the builtin (which the regular `visit_Name`
        resolution materialises statically), not raise NameError.
        """
        return (
            name in BUILTIN_TYPE_TAGS
            or name in BUILTIN_FUNC_SPECS
            or name in BUILTIN_EXCEPTION_NAMES
        )

    def _emit_globals_dict(self) -> MoltValue:
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        dict_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__dict__"], result=dict_name))
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, dict_name], result=res)
        )
        return res

    def _emit_globals_builtin_obj(self) -> MoltValue:
        if self.globals_builtin_val is not None:
            return self.globals_builtin_val
        func_symbol = self._function_symbol(_MOLT_GLOBALS_BUILTIN)
        func_val = MoltValue(self.next_var(), type_hint=f"Func:{func_symbol}")
        self.emit(MoltOp(kind="FUNC_NEW", args=[func_symbol, 0], result=func_val))
        self._emit_function_metadata(
            func_val,
            name="globals",
            qualname="globals",
            trace_lineno=None,
            posonly_params=[],
            pos_or_kw_params=[],
            kwonly_params=[],
            vararg=None,
            varkw=None,
            default_exprs=[],
            kw_default_exprs=[],
            docstring="Return the current module globals.",
            module_override="builtins",
        )
        set_builtin = self._emit_builtin_function("_molt_function_set_builtin")
        builtin_res = MoltValue(self.next_var(), type_hint="None")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[set_builtin, func_val], result=builtin_res)
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        self.start_function(
            func_symbol, params=[], type_facts_name=_MOLT_GLOBALS_BUILTIN
        )
        res = self._emit_globals_dict()
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        self.globals_builtin_val = func_val
        return func_val

    def _ensure_globals_builtin(self) -> None:
        if (
            self.globals_builtin_emitted
            or self.current_func_name != "molt_main"
            or self.module_obj is None
        ):
            return
        func_val = self._emit_globals_builtin_obj()
        self._emit_module_attr_set(_MOLT_GLOBALS_BUILTIN, func_val)
        self.globals_builtin_emitted = True

    def _emit_globals_builtin_ref(self) -> MoltValue:
        if not self.globals_builtin_emitted:
            self._ensure_globals_builtin()
        return self._emit_module_attr_get(_MOLT_GLOBALS_BUILTIN)

    def _init_locals_cache(self) -> None:
        if self.locals_cache_val is not None:
            return
        cache_val = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=cache_val))
        self.locals_cache_val = cache_val

    def _init_locals_cache_and_pin(self) -> None:
        """Allocate the locals cache dict and pin it on the frame stack.

        This should be called from function visitors when the function body
        contains a ``locals()`` call.  It combines ``_init_locals_cache()``
        with the ``FRAME_LOCALS_SET`` emission that was previously done
        unconditionally in ``start_function()``.
        """
        self._init_locals_cache()
        cache_val = self.locals_cache_val
        if cache_val is not None:
            self.emit(
                MoltOp(
                    kind="FRAME_LOCALS_SET",
                    args=[cache_val],
                    result=MoltValue("none"),
                )
            )

    @staticmethod
    def _normalize_allowlist_module(module_name: str | None) -> str | None:
        if not module_name or module_name == "molt.stdlib":
            return None
        if module_name.startswith("molt.stdlib."):
            return module_name[len("molt.stdlib.") :]
        return module_name

    @staticmethod
    def _spec_parent(spec_name: str, is_package: bool) -> str:
        if is_package:
            return spec_name
        if "." in spec_name:
            return spec_name.rsplit(".", 1)[0]
        return ""

    def _relative_import_package(self) -> str:
        if self.module_package_override_set:
            return self.module_package_override or ""
        spec_is_package = self.module_is_package
        spec_name = None
        if self.module_spec_override_set and self.module_spec_override:
            spec_name = self.module_spec_override
            if self.module_spec_override_is_package is not None:
                spec_is_package = self.module_spec_override_is_package
        if spec_name is None:
            spec_name = self.module_spec_name or self.module_name or ""
        return self._spec_parent(spec_name, spec_is_package)

    def _resolve_relative_import(
        self, module: str | None, level: int
    ) -> tuple[str | None, str | None]:
        if level <= 0:
            return module, None
        package = self._relative_import_package()
        if not package:
            return None, "no_parent"
        parts = package.split(".")
        if level > len(parts):
            return None, "beyond_top"
        base_parts = parts[: len(parts) - (level - 1)]
        base_name = ".".join(base_parts)
        if module:
            if base_name:
                return f"{base_name}.{module}", None
            return module, None
        return base_name or None, None

    def _emit_relative_import_error(self, kind: str | None) -> None:
        if kind == "beyond_top":
            message = "attempted relative import beyond top-level package"
        else:
            message = "attempted relative import with no known parent package"
        exc_val = self._emit_exception_new("ImportError", message)
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))

    def _should_track_module_overrides(self) -> bool:
        # Names assigned inside a class body lowered as a block (P0 #50) are
        # class-namespace members, never module overrides — even though the
        # outermost class may live at module scope with ``control_flow_depth``
        # 0.  The class-ns stack being non-empty means we are emitting such a
        # body; suppress module-override tracking for it.
        if self._class_ns_stack:
            return False
        return self.current_func_name == "molt_main" and self.control_flow_depth == 0

    @staticmethod
    def _is_modulespec_ctor(node: ast.AST) -> bool:
        if isinstance(node, ast.Name):
            return node.id == "ModuleSpec"
        if isinstance(node, ast.Attribute):
            return node.attr == "ModuleSpec"
        return False

    def _parse_modulespec_override(
        self, value: ast.AST
    ) -> tuple[str, bool | None] | None:
        if not isinstance(value, ast.Call):
            return None
        if not self._is_modulespec_ctor(value.func):
            return None
        spec_name = None
        if value.args:
            first = value.args[0]
            if isinstance(first, ast.Constant) and isinstance(first.value, str):
                spec_name = first.value
        for kw in value.keywords:
            if (
                kw.arg == "name"
                and spec_name is None
                and isinstance(kw.value, ast.Constant)
                and isinstance(kw.value.value, str)
            ):
                spec_name = kw.value.value
        if spec_name is None:
            return None
        is_package = None
        if len(value.args) >= 4:
            arg = value.args[3]
            if isinstance(arg, ast.Constant) and isinstance(arg.value, bool):
                is_package = arg.value
        for kw in value.keywords:
            if (
                kw.arg == "is_package"
                and isinstance(kw.value, ast.Constant)
                and isinstance(kw.value.value, bool)
            ):
                is_package = kw.value.value
        return spec_name, is_package

    def _record_module_override(self, target: ast.AST, value: ast.AST) -> None:
        if not isinstance(target, ast.Name):
            return
        if target.id == "__package__":
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                self.module_package_override_set = True
                self.module_package_override = value.value
            elif isinstance(value, ast.Constant) and value.value is None:
                self.module_package_override_set = False
                self.module_package_override = None
            else:
                self.module_package_override_set = False
                self.module_package_override = None
            return
        if target.id == "__spec__":
            if isinstance(value, ast.Constant) and value.value is None:
                self.module_spec_override_set = False
                self.module_spec_override = None
                self.module_spec_override_is_package = None
                return
            parsed = self._parse_modulespec_override(value)
            if parsed is None:
                return
            spec_name, is_package = parsed
            self.module_spec_override_set = True
            self.module_spec_override = spec_name
            self.module_spec_override_is_package = None
            if is_package is not None:
                self.module_spec_override_is_package = is_package

    def _maybe_record_module_overrides(
        self, targets: Sequence[ast.AST], value: ast.AST
    ) -> None:
        if not self._should_track_module_overrides():
            return
        for target in targets:
            self._record_module_override(target, value)

    def _is_known_project_module(self, module_name: str | None) -> bool:
        """Return True only when *module_name* was discovered in the graph.

        Project/external module authority is exact: a discovered package does
        not authorize arbitrary children. Child modules must be present in the
        module graph with their own exact path/case proof.
        """
        if not module_name or not self.known_modules:
            return False
        return module_name in self.known_modules

    def _is_linkable_module_function_symbol(self, module_name: str | None) -> bool:
        """Return whether a direct ``module__function`` symbol can be emitted.

        The frontend may know defaults/signatures for many stdlib functions from
        source indexes.  That metadata is not link authority.  Once the build
        provides a closed module graph, cross-module direct calls are legal only
        to modules in that graph; absent modules must go through import/bound
        call lowering so missing optional paths do not leak undefined symbols
        into shared stdlib partitions.
        """
        if not module_name:
            return False
        normalized = self._normalize_allowlist_module(module_name) or module_name
        if normalized == self.module_name:
            return True
        if not self.known_modules:
            return True
        return normalized in self.known_modules

    def _imported_module_binding_target(self, binding_name: str) -> str | None:
        if self._local_name_shadows_import_binding(binding_name):
            return None
        module_name = self.imported_modules.get(binding_name)
        if module_name is None:
            module_name = self.global_imported_modules.get(binding_name)
        return module_name

    def _record_imported_module_attr_mutation(self, target: ast.Attribute) -> None:
        if not isinstance(target.value, ast.Name):
            return
        module_name = self._imported_module_binding_target(target.value.id)
        if module_name is None:
            return
        mutation = (module_name, target.attr)
        self.imported_module_attr_mutations.add(mutation)
        self.global_imported_module_attr_mutations.add(mutation)

    def _imported_module_attr_is_stable(self, module_name: str, attr: str) -> bool:
        mutation = (module_name, attr)
        return (
            mutation not in self.imported_module_attr_mutations
            and mutation not in self.global_imported_module_attr_mutations
        )

    def _emit_module_attr_set_runtime(self, name: str, value: MoltValue) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            module_val = self.module_obj
        else:
            module_val = self._get_or_emit_module_cache(self.module_name)
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _should_attempt_runtime_module_import(self, module_name: str) -> bool:
        if module_name in self.known_modules:
            return True
        if module_name in self.stdlib_allowlist:
            return True
        normalized_name = self._normalize_allowlist_module(module_name)
        if normalized_name and (
            normalized_name in self.stdlib_allowlist
            or normalized_name in self.known_modules
        ):
            return True
        if "." not in module_name:
            return False
        top_level = module_name.split(".", 1)[0]
        if top_level in self.stdlib_allowlist:
            return True
        normalized_top = self._normalize_allowlist_module(top_level)
        return bool(normalized_top and normalized_top in self.stdlib_allowlist)

    def _emit_import_transaction(
        self,
        module_name: str,
        *,
        fromlist_names: Sequence[str],
        level: int = 0,
        globals_val: MoltValue | None = None,
    ) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))

        if globals_val is None:
            globals_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=globals_val))
        locals_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=locals_val))

        fromlist_items: list[MoltValue] = []
        for name in fromlist_names:
            item_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=item_val))
            fromlist_items.append(item_val)
        fromlist_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=fromlist_items, result=fromlist_val))

        level_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[level], result=level_val))

        transaction_func = self._emit_intrinsic_function(
            "molt_importlib_import_transaction"
        )
        imported_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[
                    transaction_func,
                    name_val,
                    globals_val,
                    locals_val,
                    fromlist_val,
                    level_val,
                ],
                result=imported_val,
            )
        )
        return imported_val

    def _emit_importlib_import_module_leaf(self, module_name: str) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))
        package_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=package_val))
        import_module_func = self._emit_intrinsic_function(
            "molt_importlib_import_module"
        )
        imported_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[import_module_func, name_val, package_val],
                result=imported_val,
            )
        )
        return imported_val

    def _emit_source_import_transaction(
        self,
        module_name: str,
        *,
        fromlist_names: Sequence[str],
        level: int = 0,
    ) -> MoltValue:
        return self._emit_import_transaction(
            module_name,
            fromlist_names=fromlist_names,
            level=level,
            globals_val=self._emit_globals_dict(),
        )

    def _emit_source_import_alias_binding(self, module_name: str) -> MoltValue:
        bound_val = self._emit_source_import_transaction(
            module_name,
            fromlist_names=(),
            level=0,
        )
        for attr_name in module_name.split(".")[1:]:
            bound_val = self._emit_module_import_from_value(bound_val, attr_name)
        return bound_val

    def _emit_module_load(self, module_name: str) -> MoltValue:
        # NOTE: Earlier versions cached loaded_val in _module_cache_values to
        # avoid redundant MODULE_CACHE_GET + conditional-init sequences.  However,
        # the WASM state-machine backend (used for module init functions with
        # jumps/labels) can split the code into states where the cached local's
        # assignment lives in a state that an exception-redirect path skips.
        # When the later state that uses the cached local runs, the local is
        # still 0 (its WASM default), causing "module attribute access expects
        # module" errors in linked WASM artifacts.  Re-emitting the full
        # load sequence each time ensures the local is populated in the state
        # that actually uses it.
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))
        uses_runtime_import = module_name in self.known_modules or (
            self._should_attempt_runtime_module_import(module_name)
        )
        if uses_runtime_import:
            imported_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(
                MoltOp(kind="MODULE_IMPORT", args=[name_val], result=imported_val)
            )
            return imported_val
        module_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(MoltOp(kind="MODULE_CACHE_GET", args=[name_val], result=module_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[module_val, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        if self.known_modules:
            exc_val = self._emit_exception_new(
                "ModuleNotFoundError", f"No module named '{module_name}'"
            )
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        loaded_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(MoltOp(kind="MODULE_CACHE_GET", args=[name_val], result=loaded_val))
        self._emit_import_guard(loaded_val, module_name)
        return loaded_val

    def _lookup_func_defaults(
        self, module_name: str | None, func_id: str
    ) -> dict[str, Any] | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        module_defaults = self.known_func_defaults.get(module_name)
        if module_defaults is None and module_name == self.module_name:
            module_defaults = self.module_func_defaults
        if module_defaults is None:
            return None
        return module_defaults.get(func_id)

    @staticmethod
    def _normalize_func_kind(kind: object) -> FunctionKind | None:
        return normalize_function_kind(kind)

    def _lookup_func_kind(self, module_name: str | None, func_id: str) -> str | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        module_kinds = self.known_func_kinds.get(module_name)
        if module_kinds is None and module_name == self.module_name:
            module_kinds = self.module_declared_funcs
        if module_kinds is None:
            return None
        return self._normalize_func_kind(module_kinds.get(func_id))

    def _imported_attr_name(self, bind_name: str) -> str:
        return self.imported_attr_names.get(
            bind_name, self.global_imported_attr_names.get(bind_name, bind_name)
        )

    def _known_function_symbol_target(self, func_symbol: str) -> tuple[str, str] | None:
        candidate_modules = set(self.known_func_defaults) | set(self.known_func_kinds)
        for raw_module_name in sorted(candidate_modules):
            module_name = (
                self._normalize_allowlist_module(raw_module_name) or raw_module_name
            )
            symbol_prefix = f"{self._sanitize_module_name(module_name)}__"
            if not func_symbol.startswith(symbol_prefix):
                continue
            func_id = func_symbol[len(symbol_prefix) :]
            if (
                self._lookup_func_defaults(module_name, func_id) is not None
                or self._lookup_func_kind(module_name, func_id) is not None
            ):
                return module_name, func_id
        return None

    def _known_module_function_type_hint(
        self, module_name: str | None, func_id: str
    ) -> str | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        info = self._lookup_func_defaults(module_name, func_id)
        info_kind = self._normalize_func_kind(info.get("kind")) if info else None
        kind = self._lookup_func_kind(module_name, func_id) or info_kind
        if info is None and kind is None:
            return None
        if info is not None and info.get("has_decorators"):
            return None
        kind = kind or FunctionKind.SYNC
        func_symbol = f"{self._sanitize_module_name(module_name)}__{func_id}"
        if kind == FunctionKind.SYNC:
            return f"Func:{func_symbol}"
        total_params = info.get("params") if info is not None else None
        param_count = total_params if isinstance(total_params, int) else 0
        frame_plan = stateful_function_frame_plan(
            kind=kind,
            poll_symbol=f"{func_symbol}_poll",
            param_count=param_count,
            has_closure=False,
            gen_control_size=GEN_CONTROL_SIZE,
        )
        closure_size = self._task_closure_size(
            frame_plan.payload_slots,
            include_gen_control=frame_plan.include_gen_control,
        )
        return frame_plan.function_type_hint(closure_size)

    def _emit_module_attr_get_on(self, module_name: str, name: str) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, name_val], result=res)
        )
        return res

    def _emit_module_attr_get_default_on(
        self, module_name: str, name: str, default_val: MoltValue
    ) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_NAME_DEFAULT",
                args=[module_val, name_val, default_val],
                result=res,
            )
        )
        return res

    def _emit_class_method_func(
        self, class_obj: MoltValue, method_name: str
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[class_obj, method_name],
                result=res,
            )
        )
        return res

    def _emit_const_value(self, value: object) -> MoltValue:
        if value is None:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if value is Ellipsis:
            res = MoltValue(self.next_var(), type_hint="ellipsis")
            self.emit(MoltOp(kind="CONST_ELLIPSIS", args=[], result=res))
            return res
        if value is NotImplemented:
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="CONST_NOT_IMPLEMENTED", args=[], result=res))
            return res
        if isinstance(value, bool):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[value], result=res))
            return res
        if isinstance(value, int):
            if _INLINE_INT_MIN <= value <= _INLINE_INT_MAX:
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[value], result=res))
            else:
                res = MoltValue(self.next_var(), type_hint="bigint")
                self.emit(MoltOp(kind="CONST_BIGINT", args=[str(value)], result=res))
            return res
        if isinstance(value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value], result=res))
            return res
        if isinstance(value, complex):
            real = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value.real], result=real))
            imag = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value.imag], result=imag))
            has_imag = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_imag))
            res = MoltValue(self.next_var(), type_hint="complex")
            self.emit(
                MoltOp(kind="COMPLEX_FROM_OBJ", args=[real, imag, has_imag], result=res)
            )
            return res
        if isinstance(value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[value], result=res))
            return res
        if isinstance(value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[value], result=res))
            return res
        raise NotImplementedError(f"Unsupported default literal: {value!r}")

    def _emit_module_load_with_parents(self, module_name: str) -> MoltValue:
        parts = module_name.split(".")
        parent_val: MoltValue | None = None
        current_val: MoltValue | None = None
        for idx, part in enumerate(parts):
            name = ".".join(parts[: idx + 1])
            current_val = self._emit_module_load(name)
            if parent_val is not None:
                self._emit_module_attr_set_on(parent_val, part, current_val)
            parent_val = current_val
        if current_val is None:
            raise NotImplementedError("Invalid module name")
        return current_val

    def _emit_module_import_from_value(
        self, module_val: MoltValue, attr_name: str
    ) -> MoltValue:
        attr_val = MoltValue(self.next_var(), type_hint="Any")
        attr_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[attr_name], result=attr_name_val))
        # `from MODULE import name` has CPython IMPORT_FROM semantics: a
        # missing attribute raises ImportError ("cannot import name ...") after
        # a sys.modules submodule fallback, NOT the AttributeError that a plain
        # `MODULE.name` (MODULE_GET_ATTR) read raises.
        self.emit(
            MoltOp(
                kind="MODULE_IMPORT_FROM",
                args=[module_val, attr_name_val],
                result=attr_val,
            )
        )
        return attr_val

    def _emit_import_guard(self, module_val: MoltValue, module_name: str) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[module_val, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        exc_val = self._emit_exception_new(
            "ImportError", f"No module named '{module_name}'"
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        # On the native backend, RAISE sets a pending exception but does not
        # alter control flow — execution falls through to END_IF and continues.
        # Without an explicit exit here, the caller proceeds to use the None
        # module_val in MODULE_GET_ATTR / MODULE_SET_ATTR, triggering a
        # "module attribute access expects module" TypeError that masks the
        # real ImportError.  Emit _emit_raise_exit() to jump to the nearest
        # exception handler (or return) so the ImportError propagates cleanly.
        self._emit_raise_exit()
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_exception_class(self, name: str) -> MoltValue:
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=kind_val))
        class_val = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="EXCEPTION_CLASS", args=[kind_val], result=class_val))
        return class_val

    def _emit_exception_new_from_args(
        self, kind: str, args: list[MoltValue]
    ) -> MoltValue:
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        if kind_tag := BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS.get(kind):
            if not args:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_NEW_BUILTIN_EMPTY",
                        args=[],
                        result=exc_val,
                        metadata={"exception_name": kind, "exception_tag": kind_tag},
                    )
                )
                return exc_val
            if len(args) == 1:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_NEW_BUILTIN_ONE",
                        args=[args[0]],
                        result=exc_val,
                        metadata={"exception_name": kind, "exception_tag": kind_tag},
                    )
                )
                return exc_val
            args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_NEW_BUILTIN",
                    args=[args_val],
                    result=exc_val,
                    metadata={"exception_name": kind, "exception_tag": kind_tag},
                )
            )
            return exc_val
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[kind], result=kind_val))
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, args_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_exception_new(self, kind: str, message: str | MoltValue) -> MoltValue:
        args: list[MoltValue] = []
        if isinstance(message, MoltValue):
            if message.type_hint == "str":
                args = [message]
            else:
                args = [self._emit_str_from_obj(message)]
        elif message:
            msg_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
            args = [msg_val]
        return self._emit_exception_new_from_args(kind, args)

    def _emit_missing_value(self) -> MoltValue:
        missing = MoltValue(self.next_var(), type_hint="missing")
        self.emit(MoltOp(kind="MISSING", args=[], result=missing))
        return missing

    def _emit_unbound_local_guard(self, value: MoltValue, name: str) -> None:
        missing = self._emit_missing_value()
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        if self.current_func_name == "molt_main":
            msg = f"name '{name}' is not defined"
            err_val = self._emit_exception_new("NameError", msg)
        else:
            msg = (
                "cannot access local variable "
                f"'{name}' where it is not associated with a value"
            )
            err_val = self._emit_exception_new("UnboundLocalError", msg)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_unbound_free_guard(self, value: MoltValue, name: str) -> None:
        missing = self._emit_missing_value()
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        msg = (
            "cannot access free variable "
            f"'{name}' where it is not associated with a value in enclosing scope"
        )
        err_val = self._emit_exception_new("NameError", msg)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_type_error(self, message: str | MoltValue) -> None:
        err_val = self._emit_exception_new("TypeError", message)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))

    def _emit_exception_match(
        self, handler: ast.ExceptHandler, exc_val: MoltValue
    ) -> MoltValue:
        if handler.type is None:
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[1], result=res))
            return res
        if (
            isinstance(handler.type, ast.Name)
            and (kind_tag := BUILTIN_EXCEPTION_CONSTRUCTOR_TAGS.get(handler.type.id))
            is not None
        ):
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="EXCEPTION_MATCH_BUILTIN",
                    args=[exc_val],
                    result=res,
                    metadata={
                        "exception_name": handler.type.id,
                        "exception_tag": kind_tag,
                    },
                )
            )
            return res
        # Evaluate the handler expression with the pending exception temporarily
        # cleared. Attribute-based handlers (e.g. `except mod.Error`) otherwise
        # fail to resolve correctly while an exception is active.
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        class_val = self.visit(handler.type)
        if class_val is None:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._bridge_fallback(
                handler,
                "except (unsupported handler)",
                alternative="use a lowered exception name or tuple",
                detail="handler expression could not be lowered",
            )
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[0], result=res))
            return res
        # Keep the pending exception cleared while matching. `isinstance`
        # only needs the explicit exception object and resolved class value;
        # restoring the global "last exception" here reintroduces stale
        # exception state into the handler CFG and is not semantically needed
        # for the match itself.
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="ISINSTANCE", args=[exc_val, class_val], result=res))
        return res

    def _apply_explicit_hint(self, name: str, value: MoltValue) -> None:
        hint = self.explicit_type_hints.get(name)
        if hint is None:
            return
        if self.type_hint_policy == "check":
            self._emit_guard_type(value, hint)
            self._apply_hint_to_value(name, value, hint)
            return
        if self.type_hint_policy == "trust" or self.stdlib_hint_trust:
            self._apply_hint_to_value(name, value, hint)

    def _emit_builtin_function(self, func_id: str) -> MoltValue:
        spec = BUILTIN_FUNC_SPECS[func_id]
        arity = len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
        if spec.vararg is not None:
            arity += 1
        func_val = MoltValue(self.next_var(), type_hint="function")
        self.emit(
            MoltOp(
                kind="BUILTIN_FUNC",
                args=[spec.runtime, arity],
                result=func_val,
            )
        )
        self._emit_function_metadata(
            func_val,
            name=func_id,
            qualname=func_id,
            posonly_params=list(spec.params),
            pos_or_kw_params=list(spec.pos_or_kw_params),
            kwonly_params=list(spec.kwonly_params),
            vararg=spec.vararg,
            varkw=None,
            default_exprs=list(spec.defaults),
            kw_default_exprs=list(spec.kw_defaults),
            docstring=None,
            bind_kind=MOLT_BIND_KIND_OPEN if func_id == "open" else None,
            module_override="builtins",
            emit_code=False,
        )
        return func_val

    def _emit_intrinsic_function(self, runtime_name: str) -> MoltValue:
        arity = _intrinsic_arity_exact(runtime_name)
        if arity is None:
            raise KeyError(runtime_name)
        return self._emit_runtime_function_with_defaults(
            _canonical_intrinsic_runtime_name(runtime_name),
            arity,
            _intrinsic_defaults_exact(runtime_name),
        )

    def _emit_optional_intrinsic_lookup_value(self, runtime_name: str) -> MoltValue:
        loader = self._emit_runtime_function("molt_load_intrinsic_runtime", 2)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[runtime_name], result=name_val))
        namespace_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=namespace_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="CALL_FUNC", args=[loader, name_val, namespace_val], result=res)
        )
        return res

    def _emit_runtime_function(self, runtime_name: str, arity: int) -> MoltValue:
        func_val = MoltValue(self.next_var(), type_hint="function")
        self.emit(
            MoltOp(
                kind="BUILTIN_FUNC",
                args=[runtime_name, arity],
                result=func_val,
            )
        )
        return func_val

    def _emit_runtime_function_with_none_defaults(
        self, runtime_name: str, arity: int, *, default_count: int
    ) -> MoltValue:
        return self._emit_runtime_function_with_defaults(
            runtime_name, arity, (None,) * max(0, default_count)
        )

    def _emit_runtime_function_with_defaults(
        self, runtime_name: str, arity: int, defaults: Sequence[object]
    ) -> MoltValue:
        func_val = self._emit_runtime_function(runtime_name, arity)
        if not defaults:
            return func_val
        default_vals = [self._emit_const_value(value) for value in defaults]
        defaults_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=default_vals, result=defaults_tuple))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[func_val, "__defaults__", defaults_tuple],
                result=MoltValue("none"),
            )
        )
        return func_val

    def _intrinsic_handle_class_spec_for_value(
        self, value: MoltValue | None
    ) -> IntrinsicHandleClassConstructorSpec | None:
        if value is None:
            return None
        return INTRINSIC_HANDLE_CLASS_CONSTRUCTORS_BY_TYPE.get(value.type_hint)

    def _emit_intrinsic_handle_class_call(
        self,
        obj: MoltValue,
        spec: IntrinsicHandleClassConstructorSpec,
        intrinsic_name: str,
        args: list[MoltValue],
        *,
        result_hint: str,
    ) -> MoltValue:
        handle = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[obj, spec.handle_attr],
                result=handle,
            )
        )
        intrinsic_func = self._emit_intrinsic_function(intrinsic_name)
        res = MoltValue(self.next_var(), type_hint=result_hint)
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[intrinsic_func, handle] + args,
                result=res,
            )
        )
        return res

    @staticmethod
    def _is_intrinsics_module_name(module_name: str | None) -> bool:
        if module_name is None:
            return False
        return module_name == "_intrinsics" or module_name.endswith("._intrinsics")

    @staticmethod
    def _is_safe_intrinsic_namespace_expr(expr: ast.expr) -> bool:
        if isinstance(expr, ast.Constant) and expr.value is None:
            return True
        if isinstance(expr, ast.Name):
            return True
        return (
            isinstance(expr, ast.Call)
            and isinstance(expr.func, ast.Name)
            and expr.func.id == "globals"
            and not expr.args
            and not expr.keywords
        )

    def _maybe_record_local_intrinsic_wrapper(self, node: ast.FunctionDef) -> None:
        if (
            node.decorator_list
            or node.args.posonlyargs
            or len(node.args.args) not in {1, 2}
            or node.args.vararg is not None
            or node.args.kwonlyargs
            or node.args.kw_defaults
            or node.args.kwarg is not None
        ):
            return
        if len(node.args.args) == 1:
            if node.args.defaults:
                return
        else:
            if len(node.args.defaults) != 1:
                return
            default = node.args.defaults[0]
            if not (isinstance(default, ast.Constant) and default.value is None):
                return
        import_alias = None
        if len(node.body) == 1 and isinstance(node.body[0], ast.Return):
            ret_stmt = node.body[0]
        elif (
            len(node.body) == 2
            and isinstance(node.body[0], ast.ImportFrom)
            and isinstance(node.body[1], ast.Return)
        ):
            import_stmt = node.body[0]
            if not self._is_intrinsics_module_name(import_stmt.module):
                return
            if len(import_stmt.names) != 1:
                return
            alias = import_stmt.names[0]
            if alias.name != "require_intrinsic":
                return
            import_alias = alias.asname or alias.name
            ret_stmt = node.body[1]
        else:
            return
        ret = ret_stmt.value
        if (
            ret is None
            or not isinstance(ret, ast.Call)
            or not isinstance(ret.func, ast.Name)
        ):
            return
        if import_alias is not None:
            if ret.func.id != import_alias:
                return
        else:
            if ret.func.id not in {"require_intrinsic", "_require_intrinsic"}:
                return
            imported_from = self.imported_names.get(ret.func.id)
            if not self._is_intrinsics_module_name(imported_from):
                return
        param_name = node.args.args[0].arg
        namespace_param = node.args.args[1].arg if len(node.args.args) == 2 else None
        if not ret.args or not isinstance(ret.args[0], ast.Name):
            return
        if ret.args[0].id != param_name or len(ret.args) > 2:
            return
        if len(ret.args) == 2:
            namespace_expr = ret.args[1]
            if namespace_param is not None:
                if not (
                    isinstance(namespace_expr, ast.Name)
                    and namespace_expr.id == namespace_param
                ):
                    return
            elif not self._is_safe_intrinsic_namespace_expr(namespace_expr):
                return
        if any(kw.arg is None for kw in ret.keywords):
            return
        for kw in ret.keywords:
            if kw.arg == "name":
                if not isinstance(kw.value, ast.Name) or kw.value.id != param_name:
                    return
            elif kw.arg == "namespace":
                if namespace_param is not None:
                    if not (
                        isinstance(kw.value, ast.Name)
                        and kw.value.id == namespace_param
                    ):
                        return
                elif not self._is_safe_intrinsic_namespace_expr(kw.value):
                    return
            else:
                return
        self.local_intrinsic_wrappers.add(node.name)

    def _emit_builtin_type_value(self, type_name: str) -> MoltValue:
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[BUILTIN_TYPE_TAGS[type_name]], result=tag_val)
        )
        res = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=res))
        return res

    def _iterable_is_indexable(self, iterable: MoltValue | None) -> bool:
        if iterable is None:
            return False
        return iterable.type_hint in {
            "list",
            "tuple",
            "range",
            "memoryview",
        }

    def _iterable_is_indexable_for_loop(self, iterable: MoltValue | None) -> bool:
        if iterable is None:
            return False
        if not self._iterable_is_indexable(iterable):
            return False
        # List iteration must observe mutations (e.g., append during iteration).
        return iterable.type_hint != "list"

    def _active_exception_value(self, exc: ActiveException) -> MoltValue:
        if self.is_async() and exc.slot is not None:
            return self._reload_async_value(exc.slot, exc.value.type_hint)
        return exc.value

    def _emit_exception_handler_exit_cleanup(
        self, exc: ActiveException | None = None
    ) -> None:
        if exc is not None:
            handlers = [exc] if exc.is_handler else []
        else:
            handlers = [entry for entry in self.active_exceptions if entry.is_handler]
        if not handlers:
            return
        cleared_ctx = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_ctx))
        self.emit(
            MoltOp(
                kind="EXCEPTION_CONTEXT_SET",
                args=[cleared_ctx],
                result=MoltValue("none"),
            )
        )
        self._emit_active_handler_name_deletes(handlers)

    def _emit_active_handler_name_deletes(
        self, handlers: list["ActiveException"]
    ) -> None:
        """Delete the `except ... as NAME` target bindings for `handlers`.

        CPython lowers `except E as e:` to an implicit ``finally: del e`` that
        runs on *every* exit edge of the handler — normal fall-through, return,
        break/continue, and an exception escaping the handler body.  The normal
        and return/break paths route through `_emit_exception_handler_exit_cleanup`
        (which also clears the handling context); a `raise` inside the handler
        must delete the same names without clearing the context it just captured
        into the new exception.  Deleting the name only drops the binding — the
        exception object itself stays alive via any reference already taken
        (e.g. `raise X from e`).
        """
        for entry in reversed(handlers):
            if entry.handler_name:
                self._emit_delete_name(entry.handler_name, allow_missing=True)

    def _emit_escaping_handler_name_deletes(self) -> None:
        """Delete `except ... as NAME` targets for handlers a `raise` escapes.

        A `raise` only leaves an active handler — and so only runs that
        handler's implicit ``del NAME`` — when it is not caught by a `try`
        opened *after* the handler body began.  `handler_try_depth` records the
        live `try` nesting at the handler's entry; a handler is escaped iff the
        current live depth is no deeper than that recorded value (no inner
        `try` is protecting the raise).  Handlers protected by a nested `try`
        (whose own cleanup deletes their name when control actually leaves them)
        are left untouched.
        """
        if not self.active_exceptions:
            return
        live_depth = len(self.try_end_labels)
        escaped = [
            entry
            for entry in self.active_exceptions
            if entry.is_handler and live_depth <= entry.handler_try_depth
        ]
        self._emit_active_handler_name_deletes(escaped)

    def _emit_expr_list(self, exprs: list[ast.expr]) -> list[MoltValue]:
        if not exprs:
            return []
        if not self.is_async():
            values: list[MoltValue] = []
            for expr in exprs:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported expression")
                values.append(val)
            return values
        yield_flags = [self._expr_may_yield(expr) for expr in exprs]
        if not any(yield_flags):
            values = []
            for expr in exprs:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported expression")
                values.append(val)
            return values
        values = []
        spills: list[tuple[int, int, str]] = []
        for idx, expr in enumerate(exprs):
            val = self.visit(expr)
            if val is None:
                raise NotImplementedError("Unsupported expression")
            values.append(val)
            if any(yield_flags[idx + 1 :]):
                slot = self._spill_async_value(
                    val, f"__expr_spill_{len(self.async_locals)}"
                )
                spills.append((idx, slot, val.type_hint))
        for idx, slot, hint in spills:
            values[idx] = self._reload_async_value(slot, hint)
        return values

    def _range_start_expr(self, node: ast.expr) -> ast.expr | None:
        if isinstance(node, ast.Constant):
            if isinstance(node.value, int) and node.value > 0:
                return node
            return None
        if isinstance(node, ast.Name):
            return node
        return None

    def _subscript_matches(self, node: ast.expr, seq_name: str, idx_name: str) -> bool:
        if not isinstance(node, ast.Subscript):
            return False
        if not isinstance(node.value, ast.Name) or node.value.id != seq_name:
            return False
        if isinstance(node.slice, ast.Name) and node.slice.id == idx_name:
            return True
        return False

    def _emit_iter_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
        item_hint = self._iterable_element_hint(iterable) or "Any"
        if self.is_async():
            iter_obj = self._emit_iter_new(iterable)
            iter_slot = self._async_local_offset(f"__for_iter_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", iter_slot, iter_obj],
                    result=MoltValue("none"),
                )
            )
            guard_map = self._emit_hoisted_loop_guards(node.body)
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            iter_val = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_val,
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            pair = self._emit_iter_next_checked(iter_val)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_TRUE",
                    args=[done],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
            self._emit_assign_target(target, item, None)
            body_terminated = self._visit_loop_body(
                node.body, guard_map, loop_break_flag=loop_break_flag
            )
            if not body_terminated:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return
        guard_map = (
            {}
            if self.current_func_name == "molt_main"
            else self._emit_hoisted_loop_guards(node.body)
        )

        def emit_loop_body() -> None:
            iter_obj = self._emit_iter_new(iterable)
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))

            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            pair = self._emit_iter_next_checked(iter_obj)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_TRUE",
                    args=[done],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
            self._emit_assign_target(target, item, None)
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            if not body_terminated:
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return

        emit_loop_body()

    def _emit_index_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
        item_hint = self._iterable_element_hint(iterable) or "Any"
        if self.is_async():
            seq_slot = self._async_local_offset(f"__for_seq_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", seq_slot, iterable],
                    result=MoltValue("none"),
                )
            )
            length_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[iterable], result=length_val))
            length_slot = self._async_local_offset(
                f"__for_len_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", length_slot, length_val],
                    result=MoltValue("none"),
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            idx_slot = self._async_local_offset(f"__for_idx_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", idx_slot, zero],
                    result=MoltValue("none"),
                )
            )
            guard_map = self._emit_hoisted_loop_guards(node.body)
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", idx_slot],
                    result=idx,
                )
            )
            seq_val = MoltValue(self.next_var(), type_hint=iterable.type_hint)
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", seq_slot],
                    result=seq_val,
                )
            )
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", length_slot],
                    result=length,
                )
            )
            cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[seq_val, idx], result=item))
            self._emit_assign_target(target, item, None)
            self.async_index_loop_stack.append(idx_slot)
            body_terminated = self._visit_loop_body(
                node.body, guard_map, loop_break_flag=loop_break_flag
            )
            self.async_index_loop_stack.pop()
            if not body_terminated:
                idx_after = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="LOAD_CLOSURE",
                        args=["self", idx_slot],
                        result=idx_after,
                    )
                )
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx_after, one], result=next_idx))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", idx_slot, next_idx],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[iterable], result=length))

            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LOOP_INDEX_START", args=[zero], result=idx))
            cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint=item_hint)
            self.emit(MoltOp(kind="INDEX", args=[iterable, idx], result=item))
            self._emit_assign_target(target, item, None)
            self.range_loop_stack.append((idx, one))
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            self.range_loop_stack.pop()
            if not body_terminated:
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
                self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return

        emit_loop_body()

    def _parse_range_call(
        self, node: ast.AST
    ) -> tuple[MoltValue, MoltValue, MoltValue, bool] | None:
        if not isinstance(node, ast.Call):
            return None
        if not isinstance(node.func, ast.Name) or node.func.id != "range":
            return None
        if len(node.args) > 3:
            return None
        if node.keywords:
            return None
        start_val: MoltValue | None = None
        stop_val: MoltValue | None = None
        step_val: MoltValue | None = None
        pos_params: list[str] = []
        if len(node.args) == 1:
            pos_params = ["stop"]
        elif len(node.args) == 2:
            pos_params = ["start", "stop"]
        elif len(node.args) == 3:
            pos_params = ["start", "stop", "step"]
        for param, arg in zip(pos_params, node.args):
            val = self.visit(arg)
            if val is None:
                return None
            if param == "start":
                if start_val is not None:
                    return None
                start_val = val
            elif param == "stop":
                if stop_val is not None:
                    return None
                stop_val = val
            else:
                if step_val is not None:
                    return None
                step_val = val
        if stop_val is None:
            return None
        if start_val is None:
            start_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
        if step_val is None:
            step_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step_val))
        int_like = {"int", "bool"}
        lowerable = {
            start_val.type_hint,
            stop_val.type_hint,
            step_val.type_hint,
        }.issubset(int_like)
        return start_val, stop_val, step_val, lowerable

    def _emit_range_obj_from_args(
        self, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="range")
        self.emit(MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res))
        return res

    def _emit_range_step_zero_guard(
        self, step: MoltValue, step_const: int | None
    ) -> None:
        if step_const is not None and step_const != 0:
            return
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        is_zero = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[step, zero], result=is_zero))
        self.emit(MoltOp(kind="IF", args=[is_zero], result=MoltValue("none")))
        err_val = self._emit_exception_new(
            "ValueError", "range() arg 3 must not be zero"
        )
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_range_loop(
        self,
        node: ast.For,
        start: MoltValue,
        stop: MoltValue,
        step: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
        if self.is_async():
            range_obj = MoltValue(self.next_var(), type_hint="range")
            self.emit(
                MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=range_obj)
            )
            self._emit_iter_loop(node, range_obj, loop_break_flag=loop_break_flag)
            return None
        step_const = self.const_ints.get(step.name)
        self._emit_range_step_zero_guard(step, step_const)
        guard_map = self._emit_hoisted_loop_guards(node.body)
        simple_name_target = isinstance(target, ast.Name)

        def emit_range_loop_body() -> None:
            if step_const is not None and step_const != 0:
                with self._suppress_check_exception(emit_on_exit=False):
                    self.emit(
                        MoltOp(kind="LOOP_START", args=[], result=MoltValue("none"))
                    )
                    idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
                    cond = MoltValue(self.next_var(), type_hint="bool")
                    if step_const > 0:
                        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
                    else:
                        self.emit(MoltOp(kind="LT", args=[stop, idx], result=cond))
                    self.emit(
                        MoltOp(
                            kind="LOOP_BREAK_IF_FALSE",
                            args=[cond],
                            result=MoltValue("none"),
                        )
                    )
                    if simple_name_target:
                        self._emit_assign_target(target, idx, None)
                if not simple_name_target:
                    self._emit_assign_target(target, idx, None)
                self.range_loop_stack.append((idx, step))
                body_terminated = self._visit_loop_body(
                    node.body, None, loop_break_flag=loop_break_flag
                )
                self.range_loop_stack.pop()
                if not body_terminated:
                    with self._suppress_check_exception(emit_on_exit=False):
                        next_idx = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                        self.emit(
                            MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx)
                        )
                        self.emit(
                            MoltOp(
                                kind="LOOP_CONTINUE", args=[], result=MoltValue("none")
                            )
                        )
                    self.emit(
                        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                    )
                return None
            with self._suppress_check_exception(emit_on_exit=False):
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                step_pos = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[zero, step], result=step_pos))
            self.emit(MoltOp(kind="IF", args=[step_pos], result=MoltValue("none")))
            with self._suppress_check_exception(emit_on_exit=False):
                self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
                idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
                cond = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
                self.emit(
                    MoltOp(
                        kind="LOOP_BREAK_IF_FALSE",
                        args=[cond],
                        result=MoltValue("none"),
                    )
                )
                if simple_name_target:
                    self._emit_assign_target(target, idx, None)
            if not simple_name_target:
                self._emit_assign_target(target, idx, None)
            self.range_loop_stack.append((idx, step))
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            self.range_loop_stack.pop()
            if not body_terminated:
                with self._suppress_check_exception(emit_on_exit=False):
                    next_idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                    self.emit(
                        MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx)
                    )
                    self.emit(
                        MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                    )
                    self.emit(
                        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                    )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            with self._suppress_check_exception(emit_on_exit=False):
                step_neg = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[step, zero], result=step_neg))
            self.emit(MoltOp(kind="IF", args=[step_neg], result=MoltValue("none")))
            with self._suppress_check_exception(emit_on_exit=False):
                self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
                idx_neg = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx_neg))
                cond_neg = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="LT", args=[stop, idx_neg], result=cond_neg))
                self.emit(
                    MoltOp(
                        kind="LOOP_BREAK_IF_FALSE",
                        args=[cond_neg],
                        result=MoltValue("none"),
                    )
                )
                if simple_name_target:
                    self._emit_assign_target(target, idx_neg, None)
            if not simple_name_target:
                self._emit_assign_target(target, idx_neg, None)
            self.range_loop_stack.append((idx_neg, step))
            body_terminated = self._visit_loop_body(
                node.body, None, loop_break_flag=loop_break_flag
            )
            self.range_loop_stack.pop()
            if not body_terminated:
                with self._suppress_check_exception(emit_on_exit=False):
                    next_idx_neg = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg)
                    )
                    self.emit(
                        MoltOp(
                            kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg
                        )
                    )
                    self.emit(
                        MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                    )
                    self.emit(
                        MoltOp(kind="LOOP_END", args=[], result=MoltValue("none"))
                    )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_range_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_range_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return None

        emit_range_loop_body()
        return None

    def _emit_intarray_from_seq(self, seq: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="intarray")
        self.emit(MoltOp(kind="INTARRAY_FROM_SEQ", args=[seq], result=res))
        self.container_elem_hints[res.name] = "int"
        return res

    def _is_flat_list_int_container(self, value: MoltValue) -> bool:
        return value.name in getattr(self, "_list_int_containers", set())

    def _emit_iter_new(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=res))
        if self.try_end_labels:
            self._emit_raise_if_pending()
        else:
            self._emit_raise_if_pending(emit_exit=True)
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[res, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        err_val = self._emit_exception_new("TypeError", "object is not iterable")
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_iter_next_checked(self, iter_obj: MoltValue) -> MoltValue:
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        if not self.try_end_labels:
            # Every function now carries a function-level exception label
            # (needs_exception_stack defaults to True), so a pending exception
            # from ITER_NEXT always routes to the function handler via
            # `_emit_raise_if_pending`.  The former `else` branch — which
            # emitted LOOP_BREAK_IF_EXCEPTION for label-less functions — is
            # unreachable and has been removed.  (The LOOP_BREAK_IF_EXCEPTION
            # opcode itself is retained for other emission sites.)
            assert self.function_exception_label is not None, (
                "every function must carry a function-level exception label"
            )
            self._emit_raise_if_pending(emit_exit=True)
        return pair

    def _emit_guarded_setattr(
        self,
        obj: MoltValue,
        attr: str,
        value: MoltValue,
        expected_class: str,
        *,
        use_init: bool = False,
        assume_exact: bool = False,
        obj_name: str | None = None,
    ) -> None:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        class_ref: MoltValue | None = None
        if class_info and self._class_is_exception_subclass(expected_class, class_info):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        # Metaclass __init__ receives `cls` which is a TYPE object, not an
        # instance. Field offsets don't apply — always use generic setattr.
        if class_info and "type" in class_info.get("bases", []):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and attr not in class_info.get("fields", {}):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                if assume_exact and self._class_layout_stable(expected_class):
                    # The caller guarantees the object is an instance of
                    # expected_class (e.g. `self` inside a method body).
                    # Emit a direct field store even when the class_ref is
                    # not available in the current scope (class defined
                    # inside a function).
                    setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
                    self.emit(
                        MoltOp(
                            kind=setattr_kind,
                            args=[obj, attr, value, expected_class],
                            result=MoltValue("none"),
                        )
                    )
                    return
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value],
                        result=MoltValue("none"),
                    )
                )
                return

        def resolve_class_ref() -> MoltValue:
            nonlocal class_ref
            if class_ref is None:
                class_ref = self._emit_class_ref(expected_class)
            return class_ref

        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
            self.emit(
                MoltOp(
                    kind=setattr_kind,
                    args=[obj, attr, value, expected_class],
                    result=MoltValue("none"),
                )
            )
            return
        if assumption is False:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if self._class_layout_stable(expected_class):
            if assume_exact or self.exact_locals.get(name) == expected_class:
                setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
                self.emit(
                    MoltOp(
                        kind=setattr_kind,
                        args=[obj, attr, value, expected_class],
                        result=MoltValue("none"),
                    )
                )
                return
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            class_ref = resolve_class_ref()
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            setattr_kind = "GUARDED_SETATTR_INIT" if use_init else "GUARDED_SETATTR"
            self.emit(
                MoltOp(
                    kind=setattr_kind,
                    args=[
                        obj,
                        class_ref,
                        expected_version,
                        attr,
                        value,
                        expected_class,
                    ],
                    result=MoltValue("none"),
                )
            )
            return

        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
        self.emit(
            MoltOp(
                kind=setattr_kind,
                args=[obj, attr, value, expected_class],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_PTR",
                args=[obj, attr, value],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_guarded_getattr(
        self,
        obj: MoltValue,
        attr: str,
        expected_class: str,
        *,
        assume_exact: bool = False,
        obj_name: str | None = None,
    ) -> MoltValue:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        class_ref: MoltValue | None = None
        if class_info and self._class_is_exception_subclass(expected_class, class_info):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        # Metaclass methods operate on TYPE objects — field offsets don't apply.
        if class_info and "type" in class_info.get("bases", []):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        if class_info and attr not in class_info.get("fields", {}):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                if assume_exact and self._class_layout_stable(expected_class):
                    # The caller guarantees the object is an instance of
                    # expected_class (e.g. `self` in a method body or a
                    # freshly created instance in the calling scope).
                    # Use a direct field load.
                    res = MoltValue(self.next_var())
                    self.emit(
                        MoltOp(
                            kind="GETATTR",
                            args=[obj, attr, expected_class],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_PTR",
                        args=[obj, attr],
                        result=res,
                        metadata={"ic_index": _next_ic_index()},
                    )
                )
                return res

        def resolve_class_ref() -> MoltValue:
            nonlocal class_ref
            if class_ref is None:
                class_ref = self._emit_class_ref(expected_class)
            return class_ref

        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, attr, expected_class],
                    result=res,
                )
            )
            return res
        if assumption is False:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        if self._class_layout_stable(expected_class):
            if assume_exact or self.exact_locals.get(name) == expected_class:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR",
                        args=[obj, attr, expected_class],
                        result=res,
                    )
                )
                return res
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            class_ref = resolve_class_ref()
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GUARDED_GETATTR",
                    args=[obj, class_ref, expected_version, attr, expected_class],
                    result=res,
                )
            )
            return res
        return self._emit_guarded_field_get_with_guard(
            obj,
            fast_attr=attr,
            fallback_attr=attr,
            expected_class=expected_class,
            guard=guard,
        )

    def _emit_layout_guard(self, obj: MoltValue, expected_class: str) -> MoltValue:
        if expected_class == "dict":
            return self._emit_guard_dict_shape(obj)
        class_info = self.classes.get(expected_class)
        if class_info and not class_info.get("static"):
            class_ref = self._load_local_value(expected_class)
            if class_ref is None:
                guard = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=guard))
                return guard
        else:
            class_ref = self._emit_class_ref(expected_class)
        expected_version = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CONST",
                args=[self.classes[expected_class].get("layout_version", 0)],
                result=expected_version,
            )
        )
        guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="GUARD_LAYOUT",
                args=[obj, class_ref, expected_version],
                result=guard,
            )
        )
        return guard

    def _emit_guard_dict_shape(self, obj: MoltValue) -> MoltValue:
        dict_type = self._emit_builtin_type_value("dict")
        expected_version = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CLASS_VERSION",
                args=[dict_type],
                result=expected_version,
            )
        )
        guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="GUARD_DICT_SHAPE",
                args=[obj, dict_type, expected_version],
                result=guard,
            )
        )
        return guard

    def _emit_inc_ref(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="INC_REF", args=[value], result=res))
        return res

    def _emit_dec_ref(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=res))
        return res

    def _emit_drop_owned_value(self, value: MoltValue | None) -> None:
        if value is None or value.name == "none":
            return
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=MoltValue("none")))

    def _emit_borrow(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="BORROW", args=[value], result=res))
        return res

    def _emit_release(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="RELEASE", args=[value], result=res))
        return res

    def _emit_box(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="BOX", args=[value], result=res))
        return res

    def _emit_unbox(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="UNBOX", args=[value], result=res))
        return res

    def _emit_cast(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="CAST", args=[value], result=res))
        return res

    def _emit_widen(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="WIDEN", args=[value], result=res))
        return res

    def _loop_guard_assumption(self, obj_name: str, expected_class: str) -> bool | None:
        for guard_map in reversed(self.loop_guard_assumptions):
            entry = guard_map.get(obj_name)
            if entry and entry[0] == expected_class:
                return entry[1]
        return None

    def _push_loop_guard_assumptions(
        self,
        guard_map: dict[str, tuple[str, MoltValue]],
        assume_true: bool,
    ) -> None:
        assumptions: dict[str, tuple[str, bool]] = {}
        for name, (expected_class, _) in guard_map.items():
            assumptions[name] = (expected_class, assume_true)
        self.loop_guard_assumptions.append(assumptions)

    def _pop_loop_guard_assumptions(self) -> None:
        if self.loop_guard_assumptions:
            self.loop_guard_assumptions.pop()

    def _loop_guard_for(
        self, obj: MoltValue, expected_class: str, *, obj_name: str | None = None
    ) -> MoltValue | None:
        if not self.loop_layout_guards:
            return None
        name = obj_name or obj.name
        if self.exact_locals.get(name) != expected_class:
            return None
        guard_map = self.loop_layout_guards[-1]
        cached = guard_map.get(name)
        if cached and cached[0] == expected_class:
            return cached[1]
        guard = self._emit_layout_guard(obj, expected_class)
        guard_map[name] = (expected_class, guard)
        return guard

    def _invalidate_loop_guard(self, name: str) -> None:
        for guard_map in self.loop_layout_guards:
            guard_map.pop(name, None)

    def _invalidate_loop_guards_for_class(self, class_name: str) -> None:
        for guard_map in self.loop_layout_guards:
            stale = [
                key for key, (klass, _) in guard_map.items() if klass == class_name
            ]
            for key in stale:
                guard_map.pop(key, None)

    def _emit_hoisted_loop_guards(
        self, body: list[ast.stmt]
    ) -> dict[str, tuple[str, MoltValue]]:
        if self.is_async():
            return {}
        candidates = self._collect_loop_guard_candidates(body)
        if not candidates:
            return {}
        guard_map: dict[str, tuple[str, MoltValue]] = {}
        for name, expected_class in sorted(candidates.items()):
            obj = self._load_local_value(name)
            if obj is None:
                obj = self.locals.get(name) or self.globals.get(name)
            if obj is None:
                continue
            guard = self._emit_layout_guard(obj, expected_class)
            guard_map[name] = (expected_class, guard)
        return guard_map

    def _emit_guard_map_condition(
        self, guard_map: dict[str, tuple[str, MoltValue]]
    ) -> MoltValue:
        condition: MoltValue | None = None
        for _, (_, guard) in sorted(guard_map.items()):
            if condition is None:
                condition = guard
                continue
            combined = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="AND", args=[condition, guard], result=combined))
            condition = combined
        if condition is None:
            condition = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=condition))
        return condition

    def _emit_guarded_field_get_with_guard(
        self,
        obj: MoltValue,
        fast_attr: str,
        fallback_attr: str,
        expected_class: str,
        guard: MoltValue,
    ) -> MoltValue:
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        # Non-phi path. Async/poll-function bodies must thread the merged
        # result through a closure slot — the LIST_NEW + STORE_INDEX cell
        # pattern was unsafe because Cranelift's loop-header phi resolver
        # could merge the cell SSA value with the entry-block default
        # (None) on the first iteration, producing store_index(None, ...)
        # crashes.
        if self.is_async():
            slot = self._async_local_offset(f"__guarded_field_{len(self.async_locals)}")
            none_init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_init],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fast_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, slow_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=merged))
            return merged

        # Sync, non-phi path: a single SSA value updated in both branches.
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=merged))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var())
        self.emit(
            MoltOp(
                kind="GETATTR",
                args=[obj, fast_attr, expected_class],
                result=fast_val,
            )
        )
        self.emit(MoltOp(kind="COPY", args=[fast_val], result=merged))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, fallback_attr],
                result=slow_val,
                metadata={"ic_index": _next_ic_index()},
            )
        )
        self.emit(MoltOp(kind="COPY", args=[slow_val], result=merged))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if fast_val.type_hint == slow_val.type_hint:
            merged.type_hint = fast_val.type_hint
        return merged

    def _emit_guarded_property_get(
        self,
        obj: MoltValue,
        attr: str,
        getter_symbol: str,
        expected_class: str,
        return_hint: str | None,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        guard = self._loop_guard_for(obj, expected_class, obj_name=obj_name)
        if guard is None:
            guard = self._emit_layout_guard(obj, expected_class)
        use_phi = self.enable_phi and not self.is_async()
        fast_hint = return_hint or "Any"
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        # Non-phi path. See `_emit_guarded_field_get_with_guard` for the full
        # rationale: in poll-function bodies we route the merged result
        # through a closure slot rather than a LIST_NEW + STORE_INDEX cell,
        # which is unsafe under Cranelift's loop-header phi resolver.
        if self.is_async():
            slot = self._async_local_offset(
                f"__guarded_property_{len(self.async_locals)}"
            )
            none_init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, none_init],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fast_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, slow_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=merged))
            return merged

        # Sync, non-phi path: a single SSA value updated in both branches.
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=merged))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
        self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
        self.emit(MoltOp(kind="COPY", args=[fast_val], result=merged))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, attr],
                result=slow_val,
                metadata={"ic_index": _next_ic_index()},
            )
        )
        self.emit(MoltOp(kind="COPY", args=[slow_val], result=merged))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if fast_hint == slow_val.type_hint:
            merged.type_hint = fast_hint
        return merged

    def _emit_aiter(self, iterable: MoltValue) -> MoltValue:
        if iterable.type_hint in {
            "list",
            "tuple",
            "dict",
            "range",
            "iter",
            "generator",
        }:
            return self._emit_iter_new(iterable)
        res = MoltValue(self.next_var(), type_hint="async_iter")
        self.emit(MoltOp(kind="AITER", args=[iterable], result=res))
        return res

    def _emit_for_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        if self._iterable_is_indexable_for_loop(iterable):
            self._emit_index_loop(node, iterable, loop_break_flag=loop_break_flag)
        else:
            self._emit_iter_loop(node, iterable, loop_break_flag=loop_break_flag)

    def _prepare_mutable_control_flow_bindings(self, names: set[str]) -> None:
        if self._class_ns_stack:
            # Names bound by the active class body are backed by its namespace
            # dict (STORE_INDEX/INDEX through ``_class_ns_store``/``_class_ns_load``),
            # which is the heap-resident, loop-carried-correct mutable home — the
            # class-scope analogue of the module dict.  They must NOT be promoted
            # into ``module_global_mutations`` (which would leak the binding into
            # the enclosing module namespace and steer reads to MODULE_GET_ATTR)
            # nor boxed into list cells.  Strip them; let any genuine
            # surrounding-scope temps fall through to the normal handling.
            names = {n for n in names if not self._is_class_body_managed_name(n)}
        if not names:
            return
        # In function scope, loop-carried values are handled natively by
        # Cranelift's SSA phi/block-argument mechanism.  Boxing variables
        # into heap-allocated list cells adds ~10 cycles per access and
        # defeats raw_int_shadow optimisation.  Only box at module scope
        # (where there's no SSA) or for closures/nonlocals that truly
        # need heap storage.
        if self.current_func_name != "molt_main" and not self.is_async():
            return
        module_backed: set[str] = set()
        if self.current_func_name == "molt_main":
            # Module-scope control-flow bindings already have a canonical mutable
            # home: the module object. Route loads through MODULE_GET_ATTR instead
            # of synthesizing one-element list cells just to model loop-carried
            # mutation. That keeps module lowering canonical and avoids ad hoc
            # boxed-local indirection for top-level loops.
            module_backed = {name for name in names if not name.startswith("__molt_")}
            if module_backed:
                # Flush any values that were previously assigned (before
                # this loop) into the module dict.  Without this, a
                # variable assigned before the loop and then mutated inside
                # the loop would lose its initial value when module_get_attr
                # reads find nothing in the module dict.
                for name in sorted(module_backed):
                    # Skip variables already flushed to the module dict
                    # by an enclosing loop.  Re-flushing would overwrite
                    # the current dynamic value with the stale SSA value
                    # from the original definition, resetting accumulators
                    # on every outer loop iteration.
                    if name in self.module_global_mutations:
                        continue
                    existing = self.globals.get(name)
                    if existing is None:
                        existing = self.locals.get(name)
                    if existing is not None and self.module_obj is not None:
                        self._emit_module_attr_set_on(self.module_obj, name, existing)
                self.module_global_mutations.update(module_backed)
                # Remove from self.locals so visit_Name falls through to
                # the module_global_mutations check (module_get_attr).
                # Without this, the cached local SSA variable shadows the
                # module dict, making while loop conditions read stale values.
                for name in module_backed:
                    self.locals.pop(name, None)
        if self.is_async():
            return
        for name in sorted(names - module_backed):
            self._box_local(name)

    def _evict_module_control_flow_bindings(self, names: set[str]) -> None:
        if self.current_func_name != "molt_main" or self.is_async():
            return
        for name in names:
            if name in self.module_global_mutations:
                self.globals.pop(name, None)
                self.locals.pop(name, None)

    def _emit_loop_orelse(self, break_name: str, orelse: list[ast.stmt]) -> None:
        break_val = self._load_local_value(break_name)
        if break_val is None and break_name in self.module_global_mutations:
            break_val = self._emit_module_attr_get(break_name)
        if break_val is None:
            raise NotImplementedError("for-else break flag not initialized")
        should_run = self._emit_not(break_val)
        self.emit(MoltOp(kind="IF", args=[should_run], result=MoltValue("none")))
        self._visit_block(orelse)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _const_int_from_expr(self, node: ast.expr) -> int | None:
        if (
            isinstance(node, ast.Constant)
            and isinstance(node.value, int)
            and not isinstance(node.value, bool)
        ):
            return node.value
        if isinstance(node, ast.Name):
            value = self.locals.get(node.id)
            if value is None and self.current_func_name == "molt_main":
                value = self.globals.get(node.id)
            if value is not None:
                return self.const_ints.get(value.name)
        return None

    def _const_int_for_local(self, name: str) -> int | None:
        value = self.locals.get(name)
        if value is None:
            return 0
        return self.const_ints.get(value.name)

    def _is_unit_increment(self, stmt: ast.stmt, name: str) -> bool:
        if isinstance(stmt, ast.AugAssign):
            if isinstance(stmt.target, ast.Name) and stmt.target.id == name:
                return (
                    isinstance(stmt.op, ast.Add)
                    and isinstance(stmt.value, ast.Constant)
                    and stmt.value.value == 1
                )
            return False
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return False
            if stmt.targets[0].id != name:
                return False
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return False
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and left.id == name
                and isinstance(right, ast.Constant)
                and right.value == 1
            ):
                return True
            if (
                isinstance(right, ast.Name)
                and right.id == name
                and isinstance(left, ast.Constant)
                and left.value == 1
            ):
                return True
        return False

    def _emit_counted_while(
        self, index_name: str, bound: int, body: list[ast.stmt]
    ) -> None:
        start = self._load_local_value(index_name)
        if start is None:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        stop = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[bound], result=stop))
        guard_map = self._emit_hoisted_loop_guards(body)
        self._push_loop_static_class_refs(body)
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(index_name, idx)
        # For module-level code, also sync to the module namespace so
        # that module_get_attr reads inside the loop body see the current
        # counter value (not the initial value from before the loop).
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[index_name], result=key))
            self.emit(
                MoltOp(
                    kind="MODULE_SET_ATTR",
                    args=[self.module_obj, key, idx],
                    result=MoltValue("none"),
                )
            )
        body_terminated = self._visit_loop_body(body, guard_map)
        if not body_terminated:
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self._pop_loop_static_class_refs()
        self._store_local_value(index_name, idx)
        if self.current_func_name == "molt_main" and self.module_obj is not None:
            key2 = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[index_name], result=key2))
            self.emit(
                MoltOp(
                    kind="MODULE_SET_ATTR",
                    args=[self.module_obj, key2, idx],
                    result=MoltValue("none"),
                )
            )

    @staticmethod
    def _try_extract_const_str(node: ast.expr) -> str | None:
        """Recursively extract a constant string from an AST node.

        Handles plain string constants and chained Add operations
        over string constants (e.g. ``"a" + "b" + "c"``).
        """
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = SimpleTIRGenerator._try_extract_const_str(node.left)
            if left is None:
                return None
            right = SimpleTIRGenerator._try_extract_const_str(node.right)
            if right is None:
                return None
            return left + right
        return None

    def _emit_str_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_repr_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="REPR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_ascii_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="ASCII_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_string_join(self, parts: list[MoltValue]) -> MoltValue:
        if not parts:
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
            return res
        if len(parts) == 1:
            return parts[0]
        sep = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=sep))
        items = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=res))
        return res

    def _emit_string_format_value(self, value: MoltValue, spec: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_FORMAT", args=[value, spec], result=res))
        return res

    def _emit_string_format(self, value: MoltValue, spec: str) -> MoltValue:
        spec_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[spec], result=spec_val))
        return self._emit_string_format_value(value, spec_val)

    def _split_format_field_name(
        self, field_name: str
    ) -> tuple[int | str, list[tuple[bool, int | str]]] | None:
        if not field_name:
            return None
        idx = 0
        while idx < len(field_name) and field_name[idx] not in ".[":
            idx += 1
        first_text = field_name[:idx]
        if not first_text:
            return None
        if first_text.isdigit():
            first: int | str = int(first_text)
        else:
            first = first_text
        rest_items: list[tuple[bool, int | str]] = []
        while idx < len(field_name):
            ch = field_name[idx]
            if ch == ".":
                idx += 1
                start = idx
                while idx < len(field_name) and field_name[idx] not in ".[":
                    idx += 1
                if idx == start:
                    return None
                rest_items.append((True, field_name[start:idx]))
                continue
            if ch == "[":
                idx += 1
                start = idx
                while idx < len(field_name) and field_name[idx] != "]":
                    idx += 1
                if idx >= len(field_name):
                    return None
                key_text = field_name[start:idx]
                if not key_text:
                    return None
                if key_text.isdigit():
                    key: int | str = int(key_text)
                else:
                    key = key_text
                rest_items.append((False, key))
                idx += 1
                continue
            return None
        return first, rest_items

    def _parse_format_tokens(
        self,
        text: str,
        arg_count: int,
        kw_names: set[str],
        state: FormatParseState,
    ) -> list[FormatToken] | None:
        tokens: list[FormatToken] = []
        try:
            parsed = _py_string.Formatter().parse(text)
        except ValueError:
            return None
        for literal_text, field_name, format_spec, conversion in parsed:
            if literal_text:
                if tokens and isinstance(tokens[-1], FormatLiteral):
                    prior = tokens[-1]
                    tokens[-1] = FormatLiteral(prior.text + literal_text)
                else:
                    tokens.append(FormatLiteral(literal_text))
            if field_name is None:
                continue
            if conversion is not None and conversion not in {"r", "s", "a"}:
                return None
            if field_name == "":
                if state.used_manual:
                    return None
                state.used_auto = True
                key: int | str = state.next_auto
                state.next_auto += 1
                rest_items: list[tuple[bool, int | str]] = []
            else:
                if state.used_auto:
                    return None
                state.used_manual = True
                parsed_field = self._split_format_field_name(field_name)
                if parsed_field is None:
                    return None
                key, rest_items = parsed_field
            if isinstance(key, int):
                if key < 0 or key >= arg_count:
                    return None
            else:
                if key not in kw_names:
                    return None
            spec_tokens: list[FormatToken] | None = None
            if format_spec:
                spec_tokens = self._parse_format_tokens(
                    format_spec,
                    arg_count,
                    kw_names,
                    state,
                )
                if spec_tokens is None:
                    return None
            tokens.append(FormatField(key, rest_items, conversion, spec_tokens))
        return tokens

    def _emit_format_tokens(
        self,
        tokens: list[FormatToken],
        args: list[MoltValue],
        kwargs: dict[str, MoltValue],
    ) -> MoltValue:
        parts: list[MoltValue] = []
        for token in tokens:
            if isinstance(token, FormatLiteral):
                parts.append(self._emit_const_value(token.text))
                continue
            if isinstance(token.key, int):
                value = args[token.key]
            else:
                value = kwargs[token.key]
            for is_attr, name in token.rest:
                if is_attr:
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="GETATTR_GENERIC_OBJ",
                            args=[value, name],
                            result=res,
                        )
                    )
                    value = res
                else:
                    key_val = self._emit_const_value(name)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[value, key_val], result=res))
                    value = res
            if token.conversion is not None:
                if token.conversion == "r":
                    value = self._emit_repr_from_obj(value)
                elif token.conversion == "s":
                    value = self._emit_str_from_obj(value)
                elif token.conversion == "a":
                    value = self._emit_ascii_from_obj(value)
            if token.format_spec is None:
                spec_val = self._emit_const_value("")
            else:
                spec_val = self._emit_format_tokens(token.format_spec, args, kwargs)
            parts.append(self._emit_string_format_value(value, spec_val))
        return self._emit_string_join(parts)

    def _emit_not(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[value], result=res))
        return res

    def _emit_contains(self, container: MoltValue, item: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONTAINS", args=[container, item], result=res))
        return res

    def _emit_compare_op(
        self, op: ast.cmpop, left: MoltValue, right: MoltValue
    ) -> MoltValue:
        if isinstance(op, ast.Eq):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="EQ", args=[left, right], result=res))
            return res
        if isinstance(op, ast.NotEq):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Lt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Gt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="GT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.LtE):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.GtE):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="GE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Is):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[left, right], result=res))
            return res
        if isinstance(op, ast.IsNot):
            is_val = self._emit_compare_op(ast.Is(), left, right)
            return self._emit_not(is_val)
        if isinstance(op, ast.In):
            return self._emit_contains(right, left)
        if isinstance(op, ast.NotIn):
            in_val = self._emit_contains(right, left)
            return self._emit_not(in_val)
        raise NotImplementedError("Comparison operator not supported")

    def _emit_format_spec_value(self, node: ast.expr) -> MoltValue:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            spec_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.value], result=spec_val))
            return spec_val
        if isinstance(node, ast.JoinedStr):
            parts: list[MoltValue] = []
            for item in node.values:
                if isinstance(item, ast.Constant) and isinstance(item.value, str):
                    lit = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[item.value], result=lit))
                    parts.append(lit)
                    continue
                if isinstance(item, ast.FormattedValue):
                    value = self.visit(item.value)
                    if value is None:
                        raise NotImplementedError(
                            "Unsupported f-string format spec value"
                        )
                    if item.conversion != -1:
                        if item.conversion == ord("r"):
                            value = self._emit_repr_from_obj(value)
                        elif item.conversion == ord("s"):
                            value = self._emit_str_from_obj(value)
                        elif item.conversion == ord("a"):
                            value = self._emit_ascii_from_obj(value)
                        else:
                            raise NotImplementedError(
                                "Formatted value conversion not supported"
                            )
                    if item.format_spec is None:
                        parts.append(self._emit_string_format(value, ""))
                    else:
                        spec_val = self._emit_format_spec_value(item.format_spec)
                        parts.append(self._emit_string_format_value(value, spec_val))
                    continue
                raise NotImplementedError("Unsupported f-string format spec segment")
            return self._emit_string_join(parts)
        spec_val = self.visit(node)
        if spec_val is None:
            raise NotImplementedError("Unsupported f-string format spec")
        return self._emit_str_from_obj(spec_val)

    def _parse_molt_buffer_call(
        self, node: ast.Call, name: str
    ) -> list[ast.expr] | None:
        if (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "molt_buffer"
            and node.func.attr == name
        ):
            return node.args
        return None

    def _emit_template_interpolation(self, node: Any) -> MoltValue:
        """Lower a single ``ast.Interpolation`` inside a ``t"..."`` literal.

        Constructs a ``string.templatelib.Interpolation`` instance with
        ``(value, expression, conversion, format_spec)`` matching CPython 3.14
        semantics. ``conversion`` is the single-letter str ('s'/'r'/'a') or
        ``None``; ``format_spec`` is the rendered format-spec text or ``""``.
        """
        value = self.visit(node.value)
        if value is None:
            raise NotImplementedError("Unsupported t-string interpolation value")
        # expression — the literal source text of the interpolated expression.
        expression_text = node.str if node.str is not None else ""
        expression_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(kind="CONST_STR", args=[expression_text], result=expression_val)
        )
        # conversion — None for -1, otherwise single-char str.
        conversion = node.conversion
        if conversion == -1:
            conversion_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=conversion_val))
        elif conversion in (ord("s"), ord("r"), ord("a")):
            conversion_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(
                    kind="CONST_STR",
                    args=[chr(conversion)],
                    result=conversion_val,
                )
            )
        else:
            raise NotImplementedError("Unsupported t-string interpolation conversion")
        # format_spec — rendered to str via shared f-string format-spec helper.
        if node.format_spec is None:
            format_spec_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=format_spec_val))
        else:
            format_spec_val = self._emit_format_spec_value(node.format_spec)
        # Construct ``Interpolation(value, expression, conversion, format_spec)``.
        interp_class = self._emit_module_attr_get_on(
            "string.templatelib", "Interpolation"
        )
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for arg in (value, expression_val, conversion_val, format_spec_val):
            push_res = MoltValue(self.next_var(), type_hint="None")
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[callargs, arg],
                    result=push_res,
                )
            )
        interp_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_BIND",
                args=[interp_class, callargs],
                result=interp_val,
            )
        )
        return interp_val

    @staticmethod
    def _can_inline_any_all_genexpr(node: ast.GeneratorExp) -> bool:
        return (
            len(node.generators) == 1
            and not node.generators[0].is_async
            and isinstance(node.generators[0].target, ast.Name)
        )

    def _function_needs_classcell(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> bool:
        for child in ast.walk(node):
            if isinstance(child, ast.Name) and child.id == "__class__":
                return True
            if (
                isinstance(child, ast.Call)
                and isinstance(child.func, ast.Name)
                and child.func.id == "super"
                and not child.args
                and not child.keywords
            ):
                return True
        return False

    def _emit_attribute_load(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: str | None,
    ) -> MoltValue:
        # Set expression-level col_offset from the Attribute AST node so
        # that get_attr ops carry the correct column range for traceback
        # caret annotations (e.g. `x.upper` not `x.upper()`).
        _prev_expr_col = getattr(self, "_expr_col", None)
        _attr_col = getattr(node, "col_offset", None)
        _attr_end_col = getattr(node, "end_col_offset", None)
        if _attr_col is not None and _attr_end_col is not None:
            self._expr_col = (_attr_col, _attr_end_col)
        try:
            return self._emit_attribute_load_inner(node, obj, obj_name, exact_class)
        finally:
            self._expr_col = _prev_expr_col

    def _emit_attribute_load_inner(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: str | None,
    ) -> MoltValue:
        if obj.type_hint.startswith("super"):
            super_class = None
            if obj.type_hint == "super":
                super_class = self.current_class
            else:
                super_class = obj.type_hint.split(":", 1)[1]
            if super_class:
                method_info, method_class = self._resolve_super_method_info(
                    super_class, node.attr
                )
                if method_info and method_info["descriptor"] in {
                    "function",
                    "classmethod",
                }:
                    owner_name = method_class or super_class
                    res = MoltValue(
                        self.next_var(),
                        type_hint=f"BoundMethod:{owner_name}:{node.attr}",
                    )
                    self.emit(
                        MoltOp(
                            kind="GETATTR_GENERIC_OBJ",
                            args=[obj, node.attr],
                            result=res,
                        )
                    )
                    return res
        class_info = self.classes.get(obj.type_hint)
        if class_info:
            getattribute_info, _ = self._resolve_method_info(
                obj.type_hint, "__getattribute__"
            )
            if getattribute_info:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_PTR",
                        args=[obj, node.attr],
                        result=res,
                        metadata={"ic_index": _next_ic_index()},
                    )
                )
                return res
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.attr not in field_map:
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_OBJ",
                        args=[obj, node.attr],
                        result=res,
                    )
                )
                return res
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[node.attr]], result=idx_val))
            hint = None
            if self._hints_enabled():
                hint = class_info.get("field_hints", {}).get(node.attr)
            res = MoltValue(self.next_var(), type_hint=hint or "Unknown")
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
            return res
        method_info = None
        method_class = None
        if class_info:
            method_info, method_class = self._resolve_method_info(
                obj.type_hint, node.attr
            )
        is_class_obj = (
            obj_name is not None
            and obj.type_hint == "type"
            and (obj_name in self.classes or obj_name in BUILTIN_TYPE_TAGS)
        )
        if method_info and method_info["descriptor"] == "function" and not is_class_obj:
            if method_class:
                method_owner_info = self.classes.get(method_class)
                if (
                    method_owner_info
                    and method_owner_info.get("module") == self.module_name
                ):
                    method_info = None
            # Avoid binding to same-module class methods directly; class method
            # objects are not guaranteed to be in scope for direct reuse.
        if method_info and method_info["descriptor"] == "function" and not is_class_obj:
            fields = class_info.get("fields", {}) if class_info else {}
            if (
                class_info
                and not class_info.get("dynamic")
                and class_info.get("module") == self.module_name
                and node.attr not in fields
                and not self._instance_attr_mutated(obj.type_hint, node.attr)
            ):
                func_val = method_info["func"]
                if self.current_func_name != "molt_main":
                    class_ref = MoltValue(self.next_var(), type_hint="type")
                    self.emit(MoltOp(kind="TYPE_OF", args=[obj], result=class_ref))
                    func_val = self._emit_class_method_func(class_ref, node.attr)
                class_name = method_class or obj.type_hint
                res = MoltValue(
                    self.next_var(),
                    type_hint=f"BoundMethod:{class_name}:{node.attr}",
                )
                self.emit(
                    MoltOp(
                        kind="BOUND_METHOD_NEW",
                        args=[func_val, obj],
                        result=res,
                    )
                )
                return res
        if (
            method_info
            and method_info["descriptor"] == "property"
            and class_info
            and not class_info.get("dynamic")
        ):
            property_field = method_info.get("property_field")
            if property_field:
                field_map = class_info.get("fields", {})
                if (
                    property_field in field_map
                    and not self._class_attr_is_data_descriptor(
                        obj.type_hint, property_field
                    )
                ):
                    guard = self._loop_guard_for(obj, obj.type_hint, obj_name=obj_name)
                    if guard is None:
                        guard = self._emit_layout_guard(obj, obj.type_hint)
                    return self._emit_guarded_field_get_with_guard(
                        obj,
                        fast_attr=property_field,
                        fallback_attr=node.attr,
                        expected_class=obj.type_hint,
                        guard=guard,
                    )
            getter_symbol = method_info["func"].type_hint.split(":", 1)[1]
            return self._emit_guarded_property_get(
                obj,
                node.attr,
                getter_symbol,
                obj.type_hint,
                method_info["return_hint"],
                obj_name=obj_name,
            )
        if obj.type_hint.startswith("module"):
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=res,
                )
            )
            return res
        # Fast-path BoundMethod hints for known built-in types.
        # When the receiver type is statically known (e.g. type_hint="str")
        # and the accessed attribute is in the fast-dispatch method table,
        # annotate the result with "BoundMethod:<type>:<method>" so that
        # _emit_dynamic_call emits CALL_METHOD and the native backend's
        # s_value match arm can avoid callargs allocation + IC lookup.
        _fast_methods = _BUILTIN_FAST_METHODS.get(obj.type_hint)
        if _fast_methods is not None and node.attr in _fast_methods:
            res = MoltValue(
                self.next_var(),
                type_hint=f"BoundMethod:{obj.type_hint}:{node.attr}",
            )
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        expected_class = obj.type_hint if obj.type_hint in self.classes else None
        if expected_class is None:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self.classes[expected_class].get("dynamic"):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        field_map = self.classes[expected_class].get("fields", {})
        if node.attr not in field_map:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        if self._class_attr_is_data_descriptor(expected_class, node.attr):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                    metadata={"ic_index": _next_ic_index()},
                )
            )
            return res
        hint = None
        if self._hints_enabled():
            hint = self.classes[expected_class].get("field_hints", {}).get(node.attr)
        assume_exact = exact_class == expected_class if exact_class else False
        res = self._emit_guarded_getattr(
            obj,
            node.attr,
            expected_class,
            assume_exact=assume_exact,
            obj_name=obj_name,
        )
        if hint is not None:
            res.type_hint = hint
        return res

    def _emit_unpack_assign(
        self, target: ast.Tuple | ast.List, value_node: MoltValue | None
    ) -> None:
        if value_node is None:
            raise NotImplementedError("Unsupported unpack assignment value")
        star_index: int | None = None
        for idx, elt in enumerate(target.elts):
            if isinstance(elt, ast.Starred):
                if star_index is not None:
                    raise NotImplementedError(
                        "Multiple starred assignment is not supported"
                    )
                star_index = idx
        seq_val: MoltValue | None = None
        length: MoltValue | None = None

        def emit_unpack_error(
            prefix: str, expected: MoltValue, got: MoltValue | None
        ) -> None:
            parts: list[MoltValue] = []
            head = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[prefix], result=head))
            parts.append(head)
            parts.append(self._emit_str_from_obj(expected))
            if got is not None:
                mid = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[", got "], result=mid))
                parts.append(mid)
                parts.append(self._emit_str_from_obj(got))
            tail = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[")"], result=tail))
            parts.append(tail)
            msg_val = self._emit_string_join(parts)
            exc_val = self._emit_exception_new("ValueError", msg_val)
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
            self._emit_raise_exit()

        # For the no-star case without an indexable hint, we used to inline
        # a materialization loop that appended elements to a LIST_NEW list.
        # That caused heap corruption when the same list was reused across
        # outer-loop iterations.  Now we always pass value_node directly to
        # UNPACK_SEQUENCE and let the runtime handle validation + extraction.
        if star_index is None and not self._iterable_is_indexable(value_node):
            pass  # seq_val stays None → handled below
        if star_index is not None:
            if seq_val is None:
                seq_val = self._emit_list_from_iter(value_node)
            if length is None:
                length = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[seq_val], result=length))
        if star_index is None:
            if seq_val is None:
                seq_val = value_node
            # Emit a single outlined unpack_sequence op that validates the
            # length and extracts all elements in one runtime call.
            item_vals: list[MoltValue] = []
            for _ in target.elts:
                item_vals.append(MoltValue(self.next_var(), type_hint="Any"))
            self.emit(
                MoltOp(
                    kind="UNPACK_SEQUENCE",
                    args=[seq_val] + item_vals,
                    result=MoltValue("none"),
                    metadata={"expected_count": len(target.elts)},
                )
            )
            for elt, item_val in zip(target.elts, item_vals):
                self._emit_assign_target(elt, item_val, None)
            return

        prefix_len = star_index
        suffix_len = len(target.elts) - star_index - 1
        min_expected = prefix_len + suffix_len
        min_expected_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[min_expected], result=min_expected_val))
        too_few = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[length, min_expected_val], result=too_few))
        self.emit(MoltOp(kind="IF", args=[too_few], result=MoltValue("none")))
        emit_unpack_error(
            "not enough values to unpack (expected at least ",
            min_expected_val,
            length,
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        for idx in range(prefix_len):
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
            item_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val))
            self._emit_assign_target(target.elts[idx], item_val, None)

        start_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[prefix_len], result=start_val))
        end_val = length
        if suffix_len:
            suffix_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[suffix_len], result=suffix_val))
            end_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="SUB", args=[length, suffix_val], result=end_val))
        slice_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(kind="SLICE", args=[seq_val, start_val, end_val], result=slice_val)
        )
        star_target = cast(ast.Starred, target.elts[star_index]).value
        self._emit_assign_target(star_target, slice_val, None)

        if suffix_len:
            suffix_base = end_val
            for offset in range(suffix_len):
                if offset == 0:
                    idx_val = suffix_base
                else:
                    offset_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                    idx_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="ADD", args=[suffix_base, offset_val], result=idx_val
                        )
                    )
                item_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val)
                )
                self._emit_assign_target(
                    target.elts[star_index + 1 + offset], item_val, None
                )
        return

    def _emit_attribute_store(
        self,
        obj: MoltValue | None,
        obj_expr: ast.AST | None,
        obj_name: str | None,
        exact_class: str | None,
        attr: str,
        value_node: MoltValue,
    ) -> None:
        if obj_expr is not None and isinstance(obj_expr, ast.Name):
            class_name = obj_expr.id
            if class_name in self.classes:
                self._invalidate_loop_guards_for_class(class_name)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if exact_class is not None:
            self._record_instance_attr_mutation(exact_class, attr)
        elif obj is not None and obj.type_hint in self.classes:
            self._record_instance_attr_mutation(obj.type_hint, attr)
        if exact_class is not None and obj is not None:
            exact_info = self.classes.get(exact_class)
            if (
                exact_info
                and not exact_info.get("dynamic")
                and not exact_info.get("dataclass")
            ):
                field_map = exact_info.get("fields", {})
                if attr in field_map and not self._class_attr_is_data_descriptor(
                    exact_class, attr
                ):
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        exact_class,
                        obj_name=obj_name,
                        assume_exact=True,
                    )
                    return
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if attr not in field_map:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
                return
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[attr]], result=idx_val))
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        field_map = class_info.get("fields", {}) if class_info else {}
        if obj is not None and obj.type_hint in self.classes:
            if class_info and class_info.get("dynamic"):
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
            elif attr in field_map:
                if self._class_attr_is_data_descriptor(obj.type_hint, attr):
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    # Inside a method body, `self` (the first parameter)
                    # is guaranteed to be an instance of the current class.
                    # Mark it as exact so the guarded setattr can use a
                    # direct field store instead of the slow generic path.
                    is_method_self = (
                        self.current_class is not None
                        and obj_expr is not None
                        and isinstance(obj_expr, ast.Name)
                        and obj_expr.id == self.current_method_first_param
                        and obj.type_hint == self.current_class
                    )
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        obj.type_hint,
                        obj_name=obj_name,
                        assume_exact=is_method_self,
                    )
            else:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
        else:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value_node],
                    result=MoltValue("none"),
                )
            )

    def _emit_assign_target(
        self,
        target: ast.AST,
        value_node: MoltValue | None,
        source_expr: ast.AST | None,
    ) -> None:
        if isinstance(target, (ast.Tuple, ast.List)):
            self._emit_unpack_assign(target, value_node)
            return
        if value_node is None:
            raise NotImplementedError("Unsupported assignment value")
        if isinstance(target, ast.Attribute):
            self._record_imported_module_attr_mutation(target)
            obj = self.visit(target.value)
            obj_name = None
            exact_class = None
            if isinstance(target.value, ast.Name):
                obj_name = target.value.id
                exact_class = self.exact_locals.get(obj_name)
            self._emit_attribute_store(
                obj,
                target.value,
                obj_name,
                exact_class,
                target.attr,
                value_node,
            )
            return
        if isinstance(target, ast.Name):
            # A class-body name (for-loop target, with-as target, tuple-unpack
            # element, plain assign) binds ONLY into the class namespace mapping
            # (P0 #50).  ``_store_local_value`` routes it there via the class-ns
            # hook; the module/global publication side effects below (module
            # attr-set, ``self.globals`` registration, exact-local tracking) are
            # for module/function scope and must NOT fire — they would leak the
            # class-body name into the enclosing namespace and steer later reads
            # away from the class dict.  Short-circuit to the single store.
            if self._active_class_ns_scope(target.id) is not None:
                self._store_local_value(target.id, value_node)
                return
            optional_intrinsic_name = (
                self._match_optional_intrinsic_loader_expr(source_expr)
                if source_expr is not None
                else None
            )
            self.imported_names.pop(target.id, None)
            self.imported_attr_names.pop(target.id, None)
            self.imported_modules.pop(target.id, None)
            self.local_imported_names.discard(target.id)
            self.local_imported_modules.discard(target.id)
            if self.current_func_name == "molt_main":
                self.global_imported_names.pop(target.id, None)
                self.global_imported_attr_names.pop(target.id, None)
                self.global_imported_modules.pop(target.id, None)
                if optional_intrinsic_name is None:
                    self.module_intrinsic_globals.pop(target.id, None)
                else:
                    runtime_name = _canonical_intrinsic_runtime_name(
                        optional_intrinsic_name
                    )
                    self.module_intrinsic_globals[target.id] = runtime_name
                    self.reserved_external_func_symbols.add(runtime_name)
            if (
                self.current_func_name == "molt_main"
                or target.id not in self.global_decls
            ):
                if source_expr is not None:
                    self._update_exact_local(target.id, source_expr)
                    self._propagate_func_type_hint(value_node, source_expr)
            if self.current_func_name != "molt_main" and target.id in self.global_decls:
                self._store_local_value(target.id, value_node)
                # Also update the module-level attribute so global assignment
                # is visible to other functions reading the module dict.
                self._emit_module_attr_set_runtime(target.id, value_node)
                return
            if self.is_async():
                self._store_local_value(target.id, value_node)
            else:
                self._apply_explicit_hint(target.id, value_node)
                self._store_local_value(target.id, value_node)
                if value_node is not None:
                    self._propagate_container_hints(target.id, value_node)
                self._emit_module_attr_set(target.id, value_node)
                if self.current_func_name == "molt_main":
                    self.module_chunk_globals.add(target.id)
                    self.globals[target.id] = value_node
            return
        if isinstance(target, ast.Subscript):
            target_obj = self.visit(target.value)
            target_name = (
                target.value.id if isinstance(target.value, ast.Name) else None
            )
            if isinstance(target.slice, ast.Slice):
                if target_obj is None:
                    raise NotImplementedError("Unsupported slice assignment target")
                if target_obj.type_hint == "bytearray":
                    self._invalidate_bytearray_len_hint(target_name, target_obj)
                if target.slice.lower is None:
                    start = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                else:
                    start = self.visit(target.slice.lower)
                if target.slice.upper is None:
                    end = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                else:
                    end = self.visit(target.slice.upper)
                if target.slice.step is None:
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                else:
                    step = self.visit(target.slice.step)
                slice_obj = MoltValue(self.next_var(), type_hint="slice")
                self.emit(
                    MoltOp(kind="SLICE_NEW", args=[start, end, step], result=slice_obj)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[target_obj, slice_obj, value_node],
                        result=MoltValue("none"),
                    )
                )
                return
            index_val = self.visit(target.slice)
            if target_obj is not None and target_obj.type_hint == "list":
                self._record_list_element_write(
                    target_obj, target_name, value_node.type_hint
                )
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[target_obj, index_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        raise NotImplementedError("Unsupported assignment target")

    def _dict_increment_key_is_single_eval_safe(self, key: ast.expr) -> bool:
        if isinstance(key, (ast.Name, ast.Constant)):
            return True
        if not isinstance(key, ast.Attribute) or not isinstance(key.value, ast.Name):
            return False
        obj_name = key.value.id
        obj_value = self.locals.get(obj_name)
        if obj_value is None and self.current_func_name == "molt_main":
            obj_value = self.globals.get(obj_name)
        class_id = self.exact_locals.get(obj_name)
        if class_id is None and obj_value is not None:
            class_id = self.boxed_local_hints.get(obj_name) or obj_value.type_hint
        class_info = self.classes.get(class_id or "")
        return bool(
            class_info
            and class_info.get("dataclass")
            and key.attr in class_info.get("fields", {})
        )

    def _emit_split_dict_increment_for_loop(self, node: ast.For) -> bool:
        match = self._match_split_dict_increment_for_loop(node)
        if match is None:
            return False
        dict_expr, line_expr, sep_expr, delta_expr = match
        dict_obj = self.visit(dict_expr)
        line_obj = self.visit(line_expr)
        delta_obj = self.visit(delta_expr)
        if dict_obj is None or line_obj is None or delta_obj is None:
            return False
        # Keep split+count lanes guarded so deopt/profile tooling can track
        # dict-shape assumptions explicitly.
        self._emit_guard_dict_shape(dict_obj)
        pair = MoltValue(self.next_var(), type_hint="tuple")
        if sep_expr is None:
            self.emit(
                MoltOp(
                    kind="STRING_SPLIT_WS_DICT_INC",
                    args=[line_obj, dict_obj, delta_obj],
                    result=pair,
                )
            )
        else:
            sep_obj = self.visit(sep_expr)
            if sep_obj is None:
                return False
            self.emit(
                MoltOp(
                    kind="STRING_SPLIT_SEP_DICT_INC",
                    args=[line_obj, sep_obj, dict_obj, delta_obj],
                    result=pair,
                )
            )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        last_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=last_val))
        has_any = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=has_any))
        self.emit(MoltOp(kind="IF", args=[has_any], result=MoltValue("none")))
        self._emit_assign_target(node.target, last_val, None)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if node.orelse:
            self._visit_block(node.orelse)
        return True

    def _is_taq_header_guard(self, stmt: ast.stmt) -> str | None:
        if not isinstance(stmt, ast.If):
            return None
        if stmt.orelse:
            return None
        if not isinstance(stmt.test, ast.Name):
            return None
        if len(stmt.body) != 2:
            return None
        assign, cont = stmt.body
        if not isinstance(cont, ast.Continue):
            return None
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        if assign.targets[0].id != stmt.test.id:
            return None
        if (
            not isinstance(assign.value, ast.Constant)
            or assign.value.value is not False
        ):
            return None
        return stmt.test.id

    def _emit_taq_ingest_loop_body(
        self,
        body: list[ast.stmt],
    ) -> bool:
        match = self._match_taq_ingest_loop_body(body)
        if match is None:
            return False
        header_name, data_name, line_name, _split_name, bucket_expr = match
        if header_name is not None:
            header_val = self._load_local_value(header_name)
            if header_val is None:
                header_val = self.locals.get(header_name) or self.globals.get(
                    header_name
                )
            if header_val is None:
                return False
            self.emit(MoltOp(kind="IF", args=[header_val], result=MoltValue("none")))
            header_false = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=header_false))
            self._emit_assign_target(
                ast.Name(id=header_name, ctx=ast.Store()),
                header_false,
                None,
            )
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        data_val = self._load_local_value(data_name)
        if data_val is None:
            data_val = self.locals.get(data_name) or self.globals.get(data_name)
        if data_val is None:
            return False
        line_val = self._load_local_value(line_name)
        if line_val is None:
            line_val = self.locals.get(line_name) or self.globals.get(line_name)
        if line_val is None:
            return False
        bucket_val = self.visit(bucket_expr)
        if bucket_val is None:
            return False
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="TAQ_INGEST_LINE",
                args=[data_val, line_val, bucket_val],
                result=res,
            )
        )
        return True

    def _emit_delete_name(self, name: str, *, allow_missing: bool = False) -> None:
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            self._class_ns_delete(class_scope, name)
            return
        if self.current_func_name == "molt_main":
            if name in self.boxed_locals:
                cell = self.boxed_locals[name]
                idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                # Read old value from cell before overwriting, then dec_ref
                # to release the initial allocation ref.  Without this, the
                # object's __del__ won't fire until function return.
                old_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=old_val))
                missing = self._emit_missing_value()
                self.globals.pop(name, None)
                if allow_missing:
                    self._emit_module_global_del_safe(name)
                else:
                    self._emit_module_global_del(name)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[cell, idx, missing],
                        result=MoltValue("none"),
                    )
                )
                self._emit_drop_owned_value(old_val)
                self.unbound_check_names.add(name)
                return
            # Module scope already has a canonical mutable store: the module
            # object. Do not synthesize boxed list cells just because a name is
            # later deleted; those cells add an extra mutable indirection layer
            # that diverges from module-backed semantics and can miscompile in
            # large chunked stdlib modules.
            local_val = self.locals.pop(name, None)
            self.globals.pop(name, None)
            if allow_missing:
                self._emit_module_global_del_safe(name)
            else:
                self._emit_module_global_del(name)
            # Emit dec_ref for the local SSA variable so refcount drops
            # to zero immediately, triggering __del__ (CPython parity).
            self._emit_drop_owned_value(local_val)
            return
        if name in self.global_decls:
            if allow_missing:
                self._emit_module_global_del_safe(name)
            else:
                self._emit_module_global_del(name)
            return
        if name in self.nonlocal_decls or name in self.free_vars:
            old_val = self._emit_free_var_load(name, guard_unbound=not allow_missing)
            missing = self._emit_missing_value()
            if not self._emit_free_var_store(name, missing):
                raise NotImplementedError("nonlocal binding not found")
            self._emit_drop_owned_value(old_val)
            return
        # Only box for closure-captured variables; non-closure locals use the
        # first-class delete_var local-slot transition so the backend sees one
        # atomic "mark unbound, then release old occupant" boundary.
        if name in self.closure_locals:
            self._box_local(name)
        old_val = self._load_local_value(name, guard_unbound=not allow_missing)
        missing = self._emit_missing_value()
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
            and name not in self.free_vars
            and name not in self.nonlocal_decls
            and old_val is not None
        ):
            self._emit_delete_local_value(name, missing, old_val)
        else:
            self._store_local_value(name, missing)
            self._emit_drop_owned_value(old_val)
        self.unbound_check_names.add(name)
        return

    def _augassign_op_kind(self, op: ast.operator) -> str:
        # Every augmented assignment lowers to a dedicated INPLACE_* kind so the
        # runtime tries the in-place dunder (__iadd__/__ifloordiv__/__ipow__/...)
        # BEFORE the binary fallback, matching CPython. The boxed runtime symbol
        # for each (molt_inplace_floordiv etc.) first calls call_inplace_dunder
        # and only falls through to the binary protocol on NotImplemented. The
        # static int/float fast lanes remain identical to the binary op because
        # builtin int/float define no in-place dunders (so += on an int is byte-
        # identical whether it routes through molt_add or molt_inplace_add).
        #
        # AUGASSIGN_OP_KIND is generated from op_kinds.toml's [[binary_op]] table,
        # which is EXHAUSTIVE over ast.operator (a missing operator is a
        # generation-time failure — the task-#27 lesson). A KeyError here would
        # mean a NEW ast.operator subclass CPython added that the registry has
        # not yet been regenerated for.
        try:
            return AUGASSIGN_OP_KIND[type(op).__name__]
        except KeyError:
            raise NotImplementedError(
                f"Unsupported augmented assignment operator: {type(op).__name__}"
            ) from None

    def _emit_static_if_live_branch(self, branch: list[ast.stmt]) -> None:
        """Emit only the statically-live branch of a constant `if`.

        The dead branch is dropped entirely (CPython parity: its assignments and
        any value/intrinsic references never reach the IR). Live-branch names are
        boxed / module-backed exactly as a normal conditional branch would do, so
        a name assigned only here behaves identically whether or not the fold
        fired.
        """
        if branch and not self.is_async():
            assigned = self._collect_assigned_names(branch)
            if self.current_func_name == "molt_main":
                module_backed = {n for n in assigned if not n.startswith("__molt_")}
                if module_backed:
                    for name in sorted(module_backed):
                        existing = self.globals.get(name)
                        if existing is None:
                            existing = self.locals.get(name)
                        if existing is not None and self.module_obj is not None:
                            self._emit_module_attr_set_on(
                                self.module_obj, name, existing
                            )
                    self.module_global_mutations.update(module_backed)
                for name in sorted(assigned - module_backed):
                    self._box_local(name)
            else:
                for name in sorted(assigned):
                    if name not in self.scope_assigned or name in self.closure_locals:
                        self._box_local(name)
        self._visit_block(branch)

    def _visit_block(self, body: list[ast.stmt]) -> bool:
        prior = self.block_terminated
        self.block_terminated = False
        terminated = False
        for stmt in body:
            self.visit(stmt)
            if self.block_terminated:
                terminated = True
                break
            # Emit a check_exception after each statement to catch any
            # pending exception from the preceding ops.  This uses the
            # same fast inline flag check as all other check_exception
            # sites, avoiding the broken exception_last → is → not → if
            # → raise pattern that produced stale-exception re-raise bugs.
            handler_label: int | None
            if self.try_end_labels:
                handler_label = self.try_end_labels[-1]
            else:
                handler_label = self.function_exception_label
            if handler_label is not None:
                self.emit(
                    MoltOp(
                        kind="CHECK_EXCEPTION",
                        args=[handler_label],
                        result=MoltValue("none"),
                    )
                )
        self.block_terminated = prior
        return terminated

    def _visit_loop_body(
        self,
        body: list[ast.stmt],
        prefill: dict[str, tuple[str, MoltValue]] | None = None,
        loop_break_flag: int | str | None = None,
    ) -> bool:
        if not self.is_async() and self._emit_taq_ingest_loop_body(body):
            return True
        if not self.is_async():
            guard_map = dict(prefill) if prefill else {}
            self.loop_layout_guards.append(guard_map)
        self.loop_break_flags.append(loop_break_flag)
        self.loop_try_depths.append(len(self.try_scopes))
        terminated = False
        # Snapshot unbound_check_names — the loop body may not execute
        # at all (empty range / false initial condition), so any
        # discards inside the body must be reverted on exit.  Inside
        # the body, post-assignment loads still skip the check, which
        # is the source of the per-iter speedup on
        # `obj = Class(...); obj.x = …; obj.y = …` patterns.
        unbound_snapshot = set(self.unbound_check_names)
        try:
            self.control_flow_depth += 1
            try:
                terminated = self._visit_block(body)
            finally:
                self.control_flow_depth -= 1
        finally:
            self.unbound_check_names = unbound_snapshot
            self.loop_break_flags.pop()
            self.loop_try_depths.pop()
            if not self.is_async():
                self.loop_layout_guards.pop()
        return terminated

    def _emit_guarded_body(
        self, body: list[ast.stmt], baseline_exc: ActiveException | None
    ) -> None:
        if not body:
            return
        self.visit(body[0])
        remaining = body[1:]
        if not remaining:
            return
        skip_label = self.next_label()
        self.emit(
            MoltOp(
                kind="CHECK_EXCEPTION",
                args=[skip_label],
                result=MoltValue("none"),
            )
        )
        self._emit_guarded_body(remaining, baseline_exc)
        self.emit(MoltOp(kind="LABEL", args=[skip_label], result=MoltValue("none")))

    def _emit_finalbody(
        self,
        finalbody: list[ast.stmt],
        baseline_exc: ActiveException | None,
        *,
        popped_scopes: int = 0,
    ) -> None:
        self.return_unwind_depth += 1
        self.finally_depth += 1
        self.return_unwind_popped_scopes.append(popped_scopes)
        self._emit_guarded_body(finalbody, baseline_exc)
        self.return_unwind_popped_scopes.pop()
        self.finally_depth -= 1
        self.return_unwind_depth -= 1

    def _ctx_mark_arg(self, scope: TryScope) -> MoltValue:
        if not scope.needs_context_unwind or scope.ctx_mark is None:
            raise AssertionError("context unwind requested without a context mark")
        if scope.ctx_mark_offset is None or not self.is_async():
            return scope.ctx_mark
        res = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", scope.ctx_mark_offset],
                result=res,
            )
        )
        return res

    def _emit_context_unwind_to(self, scope: TryScope, exc_val: MoltValue) -> None:
        if not scope.needs_context_unwind:
            return
        ctx_arg = self._ctx_mark_arg(scope)
        self.emit(
            MoltOp(
                kind="CONTEXT_UNWIND_TO",
                args=[ctx_arg, exc_val],
                result=MoltValue("none"),
            )
        )

    def _emit_control_flow_scope_unwind(self, scopes: Sequence[TryScope]) -> list[int]:
        unwind_scopes = list(scopes)
        if not unwind_scopes:
            return []
        none_exc = None
        if self.context_depth > 0 and any(
            scope.needs_context_unwind for scope in unwind_scopes
        ):
            none_exc = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
        skip_pops = 0
        if self.return_unwind_depth > 0 and self.return_unwind_popped_scopes:
            skip_pops = self.return_unwind_popped_scopes[-1]
        popped_scopes = 0
        skip_finalbody = self.return_unwind_depth
        popped_labels: list[int] = []
        for scope in reversed(unwind_scopes):
            if skip_pops > 0:
                skip_pops -= 1
                popped_scopes += 1
                if skip_finalbody > 0:
                    skip_finalbody -= 1
                continue
            if none_exc is not None:
                self._emit_context_unwind_to(scope, none_exc)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_POP",
                    args=[],
                    result=MoltValue("none"),
                )
            )
            if scope.handler_label is not None:
                if scope.handler_label not in self.try_end_labels:
                    pass
                elif self.try_end_labels[-1] != scope.handler_label:
                    raise AssertionError(
                        "control-flow unwind tried to pop handler label "
                        f"{scope.handler_label}, active labels={self.try_end_labels}"
                    )
                else:
                    popped_labels.append(self.try_end_labels.pop())
            popped_scopes += 1
            if scope.finalbody:
                if skip_finalbody > 0:
                    skip_finalbody -= 1
                else:
                    prior_active = self.active_exceptions[:]
                    self.active_exceptions.clear()
                    self._emit_finalbody(
                        scope.finalbody, None, popped_scopes=popped_scopes
                    )
                    self.active_exceptions = prior_active
        return popped_labels

    def _restore_control_flow_unwind_labels(self, popped_labels: Sequence[int]) -> None:
        for label in reversed(popped_labels):
            self.try_end_labels.append(label)

    def _emit_raise_exit(self) -> None:
        if self.try_end_labels:
            if (
                self.try_suppress_depth is None
                or len(self.try_end_labels) > self.try_suppress_depth
            ):
                self.emit(
                    MoltOp(
                        kind="CHECK_EXCEPTION",
                        args=[self.try_end_labels[-1]],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(
                        kind="JUMP",
                        args=[self.try_end_labels[-1]],
                        result=MoltValue("none"),
                    )
                )
                return
        if self.try_handler_scopes:
            done_label = self.try_handler_scopes[-1].done_label
            if done_label is not None:
                self.emit(
                    MoltOp(
                        kind="CHECK_EXCEPTION",
                        args=[done_label],
                        result=MoltValue("none"),
                    )
                )
                self.emit(
                    MoltOp(
                        kind="JUMP",
                        args=[done_label],
                        result=MoltValue("none"),
                    )
                )
                return
        if self.function_exception_label is not None:
            self._emit_restore_exception_stack_depth()
            self.emit(
                MoltOp(
                    kind="CHECK_EXCEPTION",
                    args=[self.function_exception_label],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="JUMP",
                    args=[self.function_exception_label],
                    result=MoltValue("none"),
                )
            )
            return
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        self.emit(MoltOp(kind="ret", args=[none_val], result=MoltValue("none")))

    def _emit_raise_if_pending(
        self,
        *,
        emit_exit: bool = False,
        clear_handlers: bool = False,
        force_exit: bool = False,
    ) -> None:
        # Use the same fast inline flag check as check_exception instead
        # of the exception_last → is → not → if → raise pattern.  The old
        # pattern produced stale-exception re-raise bugs because
        # exception_last() and the inline flag byte could disagree, and
        # the Cranelift-compiled if/raise/end_if sometimes executed the
        # raise unconditionally.
        handler_label: int | None
        if self.try_end_labels:
            handler_label = self.try_end_labels[-1]
        else:
            handler_label = self.function_exception_label
        if handler_label is not None:
            if (
                self.current_func_name == "molt_main"
                or self.current_func_name.startswith("molt_init_")
            ):
                self._emit_line_marker_force()
            self.emit(
                MoltOp(
                    kind="CHECK_EXCEPTION",
                    args=[handler_label],
                    result=MoltValue("none"),
                )
            )
        # The CHECK_EXCEPTION above handles all exception propagation.
        # Generator-specific GeneratorExit detection is handled by the
        # exception handler label target, which routes to the appropriate
        # cleanup code.

    def _emit_call_bound_or_func(
        self, callee: MoltValue, args: list[MoltValue]
    ) -> MoltValue:
        # Use CALL_FUNC to centralize bound-method handling and keep async IR linear.
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
        return res

    def _emit_sync_try_except_split(
        self,
        node: ast.Try,
        scope: TryScope,
        unbound_snapshot_try: set[str],
        prior_terminated: bool,
    ) -> None:
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_exc_label = self.next_label()
        try_normal_label = self.next_label()
        try_clean_cleanup_label = self.next_label()
        try_pending_cleanup_label = self.next_label()
        try_done_label = self.next_label()
        scope.handler_label = try_exc_label
        scope.done_label = try_pending_cleanup_label
        self.try_end_labels.append(try_exc_label)
        self.emit(
            MoltOp(
                kind="TRY_START",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self._visit_block(node.body)
        body_terminated = self.block_terminated
        self.block_terminated = False
        if not body_terminated:
            self.emit(
                MoltOp(
                    kind="TRY_END",
                    args=[try_exc_label],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(kind="JUMP", args=[try_normal_label], result=MoltValue("none"))
            )
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="TRY_END",
                args=[try_exc_label],
                result=MoltValue("none"),
            )
        )
        self.try_end_labels.pop()
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)
        self.try_handler_scopes.append(scope)

        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST_PENDING", args=[], result=exc_val))
        self._emit_context_unwind_to(scope, exc_val)

        def emit_handlers(handlers: list[ast.ExceptHandler]) -> None:
            if not handlers:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
                return
            handler = handlers[0]
            match_val = self._emit_exception_match(handler, exc_val)
            self.emit(MoltOp(kind="IF", args=[match_val], result=MoltValue("none")))
            exc_slot_offset = None
            if handler.name:
                if self.current_func_name == "molt_main":
                    self.module_global_mutations.add(handler.name)
                self._store_local_value(handler.name, exc_val)
            exc_entry = ActiveException(
                value=exc_val,
                slot=exc_slot_offset,
                handler_name=handler.name,
                is_handler=True,
                handler_try_depth=len(self.try_end_labels),
            )
            self.active_exceptions.append(exc_entry)
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._emit_guarded_body(handler.body, exc_entry)
            handler_terminated = self.block_terminated
            if not handler_terminated:
                self._emit_exception_handler_exit_cleanup(exc_entry)
            self.active_exceptions.pop()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            if len(handlers) > 1:
                emit_handlers(handlers[1:])
            else:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        emit_handlers(node.handlers)
        self.emit(
            MoltOp(
                kind="JUMP",
                args=[try_pending_cleanup_label],
                result=MoltValue("none"),
            )
        )

        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_normal_label],
                result=MoltValue("none"),
            )
        )
        if node.orelse:
            self._emit_guarded_body(node.orelse, None)
            self.emit(
                MoltOp(
                    kind="JUMP",
                    args=[try_pending_cleanup_label],
                    result=MoltValue("none"),
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="JUMP",
                    args=[try_clean_cleanup_label],
                    result=MoltValue("none"),
                )
            )
        self.try_handler_scopes.pop()
        self.try_suppress_depth = prior_suppress
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_clean_cleanup_label],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="JUMP", args=[try_done_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_pending_cleanup_label],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        self.emit(MoltOp(kind="JUMP", args=[try_done_label], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_done_label],
                result=MoltValue("none"),
            )
        )
        self.try_scopes.pop()
        self.unbound_check_names = unbound_snapshot_try
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated

    def _emit_loop_unwind(self) -> list[int]:
        if not self.loop_try_depths:
            return []
        max_scopes = len(self.try_scopes)
        loop_depth = self.loop_try_depths[-1]
        if loop_depth >= max_scopes:
            return []
        return self._emit_control_flow_scope_unwind(
            self.try_scopes[loop_depth:max_scopes]
        )

    # Modules whose API calls are lowered directly to IR ops by the frontend.
    # ``import molt_buffer`` etc. are no-ops: the module object is never used
    # at runtime because every ``molt_buffer.new()`` / ``molt_msgpack.parse()``
    # call is already emitted as specialised IR (BUFFER2D_NEW, MSGPACK_PARSE, …).
    _STUB_IMPORT_MODULES: frozenset[str] = frozenset(
        {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}
    )
    _IMPORT_TRANSACTION_BOOTSTRAP_MODULES: frozenset[str] = frozenset(
        {"builtins", "_molt_importer"}
    )

    def _source_imports_use_transaction(self) -> bool:
        return not (
            self.module_name in self._IMPORT_TRANSACTION_BOOTSTRAP_MODULES
            or self.module_name == "importlib"
            or self.module_name.startswith("importlib.")
        )

    def _finalize_code_ids(self) -> None:
        for data in self.funcs_map.values():
            for op in data["ops"]:
                if op.kind in {"CALL", "CALL_INTERNAL"} and op.args:
                    target = op.args[0]
                    if isinstance(target, str):
                        self._register_code_symbol(target)

    def _ensure_code_slots_init(self) -> None:
        if self.code_slots_emitted:
            return
        self.code_slots_emitted = True
        max_code_id = max(self.func_code_ids.values(), default=-1)
        for data in self.funcs_map.values():
            for op in data["ops"]:
                if op.kind == "CODE_SLOT_SET" and op.metadata:
                    code_id = op.metadata.get("code_id")
                    if code_id is not None:
                        max_code_id = max(max_code_id, int(code_id))
        count = max_code_id + 1
        init_op = MoltOp(
            kind="CODE_SLOTS_INIT",
            args=[count],
            result=MoltValue("none"),
        )
        ops = self.funcs_map.get("molt_main", {}).get("ops")
        if ops is not None:
            ops.insert(0, init_op)

    @staticmethod
    def _analyze_borrowing(params: list[str], ops: list[dict[str, Any]]) -> list[int]:
        """Perceus-style borrowing analysis for function parameters.

        Returns the list of parameter indices that are provably *borrowed* --
        i.e., the callee never stores, returns, yields, or otherwise causes
        the parameter value to escape the function scope.

        The analysis is conservative: if there is any doubt, the parameter is
        treated as escaping (owned), which preserves the status-quo RC behavior.
        False negatives (marking a borrowable param as owned) are safe; false
        positives (marking an escaping param as borrowed) would cause
        use-after-free bugs.

        The analysis operates on the serialized JSON ops (post-midend).  It
        performs a forward data-flow walk that tracks which SSA variable names
        are *tainted* -- meaning they carry (or may carry) the identity of a
        parameter value.  A tainted variable that appears in an escaping
        position causes the originating parameter to be marked as escaping.

        Escaping positions:
          - ``ret`` / ``ret_tuple`` operand (value returned to caller)
          - Stored into a container: ``list_new`` args, ``list_append`` value,
            ``tuple_new`` args, ``dict_set`` value, ``set_add`` value,
            ``store_index`` value, ``dict_setdefault`` value
          - Stored as an object attribute: ``set_attr_*`` value arg
          - Stored into a global/module: ``module_set_attr`` value arg,
            ``global_set`` value arg
          - Passed to any function call (conservative -- we lack interprocedural
            info): ``call``, ``call_func``, ``call_bind``, ``call_method``,
            ``call_internal``, ``call_indirect``, ``call_guarded``,
            ``call_async``, ``callargs_push_pos``, ``callargs_push_kw``,
            ``callargs_expand_star``, ``callargs_expand_kwstar``
          - Yielded via generator state ops: ``state_yield``
          - Closure capture: ``closure_store``
          - Exception creation: ``exception_new`` args and single-arg
            ``exception_new_builtin_one`` payloads

        Non-escaping (safe) uses:
          - Binary/unary arithmetic and comparison ops produce a *new* value;
            the operands do not escape.  ``add``, ``sub``, ``mul``, ``div``,
            ``mod``, ``pow``, ``floor_div``, ``lshift``, ``rshift``,
            ``bit_and``, ``bit_or``, ``bit_xor``, ``matmul``,
            ``compare_*``, ``is``, ``is_not``, ``contains``,
            ``not``, ``neg``, ``pos``, ``invert``, ``bool_cast``,
            ``int_cast``, ``float_cast``, ``str_cast``, ``repr_cast``,
            ``len``, ``hash``, ``type_check``, ``isinstance``,
            ``issubclass``, ``hasattr``, ``getattr``
          - Control flow / metadata: ``if``, ``else``, ``end_if``, ``loop_*``,
            ``label``, ``jump``, ``line``, ``check_exception``,
            ``exception_stack_*``, ``frame_locals_set``, ``trace_*``,
            ``nop``, ``ret_void``, ``code_slots_init``, ``code_slot_set``,
            ``phi``, ``phi_select``
          - Read-only indexing: ``index`` (reads from a container, does not
            store the operand)
          - ``print`` (consumes value for display, does not store)
          - ``get_iter``, ``iter_next``, ``iter_next_checked``
          - ``format``, ``format_spec``, ``str_concat``, ``str_join``,
            ``str_format``
        """
        if not params:
            return []
        # taint_map: variable_name -> set of param names whose identity it
        # may carry.  Params start tainted with themselves.
        taint_map: dict[str, set[str]] = {p: {p} for p in params}

        # escaped: param names that have been proven to escape.
        escaped: set[str] = set()

        # ---- Op classification tables ----

        # Ops that are purely safe for all their operands (operands do not
        # escape and the result is a fresh value).
        _SAFE_OPS: set[str] = {
            # Arithmetic / bitwise
            "add",
            "sub",
            "mul",
            "div",
            "mod",
            "pow",
            "floor_div",
            "lshift",
            "rshift",
            "bit_and",
            "bit_or",
            "bit_xor",
            "matmul",
            "iadd",
            "isub",
            "imul",
            "idiv",
            "imod",
            "ipow",
            "ifloor_div",
            "ilshift",
            "irshift",
            "ibit_and",
            "ibit_or",
            "ibit_xor",
            # Comparison
            "compare_eq",
            "compare_ne",
            "compare_lt",
            "compare_le",
            "compare_gt",
            "compare_ge",
            "lt",
            "le",
            "gt",
            "ge",
            "eq",
            "ne",
            "is",
            "is_not",
            "contains",
            "not_contains",
            # Unary
            "not",
            "neg",
            "pos",
            "invert",
            # Casts / introspection
            "bool_cast",
            "int_cast",
            "float_cast",
            "str_cast",
            "repr_cast",
            "len",
            "hash",
            "type_check",
            "isinstance",
            "issubclass",
            "hasattr",
            "id",
            # Read-only container access (reads, does not store the operand)
            "index",
            "get_iter",
            "iter_next",
            "iter_next_checked",
            # String ops (produce new strings)
            "format",
            "format_spec",
            "str_concat",
            "str_join",
            "str_format",
            "str_replace",
            "str_split",
            "str_strip",
            "str_lstrip",
            "str_rstrip",
            "str_lower",
            "str_upper",
            "str_startswith",
            "str_endswith",
            "str_find",
            "str_rfind",
            "str_count",
            "str_encode",
            "str_decode",
            # Print (consumes for display only)
            "print",
            # Attribute read (does not store the operand -- reads from it)
            "get_attr_name",
            "get_attr_name_default",
            "get_attr_generic_obj",
            "get_attr_generic_ptr",
            "module_get_attr",
            # Constants / metadata
            "const",
            "const_bool",
            "const_float",
            "const_str",
            "const_bytes",
            "const_none",
            "const_bigint",
            "missing",
            # Control flow / structural
            "if",
            "else",
            "end_if",
            "loop_start",
            "loop_end",
            "loop_continue",
            "loop_break",
            "loop_break_if_true",
            "loop_break_if_false",
            "label",
            "jump",
            "line",
            "nop",
            "ret_void",
            "check_exception",
            "exception_stack_enter",
            "exception_stack_exit",
            "exception_stack_depth",
            "exception_stack_set_depth",
            "exception_stack_clear",
            "exception_last",
            "exception_last_pending",
            "frame_locals_set",
            "trace_enter_slot",
            "trace_exit",
            "code_slots_init",
            "code_slot_set",
            "code_new",
            "phi",
            "phi_select",
            "func_new",
            "builtin_func",
            "class_new",
            "class_def",
            # Unpack operations (produce new values from a container)
            "unpack_sequence",
            "unpack_ex",
            # Slice
            "slice_new",
            "get_slice",
            # Variable ops (SSA-level, no heap escape)
            "store_var",
            "load_var",
        }

        # Ops where certain arg positions store the value into a container
        # (the value escapes into the heap).
        # Format: op_kind -> set of arg indices that are "value" positions
        # (0-based into the 'args' list).
        _CONTAINER_STORE_OPS: dict[str, set[int]] = {
            "list_append": {1},  # list_append(list, value)
            "dict_set": {2},  # dict_set(dict, key, value)
            "dict_setdefault": {2},  # dict_setdefault(dict, key, value)
            "dict_setdefault_empty_list": {2},
            "set_add": {1},  # set_add(set, value)
            "store_index": {2},  # store_index(container, index, value)
            "store_slice": {2},  # store_slice(container, slice, value)
        }

        # Ops that store a value as an object attribute.
        # The value arg is at index 1 in 'args': set_attr_*(obj, value)
        _ATTR_STORE_OPS: set[str] = {
            "set_attr_generic_obj",
            "set_attr_generic_ptr",
            "set_attr_name",
            "module_set_attr",
            "global_set",
        }

        # Ops where all args escape (function calls -- conservative).
        _CALL_OPS: set[str] = {
            "call",
            "call_func",
            "call_bind",
            "call_method",
            "call_internal",
            "call_indirect",
            "call_guarded",
            "call_async",
            "class_merge_layout",
        }

        # CallArgs ops where the value arg escapes into the callargs builder.
        _CALLARGS_ESCAPE_OPS: set[str] = {
            "callargs_push_pos",  # callargs_push_pos(builder, value)
            "callargs_push_kw",  # callargs_push_kw(builder, key, value)
            "callargs_expand_star",  # callargs_expand_star(builder, iterable)
            "callargs_expand_kwstar",
        }

        # Ops that create a new container with initial elements.
        # All args are stored into the container (escape).
        _CONTAINER_BUILD_OPS: set[str] = {
            "list_new",  # list_new(elem1, elem2, ...)
            "tuple_new",  # tuple_new(elem1, elem2, ...)
            "dict_new",  # dict_new(key1, val1, key2, val2, ...) -- safe if empty
            "set_new",  # set_new(elem1, elem2, ...)
        }

        # Other escaping ops
        _YIELD_OPS: set[str] = {
            "state_yield",
            "yield_value",
        }
        _CLOSURE_STORE_OPS: set[str] = {
            "closure_store",
            "closure_set",
        }

        def _get_op_args(op: dict[str, Any]) -> list[str]:
            """Extract string variable names from an op's args list."""
            args = op.get("args", [])
            return [a for a in args if isinstance(a, str)]

        def _mark_escaped(var_names: list[str]) -> None:
            """Mark all params tainted by these variables as escaped."""
            for v in var_names:
                taints = taint_map.get(v)
                if taints:
                    escaped.update(taints)

        def _propagate_taint(src_vars: list[str], dest: str | None) -> None:
            """Propagate taint from source variables to a destination variable.

            Used for ops like ``copy`` or aliasing where the output may carry
            the identity of an input.
            """
            if dest is None:
                return
            combined: set[str] = set()
            for v in src_vars:
                taints = taint_map.get(v)
                if taints:
                    combined.update(taints)
            if combined:
                existing = taint_map.get(dest)
                if existing:
                    existing.update(combined)
                else:
                    taint_map[dest] = set(combined)

        for op in ops:
            kind = op.get("kind", "")
            out = op.get("out")

            # Early exit: if all params already escaped, no point continuing.
            if len(escaped) >= len(params):
                break

            # --- Return: the returned value escapes ---
            if kind == "ret":
                var = op.get("var", "")
                if isinstance(var, str):
                    _mark_escaped([var])
                continue

            if kind == "ret_tuple":
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Container builds: all args escape into the new container ---
            if kind in _CONTAINER_BUILD_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                # The output is a new container; it doesn't carry param identity.
                continue

            # --- Container stores: value position escapes ---
            if kind in _CONTAINER_STORE_OPS:
                escape_indices = _CONTAINER_STORE_OPS[kind]
                args = op.get("args", [])
                for idx in escape_indices:
                    if idx < len(args) and isinstance(args[idx], str):
                        _mark_escaped([args[idx]])
                continue

            # --- Attribute stores: value escapes ---
            if kind in _ATTR_STORE_OPS:
                args = op.get("args", [])
                # For set_attr ops, the value is at index 1: set_attr(obj, value)
                if len(args) >= 2 and isinstance(args[1], str):
                    _mark_escaped([args[1]])
                continue

            # --- Call ops: all args escape (conservative) ---
            if kind in _CALL_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- CallArgs escape ops ---
            if kind in _CALLARGS_ESCAPE_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Yield / closure store ---
            if kind in _YIELD_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue
            if kind in _CLOSURE_STORE_OPS:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Exception creation: args escape ---
            if kind in {
                "exception_new",
                "exception_new_builtin",
                "exception_new_builtin_one",
            }:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Raise: the value escapes ---
            if kind in {"raise", "raise_cause", "reraise"}:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Safe ops: operands do not escape. ---
            if kind in _SAFE_OPS:
                # The output of safe ops is a *new* value, not tainted by
                # inputs (e.g., x + y produces a new int, not x or y).
                continue

            # --- Copy / alias ops: propagate taint ---
            if kind in {"copy", "alias", "move"}:
                args = _get_op_args(op)
                _propagate_taint(args, out)
                continue

            # --- Callargs builder creation: safe (no values yet) ---
            if kind == "callargs_new":
                continue

            # --- Dict update / merge ops: value args escape ---
            if kind in {"dict_update", "dict_update_missing", "dict_merge"}:
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Bridge / intrinsic calls: treat as escaping (conservative) ---
            if kind.startswith("bridge_") or kind.startswith("intrinsic_"):
                args = _get_op_args(op)
                _mark_escaped(args)
                continue

            # --- Unknown op: be conservative. Mark all args as escaping. ---
            # This ensures safety for any op kind we haven't explicitly
            # classified.  New op kinds added to the compiler will default
            # to the safe (conservative) behavior.
            args = _get_op_args(op)
            _mark_escaped(args)

        # Build result: param indices that are NOT in the escaped set.
        borrowed_indices: list[int] = []
        for i, p in enumerate(params):
            if p not in escaped:
                borrowed_indices.append(i)
        return borrowed_indices

    def to_json(
        self, *, midend_stage: Literal["pre-midend", "post-midend"] = "post-midend"
    ) -> dict[str, Any]:
        if midend_stage not in {"pre-midend", "post-midend"}:
            raise ValueError(f"unsupported IR serialization stage: {midend_stage}")
        self._finalize_code_ids()
        self._ensure_code_slots_init()
        funcs_json: list[dict[str, Any]] = []
        # DETERMINISM: sort to ensure stable output regardless of dict insertion order
        for name, data in sorted(self.funcs_map.items()):
            json_ops = self.map_ops_to_json(
                data["ops"],
                function_name=name,
                run_midend=midend_stage == "post-midend",
            )
            func_entry: dict[str, Any] = {
                "name": name,
                "params": data["params"],
                "ops": json_ops,
            }
            # Always emit param_types so the backend creates Cranelift block
            # params for function arguments. Without this, parameters are
            # uninitialized (read as 0x0 = float +0.0 in NaN-boxing).
            explicit_types = list(data.get("param_types") or [])
            if data["params"]:
                if len(explicit_types) < len(data["params"]):
                    explicit_types.extend(
                        ["i64"] * (len(data["params"]) - len(explicit_types))
                    )
                func_entry["param_types"] = explicit_types
            if self.source_path:
                func_entry["source_file"] = self.source_path
            # Perceus-style borrowing analysis: identify parameters that can
            # be treated as borrowed (no inc_ref on entry, no dec_ref on exit).
            if data["params"]:
                borrowed = self._analyze_borrowing(data["params"], json_ops)
                # Methods: `self` is always borrowed — the caller (class
                # dispatch / bound method) owns the reference. Without this,
                # the compiled __init__ dec-refs self on return, freeing the
                # instance before the caller can use it.
                if data["params"][0] == "self" and 0 not in borrowed:
                    borrowed.append(0)
                    borrowed.sort()
                if borrowed:
                    func_entry["borrowed_params"] = borrowed
            funcs_json.append(func_entry)
        max_code_id = -1
        for func in funcs_json:
            for op in func["ops"]:
                kind = op.get("kind")
                if kind in {"code_slot_set", "call"}:
                    max_code_id = max(max_code_id, int(op.get("value", -1)))
        if max_code_id >= 0:
            for func in funcs_json:
                if func["name"] != "molt_main":
                    continue
                for op in func["ops"]:
                    if op.get("kind") == "code_slots_init":
                        op["value"] = max_code_id + 1
                        break
                else:
                    func["ops"].insert(
                        0, {"kind": "code_slots_init", "value": max_code_id + 1}
                    )
                break
        self._maybe_report_midend_stats()
        return {"functions": funcs_json}


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
