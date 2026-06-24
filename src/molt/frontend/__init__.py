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
import bisect
import json
import os
import sys
import time
from collections import deque
from contextlib import contextmanager
from pathlib import Path
import string as _py_string
from typing import (
    TYPE_CHECKING,
    Any,
    Callable,
    Iterable,
    Literal,
    NoReturn,
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
    _MIDEND_DEGRADE_CHECKPOINTS,
    _MIDEND_WORK_GROWTH_HEADROOM,
    _MIDEND_WORK_BASE_UNITS_PER_MS,
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
from molt.frontend.sema import SemaResult, analyze_module

# Visitor / lowering mixins composed into SimpleTIRGenerator (F1 decomposition).
from molt.frontend.lowering.serialization import SerializationMixin
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
    SerializationMixin,
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
        self.module_declared_funcs: dict[str, str] = {}
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

    def _emit_module_metadata(self) -> None:
        if self.module_obj is None:
            return
        path_obj: Path | None = None
        origin_val: MoltValue | None = None
        path_list_val: MoltValue | None = None
        if self.source_path:
            path_obj = Path(self.source_path)
            normalized = path_obj.as_posix()
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[normalized], result=file_val))
            self._emit_module_attr_set_on(self.module_obj, "__file__", file_val)
            origin_val = file_val
        is_package = self.module_is_package
        spec_name = self.module_spec_name or self.module_name or ""
        package_name = self._spec_parent(spec_name, is_package)
        package_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[package_name], result=package_val))
        self._emit_module_attr_set_on(self.module_obj, "__package__", package_val)
        if is_package and path_obj is not None and not self.module_is_namespace:
            package_dir = path_obj.parent.as_posix()
            path_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[package_dir], result=path_val))
            list_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[path_val], result=list_val))
            self._emit_module_attr_set_on(self.module_obj, "__path__", list_val)
            path_list_val = list_val
        if (
            self.module_name == "importlib.machinery"
            or "importlib.machinery" not in self.known_modules
        ):
            spec_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=spec_none))
            self._emit_module_attr_set_on(self.module_obj, "__spec__", spec_none)
            return
        spec_name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(kind="CONST_STR", args=[self.module_spec_name], result=spec_name_val)
        )
        loader_default = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=loader_default))
        loader_val = self._emit_module_attr_get_default_on(
            "importlib.machinery", "MOLT_LOADER", loader_default
        )
        if origin_val is None:
            origin_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=origin_val))
        is_package_val = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[is_package], result=is_package_val))
        spec_cls = self._emit_module_attr_get_on("importlib.machinery", "ModuleSpec")
        spec_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_FUNC",
                args=[spec_cls, spec_name_val, loader_val, origin_val, is_package_val],
                result=spec_val,
            )
        )
        if path_list_val is not None:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[spec_val, "submodule_search_locations", path_list_val],
                    result=MoltValue("none"),
                )
            )
        self._emit_module_attr_set_on(self.module_obj, "__spec__", spec_val)

    def _emit_module_frame_enter(self, node: ast.Module) -> None:
        if (
            self.current_func_name != "molt_main"
            and not self.current_func_name.startswith("molt_init_")
        ) or self.module_frame_entered:
            return
        if self.module_name in _BOOTSTRAP_TRACE_EXEMPT_MODULES:
            return
        self.module_frame_entered = True
        code_id = self.module_frame_code_id
        if code_id is None:
            current_func = self.current_func_name
            code_id = self.func_code_ids.get(current_func)
            if code_id is None:
                code_id = self._register_code_symbol(current_func)
            self.module_frame_code_id = code_id
        if not self.module_frame_emitted:
            self.module_frame_emitted = True
            filename = self.source_path or "<unknown>"
            first_line = 1
            if node.body:
                first_line = int(getattr(node.body[0], "lineno", 1) or 1)
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[filename], result=file_val))
            line_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[first_line], result=line_val))
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=["<module>"], result=name_val))
            linetable_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=linetable_val))
            varnames_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=varnames_val))
            names_vals: list[MoltValue] = []
            for code_name in self._collect_code_names_for_body(
                node.body,
                varnames=[],
                free_vars=[],
                module_scope=True,
            ):
                name_item = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[code_name], result=name_item))
                names_vals.append(name_item)
            names_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=names_vals, result=names_tuple))
            argcount_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=argcount_val))
            posonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=posonly_val))
            kwonly_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=kwonly_val))
            code_val = MoltValue(self.next_var(), type_hint="code")
            self.emit(
                MoltOp(
                    kind="CODE_NEW",
                    args=[
                        file_val,
                        name_val,
                        line_val,
                        linetable_val,
                        varnames_val,
                        names_tuple,
                        argcount_val,
                        posonly_val,
                        kwonly_val,
                    ],
                    result=code_val,
                )
            )
            self.emit(
                MoltOp(
                    kind="CODE_SLOT_SET",
                    args=[code_val],
                    result=MoltValue("none"),
                    metadata={"code_id": code_id},
                )
            )
        self.emit(
            MoltOp(
                kind="TRACE_ENTER_SLOT",
                args=[code_id],
                result=MoltValue("none"),
            )
        )
        # Module-scope locals() must behave like globals(); pin the module dict on
        # the frame entry so builtins.locals/globals work even via getattr aliases.
        locals_dict = self._emit_globals_dict()
        self.emit(
            MoltOp(
                kind="FRAME_LOCALS_SET", args=[locals_dict], result=MoltValue("none")
            )
        )

    def _emit_module_frame_exit(self) -> None:
        if (
            (
                self.current_func_name != "molt_main"
                and not self.current_func_name.startswith("molt_init_")
            )
            or not self.module_frame_entered
            or self.module_frame_exited
        ):
            return
        self.module_frame_exited = True
        self.emit(MoltOp(kind="TRACE_EXIT", args=[], result=MoltValue("none")))

    def _function_needs_frame_trace(self, name: str | None = None) -> bool:
        func_name = self.current_func_name if name is None else name
        if func_name is None:
            return False
        if func_name == "molt_main" or func_name.startswith("molt_init_"):
            return False
        if func_name == _MOLT_GLOBALS_BUILTIN or func_name.endswith(
            f"__{_MOLT_GLOBALS_BUILTIN}"
        ):
            return False
        if name is not None and func_name not in self.funcs_map:
            return False
        return True

    def _module_chunk_param_value(self) -> MoltValue:
        return MoltValue(_MOLT_MODULE_CHUNK_PARAM, type_hint="module")

    def _new_module_chunk_symbol(self) -> str:
        self.module_chunk_counter += 1
        symbol = f"{self.module_prefix}{_MOLT_MODULE_CHUNK_PREFIX}_{self.module_chunk_counter}"
        while symbol in self.funcs_map:
            self.module_chunk_counter += 1
            symbol = f"{self.module_prefix}{_MOLT_MODULE_CHUNK_PREFIX}_{self.module_chunk_counter}"
        self.func_symbol_names[symbol] = "<module_chunk>"
        self._register_code_symbol(symbol)
        self.funcs_map[symbol] = FuncInfo(
            params=[_MOLT_MODULE_CHUNK_PARAM],
            param_types=[],
            return_hint=None,
            ops=self._new_tracked_ops(count_function=True),
        )
        self.module_chunk_symbols.append(symbol)
        return symbol

    def _reset_module_chunk_state(self) -> None:
        # Merge all module-level names defined so far into module_chunk_globals
        # so subsequent chunks can resolve them via MODULE_GET_GLOBAL.  This
        # covers class definitions, function definitions, imports, and plain
        # assignments — any name that was added to self.globals during prior
        # chunks.  Without this, names defined in chunk N but referenced in
        # chunk N+M would fall through to incorrect resolution paths (e.g.
        # stdlib_allowlist matching a variable alias against a module name).
        self.module_chunk_globals.update(self.globals.keys())
        self.locals = {}
        self.boxed_locals = {}
        self.closure_locals = set()
        self.comp_shadow_locals = set()
        self.boxed_local_hints = {}
        self.free_vars = {}
        self.free_var_hints = {}
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.unbound_check_names = set()
        self.exact_locals = {}
        self.exact_builtin_locals = {}
        self.globals = {}
        self.imported_names = dict(self.global_imported_names)
        self.imported_attr_names = dict(self.global_imported_attr_names)
        self.imported_modules = dict(self.global_imported_modules)
        self.local_imported_names = set()
        self.local_imported_modules = set()
        # Clear the per-function module cache so that module references are
        # re-fetched via MODULE_CACHE_GET in each new chunk function.  Without
        # this, a cached MoltValue from a previous chunk's WASM locals would be
        # reused, but the corresponding WASM local does not exist in the new
        # chunk — leaving the variable at its zero-initialized default (0x0),
        # which is not a valid module object.
        self._module_cache_values = {}
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
        self.bytearray_len_hints = {}
        self.context_depth = 0
        self.control_flow_depth = 0
        self.const_ints = {}
        self._op_by_result = {}
        self.in_generator = False
        self.async_context = False
        self.current_line = None
        self.module_annotations = None
        self.module_annotations_conditional = True
        self.module_annotation_exec_map = None
        self.try_end_labels = []
        self.try_scopes = []
        self.try_suppress_depth = None
        self.try_handler_scopes = []
        self.exception_stack_depth_baseline = None
        self.exception_stack_prev_baseline = None
        self.return_unwind_depth = 0
        self.return_unwind_popped_scopes = []
        self.finally_depth = 0
        self.return_label = None
        self.return_slot = None
        self.return_slot_index = None
        self.return_slot_offset = None
        self.block_terminated = False
        self.range_loop_stack = []
        self.async_index_loop_stack = []
        self.loop_break_flags = []
        self.loop_try_depths = []
        self.loop_break_counter = 0
        self.loop_layout_guards = []
        self.loop_guard_assumptions = []
        self.loop_static_class_refs = []
        self.loop_static_class_eager_refs = []
        self.loop_static_class_counter = 0
        self.active_exceptions = []

    def _module_chunk_stmt_cost(self, stmt: ast.stmt) -> int:
        # Chunking decisions happen before lowering the next top-level
        # statement, so use a cheap AST-size heuristic to avoid letting one
        # expensive statement balloon the chunk that came before it.
        node_cost = sum(1 for _ in ast.walk(stmt)) * 3
        line_span = (
            max(1, getattr(stmt, "end_lineno", stmt.lineno) - stmt.lineno + 1)
            if getattr(stmt, "lineno", None) is not None
            else 1
        )
        span_cost = line_span * 20
        dominant = max(node_cost, span_cost)
        secondary = min(node_cost, span_cost)
        # Reserve headroom for lowering-time metadata expansion
        # (labels/check_exception/class wiring) so large statements start a
        # fresh chunk before they poison the preceding one.
        return max(1, dominant + secondary // 4)

    def _c3_merge(self, seqs: list[list[str]]) -> list[str] | None:
        merged: list[str] = []
        working = [list(seq) for seq in seqs]
        heads = [0] * len(working)
        tail_counts: dict[str, int] = {}
        for seq in working:
            for name in seq[1:]:
                tail_counts[name] = tail_counts.get(name, 0) + 1

        while True:
            remaining = 0
            for idx, seq in enumerate(working):
                if heads[idx] < len(seq):
                    remaining += 1
            if remaining == 0:
                return merged

            candidate: str | None = None
            for idx, seq in enumerate(working):
                head_idx = heads[idx]
                if head_idx >= len(seq):
                    continue
                head = seq[head_idx]
                if tail_counts.get(head, 0) == 0:
                    candidate = head
                    break

            if candidate is None:
                return None

            merged.append(candidate)
            for idx, seq in enumerate(working):
                head_idx = heads[idx]
                if head_idx < len(seq) and seq[head_idx] == candidate:
                    heads[idx] += 1
                    next_head_idx = heads[idx]
                    if next_head_idx < len(seq):
                        next_head = seq[next_head_idx]
                        count = tail_counts.get(next_head, 0)
                        if count <= 1:
                            tail_counts.pop(next_head, None)
                        else:
                            tail_counts[next_head] = count - 1

    def _class_mro_names(self, name: str) -> list[str]:
        if name == "object":
            return ["object"]
        info = self.classes.get(name)
        if info is None:
            return [name]
        cached = info.get("mro")
        if cached:
            return cached
        bases = info.get("bases", [])
        seqs = [self._class_mro_names(base) for base in bases]
        seqs.append(list(bases))
        merged = self._c3_merge(seqs)
        if merged is None:
            mro = [name] + list(bases)
            info["mro"] = mro
            return mro
        mro = [name] + merged
        info["mro"] = mro
        return mro

    def _class_is_exception_subclass(
        self, class_name: str, class_info: ClassInfo
    ) -> bool:
        cached = class_info.get("exception_subclass")
        if cached is not None:
            return cached
        for base_name in self._class_mro_names(class_name)[1:]:
            if base_name in BUILTIN_EXCEPTION_NAMES and base_name not in self.classes:
                class_info["exception_subclass"] = True
                return True
            base_info = self.classes.get(base_name)
            if base_info and self._class_is_exception_subclass(base_name, base_info):
                class_info["exception_subclass"] = True
                return True
        class_info["exception_subclass"] = False
        return False

    def _resolve_method_info(
        self, class_name: str, method: str
    ) -> tuple[MethodInfo | None, str | None]:
        if class_name in self.class_definition_pending:
            return None, None
        for name in self._class_mro_names(class_name):
            info = self.classes.get(name)
            if not info:
                continue
            methods = info.get("methods", {})
            class_attrs = info.get("class_attrs", {})
            pending = info.get("pending_methods")
            # Avoid early binding to base methods when the current class
            # defines the method later in the class body.
            if pending and method in pending and method not in methods:
                return None, name
            # Avoid binding to base methods when a class-level assignment
            # overrides the attribute with a non-method value.
            if method in class_attrs and method not in methods:
                return None, name
            if method in methods:
                return methods[method], name
        return None, None

    def _static_class_bases(self, class_name: str) -> list[str] | None:
        """Return the single static base-name list for ``class_name`` usable to
        compute a C3 MRO, or ``None`` when it cannot be computed soundly.

        Sources, in order: the module class graph collected pre-pass
        (``module_class_bases`` — covers classes defined *later* in source than
        the current method body), then the dependency-closure class table
        (``self.classes``).  Returns ``None`` if the class has multiple
        conflicting definitions, an opaque (non-simple-name / keyword) base, a
        dynamically-built class, or is otherwise not statically resolvable.
        """
        if class_name == "object":
            return ["object"]
        defs = self.module_class_bases.get(class_name)
        if defs is not None:
            if len(defs) != 1:
                return None  # re-bound / conditional class def — not foldable
            entry = defs[0]
            if "<opaque>" in entry:
                return None
            return list(entry)
        info = self.classes.get(class_name)
        if info is not None:
            if info.get("dynamic") or info.get("custom_metaclass"):
                return None
            return list(info.get("bases", []) or ["object"])
        # Unknown name (builtin base like Exception, or not-yet-seen): treat as
        # un-foldable so a hidden interposition can never be missed.
        return None

    def _static_mro_names(
        self, class_name: str, _stack: tuple[str, ...] = ()
    ) -> list[str] | None:
        """Compute the C3 linearization of ``class_name`` from the static class
        graph, or ``None`` when any contributing class is not statically
        resolvable (forcing the super fold to fail-closed).
        """
        if class_name in _stack:
            return None  # cyclic inheritance — not resolvable
        if class_name == "object":
            return ["object"]
        bases = self._static_class_bases(class_name)
        if bases is None:
            return None
        base_mros: list[list[str]] = []
        for base in bases:
            base_mro = self._static_mro_names(base, _stack + (class_name,))
            if base_mro is None:
                return None
            base_mros.append(base_mro)
        base_mros.append(list(bases))
        merged = self._c3_merge(base_mros)
        if merged is None:
            return None  # C3 inconsistency — CPython would raise; don't fold
        return [class_name] + merged

    def _reachable_base_names(
        self, class_name: str, _seen: set[str] | None = None
    ) -> set[str]:
        """Transitive set of base names reachable from ``class_name`` over the
        static module class graph (best-effort; used only to decide whether an
        un-resolvable class might be a subclass of the fold target)."""
        if _seen is None:
            _seen = set()
        if class_name in _seen:
            return _seen
        _seen.add(class_name)
        defs = self.module_class_bases.get(class_name)
        if not defs:
            return _seen
        for entry in defs:
            for base in entry:
                if base != "<opaque>":
                    self._reachable_base_names(base, _seen)
        return _seen

    def _resolve_super_method_info(
        self, class_name: str, method: str
    ) -> tuple[MethodInfo | None, str | None]:
        mro = self._class_mro_names(class_name)
        found_start = False
        for name in mro:
            if not found_start:
                if name == class_name:
                    found_start = True
                continue
            info = self.classes.get(name)
            if info and "methods" in info and method in info["methods"]:
                return info["methods"][method], name
        return None, None

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

    _emitted_syntax_warnings: set[tuple[str, int, str]]
    _deferred_runtime_warnings: list[str]

    def _emit_deprecation_warning(self, node: ast.AST, message: str) -> None:
        """Emit a DeprecationWarning to stderr, matching CPython's format."""
        lineno = getattr(node, "lineno", 0)
        source = self.source_path or "<string>"
        key = (source, lineno, message)
        if key in self._emitted_syntax_warnings:
            return
        self._emitted_syntax_warnings.add(key)
        # Read the source line for context (matches CPython's warning format).
        src_line = ""
        try:
            with open(source) as f:
                for i, line in enumerate(f, 1):
                    if i == lineno:
                        src_line = line.rstrip()
                        break
        except (OSError, UnicodeDecodeError):
            pass
        import sys

        print(f"{source}:{lineno}: DeprecationWarning: {message}", file=sys.stderr)
        if src_line:
            print(f"  {src_line}", file=sys.stderr)

    def _prescan_compile_warnings(self, module_node: ast.Module) -> None:
        """Pre-scan AST for patterns that need compile-time warnings."""
        source = self.source_path or "<string>"
        cached_source_lines: list[str] | None | Literal[False] = False

        def source_line_for(lineno: int) -> str | None:
            nonlocal cached_source_lines
            if cached_source_lines is False:
                if source == "<string>":
                    cached_source_lines = None
                else:
                    try:
                        with open(source) as f:
                            cached_source_lines = [line.rstrip("\n") for line in f]
                    except (OSError, UnicodeDecodeError):
                        cached_source_lines = None
            if (
                not cached_source_lines
                or lineno <= 0
                or lineno > len(cached_source_lines)
            ):
                return None
            return cached_source_lines[lineno - 1].strip()

        def record_warning(lineno: int, category: str, message: str) -> None:
            key = (source, lineno, message)
            if key in self._emitted_syntax_warnings:
                return
            self._emitted_syntax_warnings.add(key)
            self._deferred_runtime_warnings.append(
                f"{source}:{lineno}: {category}: {message}"
            )
            src_line = source_line_for(lineno)
            if src_line:
                self._deferred_runtime_warnings.append(f"  {src_line}")

        scope_barriers = (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
        invert_bool_msg = (
            "Bitwise inversion '~' on bool is deprecated and will be "
            "removed in Python 3.16. This returns the bitwise inversion "
            "of the underlying int object and is usually not what you "
            "expect from negating a bool. Use the 'not' operator for "
            "boolean negation or ~int(x) if you really want the bitwise "
            "inversion of the underlying int."
        )

        stack: list[tuple[ast.AST, bool, bool]] = [(module_node, False, False)]
        while stack:
            node, in_finally, finally_checks_blocked = stack.pop()

            if (
                isinstance(node, ast.UnaryOp)
                and isinstance(node.op, ast.Invert)
                and isinstance(node.operand, ast.Constant)
                and isinstance(node.operand.value, bool)
            ):
                record_warning(
                    getattr(node, "lineno", 0),
                    "DeprecationWarning",
                    invert_bool_msg,
                )

            if in_finally and not finally_checks_blocked:
                warn_msg = None
                if isinstance(node, ast.Return):
                    warn_msg = "'return' in a 'finally' block"
                elif isinstance(node, ast.Break):
                    warn_msg = "'break' in a 'finally' block"
                elif isinstance(node, ast.Continue):
                    warn_msg = "'continue' in a 'finally' block"
                if warn_msg is not None:
                    record_warning(
                        getattr(node, "lineno", 0),
                        "SyntaxWarning",
                        warn_msg,
                    )

            child_finally_checks_blocked = finally_checks_blocked or isinstance(
                node, scope_barriers
            )
            child_entries: list[tuple[ast.AST, bool, bool]] = []
            if isinstance(node, ast.Try):
                for field_name, value in ast.iter_fields(node):
                    if isinstance(value, list):
                        children = [item for item in value if isinstance(item, ast.AST)]
                    elif isinstance(value, ast.AST):
                        children = [value]
                    else:
                        continue
                    child_in_finally = in_finally or field_name == "finalbody"
                    for child in children:
                        child_entries.append(
                            (
                                child,
                                child_in_finally,
                                child_finally_checks_blocked,
                            )
                        )
            else:
                for child in ast.iter_child_nodes(node):
                    child_entries.append(
                        (
                            child,
                            in_finally,
                            child_finally_checks_blocked,
                        )
                    )
            stack.extend(reversed(child_entries))

    def _emit_deferred_warnings(self) -> None:
        """Emit deferred runtime warnings as WARN_STDERR ops.

        Called at the start of module compilation so warnings appear before
        any print output, matching CPython's behavior of emitting compile-time
        warnings before executing any code.
        """
        for line in self._deferred_runtime_warnings:
            val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[line], result=val))
            self.emit(MoltOp(kind="WARN_STDERR", args=[val], result=MoltValue("none")))
        self._deferred_runtime_warnings.clear()

    def _emit_syntax_warning(self, node: ast.AST, message: str) -> None:
        """Emit a SyntaxWarning to stderr, matching CPython's format.

        Deduplicated: each (file, line, message) triple is emitted at most
        once per process, matching CPython's behaviour.
        """
        import warnings

        lineno = getattr(node, "lineno", 0)
        source = self.source_path or "<string>"
        key = (source, lineno, message)
        if key in self._emitted_syntax_warnings:
            return
        self._emitted_syntax_warnings.add(key)
        warnings.warn_explicit(
            message,
            SyntaxWarning,
            source,
            lineno,
        )

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

    def _emit_asyncio_sleep(
        self, args: list[ast.expr], keywords: list[ast.keyword]
    ) -> MoltValue:
        delay_expr: ast.expr | None = None
        result_expr: ast.expr | None = None
        if len(args) > 2:
            raise NotImplementedError("asyncio.sleep expects 0-2 arguments")
        if args:
            delay_expr = args[0]
            if len(args) == 2:
                result_expr = args[1]
        for keyword in keywords:
            if keyword.arg is None:
                raise NotImplementedError("asyncio.sleep does not support **kwargs")
            if keyword.arg == "delay":
                if delay_expr is not None:
                    raise NotImplementedError(
                        "asyncio.sleep got multiple values for delay"
                    )
                delay_expr = keyword.value
            elif keyword.arg == "result":
                if result_expr is not None:
                    raise NotImplementedError(
                        "asyncio.sleep got multiple values for result"
                    )
                result_expr = keyword.value
            else:
                raise NotImplementedError(
                    f"asyncio.sleep got unexpected keyword {keyword.arg}"
                )
        if delay_expr is None:
            delay_val = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=delay_val))
        else:
            delay_val = self.visit(delay_expr)
            if delay_val is None:
                raise NotImplementedError("Unsupported delay in asyncio.sleep")
        call_args = [delay_val]
        if result_expr is not None:
            result_val = self.visit(result_expr)
            if result_val is None:
                raise NotImplementedError("Unsupported result in asyncio.sleep")
            call_args.append(result_val)
        res = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="CALL_ASYNC",
                args=["molt_async_sleep_poll", *call_args],
                result=res,
            )
        )
        return res

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
    def _function_contains_yield(
        node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        def push_arg_annotations(stack: list[ast.AST], args: ast.arguments) -> None:
            for arg in (
                args.posonlyargs
                + args.args
                + args.kwonlyargs
                + ([] if args.vararg is None else [args.vararg])
                + ([] if args.kwarg is None else [args.kwarg])
            ):
                if arg.annotation is not None:
                    stack.append(arg.annotation)

        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.Yield, ast.YieldFrom)):
                return True
            if isinstance(current, (ast.FunctionDef, ast.AsyncFunctionDef)):
                stack.extend(current.decorator_list)
                stack.extend(current.args.defaults)
                stack.extend(
                    default
                    for default in current.args.kw_defaults
                    if default is not None
                )
                push_arg_annotations(stack, current.args)
                if current.returns is not None:
                    stack.append(current.returns)
                continue
            if isinstance(current, ast.ClassDef):
                stack.extend(current.decorator_list)
                stack.extend(current.bases)
                stack.extend(keyword.value for keyword in current.keywords)
                continue
            if isinstance(current, ast.Lambda):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

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
    def _async_generator_contains_yield_from(node: ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.YieldFrom):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _async_generator_contains_return_value(node: ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.Return) and current.value is not None:
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

    def _signature_contains_yield(
        self,
        *,
        decorators: list[ast.expr],
        args: ast.arguments,
        returns: ast.expr | None,
    ) -> bool:
        exprs: list[ast.expr] = list(decorators)
        exprs.extend(args.defaults)
        exprs.extend(expr for expr in args.kw_defaults if expr is not None)
        for arg in (
            args.posonlyargs
            + args.args
            + args.kwonlyargs
            + ([] if args.vararg is None else [args.vararg])
            + ([] if args.kwarg is None else [args.kwarg])
        ):
            if arg.annotation is not None:
                exprs.append(arg.annotation)
        if returns is not None:
            exprs.append(returns)
        return any(self._expr_contains_yield(expr) for expr in exprs)

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

    def _function_symbol(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None and self.current_func_name == "molt_main":
            self.func_symbol_names[reserved] = name
            self._register_code_symbol(reserved)
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while symbol in self.funcs_map or f"{symbol}_poll" in self.funcs_map:
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        while (
            symbol in self.func_symbol_names
            or symbol in self.reserved_func_symbols.values()
            or symbol in self.reserved_external_func_symbols
            or f"{symbol}_poll" in self.funcs_map
        ):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.func_symbol_names[symbol] = name
        self._register_code_symbol(symbol)
        return symbol

    def _reserve_function_symbol(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while (
            symbol in self.funcs_map
            or f"{symbol}_poll" in self.funcs_map
            or symbol in self.func_symbol_names
            or symbol in self.reserved_func_symbols.values()
            or symbol in self.reserved_external_func_symbols
        ):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.reserved_func_symbols[name] = symbol
        self.func_symbol_names[symbol] = name
        self._register_code_symbol(symbol)
        return symbol

    def _lambda_symbol(self) -> str:
        self.lambda_counter += 1
        symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        while symbol in self.funcs_map:
            self.lambda_counter += 1
            symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        self.func_symbol_names[symbol] = "<lambda>"
        self._register_code_symbol(symbol)
        return symbol

    def _genexpr_symbol(self) -> str:
        self.genexpr_counter += 1
        symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        while symbol in self.funcs_map:
            self.genexpr_counter += 1
            symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        self.func_symbol_names[symbol] = "<genexpr>"
        self._register_code_symbol(symbol)
        return symbol

    def _register_code_symbol(self, symbol: str) -> int:
        code_id = self.func_code_ids.get(symbol)
        if code_id is None:
            code_id = self.code_id_counter
            self.func_code_ids[symbol] = code_id
            self.code_id_counter += 1
        return code_id

    def _code_symbol_for_value(self, func_val: MoltValue) -> str | None:
        hint = func_val.type_hint
        if isinstance(hint, str):
            if hint.startswith("Func:") or hint.startswith("ClosureFunc:"):
                return hint.split(":", 1)[1]
        return None

    def _qualname_prefix(self) -> str:
        if not self.qualname_stack:
            return ""
        parts: list[str] = []
        for name, is_function in self.qualname_stack:
            parts.append(name)
            if is_function:
                parts.append("<locals>")
        return ".".join(parts)

    def _qualname_for_def(self, name: str) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return name
        return f"{prefix}.{name}"

    def _push_qualname(self, name: str, is_function: bool) -> None:
        self.qualname_stack.append((name, is_function))

    def _pop_qualname(self) -> None:
        if self.qualname_stack:
            self.qualname_stack.pop()

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
        # See bench/results/fib_regression_analysis.md for details.

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

    def _collect_module_annotation_items(
        self, node: ast.Module
    ) -> tuple[list[tuple[str, ast.expr, int]], dict[int, int]]:
        items: list[tuple[str, ast.expr, int]] = []
        id_map: dict[int, int] = {}
        outer = self

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_If(self, node: ast.If) -> None:
                # CPython does not record annotations from a statically-dead
                # branch (`if False:`/`if TYPE_CHECKING:`) in `__annotations__`.
                static_branch = outer._static_if_live_branch(node)
                if static_branch is not None:
                    for stmt in static_branch:
                        self.visit(stmt)
                    return None
                self.generic_visit(node)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    exec_id = len(items)
                    items.append((node.target.id, node.annotation, exec_id))
                    id_map[id(node)] = exec_id

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return items, id_map

    def _collect_global_rebinds(self, node: ast.AST) -> set[str]:
        names: set[str] = set()
        for current in ast.walk(node):
            if isinstance(current, ast.Global):
                names.update(current.names)
        return names

    def _collect_module_assignments(
        self, node: ast.Module
    ) -> tuple[dict[str, int], set[str], bool]:
        counts: dict[str, int] = {}
        func_defs: set[str] = set()
        has_dynamic_bind = False
        outer = self

        def record(name: str) -> None:
            counts[name] = counts.get(name, 0) + 1

        def record_target(target: ast.AST) -> None:
            if isinstance(target, ast.Name):
                record(target.id)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt)
            elif isinstance(target, ast.Starred):
                record_target(target.value)

        def record_pattern(pattern: ast.pattern) -> None:
            if isinstance(pattern, ast.MatchAs):
                if pattern.name and pattern.name != "_":
                    record(pattern.name)
                if pattern.pattern is not None:
                    record_pattern(pattern.pattern)
            elif isinstance(pattern, ast.MatchStar):
                if pattern.name and pattern.name != "_":
                    record(pattern.name)
            elif isinstance(pattern, ast.MatchMapping):
                for sub in pattern.patterns:
                    record_pattern(sub)
                if pattern.rest and pattern.rest != "_":
                    record(pattern.rest)
            elif isinstance(pattern, ast.MatchSequence):
                for sub in pattern.patterns:
                    record_pattern(sub)
            elif isinstance(pattern, ast.MatchClass):
                for sub in pattern.patterns:
                    record_pattern(sub)
                for sub in pattern.kwd_patterns:
                    record_pattern(sub)
            elif isinstance(pattern, ast.MatchOr):
                for sub in pattern.patterns:
                    record_pattern(sub)

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                return None

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                return None

            def visit_ClassDef(self, node: ast.ClassDef) -> Any:
                record(node.name)
                return None

            def visit_Lambda(self, node: ast.Lambda) -> Any:
                return None

            def visit_ListComp(self, node: ast.ListComp) -> Any:
                return None

            def visit_SetComp(self, node: ast.SetComp) -> Any:
                return None

            def visit_DictComp(self, node: ast.DictComp) -> Any:
                return None

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
                return None

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                record_target(node.target)
                self.visit(node.iter)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                record_target(node.target)
                self.visit(node.iter)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_While(self, node: ast.While) -> None:
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_If(self, node: ast.If) -> None:
                static_branch = outer._static_if_live_branch(node)
                if static_branch is not None:
                    for stmt in static_branch:
                        self.visit(stmt)
                    return None
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    self.visit(item.context_expr)
                    if item.optional_vars is not None:
                        record_target(item.optional_vars)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    self.visit(item.context_expr)
                    if item.optional_vars is not None:
                        record_target(item.optional_vars)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_Try(self, node: ast.Try) -> None:
                for stmt in node.body:
                    self.visit(stmt)
                for handler in node.handlers:
                    self.visit(handler)
                for stmt in node.orelse:
                    self.visit(stmt)
                for stmt in node.finalbody:
                    self.visit(stmt)

            def visit_TryStar(self, node: ast.TryStar) -> None:
                for stmt in node.body:
                    self.visit(stmt)
                for handler in node.handlers:
                    self.visit(handler)
                for stmt in node.orelse:
                    self.visit(stmt)
                for stmt in node.finalbody:
                    self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    record(node.name)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    record_pattern(case.pattern)
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_Import(self, node: ast.Import) -> None:
                for alias in node.names:
                    name = alias.asname or alias.name.split(".", 1)[0]
                    record(name)

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                nonlocal has_dynamic_bind
                for alias in node.names:
                    if alias.name == "*":
                        has_dynamic_bind = True
                        continue
                    name = alias.asname or alias.name
                    record(name)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target)

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return counts, func_defs, has_dynamic_bind

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
    # _populate_sema_state.  (The per-construct helpers _function_contains_yield /
    # _function_param_names / _split_function_args / _default_specs_from_args /
    # _default_spec_for_expr STAY on the class — the lowering walk still calls them
    # at ~30 sites; sema/funcmeta.py carries its own private copies.)

    def _collect_module_class_mutations(self, node: ast.Module) -> set[str]:
        class_names = {
            stmt.name for stmt in node.body if isinstance(stmt, ast.ClassDef)
        }
        if not class_names:
            return set()
        mutated: set[str] = set()

        def record_target(target: ast.AST) -> None:
            if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name):
                if target.value.id in class_names:
                    mutated.add(target.value.id)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt)
            elif isinstance(target, ast.Starred):
                record_target(target.value)

        class Collector(ast.NodeVisitor):
            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target)

            def visit_Call(self, node: ast.Call) -> None:
                if (
                    isinstance(node.func, ast.Name)
                    and node.func.id in {"setattr", "delattr"}
                    and node.args
                ):
                    target = node.args[0]
                    if isinstance(target, ast.Name) and target.id in class_names:
                        mutated.add(target.id)
                self.generic_visit(node)

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return mutated

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

    def is_async(self) -> bool:
        return self.current_func_name.endswith("_poll")

    def is_async_context(self) -> bool:
        return self.async_context

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

    def _async_local_offset(self, name: str) -> int:
        if name not in self.async_locals:
            self.async_locals[name] = (
                self.async_locals_base + len(self.async_locals) * 8
            )
            if self.async_public_locals:
                if name not in self.async_public_locals:
                    self.async_internal_locals.add(name)
                else:
                    self.async_internal_locals.discard(name)
            else:
                self.async_internal_locals.add(name)
        return self.async_locals[name]

    def _async_locals_public_entries(self) -> list[tuple[str, int]]:
        if not self.async_locals:
            return []
        entries = [
            (name, offset)
            for name, offset in self.async_locals.items()
            if name not in self.async_internal_locals
        ]
        entries.sort(key=lambda item: item[1])
        return entries

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

    def _init_scope_async_locals(self, arg_nodes: list[ast.arg]) -> None:
        if not self.scope_assigned:
            return
        arg_names = {arg.arg for arg in arg_nodes}
        missing_val: MoltValue | None = None
        for name in sorted(self.scope_assigned):
            if (
                name in arg_names
                or name in self.global_decls
                or name in self.nonlocal_decls
            ):
                continue
            if name in self.async_locals:
                continue
            if missing_val is None:
                missing_val = self._emit_missing_value()
            offset = self._async_local_offset(name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, missing_val],
                    result=MoltValue("none"),
                )
            )

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

    def _collect_annotation_free_vars(self, node: ast.AST) -> list[str]:
        if self.current_func_name == "molt_main":
            return []
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> None:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node)
        used -= self.global_decls
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in used if name in outer_scope)

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
                self._expr_contains_yield(expr) for expr in default_exprs
            )
            yield_in_kwdefaults = any(
                self._expr_contains_yield(expr)
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
    def _normalize_func_kind(kind: object) -> str | None:
        if kind == "async_gen":
            return "asyncgen"
        if kind in {"sync", "async", "gen", "asyncgen"}:
            return cast(str, kind)
        return None

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
        kind = kind or "sync"
        func_symbol = f"{self._sanitize_module_name(module_name)}__{func_id}"
        if kind == "sync":
            return f"Func:{func_symbol}"
        total_params = info.get("params") if info is not None else None
        payload_slots = total_params if isinstance(total_params, int) else 0
        if kind == "gen":
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            return f"GenFunc:{func_symbol}_poll:{closure_size}"
        if kind == "async":
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=False
            )
            return f"AsyncFunc:{func_symbol}_poll:{closure_size}"
        if kind == "asyncgen":
            closure_size = self._task_closure_size(
                payload_slots, include_gen_control=True
            )
            return f"AsyncGenFunc:{func_symbol}_poll:{closure_size}"
        raise ValueError(
            f"unsupported function kind for {module_name}.{func_id}: {kind!r}"
        )

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

    @staticmethod
    def _match_optional_intrinsic_loader_expr(expr: ast.AST) -> str | None:
        if not isinstance(expr, ast.Call) or expr.keywords or len(expr.args) != 1:
            return None
        if (
            not isinstance(expr.func, ast.Name)
            or expr.func.id != "_load_optional_intrinsic"
        ):
            return None
        arg = expr.args[0]
        if not isinstance(arg, ast.Constant) or not isinstance(arg.value, str):
            return None
        return arg.value

    def _collect_module_optional_intrinsic_globals(
        self, node: ast.Module
    ) -> dict[str, str]:
        bindings: dict[str, str] = {}

        def clear_name(name: str) -> None:
            bindings.pop(name, None)

        def assigned_names(target: ast.AST) -> list[str]:
            if isinstance(target, ast.Name):
                return [target.id]
            if isinstance(target, (ast.Tuple, ast.List)):
                names: list[str] = []
                for elt in target.elts:
                    names.extend(assigned_names(elt))
                return names
            return []

        for stmt in node.body:
            if isinstance(stmt, ast.Assign):
                runtime_name = self._match_optional_intrinsic_loader_expr(stmt.value)
                for target in stmt.targets:
                    for name in assigned_names(target):
                        if runtime_name is None:
                            clear_name(name)
                        else:
                            bindings[name] = _canonical_intrinsic_runtime_name(
                                runtime_name
                            )
                continue
            if isinstance(stmt, ast.AnnAssign):
                for name in assigned_names(stmt.target):
                    if stmt.value is None:
                        continue
                    runtime_name = self._match_optional_intrinsic_loader_expr(
                        stmt.value
                    )
                    if runtime_name is None:
                        clear_name(name)
                    else:
                        bindings[name] = _canonical_intrinsic_runtime_name(runtime_name)
                continue
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                clear_name(stmt.name)
                continue
            if isinstance(stmt, ast.Import):
                for alias in stmt.names:
                    clear_name(alias.asname or alias.name.split(".")[0])
                continue
            if isinstance(stmt, ast.ImportFrom):
                for alias in stmt.names:
                    if alias.name != "*":
                        clear_name(alias.asname or alias.name)
                continue
            if isinstance(stmt, (ast.For, ast.AsyncFor)):
                for name in assigned_names(stmt.target):
                    clear_name(name)
                continue
            if isinstance(stmt, ast.With):
                for item in stmt.items:
                    if item.optional_vars is not None:
                        for name in assigned_names(item.optional_vars):
                            clear_name(name)
                continue
        return bindings

    def _emit_builtin_type_value(self, type_name: str) -> MoltValue:
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[BUILTIN_TYPE_TAGS[type_name]], result=tag_val)
        )
        res = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=res))
        return res

    def _emit_name_from_obj(self, obj: MoltValue) -> MoltValue:
        name_key = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__name__"], result=name_key))
        missing = self._emit_missing_value()
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(
                kind="GETATTR_NAME_DEFAULT",
                args=[obj, name_key, missing],
                result=name_val,
            )
        )
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[name_val, missing], result=is_missing))
        # Async/poll-function bodies must thread the result through a closure
        # slot rather than a LIST_NEW + STORE_INDEX cell. The cell pattern is
        # unsafe under Cranelift's loop-header phi resolver: the cell SSA
        # value can be merged with the entry-block default (None) on the
        # first iteration, producing store_index(None, ...) crashes.
        if self.is_async():
            slot = self._async_local_offset(f"__name_from_obj_{len(self.async_locals)}")
            placeholder = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=placeholder))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, placeholder],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
            fallback = self._emit_str_from_obj(obj)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, fallback],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", slot, name_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", slot], result=res))
            return res

        # Sync path: a single SSA value updated in both branches replaces the
        # LIST_NEW + STORE_INDEX cell.
        res = MoltValue(self.next_var(), type_hint="str")
        placeholder = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=placeholder))
        self.emit(MoltOp(kind="COPY", args=[placeholder], result=res))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        fallback = self._emit_str_from_obj(obj)
        self.emit(MoltOp(kind="COPY", args=[fallback], result=res))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="COPY", args=[name_val], result=res))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_type_name(self, value: MoltValue) -> MoltValue:
        type_val = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="TYPE_OF", args=[value], result=type_val))
        return self._emit_name_from_obj(type_val)




    def _box_local(self, name: str) -> None:
        if name in self.global_decls:
            return
        if name in self.boxed_locals:
            return
        if name in self.free_vars:
            cell = self._load_free_var_cell(name)
            if cell is None:
                return
            self.boxed_locals[name] = cell
            hint = self.free_var_hints.get(name)
            self.boxed_local_hints[name] = hint or "Any"
            self.locals[name] = cell
            return
        init: MoltValue
        if self.is_async() and name in self.async_locals:
            init = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_locals[name]],
                    result=init,
                )
            )
        elif name in self.locals:
            init = self.locals[name]
        else:
            if name in self.scope_assigned or name in self.del_targets:
                init = self._emit_missing_value()
            else:
                init = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=cell))
        self.boxed_locals[name] = cell
        if init.type_hint:
            self.boxed_local_hints[name] = init.type_hint
        else:
            self.boxed_local_hints[name] = "Unknown"
        self.locals[name] = cell
        if self.is_async():
            offset = self._async_local_offset(name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, cell],
                    result=MoltValue("none"),
                )
            )

    def _load_boxed_cell(self, name: str) -> MoltValue | None:
        cell = self.boxed_locals.get(name)
        if cell is None:
            return None
        if not self.is_async():
            return cell
        if name not in self.async_locals:
            return cell
        slot_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", self.async_locals[name]],
                result=slot_val,
            )
        )
        return slot_val

    def _closure_cells_for(self, names: Sequence[str]) -> list[MoltValue]:
        items: list[MoltValue] = []
        for name in names:
            cell = self._load_boxed_cell(name)
            if cell is None:
                cell = self.boxed_locals[name]
            items.append(cell)
        return items

    def _collect_pattern_capture_names(self, pattern: ast.pattern) -> list[str]:
        # Source order, deduplicated.  A set leaked PYTHONHASHSEED ordering into
        # emitted IR because _collect_assigned_names_ordered feeds these capture
        # names positionally into the function's co_varnames tuple (#34,
        # match-capture class).  Callers that need set semantics (e.g. MatchOr
        # binding-equality) wrap in set(...).
        names: list[str] = []
        seen: set[str] = set()

        def add(name: str) -> None:
            if name not in seen:
                seen.add(name)
                names.append(name)

        def visit(current: ast.pattern) -> None:
            if isinstance(current, ast.MatchAs):
                if current.name and current.name != "_":
                    add(current.name)
                if current.pattern is not None:
                    visit(current.pattern)
                return
            if isinstance(current, ast.MatchStar):
                if current.name and current.name != "_":
                    add(current.name)
                return
            if isinstance(current, ast.MatchMapping):
                for sub in current.patterns:
                    visit(sub)
                if current.rest and current.rest != "_":
                    add(current.rest)
                return
            if isinstance(current, ast.MatchSequence):
                for sub in current.patterns:
                    visit(sub)
                return
            if isinstance(current, ast.MatchClass):
                for sub in current.patterns:
                    visit(sub)
                for sub in current.kwd_patterns:
                    visit(sub)
                return
            if isinstance(current, ast.MatchOr):
                for sub in current.patterns:
                    visit(sub)
                return

        visit(pattern)
        return names

    def _collect_assigned_names(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self.names.update(outer._collect_target_names(node.target))
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self.names.update(
                            outer._collect_target_names(item.optional_vars)
                        )
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self.names.update(
                            outer._collect_target_names(item.optional_vars)
                        )
                self.generic_visit(node)

            def visit_If(self, node: ast.If) -> None:
                # Binding analysis mirrors CPython's symbol table, which records
                # every assignment target regardless of static reachability: a
                # name bound only in a statically-dead branch (`if 0: x = 1`) is
                # still a local of the enclosing scope, so reading it raises
                # UnboundLocalError, not NameError. The static-if fold is a
                # codegen/emission concern (drop dead-branch *code* and its
                # const_str/intrinsic refs), handled in the emission `visit_If`
                # via `_emit_static_if_live_branch`; pruning scope bindings here
                # would diverge from CPython and is intentionally NOT done.
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    self.names.update(
                        outer._collect_pattern_capture_names(case.pattern)
                    )
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    self.names.add(node.name)
                self.generic_visit(node)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self.names.add(node.name)
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self.names.add(node.name)
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self.names.add(node.name)
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_assigned_names_ordered(self, nodes: list[ast.stmt]) -> list[str]:
        outer = self

        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: list[str] = []
                self.seen: set[str] = set()

            def _add(self, name: str) -> None:
                if name not in self.seen:
                    self.seen.add(name)
                    self.names.append(name)

            def _add_targets(self, target: ast.AST) -> None:
                for name in outer._collect_target_names(target):
                    self._add(name)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self._add_targets(target)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self._add_targets(node.target)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self._add_targets(node.target)
                self.generic_visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self._add_targets(node.target)
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self._add_targets(node.target)
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._add_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._add_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_If(self, node: ast.If) -> None:
                # Mirror CPython's symbol table (see `_collect_assigned_names`):
                # a name bound only in a statically-dead branch is still a local,
                # so this binding walk does NOT apply the static-if fold. The
                # fold is emission-only (`_emit_static_if_live_branch`).
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    for name in outer._collect_pattern_capture_names(case.pattern):
                        self._add(name)
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    self._add(node.name)
                self.generic_visit(node)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self._add_targets(target)

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name):
                    self._add(node.target.id)
                self.generic_visit(node.value)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._add(node.name)
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._add(node.name)
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self._add(node.name)
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _varnames_from_params(
        self,
        *,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
    ) -> list[str]:
        names: list[str] = []
        names.extend(posonly_params)
        names.extend(pos_or_kw_params)
        names.extend(kwonly_params)
        if vararg is not None:
            names.append(vararg)
        if varkw is not None:
            names.append(varkw)
        return names

    def _collect_varnames_for_body(
        self,
        *,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
        body: list[ast.stmt],
    ) -> list[str]:
        params = self._varnames_from_params(
            posonly_params=posonly_params,
            pos_or_kw_params=pos_or_kw_params,
            kwonly_params=kwonly_params,
            vararg=vararg,
            varkw=varkw,
        )
        assigned = self._collect_assigned_names_ordered(body)
        global_decls = self._collect_global_decls(body)
        nonlocal_decls = self._collect_nonlocal_decls(body)
        locals_only: list[str] = []
        for name in assigned:
            if (
                name in params
                or name in global_decls
                or name in nonlocal_decls
                or name in locals_only
            ):
                continue
            locals_only.append(name)
        return params + locals_only

    def _collect_code_names_for_body(
        self,
        nodes: Sequence[ast.AST],
        *,
        varnames: Sequence[str],
        free_vars: Sequence[str],
        module_scope: bool = False,
    ) -> list[str]:
        """Collect the ordered name table backing ``code.co_names``.

        The table is a runtime introspection fact, not an execution fallback:
        it mirrors the names referenced by bytecode-style name operations for
        the current code object while leaving nested code objects to describe
        their own bodies.
        """

        local_names = set(varnames)
        free_var_names = set(free_vars)
        stmt_nodes = [node for node in nodes if isinstance(node, ast.stmt)]
        global_decls = self._collect_global_decls(stmt_nodes)
        nonlocal_decls = self._collect_nonlocal_decls(stmt_nodes)
        names: list[str] = []
        seen: set[str] = set()

        def add(name: str | None) -> None:
            if not name or name in seen:
                return
            seen.add(name)
            names.append(name)

        def import_store_name(alias: ast.alias) -> str:
            if alias.asname:
                return alias.asname
            return alias.name.split(".", 1)[0]

        class CodeNamesCollector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> None:
                if module_scope:
                    add(node.id)
                    return
                if node.id in nonlocal_decls or node.id in free_var_names:
                    return
                if node.id in global_decls:
                    add(node.id)
                    return
                if isinstance(node.ctx, ast.Load) and node.id not in local_names:
                    add(node.id)

            def visit_Attribute(self, node: ast.Attribute) -> None:
                self.visit(node.value)
                add(node.attr)

            def visit_Import(self, node: ast.Import) -> None:
                for alias in node.names:
                    add(alias.name)
                    if "." in alias.name:
                        if module_scope:
                            add(import_store_name(alias))
                        else:
                            add(alias.name.rsplit(".", 1)[1])
                    elif module_scope:
                        add(import_store_name(alias))

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                add("." * int(node.level or 0) + (node.module or ""))
                for alias in node.names:
                    add(alias.name)
                    if module_scope and alias.asname:
                        add(alias.asname)

            def _visit_function_signature(
                self, node: ast.FunctionDef | ast.AsyncFunctionDef
            ) -> None:
                for deco in node.decorator_list:
                    self.visit(deco)
                for default in node.args.defaults:
                    self.visit(default)
                for default in node.args.kw_defaults:
                    if default is not None:
                        self.visit(default)
                for arg in (
                    list(node.args.posonlyargs)
                    + list(node.args.args)
                    + list(node.args.kwonlyargs)
                ):
                    if arg.annotation is not None:
                        self.visit(arg.annotation)
                if (
                    node.args.vararg is not None
                    and node.args.vararg.annotation is not None
                ):
                    self.visit(node.args.vararg.annotation)
                if (
                    node.args.kwarg is not None
                    and node.args.kwarg.annotation is not None
                ):
                    self.visit(node.args.kwarg.annotation)
                if node.returns is not None:
                    self.visit(node.returns)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._visit_function_signature(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._visit_function_signature(node)
                if module_scope or node.name in global_decls:
                    add(node.name)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                for default in node.args.defaults:
                    self.visit(default)
                for default in node.args.kw_defaults:
                    if default is not None:
                        self.visit(default)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                for deco in node.decorator_list:
                    self.visit(deco)
                for base in node.bases:
                    self.visit(base)
                for keyword in node.keywords:
                    self.visit(keyword.value)
                if module_scope or node.name in global_decls:
                    add(node.name)

        collector = CodeNamesCollector()
        for node in nodes:
            collector.visit(node)
        return names

    @staticmethod
    def _is_type_checking_test(expr: ast.expr) -> bool:
        if isinstance(expr, ast.Name):
            return expr.id == "TYPE_CHECKING"
        if isinstance(expr, ast.Attribute):
            if expr.attr != "TYPE_CHECKING":
                return False
            if isinstance(expr.value, ast.Name):
                return expr.value.id in {"typing", "typing_extensions"}
        return False

    @staticmethod
    def _static_test_truthiness(expr: ast.expr) -> bool | None:
        """Return the compile-time truth value of an `if`/`while` test, or None.

        CPython's compiler eliminates the dead branch of an `if` whose test is a
        compile-time constant (`if False:`, `if 0:`, `if "":`, `if True:`,
        `if None:`), so the dead branch never reaches bytecode — names assigned
        only there stay unbound and references inside it are never emitted. Molt
        must match this exactly: a `const_str` left inside a never-executed
        `if False:` body (e.g. the `__annotations__` keys of a
        `if False:  # TYPE_CHECKING` block) would otherwise leak into the
        per-app intrinsic manifest and pin runtime intrinsics that the program
        never resolves.

        `TYPE_CHECKING` is always statically False here: Molt compiles code, it
        never runs a type checker, so a `if TYPE_CHECKING:` guard's body is dead
        exactly like `if False:`. Returning False for it unifies the existing
        TYPE_CHECKING-skip with general constant folding (one code path, not two).

        Returns None when the test is not a compile-time constant — the caller
        must then emit both branches under a runtime guard.
        """
        if SimpleTIRGenerator._is_type_checking_test(expr):
            return False
        if isinstance(expr, ast.Constant):
            # Mirror CPython's constant folding: any literal test value collapses
            # to its truthiness (None/bool/int/float/str/bytes/tuple-of-consts).
            return bool(expr.value)
        return None

    @staticmethod
    def _static_if_live_branch(node: ast.If) -> list[ast.stmt] | None:
        """Statically-live branch of `node` when its test is constant, else None.

        Constant-true selects `node.body`; constant-false (including
        `TYPE_CHECKING`) selects `node.orelse`. None means the test is
        runtime-conditional and both branches may execute.
        """
        truth = SimpleTIRGenerator._static_test_truthiness(node.test)
        if truth is None:
            return None
        return node.body if truth else node.orelse

    def _collect_namedexpr_names(self, node: ast.AST) -> list[str]:
        # Source order, deduplicated.  Walrus (:=) targets are synced to the
        # enclosing scope by iterating this result and emitting INDEX / module-
        # attr-set ops per name (see _collect_inline_comp_walrus_names callers),
        # so a set leaked PYTHONHASHSEED order into the emitted IR (#34,
        # walrus-target class).  Set-semantics consumers wrap in set(...).
        names: list[str] = []
        seen: set[str] = set()

        class NamedExprCollector(ast.NodeVisitor):
            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name) and node.target.id not in seen:
                    seen.add(node.target.id)
                    names.append(node.target.id)
                self.generic_visit(node.value)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        NamedExprCollector().visit(node)
        return names

    def _collect_deleted_names(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        class DeleteCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                # `except E as e:` implicitly `del e` at handler exit (CPython
                # unconditionally deletes the target even when the handler body
                # raises). A subsequent read of `e` is therefore an unbound
                # name — NameError at module scope, UnboundLocalError in a
                # function — so the target must be tracked alongside explicit
                # `del` names to route post-block reads through the correct
                # unbound-name path rather than an attribute access.
                if node.name:
                    self.names.add(node.name)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = DeleteCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_free_vars(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names(node.body)
        comp_targets = self._collect_comprehension_target_names(node.body)
        global_decls = self._collect_global_decls(node.body)
        nonlocal_decls = self._collect_nonlocal_decls(node.body)
        local_names = params | comp_targets | (assigned - nonlocal_decls)
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        used.update(nonlocal_decls)
        used.update(self._collect_nested_free_vars(node.body))
        # Implicit ``__class__`` closure variable: a method/nested function
        # that references zero-arg ``super()`` or ``__class__`` closes over the
        # enclosing class's ``__class__`` cell exactly as CPython does.  The
        # cell lives in ``self.boxed_locals['__class__']`` (pre-created by
        # visit_ClassDef), so adding ``__class__`` here threads it through the
        # closure and lets ``super()``/``__class__`` read the finished class
        # object from the cell rather than re-deriving it by module name.
        if self._active_classcell_cell is not None and self._function_needs_classcell(
            node
        ):
            used.add("__class__")
        candidates = {
            name
            for name in used
            if name not in local_names and name not in global_decls
        }
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars_expr(self, node: ast.Lambda) -> list[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
        comp_targets = self._collect_comprehension_target_names([node.body])
        local_names = params | comp_targets | assigned
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node.body)
        used.update(self._collect_nested_free_vars([node.body]))
        candidates = {name for name in used if name not in local_names}
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars_raw(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> set[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names(node.body)
        comp_targets = self._collect_comprehension_target_names(node.body)
        global_decls = self._collect_global_decls(node.body)
        nonlocal_decls = self._collect_nonlocal_decls(node.body)
        local_names = params | comp_targets | (assigned - nonlocal_decls)
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        used.update(nonlocal_decls)
        used.update(self._collect_nested_free_vars_raw(node.body))
        return {
            name
            for name in used
            if name not in local_names and name not in global_decls
        }

    def _collect_free_vars_expr_raw(self, node: ast.Lambda) -> set[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
        comp_targets = self._collect_comprehension_target_names([node.body])
        local_names = params | comp_targets | assigned
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node.body)
        used.update(self._collect_nested_free_vars_raw([node.body]))
        return {name for name in used if name not in local_names}

    def _collect_free_vars_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
        target_names: set[str] = set()
        exprs: list[ast.expr] = []
        for comp in node.generators:
            target_names.update(self._collect_target_names(comp.target))
            exprs.append(comp.iter)
            exprs.extend(comp.ifs)
        if isinstance(node, ast.DictComp):
            exprs.append(node.key)
            exprs.append(node.value)
        else:
            exprs.append(node.elt)
        namedexpr_targets: set[str] = set()
        for expr in exprs:
            namedexpr_targets |= set(self._collect_namedexpr_names(expr))
        assigned = self._collect_assigned_names(
            [ast.Expr(value=expr) for expr in exprs]
        )
        local_names = target_names | assigned
        used: set[str] = set()
        # Capture the method's first param name so we can detect implicit
        # super() references inside comprehensions.
        _method_first_param = self.current_method_first_param
        _current_class = self.current_class

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)
                    if (
                        node.id == "super"
                        and _method_first_param is not None
                        and _current_class is not None
                    ):
                        used.add(_method_first_param)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for expr in exprs:
            collector.visit(expr)
        used |= namedexpr_targets
        used.update(self._collect_nested_free_vars(exprs))
        candidates = {name for name in used if name not in local_names}
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_nested_free_vars(self, nodes: Sequence[ast.AST]) -> set[str]:
        nested: set[str] = set()
        outer = self

        class NestedCollector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested.update(outer._collect_free_vars(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested.update(outer._collect_free_vars(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested.update(outer._collect_free_vars_expr(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = NestedCollector()
        for node in nodes:
            collector.visit(node)
        return nested

    def _collect_nested_free_vars_raw(self, nodes: Sequence[ast.AST]) -> set[str]:
        nested: set[str] = set()
        outer = self

        class NestedCollector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested.update(outer._collect_free_vars_raw(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested.update(outer._collect_free_vars_raw(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested.update(outer._collect_free_vars_expr_raw(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = NestedCollector()
        for node in nodes:
            collector.visit(node)
        return nested

    def _collect_comprehension_cell_vars(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
        target_names: set[str] = set()
        exprs: list[ast.expr] = []
        for comp in node.generators:
            target_names.update(self._collect_target_names(comp.target))
            exprs.append(comp.iter)
            exprs.extend(comp.ifs)
        if isinstance(node, ast.DictComp):
            exprs.append(node.key)
            exprs.append(node.value)
        else:
            exprs.append(node.elt)
        nested_free: set[str] = set()
        outer = self

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested_free.update(outer._collect_free_vars_raw(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested_free.update(outer._collect_free_vars_raw(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested_free.update(outer._collect_free_vars_expr_raw(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = Collector()
        for expr in exprs:
            collector.visit(expr)
        return sorted(name for name in nested_free if name in target_names)

    def _collect_comprehension_target_names(self, nodes: Sequence[ast.AST]) -> set[str]:
        names: set[str] = set()
        outer = self

        class Collector(ast.NodeVisitor):
            def visit_ListComp(self, node: ast.ListComp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_SetComp(self, node: ast.SetComp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_DictComp(self, node: ast.DictComp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                for comp in node.generators:
                    names.update(outer._collect_target_names(comp.target))
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for node in nodes:
            collector.visit(node)
        return names

    def _collect_namedexpr_targets_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> set[str]:
        target_names: set[str] = set()
        exprs: list[ast.expr] = []
        for comp in node.generators:
            target_names.update(self._collect_target_names(comp.target))
            exprs.append(comp.iter)
            exprs.extend(comp.ifs)
        if isinstance(node, ast.DictComp):
            exprs.append(node.key)
            exprs.append(node.value)
        else:
            exprs.append(node.elt)
        names: set[str] = set()
        for expr in exprs:
            names |= set(self._collect_namedexpr_names(expr))
        names -= target_names
        return names

    def _collect_scope_cell_vars(
        self, body: Sequence[ast.stmt], local_candidates: set[str]
    ) -> set[str]:
        if not local_candidates:
            return set()
        captured: set[str] = set()
        outer = self

        class Collector(ast.NodeVisitor):
            def _record(self, names: Iterable[str]) -> None:
                for name in names:
                    if name in local_candidates:
                        captured.add(name)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self._record(outer._collect_free_vars_raw(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self._record(outer._collect_free_vars_raw(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                self._record(outer._collect_free_vars_expr_raw(node))
                return

            def visit_Call(self, node: ast.Call) -> None:
                if (
                    isinstance(node.func, ast.Name)
                    and len(node.args) == 1
                    and not node.keywords
                    and isinstance(node.args[0], ast.GeneratorExp)
                    and (
                        (
                            node.func.id == "sum"
                            and outer._can_inline_sum_genexpr(node.args[0])
                        )
                        or (
                            node.func.id in {"any", "all"}
                            and outer._can_inline_any_all_genexpr(node.args[0])
                        )
                    )
                ):
                    genexpr = node.args[0]
                    for comp in genexpr.generators:
                        self.visit(comp.iter)
                        for if_node in comp.ifs:
                            self.visit(if_node)
                    self.visit(genexpr.elt)
                    return
                self.generic_visit(node)

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_ListComp(self, node: ast.ListComp) -> None:
                if not (
                    not outer._comprehension_requires_async(node.generators, [node.elt])
                    and outer._can_inline_list_comp(node)
                ):
                    self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_SetComp(self, node: ast.SetComp) -> None:
                if not (
                    not outer._comprehension_requires_async(node.generators, [node.elt])
                    and outer._can_inline_set_comp(node)
                ):
                    self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_DictComp(self, node: ast.DictComp) -> None:
                if not (
                    not outer._comprehension_requires_async(
                        node.generators, [node.key, node.value]
                    )
                    and outer._can_inline_dict_comp(node)
                ):
                    self._record(outer._collect_free_vars_comprehension(node))
                self.generic_visit(node)

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = Collector()
        for stmt in body:
            collector.visit(stmt)
        return captured

    def _collect_comp_walrus_shared_names(self, body: Sequence[ast.stmt]) -> list[str]:
        """Names that are a comprehension walrus (``:=``) target AND are also
        bound by a non-comprehension assignment in the same function scope.

        A walrus inside a comprehension leaks its binding to the enclosing
        function scope (PEP 572), but the inline-comprehension lowering stores
        that target through a boxed cell while a *separate* binding of the same
        name (a plain assignment, a ``while``/``if`` test walrus, a ``for``
        target, ...) is lowered as a plain SSA local.  When such a name lives
        across a loop back-edge the two representations diverge — the comp cell
        is never updated by the SSA writer and vice-versa — producing a stale
        post-loop value (e.g. ``while (n := next(it)) is not None: xs = [n := n
        + 1 for _ in r]`` leaving ``n`` at the last comp value instead of the
        loop-terminating ``None``).  Returning such names lets the caller box
        them at function entry so every binding site shares one cell.

        Names bound *only* by a comprehension walrus are excluded: their cell is
        the single source of truth (the post-comp sync mirrors it into the SSA
        local) and needs no unification.  Nested functions/classes are separate
        scopes and are not traversed.
        """

        outer = self

        comp_walrus: set[str] = set()
        non_comp_assigned: set[str] = set()

        class _Scan(ast.NodeVisitor):
            def __init__(self) -> None:
                self._in_comp_depth = 0

            def _record_assign_targets(self, target: ast.expr) -> None:
                for name in outer._collect_target_names(target):
                    non_comp_assigned.add(name)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self._record_assign_targets(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self._record_assign_targets(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self._record_assign_targets(node.target)
                self.visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self._record_assign_targets(node.target)
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self._record_assign_targets(node.target)
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._record_assign_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self._record_assign_targets(item.optional_vars)
                self.generic_visit(node)

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name):
                    if self._in_comp_depth > 0:
                        comp_walrus.add(node.target.id)
                    else:
                        non_comp_assigned.add(node.target.id)
                self.visit(node.value)

            def _visit_comprehension(
                self,
                node: ast.ListComp | ast.SetComp | ast.GeneratorExp | ast.DictComp,
                parts: Sequence[ast.expr],
            ) -> None:
                # The iterable of the *first* generator is evaluated in the
                # enclosing scope; everything else (element, filters, nested
                # generators) is comprehension-internal for walrus-leak purposes.
                # Every caller passes a comprehension node, all four of which
                # carry ``.generators``.
                generators = node.generators
                if generators:
                    self.visit(generators[0].iter)
                self._in_comp_depth += 1
                try:
                    for part in parts:
                        self.visit(part)
                    for idx, comp in enumerate(generators):
                        if idx != 0:
                            self.visit(comp.iter)
                        for if_node in comp.ifs:
                            self.visit(if_node)
                finally:
                    self._in_comp_depth -= 1

            def visit_ListComp(self, node: ast.ListComp) -> None:
                self._visit_comprehension(node, [node.elt])

            def visit_SetComp(self, node: ast.SetComp) -> None:
                self._visit_comprehension(node, [node.elt])

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> None:
                self._visit_comprehension(node, [node.elt])

            def visit_DictComp(self, node: ast.DictComp) -> None:
                self._visit_comprehension(node, [node.key, node.value])

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        scanner = _Scan()
        for stmt in body:
            scanner.visit(stmt)
        shared = comp_walrus & non_comp_assigned
        shared -= self.global_decls
        shared -= self.nonlocal_decls
        return sorted(shared)

    def _prebox_scope_cell_vars(
        self, *, body: Sequence[ast.stmt], arg_nodes: Sequence[ast.arg]
    ) -> None:
        local_candidates = set(self.scope_assigned)
        local_candidates.update(arg.arg for arg in arg_nodes)
        local_candidates -= self.global_decls
        local_candidates -= self.nonlocal_decls
        if not local_candidates:
            return
        for name in sorted(self._collect_scope_cell_vars(body, local_candidates)):
            self._box_local(name)
            self.closure_locals.add(name)
        # Unify storage for names bound by both a comprehension walrus and a
        # non-comprehension assignment: box them so the inline-comprehension
        # cell and the SSA-local writer share one cell across loop back-edges.
        for name in self._collect_comp_walrus_shared_names(body):
            self._box_local(name)

    def _emit_free_var_load(
        self, name: str, *, guard_unbound: bool = True
    ) -> MoltValue | None:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return None
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        hint = self.free_var_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=res))
        if guard_unbound:
            self._emit_unbound_free_guard(res, name)
        return res

    def _emit_free_var_store(self, name: str, value: MoltValue) -> bool:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return False
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, zero, value],
                result=MoltValue("none"),
            )
        )
        return True

    def _load_free_var_cell(self, name: str) -> MoltValue | None:
        closure = self.locals.get(_MOLT_CLOSURE_PARAM)
        if (
            closure is None
            and self.is_async()
            and self.async_closure_offset is not None
        ):
            closure = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_closure_offset],
                    result=closure,
                )
            )
        if closure is None:
            return None
        idx = self.free_vars.get(name)
        if idx is None:
            return None
        idx_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="INDEX", args=[closure, idx_val], result=cell))
        return cell

    def _collect_class_mutations(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        def record_target(target: ast.AST, names: set[str]) -> None:
            if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name):
                class_name = target.value.id
                if class_name in outer.classes:
                    names.add(class_name)
            elif isinstance(target, ast.Starred):
                record_target(target.value, names)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt, names)

        class ClassMutationCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target, self.names)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target, self.names)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target, self.names)
                self.generic_visit(node.value)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target, self.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = ClassMutationCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_loop_guard_candidates(self, body: list[ast.stmt]) -> dict[str, str]:
        if self.is_async():
            return {}
        assigned = self._collect_assigned_names(body)
        mutated_classes = self._collect_class_mutations(body)
        attr_names: set[str] = set()

        class AttrCollector(ast.NodeVisitor):
            def visit_Attribute(self, node: ast.Attribute) -> None:
                if isinstance(node.value, ast.Name) and isinstance(node.ctx, ast.Load):
                    attr_names.add(node.value.id)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AttrCollector()
        for stmt in body:
            collector.visit(stmt)
        candidates: dict[str, str] = {}
        for name in sorted(attr_names):
            if name in assigned:
                continue
            expected_class = self.exact_locals.get(name)
            if expected_class is None:
                continue
            if expected_class in mutated_classes:
                continue
            candidates[name] = expected_class
        return candidates

    def _collect_loop_static_class_candidates(self, body: list[ast.stmt]) -> list[str]:
        if (
            self.is_async()
            or self.current_func_name == "molt_main"
            or not self.stable_module_classes
        ):
            return []
        assigned = self._collect_assigned_names(body)
        assigned |= {
            name for stmt in body for name in self._collect_namedexpr_names(stmt)
        }
        candidates: set[str] = set()
        outer = self

        class ClassCallCollector(ast.NodeVisitor):
            def visit_Call(self, node: ast.Call) -> None:
                if isinstance(node.func, ast.Name):
                    class_name = node.func.id
                    if (
                        class_name in outer.stable_module_classes
                        and class_name not in assigned
                        and class_name not in outer.scope_assigned
                        and class_name not in outer.global_decls
                        and outer._class_layout_stable(class_name)
                    ):
                        candidates.add(class_name)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = ClassCallCollector()
        for stmt in body:
            collector.visit(stmt)
        return sorted(candidates)

    def _push_loop_static_class_refs(self, body: list[ast.stmt]) -> None:
        refs: dict[str, MoltValue] = {}
        eager_refs: set[str] = set()
        for class_name in self._collect_loop_static_class_candidates(body):
            self.loop_static_class_counter += 1
            slot = f"__molt_static_class_{self.loop_static_class_counter}_{class_name}"
            init = self._emit_module_attr_get(
                class_name, effect_proof=_STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF
            )
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[init],
                    result=MoltValue("none"),
                    metadata={"var": slot},
                )
            )
            refs[class_name] = MoltValue(slot, type_hint="type")
            eager_refs.add(class_name)
        self.loop_static_class_refs.append(refs)
        self.loop_static_class_eager_refs.append(eager_refs)

    def _pop_loop_static_class_refs(self) -> None:
        if self.loop_static_class_refs:
            self.loop_static_class_refs.pop()
        if self.loop_static_class_eager_refs:
            self.loop_static_class_eager_refs.pop()

    def _collect_target_names(self, target: ast.AST) -> list[str]:
        # Source (left-to-right) order, deduplicated.  A set would be lossy: its
        # iteration order is PYTHONHASHSEED-dependent, and several callers feed
        # these names positionally into emitted IR (e.g. the co_varnames tuple
        # via _collect_assigned_names_ordered), so a set leaked hash order into
        # the compiled output (#34, unpack-target class).  Returning an ordered
        # list keeps that deterministic; set-semantics callers wrap in set(...).
        if isinstance(target, ast.Name):
            return [target.id]
        if isinstance(target, ast.Starred):
            return self._collect_target_names(target.value)
        if isinstance(target, (ast.Tuple, ast.List)):
            names: list[str] = []
            seen: set[str] = set()
            for elt in target.elts:
                for name in self._collect_target_names(elt):
                    if name not in seen:
                        seen.add(name)
                        names.append(name)
            return names
        return []

    def _collect_global_decls(self, nodes: list[ast.stmt]) -> set[str]:
        class GlobalCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Global(self, node: ast.Global) -> None:
                self.names.update(node.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = GlobalCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _module_globals_dict_escapes(self, node: ast.Module) -> bool:
        for child in ast.walk(node):
            if (
                isinstance(child, ast.Call)
                and isinstance(child.func, ast.Name)
                and child.func.id in {"globals", "vars"}
                and not child.args
                and not child.keywords
            ):
                return True
            if (
                isinstance(child, ast.Name)
                and isinstance(child.ctx, ast.Load)
                and child.id in {"globals", "vars"}
            ):
                return True
        return False

    def _collect_stable_module_classes(self, node: ast.Module) -> set[str]:
        if self._module_globals_dict_escapes(node):
            return set()
        class_defs: dict[str, int] = {}
        rebound: set[str] = set()
        deleted: set[str] = set()
        global_decls: set[str] = set()

        def record_target(target: ast.AST, names: set[str]) -> None:
            if isinstance(target, ast.Name):
                names.add(target.id)
                return
            if isinstance(target, ast.Starred):
                record_target(target.value, names)
                return
            if isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt, names)

        for stmt in node.body:
            if isinstance(stmt, ast.ClassDef):
                class_defs[stmt.name] = class_defs.get(stmt.name, 0) + 1
                continue
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if stmt.name in class_defs:
                    rebound.add(stmt.name)
                global_decls.update(self._collect_global_decls(stmt.body))
                continue
            if isinstance(stmt, ast.Assign):
                for target in stmt.targets:
                    record_target(target, rebound)
                continue
            if isinstance(stmt, ast.AnnAssign):
                record_target(stmt.target, rebound)
                continue
            if isinstance(stmt, ast.AugAssign):
                record_target(stmt.target, rebound)
                continue
            if isinstance(stmt, ast.Delete):
                for target in stmt.targets:
                    record_target(target, deleted)
                continue
            if isinstance(stmt, (ast.Import, ast.ImportFrom)):
                for alias in stmt.names:
                    rebound.add(alias.asname or alias.name.split(".", 1)[0])

        return {
            name
            for name, count in class_defs.items()
            if count == 1
            and name not in rebound
            and name not in deleted
            and name not in global_decls
        }

    def _collect_nonlocal_decls(self, nodes: list[ast.stmt]) -> set[str]:
        class NonlocalCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
                self.names.update(node.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = NonlocalCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _class_id_from_call(self, node: ast.Call) -> str | None:
        if isinstance(node.func, ast.Name) and node.func.id in self.classes:
            return node.func.id
        return None

    def _builtin_exact_type_from_expr(self, value: ast.AST | None) -> str | None:
        if isinstance(value, (ast.Dict, ast.DictComp)):
            return "dict"
        if isinstance(value, (ast.List, ast.ListComp)):
            return "list"
        if isinstance(value, (ast.Set, ast.SetComp)):
            return "set"
        if isinstance(value, ast.Tuple):
            return "tuple"
        if isinstance(value, ast.Call) and isinstance(value.func, ast.Name):
            func_id = value.func.id
            if func_id in {"dict", "list", "set", "tuple"}:
                return func_id
            if (
                func_id in {"globals", "locals", "vars"}
                and not value.args
                and not value.keywords
            ):
                return "dict"
        return None

    def _update_exact_local(self, name: str, value: ast.AST | None) -> None:
        builtin_exact = self._builtin_exact_type_from_expr(value)
        if builtin_exact is not None:
            self.exact_builtin_locals[name] = builtin_exact
            self.exact_locals.pop(name, None)
            return
        if isinstance(value, ast.Call):
            class_id = self._class_id_from_call(value)
            if class_id is not None:
                class_info = self.classes.get(class_id)
                if (
                    class_info
                    and not class_info.get("dynamic")
                    and not class_info.get("dataclass")
                ):
                    self.exact_locals[name] = class_id
                    self.exact_builtin_locals.pop(name, None)
                    return
        if isinstance(value, ast.Name):
            if value.id in self.exact_locals and (
                self.current_func_name == "molt_main"
                or value.id not in self.global_decls
            ):
                self.exact_locals[name] = self.exact_locals[value.id]
                self.exact_builtin_locals.pop(name, None)
                return
            if value.id in self.exact_builtin_locals and (
                self.current_func_name == "molt_main"
                or value.id not in self.global_decls
            ):
                self.exact_builtin_locals[name] = self.exact_builtin_locals[value.id]
                self.exact_locals.pop(name, None)
                return
        self.exact_locals.pop(name, None)
        self.exact_builtin_locals.pop(name, None)

    def _propagate_func_type_hint(
        self, value_node: MoltValue, source_expr: ast.AST | None
    ) -> None:
        if not isinstance(source_expr, ast.Name):
            return
        source_info = self.locals.get(source_expr.id) or self.globals.get(
            source_expr.id
        )
        if source_info is None:
            return
        hint = source_info.type_hint
        if not isinstance(hint, str):
            return
        if hint.startswith(
            (
                "AsyncFunc:",
                "AsyncClosureFunc:",
                "AsyncGenFunc:",
                "AsyncGenClosureFunc:",
                "GenFunc:",
                "GenClosureFunc:",
            )
        ):
            symbol = hint.split(":")[1]
            base_symbol = (
                symbol[: -len("_poll")] if symbol.endswith("_poll") else symbol
            )
            if (
                base_symbol in self.func_default_specs
                or self._known_function_symbol_target(base_symbol) is not None
            ):
                value_node.type_hint = hint
            return
        if hint.startswith("Func:"):
            symbol = hint.split(":")[1]
            if (
                symbol in self.func_default_specs
                or self._known_function_symbol_target(symbol) is not None
            ):
                value_node.type_hint = hint

    @staticmethod
    def _is_class_body_managed_name(name: str) -> bool:
        # Internal lowering scaffolding (loop break flags, the locals cache, any
        # ``__molt_*`` temp) is NOT a Python-visible class-body name and must
        # never be threaded through the namespace mapping — it is pure SSA
        # plumbing.  Everything else (including dunders the body assigns) routes
        # through the class namespace.
        return not (
            name == _MOLT_LOCALS_CACHE
            or name == _MOLT_CLOSURE_PARAM
            or name.startswith("__molt_")
        )

    def _active_class_ns_scope(self, name: str) -> "_ClassNsScope | None":
        # The innermost class-body scope manages ``name`` when the body is being
        # lowered as a block.  A nested ``class``/``def`` pushes its own scope (or
        # a function frame), so only the top-of-stack entry — and only while we
        # are still emitting that class's body statements (``_class_body_depth``
        # tracks the active body) — is consulted.  Names that are loop/scaffold
        # temps are excluded so the SSA machinery handles them unchanged.
        if not self._class_ns_stack:
            return None
        if not self._is_class_body_managed_name(name):
            return None
        return self._class_ns_stack[-1]

    def _class_ns_store(
        self, scope: "_ClassNsScope", name: str, value: MoltValue
    ) -> None:
        # Bind a class-body name: snapshot the SSA value for the static fast path
        # AND, when a runtime namespace dict exists, publish it there so the dict
        # is the loop-carried-correct mutable home (and so a custom mapping's
        # ``__setitem__`` observes the store, matching CPython's class body).
        scope.names.add(name)
        scope.attr_values[name] = value
        if scope.ns is not None:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[scope.ns, key_val, value],
                    result=MoltValue("none"),
                )
            )

    def _class_ns_load(self, scope: "_ClassNsScope", name: str) -> MoltValue | None:
        # Read a class-body name.  When a runtime namespace dict backs the body
        # the dict is authoritative (it survives loop back-edges and reflects
        # mutations done through control flow), so read from it via INDEX.  With
        # no dict (a straight-line static body that never entered this path under
        # control flow) the SSA snapshot is exact.  A name this body never bound
        # returns None so ``visit_Name`` falls through to global/builtin
        # resolution — CPython's ``LOAD_NAME`` KeyError fallthrough.
        if name not in scope.names:
            return None
        if scope.ns is not None:
            hint = "Any"
            cached = scope.attr_values.get(name)
            if cached is not None and cached.type_hint:
                hint = cached.type_hint
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            res = MoltValue(self.next_var(), type_hint=hint)
            self.emit(MoltOp(kind="INDEX", args=[scope.ns, key_val], result=res))
            return res
        return scope.attr_values.get(name)

    def _class_ns_delete(self, scope: "_ClassNsScope", name: str) -> None:
        # ``del name`` in a class body removes the binding from the namespace.
        # CPython raises NameError if the name is unbound; the binding-presence
        # check is the namespace dict's own ``__delitem__`` (which raises
        # KeyError -> NameError at the boundary).  Drop the SSA snapshot and,
        # when a dict backs the body, DEL_INDEX it (routing a custom mapping's
        # ``__delitem__``).
        scope.names.discard(name)
        scope.attr_values.pop(name, None)
        if scope.ns is not None:
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
            self.emit(
                MoltOp(
                    kind="DEL_INDEX",
                    args=[scope.ns, key_val],
                    result=MoltValue("none"),
                )
            )

    def _load_local_value(
        self, name: str, *, guard_unbound: bool = True
    ) -> MoltValue | None:
        # A class-body name resolves through the class namespace; but a name that
        # the class body has NOT bound (e.g. a parameter of a function inlined
        # into the body, like an inlined ``__init__``'s args, or an enclosing
        # local) must fall through to ordinary resolution.  Only short-circuit
        # when the active class scope actually owns ``name`` — otherwise continue
        # below so genuine locals/cells/globals still resolve.  (P0 #50.)
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None and name in class_scope.names:
            return self._class_ns_load(class_scope, name)
        if name in self.comp_shadow_locals:
            return self.locals.get(name)
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return self._emit_global_get(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            res = MoltValue(self.next_var())
            hint = self.boxed_local_hints.get(name)
            if hint is not None:
                res.type_hint = hint
            self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
            self._copy_container_hints_for_name_load(name, res.name)
            if guard_unbound and name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        if self.is_async() and name in self.async_locals:
            offset = self.async_locals[name]
            res = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
            if guard_unbound and name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        cached = self.locals.get(name)
        if cached is None:
            return None
        # Emit explicit load_var for non-boxed function locals so TIR can
        # track variable mutations through loop iterations via SSA phis.
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
        ):
            res = MoltValue(self.next_var(), type_hint=cached.type_hint)
            self.emit(
                MoltOp(
                    kind="LOAD_VAR",
                    args=[],
                    result=res,
                    metadata={"var": name},
                )
            )
            self._copy_container_hints_for_name_load(name, res.name)
            if guard_unbound and name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        return cached

    def _plain_local_del_boundary_value(
        self, name: str, value: MoltValue | None
    ) -> MoltValue | None:
        if (
            value is None
            or value.name in ("none", "")
            or value.type_hint == "missing"
            or self.current_func_name == "molt_main"
            or name not in self.scope_assigned
            or name in self.closure_locals
            or name in self.boxed_locals
            or self.is_async()
        ):
            return None
        return value

    def _plain_local_del_boundary_enabled(
        self, name: str, value: MoltValue | None
    ) -> bool:
        return self._plain_local_del_boundary_value(name, value) is not None

    def _emit_plain_local_del_boundary(
        self, name: str, value: MoltValue | None
    ) -> None:
        boundary_source = self._plain_local_del_boundary_value(name, value)
        if boundary_source is None:
            return
        # `self.locals[name]` is the syntactic cached producer. Across loop
        # phis the current frame slot may be a block argument, so make the
        # boundary read the slot at the release point instead of retaining a
        # stale pre-loop SSA value.
        boundary_value = MoltValue(
            self.next_var(), type_hint=boundary_source.type_hint
        )
        self.emit(
            MoltOp(
                kind="LOAD_VAR",
                args=[],
                result=boundary_value,
                metadata={"var": name},
            )
        )
        self.emit(
            MoltOp(
                kind="DEL_BOUNDARY",
                args=[boundary_value],
                result=MoltValue("none"),
                metadata={"var": name},
            )
        )

    def _emit_plain_local_alias_retain(self, name: str, value: MoltValue) -> MoltValue:
        if (
            value.name in ("none", "")
            or value.type_hint == "missing"
            or self.current_func_name == "molt_main"
            or name not in self.scope_assigned
            or name in self.closure_locals
            or name in self.boxed_locals
            or self.is_async()
        ):
            return value
        producer = self._op_by_result.get(value.name)
        loaded_plain_local = False
        if producer is not None and producer.kind == "LOAD_VAR":
            source_name = (producer.metadata or {}).get("var")
            loaded_plain_local = (
                isinstance(source_name, str)
                and source_name != name
                and source_name in self.locals
                and source_name not in self.closure_locals
                and source_name not in self.boxed_locals
            )
        has_existing_binding = any(
            other_name != name and other_value.name == value.name
            for other_name, other_value in self.locals.items()
        )
        if not has_existing_binding and not loaded_plain_local:
            return value
        # `alias = local` gives the alias its own frame-owned reference in
        # CPython. Model that as a value-producing alias so TIR ownership sees a
        # distinct droppable root instead of a side-effect retain on shared bits.
        retained = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="BINDING_ALIAS", args=[value], result=retained))
        return retained

    def _plain_local_scope_exit_bindings(self) -> list[tuple[str, MoltValue]]:
        if (
            self.current_func_name == "molt_main"
            or self.is_async()
            or self.in_generator
        ):
            return []
        params = set(self.funcs_map.get(self.current_func_name, {}).get("params", []))
        candidate_names = sorted(set(self.scope_assigned) | params)
        bindings: list[tuple[str, MoltValue]] = []
        for name in candidate_names:
            if (
                name == _MOLT_CLOSURE_PARAM
                or name == _MOLT_LOCALS_CACHE
                or name.startswith("__molt_")
                or name in self.closure_locals
                or name in self.boxed_locals
                or name in self.global_decls
                or name in self.nonlocal_decls
            ):
                continue
            value = self.locals.get(name)
            if (
                value is None
                or value.name in ("none", "")
                or value.type_hint == "missing"
                or self._plain_local_scope_exit_boundary_exempt(value)
            ):
                continue
            bindings.append((name, value))
        return bindings

    @staticmethod
    def _plain_local_scope_exit_boundary_exempt(value: MoltValue) -> bool:
        hint = value.type_hint or ""
        return hint == "code"

    def _value_reads_plain_local_binding(
        self, value: MoltValue, bindings: list[tuple[str, MoltValue]]
    ) -> bool:
        if value.name in ("none", "") or value.type_hint == "missing":
            return False
        for _, bound_value in bindings:
            if bound_value.name == value.name:
                return True
        producer = self._op_by_result.get(value.name)
        if producer is not None and producer.kind == "LOAD_VAR":
            source_name = (producer.metadata or {}).get("var")
            return any(name == source_name for name, _ in bindings)
        return False

    def _emit_plain_local_scope_exit_boundaries(
        self, preserve: MoltValue | None = None
    ) -> None:
        bindings = self._plain_local_scope_exit_bindings()
        if not bindings:
            return
        if preserve is not None and self._value_reads_plain_local_binding(
            preserve, bindings
        ):
            self.emit(MoltOp(kind="INC_REF", args=[preserve], result=MoltValue("none")))
        for name, value in bindings:
            boundary_value = self._load_local_value(name, guard_unbound=False) or value
            self._emit_plain_local_del_boundary(name, boundary_value)

    def _store_local_value(
        self,
        name: str,
        value: MoltValue,
        *,
        emit_rebind_boundary: bool = True,
    ) -> None:
        def update_locals_cache() -> None:
            self._emit_locals_cache_update(name, value)

        self._invalidate_loop_guard(name)
        class_scope = self._active_class_ns_scope(name)
        if class_scope is not None:
            self._class_ns_store(class_scope, name, value)
            return
        if name in self.comp_shadow_locals:
            self.locals[name] = value
            return
        if self.current_func_name != "molt_main" and name in self.global_decls:
            self._emit_module_attr_set_runtime(name, value)
            return
        if (
            self.current_func_name == "molt_main"
            and name in self.module_global_mutations
            and hasattr(self, "module_obj")
            and self.module_obj is not None
        ):
            self._emit_module_attr_set_on(self.module_obj, name, value)
        # Module-level stores inside loops must sync to the module dict
        # so that module_get_attr reads see the updated value on each iteration.
        if (
            self.current_func_name == "molt_main"
            and self.control_flow_depth > 0
            and hasattr(self, "module_obj")
            and self.module_obj is not None
            and not name.startswith("__molt_")
            and name in self.scope_assigned
        ):
            self._emit_module_attr_set_on(self.module_obj, name, value)
        if name in self.nonlocal_decls and name not in self.free_vars:
            raise NotImplementedError("nonlocal binding not found")
        if name in self.free_vars or name in self.nonlocal_decls:
            if self._emit_free_var_store(name, value):
                return
        # Discard the name from the unbound-check set — at any flow
        # depth.  Within the current basic block, the assignment we're
        # about to emit dominates all subsequent loads of `name` until
        # the next flow boundary.  The flow visitors (visit_If,
        # visit_While, visit_For, visit_Try, visit_With,
        # visit_AsyncWith) snapshot `unbound_check_names` on entry and
        # restore on exit, so a name discarded inside a loop or branch
        # becomes "checked again" after the flow exits — the parent
        # path can't rely on the inner assignment having happened.
        # Inside the current scope (until the next flow boundary), the
        # discard eliminates the redundant `is missing → raise
        # UnboundLocalError` guard that would otherwise be emitted on
        # every subsequent load_var, which is the dominant per-iter
        # overhead in `obj = Class(...)` / `obj.x = …` / `obj.y = …`
        # loop bodies (bench_struct).
        if name in self.unbound_check_names:
            self.unbound_check_names.discard(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            update_locals_cache()
            return
        if self.is_async():
            if name not in self.async_locals:
                self._async_local_offset(name)
            offset = self.async_locals[name]
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.async_local_hints[name] = value.type_hint
            update_locals_cache()
            return
        # Do NOT cache in self.locals when the variable is module-backed
        # (in module_global_mutations). The canonical store is the module dict
        # and reads must go through MODULE_GET_ATTR to see the latest value
        # across loop iterations. Without this guard, the stale local SSA
        # value shadows the module dict, causing while-loop conditions and
        # augmented assignments to read outdated values.
        if (
            self.current_func_name == "molt_main"
            and name in self.module_global_mutations
        ):
            update_locals_cache()
            return
        if value.name in self.bytearray_len_hints:
            self.bytearray_len_hints[name] = self.bytearray_len_hints[value.name]
        else:
            self.bytearray_len_hints.pop(name, None)
        if emit_rebind_boundary:
            value = self._emit_plain_local_alias_retain(name, value)
            previous = self.locals.get(name)
            if (
                previous is not None
                and previous.name != value.name
                and self._plain_local_del_boundary_enabled(name, previous)
            ):
                boundary_value = (
                    self._load_local_value(name, guard_unbound=False) or previous
                )
                self._emit_plain_local_del_boundary(name, boundary_value)
        self.locals[name] = value
        # Named-local fact (#58 ordering keystone): stamp `bound_local` on the
        # op that PRODUCED the bound value. CPython holds a named local in the
        # frame until `del`/rebinding/scope exit, so a finalizer-sensitive
        # value bound to a name must not be released at its SSA last-use; an
        # UNNAMED expression temp (e.g. `bag.append(A())`'s argument) dies at
        # the statement like CPython's stack ref. The IR otherwise erases this
        # distinction. Same condition as the named-local STORE_VAR below —
        # this is metadata on an already-emitted op, not a new op.
        if (
            self.current_func_name != "molt_main"
            and name in self.scope_assigned
            and value.name not in ("none", "")
        ):
            producer = self._op_by_result.get(value.name)
            if producer is not None:
                if producer.metadata is None:
                    producer.metadata = {}
                producer.metadata["bound_local"] = True
        # Emit explicit store_var for non-boxed function locals so TIR can
        # track variable mutations through loop iterations via SSA phis.
        if (
            self.current_func_name != "molt_main"
            and not self.is_async()
            and name in self.scope_assigned
            and name not in self.boxed_locals
        ):
            self.emit(
                MoltOp(
                    kind="STORE_VAR",
                    args=[value],
                    result=MoltValue("none"),
                    metadata={"var": name},
                )
            )
        update_locals_cache()

    def _emit_locals_cache_update(self, name: str, value: MoltValue) -> None:
        # Never include internal Molt scaffolding in `locals()`.
        if (
            name == _MOLT_LOCALS_CACHE
            or name == _MOLT_CLOSURE_PARAM
            or name.startswith("__molt_")
        ):
            return
        # Only include Python-visible locals (params + assigned names). The compiler
        # may introduce additional locals/temps for lowering; those must never leak
        # into `locals()` (CPython parity).
        params = self.funcs_map.get(self.current_func_name, {}).get("params", [])
        if name not in self.scope_assigned and name not in params:
            return
        cache = self.locals_cache_val
        if cache is None:
            return
        key = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
        if value.type_hint == "missing":
            # Keep the pinned frame-locals cache in sync for `del`/unbound transitions.
            self.emit(
                MoltOp(
                    kind="DICT_UPDATE_MISSING",
                    args=[cache, key, value],
                    result=MoltValue("none"),
                )
            )
            return
        self.emit(
            MoltOp(
                kind="DICT_SET",
                args=[cache, key, value],
                result=MoltValue("none"),
            )
        )

    def _emit_delete_local_value(
        self, name: str, missing: MoltValue, old_value: MoltValue
    ) -> None:
        self._invalidate_loop_guard(name)
        self.bytearray_len_hints.pop(name, None)
        self.locals[name] = missing
        self.emit(
            MoltOp(
                kind="DELETE_VAR",
                args=[missing, old_value],
                result=MoltValue("none"),
                metadata={"var": name},
            )
        )
        self._emit_locals_cache_update(name, missing)

    def _store_comprehension_local_value(self, name: str, value: MoltValue) -> None:
        self._invalidate_loop_guard(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            self.locals[name] = value
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            return
        self.locals[name] = value

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

    def _expr_may_yield(self, node: ast.AST) -> bool:
        if not self.is_async():
            return False

        class YieldVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.may_yield = False

            def visit_Await(self, node: ast.Await) -> None:
                self.may_yield = True

            def visit_Call(self, node: ast.Call) -> None:
                if isinstance(node.func, ast.Name) and node.func.id in {
                    "molt_chan_send",
                    "molt_chan_recv",
                }:
                    self.may_yield = True
                    return
                self.generic_visit(node)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

        visitor = YieldVisitor()
        visitor.visit(node)
        return visitor.may_yield

    def _expr_needs_async(self, node: ast.AST) -> bool:
        class AsyncVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.needs_async = False

            def visit_Await(self, node: ast.Await) -> None:
                self.needs_async = True

            def visit_Call(self, node: ast.Call) -> None:
                if isinstance(node.func, ast.Name) and node.func.id in {
                    "molt_chan_send",
                    "molt_chan_recv",
                }:
                    self.needs_async = True
                    return
                self.generic_visit(node)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        visitor = AsyncVisitor()
        visitor.visit(node)
        return visitor.needs_async

    def _expr_contains_yield(self, node: ast.AST) -> bool:
        class YieldVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.found = False

            def visit_Yield(self, node: ast.Yield) -> None:
                self.found = True

            def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
                self.found = True

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        visitor = YieldVisitor()
        visitor.visit(node)
        return visitor.found

    def _spill_async_value(self, value: MoltValue, name: str) -> int:
        offset = self._async_local_offset(name)
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", offset, value],
                result=MoltValue("none"),
            )
        )
        return offset

    def _reload_async_value(self, offset: int, hint: str) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
        return res

    def _spill_async_temporaries(self) -> None:
        label_indices = [
            idx for idx, op in enumerate(self.current_ops) if op.kind == "STATE_LABEL"
        ]
        if not label_indices:
            return
        state_label_indices: dict[int, int] = {}
        for idx in label_indices:
            op = self.current_ops[idx]
            if op.args and isinstance(op.args[0], int):
                state_label_indices[op.args[0]] = idx
        params = set(self.funcs_map[self.current_func_name]["params"])
        spillable: set[str] = set(self.async_locals)
        spillable.update(self.scope_assigned)
        spillable.update(params)
        spillable.update(self.closure_locals)
        free_vars = getattr(self, "free_vars", None)
        if free_vars:
            spillable.update(free_vars)
        type_hints: dict[str, str] = {}
        for op in self.current_ops:
            for arg in op.args:
                if not isinstance(arg, MoltValue):
                    continue
                name = arg.name
                if name in {"self", "none"}:
                    continue
                spillable.add(name)
                if arg.type_hint:
                    type_hints.setdefault(name, arg.type_hint)
            out_name = op.result.name
            if out_name != "none":
                spillable.add(out_name)
                if op.result.type_hint:
                    type_hints.setdefault(out_name, op.result.type_hint)
        last_def: dict[str, int] = {name: -1 for name in spillable}
        label_spills: dict[int, set[str]] = {idx: set() for idx in label_indices}
        spill_names: set[str] = set()
        for idx, op in enumerate(self.current_ops):
            for arg in op.args:
                if not isinstance(arg, MoltValue):
                    continue
                name = arg.name
                if name in {"self", "none"} or name not in spillable:
                    continue
                def_idx = last_def.get(name)
                if def_idx is None:
                    continue
                start = bisect.bisect_right(label_indices, def_idx)
                end = bisect.bisect_left(label_indices, idx)
                if start >= end:
                    continue
                for label_idx in label_indices[start:end]:
                    label_spills[label_idx].add(name)
                spill_names.add(name)
            out_name = op.result.name
            if out_name != "none" and out_name in spillable:
                last_def[out_name] = idx
        if not spill_names:
            return
        # `spill_names` is a set, whose iteration order is hash-seeded and so
        # varies with PYTHONHASHSEED.  `_async_local_offset` assigns each new
        # name the next `len(async_locals) * 8` slot, so iterating the set
        # directly here would let the closure-slot offsets baked into the
        # emitted IR depend on hash order — a non-determinism bug (#34).  Any
        # iteration order that feeds IR emission MUST be deterministic, so we
        # assign offsets in sorted name order (matching the `sorted(...)`
        # used by the store/load emit loops below).
        for name in sorted(spill_names):
            self._async_local_offset(name)
            hint = type_hints.get(name)
            if hint is not None:
                self.async_local_hints.setdefault(name, hint)

        new_ops: list[MoltOp] = []

        def _emit_store_for_label(label_idx: int) -> None:
            for name in sorted(label_spills.get(label_idx, set())):
                if name not in self.async_locals:
                    self._async_local_offset(name)
                offset = self.async_locals[name]
                hint = type_hints.get(name, "Unknown")
                new_ops.append(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", offset, MoltValue(name, type_hint=hint)],
                        result=MoltValue("none"),
                    )
                )

        def _emit_loads_for_label(label_idx: int) -> None:
            for name in sorted(label_spills.get(label_idx, set())):
                if name not in self.async_locals:
                    self._async_local_offset(name)
                offset = self.async_locals[name]
                hint = type_hints.get(name, "Unknown")
                new_ops.append(
                    MoltOp(
                        kind="LOAD_CLOSURE",
                        args=["self", offset],
                        result=MoltValue(name, type_hint=hint),
                    )
                )

        def _state_label_before_try_start_run(end_idx: int) -> int | None:
            cursor = end_idx
            while cursor >= 0 and self.current_ops[cursor].kind == "TRY_START":
                cursor -= 1
            if cursor >= 0 and self.current_ops[cursor].kind == "STATE_LABEL":
                return cursor
            return None

        for idx, op in enumerate(self.current_ops):
            if op.kind in {"STATE_TRANSITION", "STATE_YIELD"}:
                label_idx = None
                if op.kind == "STATE_TRANSITION":
                    pending_arg = op.args[1] if len(op.args) == 2 else op.args[2]
                    pending_state = None
                    if isinstance(pending_arg, MoltValue):
                        pending_state = self.const_ints.get(pending_arg.name)
                    elif isinstance(pending_arg, int):
                        pending_state = pending_arg
                    if pending_state is not None:
                        label_idx = state_label_indices.get(pending_state)
                else:
                    pending_arg = op.args[1] if len(op.args) > 1 else None
                    if isinstance(pending_arg, int):
                        label_idx = state_label_indices.get(pending_arg)
                if label_idx is not None:
                    _emit_store_for_label(label_idx)
            new_ops.append(op)
            if op.kind == "STATE_LABEL":
                next_op = (
                    self.current_ops[idx + 1]
                    if idx + 1 < len(self.current_ops)
                    else None
                )
                if next_op is None or next_op.kind != "TRY_START":
                    _emit_loads_for_label(idx)
                continue
            if op.kind == "TRY_START":
                next_op = (
                    self.current_ops[idx + 1]
                    if idx + 1 < len(self.current_ops)
                    else None
                )
                if next_op is None or next_op.kind != "TRY_START":
                    label_idx = _state_label_before_try_start_run(idx)
                    if label_idx is not None:
                        _emit_loads_for_label(label_idx)
        self.current_ops[:] = new_ops

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

    def _match_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        target_name = node.target.id
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if not isinstance(stmt.value, ast.Name):
                return None
            if stmt.value.id != target_name:
                return None
            if stmt.target.id == target_name:
                return None
            kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
            return (stmt.target.id, target_name, kind)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == target_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == dest:
                if isinstance(right, ast.Name) and right.id == target_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, target_name, kind)
            if isinstance(right, ast.Name) and right.id == dest:
                if isinstance(left, ast.Name) and left.id == target_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, target_name, kind)
        return None

    def _range_start_expr(self, node: ast.expr) -> ast.expr | None:
        if isinstance(node, ast.Constant):
            if isinstance(node.value, int) and node.value > 0:
                return node
            return None
        if isinstance(node, ast.Name):
            return node
        return None

    def _match_indexed_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        idx_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Call):
            return None
        if not isinstance(node.iter.func, ast.Name) or node.iter.func.id != "range":
            return None
        args = node.iter.args
        if not args or len(args) > 3:
            return None
        start = None
        stop = None
        step = None
        if len(args) == 1:
            stop = args[0]
            step = ast.Constant(value=1)
        elif len(args) == 2:
            start = args[0]
            stop = args[1]
            step = ast.Constant(value=1)
        else:
            start = args[0]
            stop = args[1]
            step = args[2]
        start_expr = None
        if start is not None:
            if isinstance(start, ast.Constant):
                if not isinstance(start.value, int) or start.value < 0:
                    return None
                if start.value > 0:
                    start_expr = start
            else:
                start_expr = self._range_start_expr(start)
                if start_expr is None:
                    return None
        if not isinstance(step, ast.Constant) or step.value != 1:
            return None
        if not isinstance(stop, ast.Call):
            return None
        if not isinstance(stop.func, ast.Name) or stop.func.id != "len":
            return None
        if len(stop.args) != 1 or not isinstance(stop.args[0], ast.Name):
            return None
        seq_name = stop.args[0].id
        stmt = node.body[0]
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if stmt.target.id == idx_name:
                return None
            if not self._subscript_matches(stmt.value, seq_name, idx_name):
                return None
            kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
            return (stmt.target.id, seq_name, kind, start_expr)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == idx_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == dest:
                if self._subscript_matches(right, seq_name, idx_name):
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, seq_name, kind, start_expr)
            if isinstance(right, ast.Name) and right.id == dest:
                if self._subscript_matches(left, seq_name, idx_name):
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, seq_name, kind, start_expr)
        return None

    def _subscript_matches(self, node: ast.expr, seq_name: str, idx_name: str) -> bool:
        if not isinstance(node, ast.Subscript):
            return False
        if not isinstance(node.value, ast.Name) or node.value.id != seq_name:
            return False
        if isinstance(node.slice, ast.Name) and node.slice.id == idx_name:
            return True
        return False

    def _match_indexed_vector_minmax_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        idx_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Call):
            return None
        if not isinstance(node.iter.func, ast.Name) or node.iter.func.id != "range":
            return None
        args = node.iter.args
        if not args or len(args) > 3:
            return None
        start = None
        stop = None
        step = None
        if len(args) == 1:
            stop = args[0]
            step = ast.Constant(value=1)
        elif len(args) == 2:
            start = args[0]
            stop = args[1]
            step = ast.Constant(value=1)
        else:
            start = args[0]
            stop = args[1]
            step = args[2]
        start_expr = None
        if start is not None:
            if isinstance(start, ast.Constant):
                if not isinstance(start.value, int) or start.value < 0:
                    return None
                if start.value > 0:
                    start_expr = start
            else:
                start_expr = self._range_start_expr(start)
                if start_expr is None:
                    return None
        if not isinstance(step, ast.Constant) or step.value != 1:
            return None
        if not isinstance(stop, ast.Call):
            return None
        if not isinstance(stop.func, ast.Name) or stop.func.id != "len":
            return None
        if len(stop.args) != 1 or not isinstance(stop.args[0], ast.Name):
            return None
        seq_name = stop.args[0].id
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        if acc_name == idx_name:
            return None
        if not self._subscript_matches(assign.value, seq_name, idx_name):
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        left_is_acc = isinstance(left, ast.Name) and left.id == acc_name
        right_is_acc = isinstance(right, ast.Name) and right.id == acc_name
        left_is_item = self._subscript_matches(left, seq_name, idx_name)
        right_is_item = self._subscript_matches(right, seq_name, idx_name)
        if not ((left_is_acc and right_is_item) or (left_is_item and right_is_acc)):
            return None
        if isinstance(op, ast.Lt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "min", start_expr
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "max", start_expr
        if isinstance(op, ast.Gt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "max", start_expr
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "min", start_expr
        return None

    def _match_iter_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        item_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Name):
            return None
        seq_name = node.iter.id
        stmt = node.body[0]
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if stmt.target.id == item_name:
                return None
            if isinstance(stmt.value, ast.Name) and stmt.value.id == item_name:
                kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
                return (stmt.target.id, seq_name, kind, None)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if acc_name == item_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == acc_name:
                if isinstance(right, ast.Name) and right.id == item_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (acc_name, seq_name, kind, None)
            if isinstance(right, ast.Name) and right.id == acc_name:
                if isinstance(left, ast.Name) and left.id == item_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (acc_name, seq_name, kind, None)
        return None

    def _match_iter_vector_minmax_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        item_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Name):
            return None
        seq_name = node.iter.id
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        if acc_name == item_name:
            return None
        if not isinstance(assign.value, ast.Name) or assign.value.id != item_name:
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        left_is_acc = isinstance(left, ast.Name) and left.id == acc_name
        right_is_acc = isinstance(right, ast.Name) and right.id == acc_name
        left_is_item = isinstance(left, ast.Name) and left.id == item_name
        right_is_item = isinstance(right, ast.Name) and right.id == item_name
        if not ((left_is_acc and right_is_item) or (left_is_item and right_is_acc)):
            return None
        if isinstance(op, ast.Lt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "min", None
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "max", None
        if isinstance(op, ast.Gt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "max", None
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "min", None
        return None

    def _match_vector_minmax_loop(self, node: ast.For) -> tuple[str, str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        item_name = node.target.id
        if acc_name == item_name:
            return None
        if not isinstance(assign.value, ast.Name) or assign.value.id != item_name:
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        if not isinstance(left, ast.Name) or not isinstance(right, ast.Name):
            return None
        if {left.id, right.id} != {item_name, acc_name}:
            return None
        if isinstance(op, ast.Lt):
            if left.id == item_name and right.id == acc_name:
                return acc_name, item_name, "min"
            if left.id == acc_name and right.id == item_name:
                return acc_name, item_name, "max"
        if isinstance(op, ast.Gt):
            if left.id == item_name and right.id == acc_name:
                return acc_name, item_name, "max"
            if left.id == acc_name and right.id == item_name:
                return acc_name, item_name, "min"
        return None

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

    def _emit_range_list(
        self, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_FROM_RANGE", args=[start, stop, step], result=res))
        # Range always produces int elements.
        if self.current_func_name == "molt_main":
            self.global_elem_hints[res.name] = "int"
        else:
            self.container_elem_hints[res.name] = "int"
        return res

    def _emit_list_int_filled(self, count: MoltValue, fill: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_INT_NEW", args=[count, fill], result=res))
        if self.current_func_name == "molt_main":
            self.global_elem_hints[res.name] = "int"
        else:
            self.container_elem_hints[res.name] = "int"
        self._list_int_containers = getattr(self, "_list_int_containers", set())
        self._list_int_containers.add(res.name)
        return res

    def _emit_list_filled(
        self, count: MoltValue, fill: MoltValue, elem_hint: str | None
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_FILL_NEW", args=[count, fill], result=res))
        if elem_hint and elem_hint not in {"Any", "Unknown"}:
            if self.current_func_name == "molt_main":
                self.global_elem_hints[res.name] = elem_hint
            else:
                self.container_elem_hints[res.name] = elem_hint
        return res

    def _match_simple_range_list_comp(
        self, node: ast.ListComp
    ) -> tuple[MoltValue, MoltValue, MoltValue] | None:
        if len(node.generators) != 1:
            return None
        comp = node.generators[0]
        if comp.is_async or comp.ifs:
            return None
        if not isinstance(comp.target, ast.Name):
            return None
        if not isinstance(node.elt, ast.Name) or node.elt.id != comp.target.id:
            return None
        parsed = self._parse_range_call(comp.iter)
        if parsed is None:
            return None
        start, stop, step, _ = parsed
        return start, stop, step

    def _match_const_int_range_list_comp(self, node: ast.ListComp) -> int | None:
        if len(node.generators) != 1:
            return None
        comp = node.generators[0]
        if comp.is_async or comp.ifs:
            return None
        if not isinstance(comp.target, ast.Name):
            return None
        if not isinstance(node.elt, ast.Constant):
            return None
        value = node.elt.value
        if not isinstance(value, int) or isinstance(value, bool):
            return None
        if not isinstance(comp.iter, ast.Call):
            return None
        if not isinstance(comp.iter.func, ast.Name) or comp.iter.func.id != "range":
            return None
        if len(comp.iter.args) > 3 or comp.iter.keywords:
            return None
        return int(value)

    def _match_const_range_list_comp(self, node: ast.ListComp) -> ast.Constant | None:
        if len(node.generators) != 1:
            return None
        comp = node.generators[0]
        if comp.is_async or comp.ifs:
            return None
        if not isinstance(comp.target, ast.Name):
            return None
        if not isinstance(node.elt, ast.Constant):
            return None
        value = node.elt.value
        if isinstance(value, int) and not isinstance(value, bool):
            return None
        if not isinstance(comp.iter, ast.Call):
            return None
        if not isinstance(comp.iter.func, ast.Name) or comp.iter.func.id != "range":
            return None
        if len(comp.iter.args) > 3 or comp.iter.keywords:
            return None
        return node.elt

    def _emit_const_int_range_list_comp(
        self, node: ast.ListComp, fill_value: int
    ) -> MoltValue:
        comp = node.generators[0]
        parsed = self._parse_range_call(comp.iter)
        if parsed is None:
            raise NotImplementedError("Unsupported range in list comprehension")
        start, stop, step, _ = parsed
        range_obj = self._emit_range_obj_from_args(start, stop, step)
        count = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[range_obj], result=count))
        fill = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[fill_value], result=fill))
        return self._emit_list_int_filled(count, fill)

    def _emit_const_range_list_comp(
        self, node: ast.ListComp, fill_node: ast.Constant
    ) -> MoltValue:
        comp = node.generators[0]
        parsed = self._parse_range_call(comp.iter)
        if parsed is None:
            raise NotImplementedError("Unsupported range in list comprehension")
        start, stop, step, _ = parsed
        range_obj = self._emit_range_obj_from_args(start, stop, step)
        count = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[range_obj], result=count))
        fill = self.visit(fill_node)
        if fill is None:
            raise NotImplementedError("Unsupported list comprehension fill value")
        elem_hint = fill.type_hint if isinstance(fill, MoltValue) else None
        return self._emit_list_filled(count, fill, elem_hint)

    def _emit_list_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        elem_hint = self._iterable_element_hint(iterable) or "Any"
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
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint=elem_hint)
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(
            MoltOp(kind="LIST_APPEND", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        if elem_hint not in {"Any", "Unknown"}:
            if self.current_func_name == "molt_main":
                self.global_elem_hints[res.name] = elem_hint
            else:
                self.container_elem_hints[res.name] = elem_hint
        return res

    def _emit_list_from_aiter(self, iterable: MoltValue) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("async list comprehension outside async context")
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        res_slot = self._async_local_offset(
            f"__async_list_comp_res_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_list_comp_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_list_comp_sentinel_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        res_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_val,
            )
        )
        self.emit(
            MoltOp(
                kind="LIST_APPEND",
                args=[res_val, item_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        res_final = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_final,
            )
        )
        return res_final

    def _emit_set_from_iter(
        self, iterable: MoltValue, probe: bool = False
    ) -> MoltValue:
        # `probe=True` realizes the operand of a probe-only set operation
        # (intersection/intersection_update/issubset). CPython hashes each
        # element to probe the receiver without inserting into a fresh set, so an
        # unhashable element raises the bare `unhashable type: 'X'` form on every
        # version (no `set element` context, even on 3.14). The Bare-context
        # add op (SET_ADD_PROBE -> molt_set_add_probe) preserves that while still
        # materializing the temporary set molt's algorithm needs.
        add_kind = "SET_ADD_PROBE" if probe else "SET_ADD"
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
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
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(MoltOp(kind=add_kind, args=[res, item], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _emit_set_from_aiter(self, iterable: MoltValue) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("async set comprehension outside async context")
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
        res_slot = self._async_local_offset(
            f"__async_set_comp_res_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_set_comp_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_set_comp_sentinel_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        res_val = MoltValue(self.next_var(), type_hint="set")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_val,
            )
        )
        self.emit(
            MoltOp(
                kind="SET_ADD",
                args=[res_val, item_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        res_final = MoltValue(self.next_var(), type_hint="set")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_final,
            )
        )
        return res_final

    def _emit_dict_fill_from_iter(self, target: MoltValue, iterable: MoltValue) -> None:
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
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        # Validate that the yielded item has at least 2 elements before
        # indexing, so non-tuple / short-sequence inputs produce a clear
        # ValueError instead of an opaque crash.
        two = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[2], result=two))
        item_len = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[item], result=item_len))
        item_too_short = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[item_len, two], result=item_too_short))
        self.emit(MoltOp(kind="IF", args=[item_too_short], result=MoltValue("none")))
        err_msg = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(
                kind="CONST_STR",
                args=["dictionary update sequence element has length less than 2"],
                result=err_msg,
            )
        )
        err_exc = self._emit_exception_new("ValueError", err_msg)
        self.emit(MoltOp(kind="RAISE", args=[err_exc], result=MoltValue("none")))
        self._emit_raise_exit()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        key = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item, zero], result=key))
        val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item, one], result=val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[target, key, val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_dict_fill_from_aiter(
        self, target: MoltValue, iterable: MoltValue
    ) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("async dict comprehension outside async context")
        target_slot = self._async_local_offset(
            f"__async_dict_comp_target_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", target_slot, target],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_dict_comp_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_dict_comp_sentinel_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        key = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item_val, zero], result=key))
        val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item_val, one], result=val))
        target_val = MoltValue(self.next_var(), type_hint=target.type_hint or "dict")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", target_slot],
                result=target_val,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[target_val, key, val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        target_final = MoltValue(self.next_var(), type_hint=target.type_hint or "dict")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", target_slot],
                result=target_final,
            )
        )
        return target_final

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

    def _match_counted_while(
        self, node: ast.While
    ) -> tuple[str, int, list[ast.stmt]] | None:
        if node.orelse:
            return None
        if not isinstance(node.test, ast.Compare):
            return None
        if len(node.test.ops) != 1 or not isinstance(node.test.ops[0], ast.Lt):
            return None
        if not isinstance(node.test.left, ast.Name):
            return None
        if len(node.test.comparators) != 1:
            return None
        bound_value = self._const_int_from_expr(node.test.comparators[0])
        if bound_value is None:
            return None
        if not node.body:
            return None
        index_name = node.test.left.id
        incr_stmt = node.body[-1]
        if not self._is_unit_increment(incr_stmt, index_name):
            return None
        if index_name in self._collect_assigned_names(node.body[:-1]):
            return None
        return index_name, bound_value, node.body[:-1]

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

    def _match_bytearray_fill_counted_while(
        self, index_name: str, bound: int, body: list[ast.stmt]
    ) -> tuple[str, int, int, int] | None:
        if len(body) != 1:
            return None
        stmt = body[0]
        if not isinstance(stmt, ast.Assign) or len(stmt.targets) != 1:
            return None
        target = stmt.targets[0]
        if not isinstance(target, ast.Subscript):
            return None
        if not isinstance(target.value, ast.Name):
            return None
        if not isinstance(target.slice, ast.Name) or target.slice.id != index_name:
            return None
        container_name = target.value.id
        container = self.locals.get(container_name)
        if container is None or container.type_hint != "bytearray":
            return None
        bytearray_len = self._bytearray_len_hint_for(container_name, container)
        if bytearray_len is None:
            return None
        start = self._const_int_for_local(index_name)
        if start is None or start < 0 or bound <= start or bound > bytearray_len:
            return None
        fill = self._const_int_from_expr(stmt.value)
        if fill is None or not 0 <= fill <= 255:
            return None
        return container_name, start, bound, fill

    def _match_counted_while_sum(
        self, index_name: str, body: list[ast.stmt]
    ) -> str | None:
        if len(body) != 1:
            return None
        stmt = body[0]
        if isinstance(stmt, ast.AugAssign):
            if (
                isinstance(stmt.op, ast.Add)
                and isinstance(stmt.target, ast.Name)
                and isinstance(stmt.value, ast.Name)
                and stmt.value.id == index_name
            ):
                return stmt.target.id
            return None
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and isinstance(right, ast.Name)
                and (
                    {left.id, right.id} == {acc_name, index_name}
                    and left.id != right.id
                )
            ):
                return acc_name
        return None

    def _match_const_increment(self, stmt: ast.stmt) -> tuple[str, int] | None:
        if isinstance(stmt, ast.AugAssign):
            if (
                isinstance(stmt.op, ast.Add)
                and isinstance(stmt.target, ast.Name)
                and isinstance(stmt.value, ast.Constant)
                and isinstance(stmt.value.value, int)
            ):
                return stmt.target.id, stmt.value.value
            return None
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and left.id == acc_name
                and isinstance(right, ast.Constant)
                and isinstance(right.value, int)
            ):
                return acc_name, right.value
            if (
                isinstance(right, ast.Name)
                and right.id == acc_name
                and isinstance(left, ast.Constant)
                and isinstance(left.value, int)
            ):
                return acc_name, left.value
        return None

    def _match_counted_while_const_increment(
        self, body: list[ast.stmt]
    ) -> tuple[str, int] | None:
        if len(body) == 1:
            return self._match_const_increment(body[0])
        if len(body) != 2:
            return None
        init, inner = body
        if not isinstance(init, ast.Assign):
            return None
        if len(init.targets) != 1 or not isinstance(init.targets[0], ast.Name):
            return None
        if not isinstance(init.value, ast.Constant) or not isinstance(
            init.value.value, int
        ):
            return None
        if not isinstance(inner, ast.While):
            return None
        inner_match = self._match_counted_while(inner)
        if inner_match is None:
            return None
        inner_index, inner_bound, inner_body = inner_match
        if inner_index != init.targets[0].id:
            return None
        inner_inc = self._match_counted_while_const_increment(inner_body)
        if inner_inc is None:
            return None
        acc_name, delta = inner_inc
        start_val = init.value.value
        if start_val >= inner_bound:
            return acc_name, 0
        return acc_name, (inner_bound - start_val) * delta

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

    def _match_matmul_loop(self, node: ast.For) -> tuple[str, str, str] | None:
        if node.orelse or not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1 or not isinstance(node.body[0], ast.For):
            return None
        outer_i = node.target.id
        j_loop = node.body[0]
        if j_loop.orelse or not isinstance(j_loop.target, ast.Name):
            return None
        inner_j = j_loop.target.id
        if len(j_loop.body) != 3:
            return None
        init = j_loop.body[0]
        k_loop = j_loop.body[1]
        store = j_loop.body[2]
        if not isinstance(init, ast.Assign):
            return None
        if len(init.targets) != 1 or not isinstance(init.targets[0], ast.Name):
            return None
        acc_name = init.targets[0].id
        if not isinstance(init.value, ast.Constant) or init.value.value != 0:
            return None
        if not isinstance(k_loop, ast.For) or k_loop.orelse:
            return None
        if not isinstance(k_loop.target, ast.Name):
            return None
        inner_k = k_loop.target.id
        if len(k_loop.body) != 1 or not isinstance(k_loop.body[0], ast.Assign):
            return None
        acc_assign = k_loop.body[0]
        if (
            len(acc_assign.targets) != 1
            or not isinstance(acc_assign.targets[0], ast.Name)
            or acc_assign.targets[0].id != acc_name
        ):
            return None
        if not isinstance(acc_assign.value, ast.BinOp) or not isinstance(
            acc_assign.value.op, ast.Add
        ):
            return None
        add_left = acc_assign.value.left
        add_right = acc_assign.value.right
        if not isinstance(add_left, ast.Name) or add_left.id != acc_name:
            return None
        if not isinstance(add_right, ast.BinOp) or not isinstance(
            add_right.op, ast.Mult
        ):
            return None
        left_get = add_right.left
        right_get = add_right.right
        if not (isinstance(left_get, ast.Call) and isinstance(right_get, ast.Call)):
            return None
        left_args = self._parse_molt_buffer_call(left_get, "get")
        right_args = self._parse_molt_buffer_call(right_get, "get")
        if left_args is None or right_args is None:
            return None
        if len(left_args) != 3 or len(right_args) != 3:
            return None
        if not all(isinstance(arg, ast.Name) for arg in left_args[1:]):
            return None
        if not all(isinstance(arg, ast.Name) for arg in right_args[1:]):
            return None
        left_buf = left_args[0]
        right_buf = right_args[0]
        if not isinstance(left_buf, ast.Name) or not isinstance(right_buf, ast.Name):
            return None
        a_name = left_buf.id
        b_name = right_buf.id
        left_i = cast(ast.Name, left_args[1]).id
        left_k = cast(ast.Name, left_args[2]).id
        right_k = cast(ast.Name, right_args[1]).id
        right_j = cast(ast.Name, right_args[2]).id
        if left_i != outer_i or left_k != inner_k:
            return None
        if right_k != inner_k or right_j != inner_j:
            return None
        if not isinstance(store, ast.Expr) or not isinstance(store.value, ast.Call):
            return None
        store_args = self._parse_molt_buffer_call(store.value, "set")
        if store_args is None or len(store_args) != 4:
            return None
        if not isinstance(store_args[0], ast.Name):
            return None
        out_name = store_args[0].id
        if not all(isinstance(arg, ast.Name) for arg in store_args[1:3]):
            return None
        if (
            cast(ast.Name, store_args[1]).id != outer_i
            or cast(ast.Name, store_args[2]).id != inner_j
        ):
            return None
        if not isinstance(store_args[3], ast.Name) or store_args[3].id != acc_name:
            return None
        return out_name, a_name, b_name


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


    def _build_comprehension_body(
        self,
        generators: list[ast.comprehension],
        inner: list[ast.stmt],
    ) -> list[ast.stmt]:
        body: list[ast.stmt] = list(inner)
        for comp in reversed(generators):
            for test in reversed(comp.ifs):
                body = [ast.If(test=test, body=list(body), orelse=[])]
            if comp.is_async:
                body = [
                    ast.AsyncFor(
                        target=comp.target,
                        iter=comp.iter,
                        body=list(body),
                        orelse=[],
                    )
                ]
            else:
                body = [
                    ast.For(
                        target=comp.target,
                        iter=comp.iter,
                        body=list(body),
                        orelse=[],
                    )
                ]
        return body

    def _comprehension_requires_async(
        self,
        generators: list[ast.comprehension],
        exprs: list[ast.AST | None],
    ) -> bool:
        if any(comp.is_async for comp in generators):
            return True
        for comp in generators:
            if self._expr_needs_async(comp.iter):
                return True
            for test in comp.ifs:
                if self._expr_needs_async(test):
                    return True
        for expr in exprs:
            if expr is None:
                continue
            if self._expr_needs_async(expr):
                return True
        return False

    def _inline_simple_comp_exprs(
        self, node: ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[ast.AST]:
        if isinstance(node, ast.DictComp):
            return [node.key, node.value]
        return [node.elt]

    def _can_inline_simple_comp(
        self,
        generators: list[ast.comprehension],
        exprs: Sequence[ast.AST],
    ) -> bool:
        """Check whether a comprehension can be lowered as an inline loop.

        Requirements: single generator, no async, simple target (Name or a
        flat Tuple of Names), no nested comprehensions in emitted element
        expressions. Multi-for comprehensions use the GeneratorExp path, which
        handles walrus scope leaking separately.

        Tuple targets such as ``for i, value in enumerate(values)`` are
        accepted: the inline emitter assigns to a temp Name and emits an
        explicit unpack, matching the semantics of CPython's tuple-target
        ``for`` loops without forcing the comprehension onto the
        generator-poll path (which has known Cranelift codegen
        fragility for large surrounding functions).
        """
        if len(generators) != 1:
            return False
        comp = generators[0]
        if comp.is_async:
            return False
        if isinstance(comp.target, ast.Name):
            pass
        elif isinstance(comp.target, ast.Tuple):
            # Only accept flat tuples of plain Name elements (no nested
            # tuples, no Starred/Subscript/Attribute targets).
            if not comp.target.elts:
                return False
            for elt in comp.target.elts:
                if not isinstance(elt, ast.Name):
                    return False
        else:
            return False
        # Reject emitted expressions that themselves contain comprehensions
        # (they would require their own generator and cannot be inlined).
        for expr in exprs:
            for child in ast.walk(expr):
                if isinstance(
                    child, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)
                ):
                    return False
        return True

    def _can_inline_list_comp(self, node: ast.ListComp) -> bool:
        return self._can_inline_simple_comp(node.generators, [node.elt])

    def _can_inline_set_comp(self, node: ast.SetComp) -> bool:
        return self._can_inline_simple_comp(node.generators, [node.elt])

    def _can_inline_dict_comp(self, node: ast.DictComp) -> bool:
        return self._can_inline_simple_comp(node.generators, [node.key, node.value])

    def _inline_simple_comp_target(
        self, comp: ast.comprehension, temp_prefix: str
    ) -> tuple[str, list[str] | None]:
        if isinstance(comp.target, ast.Name):
            return comp.target.id, None
        if isinstance(comp.target, ast.Tuple) and all(
            isinstance(e, ast.Name) for e in comp.target.elts
        ):
            tuple_target_names = [cast(ast.Name, e).id for e in comp.target.elts]
            target_name = f"{temp_prefix}_{self.next_var()}"
            return target_name, tuple_target_names
        raise NotImplementedError("Only simple comprehension targets supported")

    def _collect_inline_comp_walrus_names(
        self, exprs: Sequence[ast.AST], ifs: Sequence[ast.AST]
    ) -> list[str]:
        # Source order, deduplicated (deterministic): the result drives boxing
        # and the post-loop walrus-target sync emission, so it must not depend
        # on hash order (#34).
        walrus_names: list[str] = []
        seen: set[str] = set()
        for node in (*exprs, *ifs):
            for name in self._collect_namedexpr_names(node):
                if name not in seen:
                    seen.add(name)
                    walrus_names.append(name)
        return walrus_names

    def _collect_inline_comp_lambda_free_vars(
        self, exprs: Sequence[ast.AST], ifs: Sequence[ast.AST]
    ) -> set[str]:
        lambda_free_vars: set[str] = set()
        for root in [*exprs, *ifs]:
            for child in ast.walk(root):
                if isinstance(
                    child, (ast.Lambda, ast.FunctionDef, ast.AsyncFunctionDef)
                ):
                    for inner in ast.walk(child):
                        if isinstance(inner, ast.Name) and isinstance(
                            inner.ctx, ast.Load
                        ):
                            lambda_free_vars.add(inner.id)
        return lambda_free_vars

    def _emit_inline_simple_comp(
        self,
        node: ast.ListComp | ast.SetComp | ast.DictComp,
        *,
        result_type_hint: str,
        result_op: str,
        temp_prefix: str,
        emit_result_values: Callable[[MoltValue, list[MoltValue]], None],
    ) -> MoltValue:
        """Emit an inline loop for a simple collection comprehension.

        Avoids generating a generator task, working around a native-backend
        Cranelift code-generation issue where generator poll functions with
        non-trivial element expressions produce corrupted state machines.
        """
        comp = node.generators[0]
        exprs = self._inline_simple_comp_exprs(node)
        target_name, tuple_target_names = self._inline_simple_comp_target(
            comp, temp_prefix
        )
        # Collect walrus (:=) targets in the element expression and
        # filters. These must leak to the enclosing scope per PEP 572.
        walrus_names = self._collect_inline_comp_walrus_names(exprs, comp.ifs)
        # At module scope the single storage authority for a name is the
        # module dict (MODULE_SET_ATTR / MODULE_GET_ATTR), not a boxed
        # function cell: other functions read the global via the module dict,
        # and module-scope SSA refs dangle across chunk boundaries (#45 item
        # 3).  A walrus target that is *also* bound non-comprehensionally (a
        # ``while`` test walrus, a plain assignment, ...) writes through the
        # module dict, so the comprehension must read/write the same dict —
        # boxing it into a transient cell forks storage and the comp reads a
        # stale/None cell instead of the loop-carried value.  Route module
        # walrus targets through the module dict, exactly as the GeneratorExp
        # path already does (visit_GeneratorExp), and box only at non-module
        # scope.
        module_walrus_names: list[str] = []
        if (
            self.current_func_name == "molt_main"
            and getattr(self, "module_obj", None) is not None
        ):
            module_walrus_names = list(walrus_names)
            if module_walrus_names:
                self.module_global_mutations.update(module_walrus_names)
                # A bare module-scope name read is a LOAD_GLOBAL: it must raise
                # NameError (not the AttributeError that MODULE_GET_ATTR yields)
                # when the binding is absent — e.g. ``[acc := acc + i for i in
                # r]`` with no prior ``acc`` reads an unbound name.  del_targets
                # is the carrier for "module name that may be read while unbound
                # → route through MODULE_GET_GLOBAL" (see _collect_deleted_names:
                # it covers except-handler targets too, not only ``del``), so
                # registering the walrus targets there gives the same NameError
                # semantics the GeneratorExp poll-fn path gets via global_decls.
                self.del_targets.update(module_walrus_names)
                # Drop any SSA/cell caches so subsequent reads at module scope
                # re-read from the module dict (the comprehension writes there
                # via _store_local_value's module_global_mutations branch).
                for wname in module_walrus_names:
                    self.locals.pop(wname, None)
                    self.globals.pop(wname, None)
                    self.exact_locals.pop(wname, None)
                    self.boxed_locals.pop(wname, None)
                    self.boxed_local_hints.pop(wname, None)
        # Box walrus targets so their values survive the loop boundary.
        # The boxed cell lives on the heap, so store_index inside the
        # loop persists and index after the loop reads the final value.
        # Module-scope walrus targets are excluded — they live in the module
        # dict (handled above).
        module_walrus_set = set(module_walrus_names)
        for wname in walrus_names:
            if wname in module_walrus_set:
                continue
            if wname not in self.boxed_locals:
                self._box_local(wname)
        # If the element expression or filters contain lambdas that
        # reference the iteration variable, box it so the lambda can
        # capture it as a closure cell.  Without boxing, the iteration
        # variable is a plain SSA local that lambdas can't close over.
        lambda_free_vars = self._collect_inline_comp_lambda_free_vars(exprs, comp.ifs)
        outer_boxed = self.boxed_locals.pop(target_name, None)
        outer_boxed_hint = self.boxed_local_hints.pop(target_name, None)
        # When unpacking a tuple target, the user-named locals are bound
        # inside the loop body too. Save their pre-comp values so we can
        # restore them after the comprehension exits (CPython per-comp
        # scoping: the iteration variables don't leak).
        saved_tuple_locals: dict[str, MoltValue | None] = {}
        saved_tuple_boxed: dict[str, MoltValue | None] = {}
        saved_tuple_boxed_hints: dict[str, str | None] = {}
        tuple_cells: dict[str, MoltValue] = {}
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                saved_tuple_locals[tname] = self.locals.get(tname)
                saved_tuple_boxed[tname] = self.boxed_locals.pop(tname, None)
                saved_tuple_boxed_hints[tname] = self.boxed_local_hints.pop(tname, None)
        comp_cell: MoltValue | None = None
        if target_name in lambda_free_vars:
            missing = MoltValue(self.next_var(), type_hint="missing")
            self.emit(MoltOp(kind="MISSING", args=[], result=missing))
            comp_cell = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[missing], result=comp_cell))
            self.boxed_locals[target_name] = comp_cell
            self.boxed_local_hints[target_name] = "Any"
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                if tname not in lambda_free_vars:
                    continue
                missing = MoltValue(self.next_var(), type_hint="missing")
                self.emit(MoltOp(kind="MISSING", args=[], result=missing))
                tcell = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_NEW", args=[missing], result=tcell))
                tuple_cells[tname] = tcell
                self.boxed_locals[tname] = tcell
                self.boxed_local_hints[tname] = "Any"
        iterable_val = self.visit(comp.iter)
        iter_obj = self._emit_iter_new(iterable_val)
        res = MoltValue(self.next_var(), type_hint=result_type_hint)
        self.emit(MoltOp(kind=result_op, args=[], result=res))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        outer_comp_shadow_locals = set(self.comp_shadow_locals)
        self.comp_shadow_locals.add(target_name)
        if tuple_target_names is not None:
            self.comp_shadow_locals.update(tuple_target_names)
        # If the iteration variable is boxed, save the current cell value so
        # we can restore it after the comprehension (CPython scoping: the comp
        # does not leak its iteration variable into the enclosing scope).
        cell = comp_cell
        saved_cell_val: MoltValue | None = None
        if cell is not None:
            _save_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=_save_idx))
            saved_cell_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(kind="INDEX", args=[cell, _save_idx], result=saved_cell_val)
            )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        iter_elem_hint = self._iterable_element_hint(iterable_val) or "Any"
        item = MoltValue(self.next_var(), type_hint=iter_elem_hint)
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        # Bind the loop variable so the element expression can reference it.
        old_local = self.locals.get(target_name)
        restore_local: MoltValue | None = old_local
        if restore_local is None and target_name not in self.boxed_locals:
            restore_local = MoltValue(self.next_var(), type_hint="missing")
            self.emit(MoltOp(kind="MISSING", args=[], result=restore_local))
        self.locals[target_name] = item
        if target_name not in self.boxed_locals:
            self._store_comprehension_local_value(target_name, item)
        # If the variable is boxed, write through to the cell so that
        # _load_local_value reads the current iteration value.
        if cell is not None:
            _box_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=_box_idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, _box_idx, item],
                    result=MoltValue("none"),
                )
            )
        # If the original target was a tuple, unpack the synthetic temp
        # Name into comprehension-local user names. Do not route this through
        # normal assignment lowering: module-level comprehension targets must
        # not publish module globals, and tuple element names need their own
        # cells when nested lambdas close over them.
        if tuple_target_names is not None:
            item_vals = [
                MoltValue(self.next_var(), type_hint="Any") for _ in tuple_target_names
            ]
            self.emit(
                MoltOp(
                    kind="UNPACK_SEQUENCE",
                    args=[item] + item_vals,
                    result=MoltValue("none"),
                    metadata={"expected_count": len(tuple_target_names)},
                )
            )
            for tname, item_val in zip(tuple_target_names, item_vals):
                self._store_comprehension_local_value(tname, item_val)
        # Evaluate optional filter conditions.
        skip_label_needed = bool(comp.ifs)
        if skip_label_needed:
            for if_node in comp.ifs:
                cond_val = self.visit(if_node)
                not_cond = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[cond_val], result=not_cond))
                self.emit(MoltOp(kind="IF", args=[not_cond], result=MoltValue("none")))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        values: list[MoltValue] = []
        for expr in exprs:
            value = self.visit(expr)
            if value is None:
                raise NotImplementedError("Unsupported comprehension expression")
            values.append(cast(MoltValue, value))
        emit_result_values(res, values)
        # Restore the previous binding (if any). When this comprehension
        # created a closure-owned cell, never write the outer value through
        # _store_local_value while boxed_locals[target_name] still points at
        # the comprehension cell; late-bound lambdas must retain the final
        # iteration value in that cell.
        if restore_local is not None and cell is None:
            self._store_local_value(target_name, restore_local)
        if old_local is not None:
            self.locals[target_name] = old_local
        else:
            self.locals.pop(target_name, None)
        # Restore any user-named locals bound by tuple-unpacking the
        # iteration value.
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                prior = saved_tuple_locals.get(tname)
                if prior is not None:
                    self.locals[tname] = prior
                else:
                    self.locals.pop(tname, None)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        # Post-loop: restore the saved cell value so that the outer scope sees
        # its original value (e.g. ``print(i)`` after the comp returns the
        # outer for-loop's final ``i``, not the comp's last iteration value).
        # Exception: if a lambda inside the comp captures the iteration variable
        # via late binding (``lambda: i``), leave the cell with the final loop
        # value — the closure needs it.  Default-arg capture (``lambda i=i: i``)
        # does NOT appear in lambda_free_vars since ``i`` is a parameter.
        _closure_captured = target_name in lambda_free_vars
        if cell is not None and saved_cell_val is not None and not _closure_captured:
            _post_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=_post_idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, _post_idx, saved_cell_val],
                    result=MoltValue("none"),
                )
            )
        # Sync walrus (:=) targets to the enclosing scope.  The boxed
        # cell was updated inside the loop; read the final value and
        # store it as the local (and module attr at module scope) so
        # subsequent code sees the walrus assignment.
        # Module-scope walrus targets are skipped: they have no boxed cell
        # (they store straight to the module dict each iteration via
        # _store_local_value's module_global_mutations branch), so the dict
        # already holds the final value and is the authoritative reader.
        for wname in walrus_names:
            if wname in module_walrus_set:
                continue
            wcell = self._load_boxed_cell(wname)
            if wcell is not None:
                _widx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=_widx))
                wval = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[wcell, _widx], result=wval))
                self.locals[wname] = wval
                if (
                    self.current_func_name == "molt_main"
                    and hasattr(self, "module_obj")
                    and self.module_obj is not None
                ):
                    self._emit_module_attr_set_on(self.module_obj, wname, wval)
        if comp_cell is not None:
            self.boxed_locals.pop(target_name, None)
            self.boxed_local_hints.pop(target_name, None)
        if outer_boxed is not None:
            self.boxed_locals[target_name] = outer_boxed
            if outer_boxed_hint is not None:
                self.boxed_local_hints[target_name] = outer_boxed_hint
        if tuple_target_names is not None:
            for tname in tuple_target_names:
                self.boxed_locals.pop(tname, None)
                self.boxed_local_hints.pop(tname, None)
                prior_boxed = saved_tuple_boxed.get(tname)
                prior_hint = saved_tuple_boxed_hints.get(tname)
                if prior_boxed is not None:
                    self.boxed_locals[tname] = prior_boxed
                    if prior_hint is not None:
                        self.boxed_local_hints[tname] = prior_hint
        self.comp_shadow_locals = outer_comp_shadow_locals
        return res

    def _emit_inline_list_comp(self, node: ast.ListComp) -> MoltValue:
        def emit_list_value(res: MoltValue, values: list[MoltValue]) -> None:
            elt_val = values[0]
            self.emit(
                MoltOp(
                    kind="LIST_APPEND",
                    args=[res, elt_val],
                    result=MoltValue("none"),
                )
            )
            # Propagate element type hint to the result list.
            elt_hint = elt_val.type_hint if isinstance(elt_val, MoltValue) else None
            if elt_hint and elt_hint not in {"Any", "Unknown"}:
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = elt_hint
                else:
                    self.container_elem_hints[res.name] = elt_hint

        return self._emit_inline_simple_comp(
            node,
            result_type_hint="list",
            result_op="LIST_NEW",
            temp_prefix="__molt_listcomp_unpack",
            emit_result_values=emit_list_value,
        )

    def _emit_inline_set_comp(self, node: ast.SetComp) -> MoltValue:
        def emit_set_value(res: MoltValue, values: list[MoltValue]) -> None:
            self.emit(
                MoltOp(
                    kind="SET_ADD",
                    args=[res, values[0]],
                    result=MoltValue("none"),
                )
            )

        return self._emit_inline_simple_comp(
            node,
            result_type_hint="set",
            result_op="SET_NEW",
            temp_prefix="__molt_setcomp_unpack",
            emit_result_values=emit_set_value,
        )

    def _emit_inline_dict_comp(self, node: ast.DictComp) -> MoltValue:
        def emit_dict_item(res: MoltValue, values: list[MoltValue]) -> None:
            key_val, item_val = values
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res, key_val, item_val],
                    result=MoltValue("none"),
                )
            )

        return self._emit_inline_simple_comp(
            node,
            result_type_hint="dict",
            result_op="DICT_NEW",
            temp_prefix="__molt_dictcomp_unpack",
            emit_result_values=emit_dict_item,
        )

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


    def _match_dict_increment_assign(
        self, node: ast.Assign
    ) -> tuple[ast.expr, ast.expr, ast.expr] | None:
        if len(node.targets) != 1:
            return None
        target = node.targets[0]
        if not isinstance(target, ast.Subscript) or isinstance(target.slice, ast.Slice):
            return None
        if not isinstance(target.value, ast.Name):
            return None
        target_key = target.slice
        if not self._dict_increment_key_is_single_eval_safe(target_key):
            return None
        if not isinstance(node.value, ast.BinOp) or not isinstance(
            node.value.op, ast.Add
        ):
            return None

        dict_name = target.value.id
        key_dump = ast.dump(target_key, include_attributes=False)

        def is_matching_get(expr: ast.expr) -> bool:
            if not isinstance(expr, ast.Call) or expr.keywords:
                return False
            if not isinstance(expr.func, ast.Attribute) or expr.func.attr != "get":
                return False
            if (
                not isinstance(expr.func.value, ast.Name)
                or expr.func.value.id != dict_name
            ):
                return False
            if len(expr.args) == 1:
                key_expr = expr.args[0]
                default_expr: ast.expr = ast.Constant(value=0)
            elif len(expr.args) == 2:
                key_expr, default_expr = expr.args
            else:
                return False
            if ast.dump(key_expr, include_attributes=False) != key_dump:
                return False
            return (
                isinstance(default_expr, ast.Constant)
                and isinstance(default_expr.value, int)
                and default_expr.value == 0
            )

        if is_matching_get(node.value.left):
            delta_expr = node.value.right
        elif is_matching_get(node.value.right):
            delta_expr = node.value.left
        else:
            return None
        return target.value, target_key, delta_expr

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

    def _match_split_dict_increment_for_loop(
        self, node: ast.For
    ) -> tuple[ast.expr, ast.expr, ast.expr | None, ast.expr] | None:
        if self.is_async():
            return None
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1 or not isinstance(node.body[0], ast.Assign):
            return None
        iter_call = node.iter
        if not isinstance(iter_call, ast.Call) or iter_call.keywords:
            return None
        if (
            not isinstance(iter_call.func, ast.Attribute)
            or iter_call.func.attr != "split"
        ):
            return None
        if len(iter_call.args) > 1:
            return None
        assign = node.body[0]
        match = self._match_dict_increment_assign(assign)
        if match is None:
            return None
        dict_expr, key_expr, delta_expr = match
        if not isinstance(key_expr, ast.Name) or key_expr.id != node.target.id:
            return None
        if not isinstance(dict_expr, ast.Name):
            return None
        sep_expr: ast.expr | None = None
        if iter_call.args:
            sep_expr = iter_call.args[0]
        return dict_expr, iter_call.func.value, sep_expr, delta_expr

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

    def _match_taq_ingest_loop_body(
        self, body: list[ast.stmt]
    ) -> tuple[str | None, str, str, str, ast.expr] | None:
        if self.is_async():
            return None
        idx = 0
        header_name: str | None = None
        if body:
            header_name = self._is_taq_header_guard(body[0])
            if header_name is not None:
                idx = 1
        rest = body[idx:]
        if len(rest) != 7:
            return None

        split_stmt = rest[0]
        guard_stmt = rest[1]
        ts_stmt = rest[2]
        sym_stmt = rest[3]
        vol_stmt = rest[4]
        setdefault_stmt = rest[5]
        append_stmt = rest[6]

        if not isinstance(split_stmt, ast.Assign):
            return None
        if len(split_stmt.targets) != 1 or not isinstance(
            split_stmt.targets[0], ast.Name
        ):
            return None
        split_name = split_stmt.targets[0].id
        if not isinstance(split_stmt.value, ast.Call) or split_stmt.value.keywords:
            return None
        split_call = split_stmt.value
        if len(split_call.args) != 1:
            return None
        if (
            not isinstance(split_call.func, ast.Attribute)
            or split_call.func.attr != "split"
        ):
            return None
        if not isinstance(split_call.func.value, ast.Name):
            return None
        line_name = split_call.func.value.id
        if not (
            isinstance(split_call.args[0], ast.Constant)
            and split_call.args[0].value == "|"
        ):
            return None

        def match_sub(name: str, index: int, expr: ast.expr) -> bool:
            return (
                isinstance(expr, ast.Subscript)
                and not isinstance(expr.slice, ast.Slice)
                and isinstance(expr.value, ast.Name)
                and expr.value.id == name
                and isinstance(expr.slice, ast.Constant)
                and expr.slice.value == index
            )

        if not isinstance(guard_stmt, ast.If):
            return None
        if guard_stmt.orelse or len(guard_stmt.body) != 1:
            return None
        if not isinstance(guard_stmt.body[0], ast.Continue):
            return None
        guard_test = guard_stmt.test
        if not isinstance(guard_test, ast.BoolOp) or not isinstance(
            guard_test.op, ast.Or
        ):
            return None
        if len(guard_test.values) != 2:
            return None
        checks: set[tuple[int, str]] = set()
        for clause in guard_test.values:
            if not isinstance(clause, ast.Compare):
                return None
            if len(clause.ops) != 1 or not isinstance(clause.ops[0], ast.Eq):
                return None
            if len(clause.comparators) != 1:
                return None
            rhs = clause.comparators[0]
            if not isinstance(rhs, ast.Constant) or not isinstance(rhs.value, str):
                return None
            if not isinstance(clause.left, ast.Subscript):
                return None
            if not isinstance(clause.left.value, ast.Name):
                return None
            if clause.left.value.id != split_name:
                return None
            if not isinstance(clause.left.slice, ast.Constant):
                return None
            idx_val = clause.left.slice.value
            if not isinstance(idx_val, int):
                return None
            if idx_val not in (0, 4):
                return None
            checks.add((idx_val, rhs.value))
        if checks != {(0, "END"), (4, "ENDP")}:
            return None

        if not isinstance(ts_stmt, ast.Assign):
            return None
        if len(ts_stmt.targets) != 1 or not isinstance(ts_stmt.targets[0], ast.Name):
            return None
        ts_name = ts_stmt.targets[0].id
        if (
            not isinstance(ts_stmt.value, ast.Call)
            or ts_stmt.value.keywords
            or len(ts_stmt.value.args) != 1
            or not isinstance(ts_stmt.value.func, ast.Name)
            or ts_stmt.value.func.id != "int"
            or not match_sub(split_name, 0, ts_stmt.value.args[0])
        ):
            return None

        if not isinstance(sym_stmt, ast.Assign):
            return None
        if len(sym_stmt.targets) != 1 or not isinstance(sym_stmt.targets[0], ast.Name):
            return None
        sym_name = sym_stmt.targets[0].id
        if not match_sub(split_name, 2, sym_stmt.value):
            return None

        if not isinstance(vol_stmt, ast.Assign):
            return None
        if len(vol_stmt.targets) != 1 or not isinstance(vol_stmt.targets[0], ast.Name):
            return None
        vol_name = vol_stmt.targets[0].id
        if (
            not isinstance(vol_stmt.value, ast.Call)
            or vol_stmt.value.keywords
            or len(vol_stmt.value.args) != 1
            or not isinstance(vol_stmt.value.func, ast.Name)
            or vol_stmt.value.func.id != "int"
            or not match_sub(split_name, 4, vol_stmt.value.args[0])
        ):
            return None

        if not isinstance(setdefault_stmt, ast.Assign):
            return None
        if len(setdefault_stmt.targets) != 1 or not isinstance(
            setdefault_stmt.targets[0], ast.Name
        ):
            return None
        series_name = setdefault_stmt.targets[0].id
        if (
            not isinstance(setdefault_stmt.value, ast.Call)
            or setdefault_stmt.value.keywords
            or len(setdefault_stmt.value.args) != 2
            or not isinstance(setdefault_stmt.value.func, ast.Attribute)
            or setdefault_stmt.value.func.attr != "setdefault"
            or not isinstance(setdefault_stmt.value.func.value, ast.Name)
            or not isinstance(setdefault_stmt.value.args[0], ast.Name)
            or not isinstance(setdefault_stmt.value.args[1], ast.List)
            or setdefault_stmt.value.args[1].elts
        ):
            return None
        data_name = setdefault_stmt.value.func.value.id
        if setdefault_stmt.value.args[0].id != sym_name:
            return None

        if not isinstance(append_stmt, ast.Expr) or not isinstance(
            append_stmt.value, ast.Call
        ):
            return None
        append_call = append_stmt.value
        if (
            append_call.keywords
            or len(append_call.args) != 1
            or not isinstance(append_call.func, ast.Attribute)
            or append_call.func.attr != "append"
            or not isinstance(append_call.func.value, ast.Name)
            or append_call.func.value.id != series_name
        ):
            return None
        arg0 = append_call.args[0]
        if not isinstance(arg0, ast.Tuple) or len(arg0.elts) != 2:
            return None
        first, second = arg0.elts
        if not (
            isinstance(first, ast.BinOp)
            and isinstance(first.op, ast.FloorDiv)
            and isinstance(first.left, ast.Name)
            and first.left.id == ts_name
            and isinstance(second, ast.Name)
            and second.id == vol_name
        ):
            return None

        return header_name, data_name, line_name, split_name, first.right

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



    def _emit_await_anext(
        self,
        iter_obj: MoltValue,
        *,
        default_val: MoltValue | None,
        has_default: bool,
    ) -> MoltValue:
        if iter_obj.type_hint in {"iter", "generator"}:
            pair = self._emit_iter_next_checked(iter_obj)
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[pair, none_val], result=is_none))
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            err_val = self._emit_exception_new("TypeError", "object is not an iterator")
            self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            if has_default:
                if default_val is None:
                    default_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
            else:
                default_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
            res_cell = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[default_val], result=res_cell))
            self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
            if not has_default:
                stop_val = self._emit_exception_new("StopAsyncIteration", "")
                self.emit(
                    MoltOp(kind="RAISE", args=[stop_val], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res_cell, zero, val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[res_cell, zero], result=res))
            return res

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        awaitable = MoltValue(self.next_var(), type_hint="Future")
        self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=awaitable))
        if has_default:
            if default_val is None:
                default_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        else:
            default_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        res_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[default_val], result=res_cell))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        cell_slot: int | None = None
        if self.is_async():
            cell_slot = self._async_local_offset(
                f"__anext_cell_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", cell_slot, res_cell],
                    result=MoltValue("none"),
                )
            )
        with self._suppress_check_exception():
            exc_val = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
            pending = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))
            self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
            kind_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="EXCEPTION_KIND", args=[exc_val], result=kind_val))
            stop_kind = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["StopAsyncIteration"], result=stop_kind)
            )
            is_stop = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="STRING_EQ", args=[kind_val, stop_kind], result=is_stop)
            )
            self.emit(MoltOp(kind="IF", args=[is_stop], result=MoltValue("none")))
            if not has_default:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
            else:
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        awaited_val = self._emit_await_value(awaitable, raise_pending=False)
        with self._suppress_check_exception():
            exc_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after)
            )
            pending_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[is_none_after], result=pending_after))
            self.emit(MoltOp(kind="IF", args=[pending_after], result=MoltValue("none")))
            kind_after = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="EXCEPTION_KIND", args=[exc_after], result=kind_after)
            )
            stop_after = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["StopAsyncIteration"], result=stop_after)
            )
            is_stop_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="STRING_EQ",
                    args=[kind_after, stop_after],
                    result=is_stop_after,
                )
            )
            self.emit(MoltOp(kind="IF", args=[is_stop_after], result=MoltValue("none")))
            if not has_default:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none"))
                )
            else:
                self.emit(
                    MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
        if cell_slot is not None:
            res_cell_after = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell_slot],
                    result=res_cell_after,
                )
            )
            zero_after = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero_after))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res_cell_after, zero_after, awaited_val],
                    result=MoltValue("none"),
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res_cell, zero, awaited_val],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        if cell_slot is not None:
            res_cell_final = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell_slot],
                    result=res_cell_final,
                )
            )
            zero_final = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero_final))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(kind="INDEX", args=[res_cell_final, zero_final], result=res)
            )
        else:
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[res_cell, zero], result=res))
        return res


    def _emit_awaitable_transform(self, awaitable: MoltValue) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__await__"], result=name_val))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[awaitable], result=cell))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        is_native = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="IS_NATIVE_AWAITABLE", args=[awaitable], result=is_native)
        )
        not_native = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_native], result=not_native))
        has_attr = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="HASATTR_NAME", args=[awaitable, name_val], result=has_attr)
        )
        should_transform = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="AND", args=[not_native, has_attr], result=should_transform)
        )
        self.emit(MoltOp(kind="IF", args=[should_transform], result=MoltValue("none")))
        method = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="GETATTR_NAME", args=[awaitable, name_val], result=method)
        )
        awaited = self._emit_call_bound_or_func(method, [])
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, zero, awaited],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        final_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=final_val))
        return final_val

    def _emit_await_value(
        self, awaitable: MoltValue, *, raise_pending: bool = True
    ) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("await outside async function")
        awaitable_slot = self._async_local_offset(
            f"__await_future_{len(self.async_locals)}"
        )
        awaitable_cached = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", awaitable_slot],
                result=awaitable_cached,
            )
        )
        none_cached = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_cached))
        is_none_cached = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="IS",
                args=[awaitable_cached, none_cached],
                result=is_none_cached,
            )
        )
        zero_cached = MoltValue(self.next_var(), type_hint="float")
        self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=zero_cached))
        is_zero_cached = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="IS",
                args=[awaitable_cached, zero_cached],
                result=is_zero_cached,
            )
        )
        is_empty_cached = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="OR",
                args=[is_none_cached, is_zero_cached],
                result=is_empty_cached,
            )
        )
        self.emit(MoltOp(kind="IF", args=[is_empty_cached], result=MoltValue("none")))
        transformed = self._emit_awaitable_transform(awaitable)
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", awaitable_slot, transformed],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.state_count += 1
        pending_state_id = self.state_count
        self.emit(
            MoltOp(
                kind="STATE_LABEL", args=[pending_state_id], result=MoltValue("none")
            )
        )
        pending_state_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[pending_state_id], result=pending_state_val)
        )
        coro = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", awaitable_slot],
                result=coro,
            )
        )
        result_slot = self._async_local_offset(
            f"__await_result_{len(self.async_locals)}"
        )
        result_slot_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[result_slot], result=result_slot_val))
        self.state_count += 1
        next_state_id = self.state_count
        res_placeholder = MoltValue(self.next_var(), type_hint="Any")
        with self._suppress_check_exception(emit_on_exit=raise_pending):
            self.emit(
                MoltOp(
                    kind="STATE_TRANSITION",
                    args=[coro, result_slot_val, pending_state_val, next_state_id],
                    result=res_placeholder,
                )
            )
            cleared_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, cleared_val],
                    result=MoltValue("none"),
                )
            )
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(kind="LOAD_CLOSURE", args=["self", result_slot], result=res)
            )
            if raise_pending:
                self._emit_raise_if_pending()
        return res

    def _emit_state_yield_resume_try_starts(self) -> None:
        if not self.in_generator:
            return
        for scope in self.try_scopes:
            handler_label = scope.handler_label
            if handler_label is None:
                continue
            args = [handler_label] if scope.try_start_has_handler_value else []
            metadata = (
                None
                if scope.try_start_has_handler_value
                else {"try_region_id": handler_label}
            )
            self.emit(
                MoltOp(
                    kind="TRY_START",
                    args=args,
                    result=MoltValue("none"),
                    metadata=metadata,
                )
            )

    def _emit_state_yield_resume_entry(self, state_id: int) -> None:
        self.emit(MoltOp(kind="STATE_LABEL", args=[state_id], result=MoltValue("none")))
        self._emit_state_yield_resume_try_starts()



    def _run_ir_midend_passes(self, ops: list[MoltOp]) -> list[MoltOp]:
        self._refresh_midend_env_config_if_needed()
        # Dev-profile mid-end gate removed: MISSING-value miscompile fixes
        # (SCCP non-propagation, DCE protection, definite-assignment hardening)
        # are now in place (ROADMAP TL2).  MOLT_MIDEND_DEV_ENABLE is no longer
        # required — mid-end runs for both dev and release profiles.
        # Correctness gate: keep stdlib modules out of mid-end canonicalization
        # until canonicalized stdlib lowering is proven stable (ROADMAP TL2).
        if self._source_is_stdlib_module:
            self.midend_stats["midend_module_skips"] += 1
            return ops
        ops = self._coalesce_check_exception_ops(ops)
        ops, structural_rewrites = self._ensure_structural_cfg_validity(
            ops, stage="midend_entry"
        )
        self.midend_stats["cfg_structural_canonicalizations"] += structural_rewrites
        oversized_skip_threshold = self.midend_env.skip_op_threshold
        _module_function_count, _module_total_ops, monolith_pressure_level = (
            self._current_module_pressure_snapshot()
        )
        effective_skip_threshold = oversized_skip_threshold
        if monolith_pressure_level >= 1:
            effective_skip_threshold = min(effective_skip_threshold, 650)
        if monolith_pressure_level >= 2:
            effective_skip_threshold = min(effective_skip_threshold, 500)
        if len(ops) >= effective_skip_threshold:
            cfg = build_cfg(ops)
            policy = self._resolve_midend_function_policy(
                ops,
                function_name=self._active_midend_function_name,
                block_count=max(1, len(cfg.blocks)),
            )
            if len(ops) >= oversized_skip_threshold or (
                not policy.promoted and policy.tier != "A"
            ):
                self.midend_stats["midend_oversized_function_skips"] += 1
                self._record_midend_policy_outcome(
                    policy=policy,
                    spent_ms=0.0,
                    work_units_spent=0.0,
                    degraded=True,
                    degrade_events=[
                        {
                            "reason": "oversized_function_skip",
                            "stage": "midend_entry",
                            "action": "emit_unoptimized_ir",
                            "spent_ms": 0.0,
                            "value": {
                                "op_count": len(ops),
                                "threshold": effective_skip_threshold,
                            },
                        }
                    ],
                    round_snapshots=[],
                )
                return ops
        return self._canonicalize_control_aware_ops(ops)

    def _midend_function_stats(self) -> dict[str, int]:
        name = self._active_midend_function_name
        stats = self.midend_stats_by_function.get(name)
        if stats is None:
            stats = {
                "sccp_attempted": 0,
                "sccp_accepted": 0,
                "sccp_iteration_cap_hits": 0,
                "edge_thread_attempted": 0,
                "edge_thread_accepted": 0,
                "edge_thread_rejected": 0,
                "loop_rewrite_attempted": 0,
                "loop_rewrite_accepted": 0,
                "loop_rewrite_rejected": 0,
                "guard_hoist_attempted": 0,
                "guard_hoist_accepted": 0,
                "guard_hoist_rejected": 0,
                "fused_dict_guard_prunes": 0,
                "cse_attempted": 0,
                "cse_accepted": 0,
                "cse_readheap_attempted": 0,
                "cse_readheap_accepted": 0,
                "cse_readheap_rejected": 0,
                "gvn_attempted": 0,
                "gvn_accepted": 0,
                "licm_attempted": 0,
                "licm_accepted": 0,
                "licm_rejected": 0,
                "dce_attempted": 0,
                "dce_accepted": 0,
                "dce_pure_op_attempted": 0,
                "dce_pure_op_accepted": 0,
                "dce_pure_op_rejected": 0,
            }
            self.midend_stats_by_function[name] = stats
        return stats

    def _midend_pass_stats(self, pass_name: str) -> dict[str, Any]:
        func_name = self._active_midend_function_name
        per_func = self.midend_pass_stats_by_function.setdefault(func_name, {})
        stats = per_func.get(pass_name)
        if stats is None:
            stats = {
                "attempted": 0,
                "accepted": 0,
                "rejected": 0,
                "degraded": 0,
                "ms_total": 0.0,
                "ms_max": 0.0,
                "samples_ms": [],
            }
            per_func[pass_name] = stats
        return stats

    @staticmethod
    def _midend_csv_tokens(value: str) -> set[str]:
        return {token.strip() for token in value.split(",") if token.strip()}

    @staticmethod
    def _midend_float_env(name: str, default: float) -> float:
        raw = os.getenv(name, "").strip()
        if not raw:
            return default
        try:
            return float(raw)
        except ValueError:
            return default

    @staticmethod
    def _midend_positive_int_env(name: str, default: int, *, minimum: int = 1) -> int:
        floor = max(1, minimum)
        fallback = max(floor, int(default))
        raw = os.getenv(name, "").strip()
        if not raw:
            return fallback
        try:
            parsed = int(raw)
        except ValueError:
            return fallback
        if parsed < floor:
            return fallback
        return parsed

    def _resolve_midend_env_config(self) -> MidendEnvConfig:
        work_budget_override_raw = os.getenv("MOLT_MIDEND_WORK_BUDGET", "").strip()
        work_budget_override: float | None = None
        if work_budget_override_raw:
            try:
                work_budget_override = max(0.0, float(work_budget_override_raw))
            except ValueError:
                work_budget_override = None
        max_rounds_override = os.getenv("MOLT_MIDEND_MAX_ROUNDS", "").strip()
        sccp_iter_cap_override = os.getenv("MOLT_SCCP_MAX_ITERS", "").strip()
        cse_iter_cap_override = os.getenv("MOLT_CSE_MAX_ITERS", "").strip()
        return MidendEnvConfig(
            skip_op_threshold=self._midend_positive_int_env(
                "MOLT_MIDEND_SKIP_OP_THRESHOLD", 800, minimum=1
            ),
            monolith_function_threshold=max(
                8,
                self._midend_positive_int_env(
                    "MOLT_MIDEND_MONOLITH_FUNCTION_THRESHOLD", 48
                ),
            ),
            monolith_total_ops_threshold=max(
                256,
                self._midend_positive_int_env(
                    "MOLT_MIDEND_MONOLITH_TOTAL_OPS_THRESHOLD", 4000
                ),
            ),
            hot_tier_promotion_enabled=os.getenv("MOLT_MIDEND_HOT_TIER_PROMOTION", "1")
            .strip()
            .lower()
            not in {"0", "false", "no", "off"},
            work_budget_override=work_budget_override,
            budget_alpha=self._midend_float_env("MOLT_MIDEND_BUDGET_ALPHA", 0.03),
            budget_beta=self._midend_float_env("MOLT_MIDEND_BUDGET_BETA", 0.75),
            budget_scale=max(
                0.0, self._midend_float_env("MOLT_MIDEND_BUDGET_SCALE", 1.0)
            ),
            max_rounds_override=(
                self._midend_positive_int_env("MOLT_MIDEND_MAX_ROUNDS", 2, minimum=2)
                if max_rounds_override
                else None
            ),
            sccp_iter_cap_override=(
                self._midend_positive_int_env("MOLT_SCCP_MAX_ITERS", 1, minimum=1)
                if sccp_iter_cap_override
                else None
            ),
            cse_iter_cap_override=(
                self._midend_positive_int_env("MOLT_CSE_MAX_ITERS", 1, minimum=1)
                if cse_iter_cap_override
                else None
            ),
            cse_fp_max_iters=self._midend_positive_int_env(
                "MOLT_CSE_FP_MAX_ITERS", 3, minimum=1
            ),
        )

    @staticmethod
    def _capture_midend_env_snapshot() -> tuple[str | None, ...]:
        return tuple(os.environ.get(name) for name in _MIDEND_ENV_KEYS)

    def _adjust_module_pressure_counts(
        self,
        *,
        function_delta: int = 0,
        ops_delta: int = 0,
    ) -> None:
        self._module_pressure_function_count += function_delta
        self._module_pressure_total_ops += ops_delta

    def _sync_module_pressure_counts_from_funcs_map(self) -> None:
        function_count = 0
        total_ops = 0
        for name, info in self.funcs_map.items():
            if not isinstance(info, dict):
                continue
            if name != "molt_main" and "ops" in info:
                function_count += 1
            total_ops += len(info.get("ops", []))
        self._module_pressure_function_count = function_count
        self._module_pressure_total_ops = total_ops
        self._module_pressure_funcs_map_ref = self.funcs_map

    def _current_module_pressure_snapshot(self) -> tuple[int, int, int]:
        if self._module_pressure_funcs_map_ref is not self.funcs_map:
            self._sync_module_pressure_counts_from_funcs_map()
        function_count = self._module_pressure_function_count
        total_ops = self._module_pressure_total_ops
        func_threshold = self.midend_env.monolith_function_threshold
        ops_threshold = self.midend_env.monolith_total_ops_threshold
        hard_func_threshold = max(func_threshold + 1, func_threshold * 2)
        hard_ops_threshold = max(ops_threshold + 1, ops_threshold * 2)
        level = 0
        if function_count >= func_threshold or total_ops >= ops_threshold:
            level = 1
        if function_count >= hard_func_threshold or total_ops >= hard_ops_threshold:
            level = 2
        return function_count, total_ops, level

    def _new_tracked_ops(
        self,
        initial: list[MoltOp] | None = None,
        *,
        count_function: bool = False,
    ) -> _TrackedOpsList:
        if count_function:
            self._module_pressure_function_count += 1
        tracked = _TrackedOpsList(self, initial)
        self._module_pressure_total_ops += len(tracked)
        return tracked

    def _refresh_midend_env_config_if_needed(self) -> None:
        snapshot = self._capture_midend_env_snapshot()
        if snapshot == self._midend_env_snapshot:
            return
        self.midend_env = self._resolve_midend_env_config()
        self._midend_env_snapshot = snapshot

    def _midend_hot_function_match(self, function_name: str) -> str | None:
        if not self.midend_hot_functions:
            return None
        module_name = self.module_name or ""
        aliases: set[str] = {function_name}
        if module_name:
            aliases.add(f"{module_name}::{function_name}")
            aliases.add(f"{module_name}.{function_name}")
        if function_name == "molt_main":
            init_symbol = self.module_init_symbol(module_name or "__main__")
            aliases.add(init_symbol)
            if module_name:
                aliases.add(f"{module_name}::{init_symbol}")
                aliases.add(f"{module_name}.{init_symbol}")
        for alias in sorted(aliases):
            if alias in self.midend_hot_functions:
                return alias
        return None

    @staticmethod
    def _promote_midend_tier_one_step(tier: MidendTier) -> MidendTier:
        if tier == "C":
            return "B"
        if tier == "B":
            return "A"
        return tier

    def _classify_midend_tier(
        self, function_name: str, ops: list[MoltOp]
    ) -> MidendTierClassification:
        forced_tier = os.getenv("MOLT_MIDEND_TIER_FORCE", "").strip().upper()
        if forced_tier in {"A", "B", "C"}:
            return MidendTierClassification(
                tier=cast(MidendTier, forced_tier),
                source="forced_env",
                allow_hot_promotion=False,
            )

        tier_a_functions = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_A_FUNCTIONS", "")
        )
        tier_b_functions = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_B_FUNCTIONS", "")
        )
        tier_c_functions = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_C_FUNCTIONS", "")
        )
        if function_name in tier_a_functions:
            return MidendTierClassification(
                tier="A",
                source="function_override",
                allow_hot_promotion=False,
            )
        if function_name in tier_c_functions:
            return MidendTierClassification(
                tier="C",
                source="function_override",
                allow_hot_promotion=False,
            )
        if function_name in tier_b_functions:
            return MidendTierClassification(
                tier="B",
                source="function_override",
                allow_hot_promotion=False,
            )

        module_name = self.module_name or ""
        tier_a_prefixes = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_A_MODULE_PREFIXES", "")
        )
        tier_b_prefixes = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_B_MODULE_PREFIXES", "")
        )
        tier_c_prefixes = self._midend_csv_tokens(
            os.getenv("MOLT_MIDEND_TIER_C_MODULE_PREFIXES", "")
        )
        for prefix in sorted(tier_a_prefixes):
            if module_name == prefix or module_name.startswith(f"{prefix}."):
                return MidendTierClassification(
                    tier="A",
                    source="module_prefix_override",
                    allow_hot_promotion=False,
                )
        for prefix in sorted(tier_c_prefixes):
            if module_name == prefix or module_name.startswith(f"{prefix}."):
                return MidendTierClassification(
                    tier="C",
                    source="module_prefix_override",
                    allow_hot_promotion=False,
                )
        for prefix in sorted(tier_b_prefixes):
            if module_name == prefix or module_name.startswith(f"{prefix}."):
                return MidendTierClassification(
                    tier="B",
                    source="module_prefix_override",
                    allow_hot_promotion=False,
                )

        op_count = len(ops)
        if function_name == "molt_main":
            if op_count >= 1800:
                return MidendTierClassification(
                    tier="C",
                    source="module_entry_oversized",
                    allow_hot_promotion=True,
                )
            return MidendTierClassification(
                tier="A",
                source="module_entry_default",
                allow_hot_promotion=True,
            )

        chunk_prefix = f"{self.module_prefix}{_MOLT_MODULE_CHUNK_PREFIX}_"
        if self._source_is_stdlib_module:
            # Stdlib defaults to the lightest tier unless explicitly elevated
            # via A/B overrides above.
            if function_name.startswith(chunk_prefix):
                return MidendTierClassification(
                    tier="C",
                    source="stdlib_chunk_default",
                    allow_hot_promotion=True,
                )
            return MidendTierClassification(
                tier="C",
                source="stdlib_default",
                allow_hot_promotion=True,
            )
        if op_count >= 1800:
            return MidendTierClassification(
                tier="C",
                source="op_count_threshold",
                allow_hot_promotion=True,
            )
        return MidendTierClassification(
            tier="B",
            source="default",
            allow_hot_promotion=True,
        )

    def _resolve_midend_function_policy(
        self,
        ops: list[MoltOp],
        *,
        function_name: str | None = None,
        block_count: int = 1,
    ) -> MidendFunctionPolicy:
        self._refresh_midend_env_config_if_needed()
        profile = self.optimization_profile
        profile_override = os.getenv("MOLT_MIDEND_PROFILE", "").strip().lower()
        if profile_override in {"dev", "release"}:
            profile = cast(MidendProfile, profile_override)

        resolved_function = function_name or self._active_midend_function_name
        tier_classification = self._classify_midend_tier(resolved_function, ops)
        tier_base = tier_classification.tier
        tier = tier_base
        promoted = False
        promotion_source = ""
        promotion_signal = ""
        module_function_count, module_total_ops, monolith_pressure_level = (
            self._current_module_pressure_snapshot()
        )
        hot_promotion_enabled = self.midend_env.hot_tier_promotion_enabled
        if hot_promotion_enabled and tier_classification.allow_hot_promotion:
            hot_signal = self._midend_hot_function_match(resolved_function)
            if hot_signal and tier_base in {"B", "C"}:
                promoted_tier = self._promote_midend_tier_one_step(tier_base)
                if promoted_tier != tier_base:
                    tier = promoted_tier
                    promoted = True
                    promotion_source = "pgo_hot_functions"
                    promotion_signal = hot_signal
        defaults: dict[tuple[MidendProfile, MidendTier], dict[str, Any]] = {
            ("dev", "A"): {
                "max_rounds": 2,
                "sccp_iter_cap": 48,
                "cse_iter_cap": 16,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 60.0,
            },
            ("dev", "B"): {
                "max_rounds": 1,
                "sccp_iter_cap": 24,
                "cse_iter_cap": 8,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 35.0,
            },
            ("dev", "C"): {
                "max_rounds": 1,
                "sccp_iter_cap": 12,
                "cse_iter_cap": 4,
                "enable_deep_edge_thread": False,
                "enable_cse": False,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 20.0,
            },
            ("release", "A"): {
                "max_rounds": 4,
                "sccp_iter_cap": 128,
                "cse_iter_cap": 48,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": True,
                "enable_guard_hoist": True,
                "budget_base_ms": 180.0,
            },
            ("release", "B"): {
                "max_rounds": 3,
                "sccp_iter_cap": 96,
                "cse_iter_cap": 32,
                "enable_deep_edge_thread": True,
                "enable_cse": True,
                "enable_licm": True,
                "enable_guard_hoist": True,
                "budget_base_ms": 110.0,
            },
            ("release", "C"): {
                "max_rounds": 2,
                "sccp_iter_cap": 48,
                "cse_iter_cap": 16,
                "enable_deep_edge_thread": False,
                "enable_cse": True,
                "enable_licm": False,
                "enable_guard_hoist": False,
                "budget_base_ms": 70.0,
            },
        }
        selected = dict(defaults[(profile, tier)])
        monolith_pressure_exempt = (
            resolved_function == "molt_main" or promoted or tier == "A"
        )
        if not monolith_pressure_exempt and monolith_pressure_level >= 1:
            selected["max_rounds"] = max(1, int(selected["max_rounds"]) - 1)
            selected["sccp_iter_cap"] = max(
                8, int(int(selected["sccp_iter_cap"]) * 0.75)
            )
            selected["cse_iter_cap"] = max(4, int(int(selected["cse_iter_cap"]) * 0.75))
            selected["budget_base_ms"] = float(selected["budget_base_ms"]) * 0.85
        if not monolith_pressure_exempt and monolith_pressure_level >= 2:
            selected["max_rounds"] = max(1, int(selected["max_rounds"]) - 1)
            selected["sccp_iter_cap"] = max(
                8, int(int(selected["sccp_iter_cap"]) * 0.75)
            )
            selected["cse_iter_cap"] = max(4, int(int(selected["cse_iter_cap"]) * 0.75))
            selected["enable_guard_hoist"] = False
            selected["budget_base_ms"] = float(selected["budget_base_ms"]) * 0.8
        alpha = self.midend_env.budget_alpha
        beta = self.midend_env.budget_beta
        scale = max(0.0, self.midend_env.budget_scale)
        budget_ms = (
            selected["budget_base_ms"]
            + alpha * max(1, len(ops))
            + beta * max(1, block_count)
        ) * scale
        budget_ms_override_raw = os.getenv("MOLT_MIDEND_BUDGET_MS", "").strip()
        if budget_ms_override_raw:
            try:
                budget_ms = max(0.0, float(budget_ms_override_raw))
            except ValueError:
                pass
        # Deterministic work-unit budget for the degrade ladder (#73).  The
        # ladder accumulates a deterministic cost — the op count processed —
        # at each inter-pass checkpoint, and degrades when the running total
        # exceeds this budget.  Because the work-units depend only on the IR
        # (op/block counts) and the deterministic per-tier round/iteration
        # caps, the resulting pass selection — and the emitted IR — is a pure
        # function of the input, independent of wall-clock timing.
        #
        # Calibration: a non-pathological function executes the full
        # `max_rounds` round loop, hitting ~`_MIDEND_DEGRADE_CHECKPOINTS`
        # work-charges per round, each ≈ the live op count.  The ceiling below
        # admits that nominal cost (plus generous per-round growth headroom and
        # the per-tier base) so normal functions never degrade, while a pass
        # that pathologically balloons the op count still trips the ladder and
        # bounds compile time — matching the original safety intent without its
        # nondeterminism.
        work_budget_override = self.midend_env.work_budget_override
        if work_budget_override is not None:
            work_budget = work_budget_override
        else:
            base_ops = max(1, len(ops))
            rounds = max(1, int(selected["max_rounds"]))
            nominal_round_work = _MIDEND_DEGRADE_CHECKPOINTS * base_ops
            work_budget = (
                float(selected["budget_base_ms"]) * _MIDEND_WORK_BASE_UNITS_PER_MS
                + _MIDEND_WORK_GROWTH_HEADROOM * rounds * nominal_round_work
                + beta * max(1, block_count)
            ) * scale
        return MidendFunctionPolicy(
            profile=profile,
            tier=tier,
            tier_base=tier_base,
            tier_source=tier_classification.source,
            promoted=promoted,
            promotion_source=promotion_source,
            promotion_signal=promotion_signal,
            max_rounds=max(2, int(selected["max_rounds"])),
            sccp_iter_cap=int(selected["sccp_iter_cap"]),
            cse_iter_cap=int(selected["cse_iter_cap"]),
            enable_deep_edge_thread=bool(selected["enable_deep_edge_thread"]),
            enable_cse=bool(selected["enable_cse"]),
            enable_licm=bool(selected["enable_licm"]),
            enable_guard_hoist=bool(selected["enable_guard_hoist"]),
            budget_ms=float(budget_ms),
            work_budget=float(work_budget),
            allow_hot_promotion=bool(
                tier_classification.allow_hot_promotion and hot_promotion_enabled
            ),
            module_function_count=module_function_count,
            module_total_ops=module_total_ops,
            monolith_pressure_level=monolith_pressure_level,
        )

    def _record_midend_pass_sample(
        self,
        pass_name: str,
        *,
        elapsed_ms: float,
        accepted: bool,
        degraded: bool = False,
    ) -> None:
        stats = self._midend_pass_stats(pass_name)
        stats["attempted"] = int(stats.get("attempted", 0)) + 1
        if accepted:
            stats["accepted"] = int(stats.get("accepted", 0)) + 1
        else:
            stats["rejected"] = int(stats.get("rejected", 0)) + 1
        if degraded:
            stats["degraded"] = int(stats.get("degraded", 0)) + 1
        stats["ms_total"] = float(stats.get("ms_total", 0.0)) + max(0.0, elapsed_ms)
        stats["ms_max"] = max(float(stats.get("ms_max", 0.0)), max(0.0, elapsed_ms))
        samples = stats.get("samples_ms")
        if not isinstance(samples, list):
            samples = []
            stats["samples_ms"] = samples
        samples.append(max(0.0, elapsed_ms))
        if len(samples) > 256:
            del samples[: len(samples) - 256]

    @staticmethod
    def _pass_stat_p95(samples: list[float]) -> float:
        if not samples:
            return 0.0
        ordered = sorted(samples)
        idx = max(0, min(len(ordered) - 1, int((len(ordered) - 1) * 0.95)))
        return float(ordered[idx])

    def _record_midend_policy_outcome(
        self,
        *,
        policy: MidendFunctionPolicy,
        spent_ms: float,
        work_units_spent: float,
        degraded: bool,
        degrade_events: list[dict[str, Any]],
        round_snapshots: list[dict[str, Any]] | None = None,
    ) -> None:
        self.midend_policy_outcomes_by_function[self._active_midend_function_name] = {
            "profile": policy.profile,
            "tier": policy.tier,
            "tier_base": policy.tier_base,
            "tier_source": policy.tier_source,
            "tier_effective": policy.tier,
            "promoted": policy.promoted,
            "promotion_source": policy.promotion_source,
            "promotion_signal": policy.promotion_signal,
            "allow_hot_promotion": policy.allow_hot_promotion,
            "module_function_count": policy.module_function_count,
            "module_total_ops": policy.module_total_ops,
            "monolith_pressure_level": policy.monolith_pressure_level,
            "budget_ms": round(policy.budget_ms, 3),
            "spent_ms": round(max(0.0, spent_ms), 3),
            "work_budget": round(max(0.0, policy.work_budget), 3),
            "work_units_spent": round(max(0.0, work_units_spent), 3),
            "degraded": degraded,
            "degrade_events": list(degrade_events),
            "round_snapshots": list(round_snapshots) if round_snapshots else [],
        }

    def _log_degrade_levels(
        self,
        degrade_level: int,
        reasons: list[str],
        budget_ms: float,
    ) -> None:
        """Log which functions hit which degrade level and why (MOL-27)."""
        func_name = self._active_midend_function_name
        outcome = self.midend_policy_outcomes_by_function.get(func_name)
        if outcome is not None:
            outcome["degrade_level"] = degrade_level
            outcome["degrade_level_reasons"] = list(reasons)
            outcome["per_func_budget_ms"] = round(budget_ms, 3)
        if os.getenv("MOLT_MIDEND_STATS") is not None:
            level_desc = {
                1: "skip LICM + guard hoist",
                2: "skip SCCP multi-pass",
                3: "skip all optimisation",
            }.get(degrade_level, f"unknown({degrade_level})")
            print(
                f"molt midend degrade: {func_name} level={degrade_level}"
                f" ({level_desc}) budget_ms={budget_ms:.1f}"
                f" reasons={reasons}",
                file=sys.stderr,
            )

    def _maybe_report_midend_stats(self) -> None:
        if self._midend_stats_reported:
            return
        if os.getenv("MOLT_MIDEND_STATS") is None:
            return
        self._midend_stats_reported = True
        ordered_keys = [
            "expanded_attempts",
            "expanded_accepted",
            "expanded_fallbacks",
            "midend_module_skips",
            "midend_oversized_function_skips",
            "invalid_unbound_rollback",
            "invalid_unbound_uses",
            "fixed_point_fail_fast",
            "cfg_structural_failures",
            "cfg_structural_canonicalizations",
            "sccp_iteration_cap_hits",
            "cse_dce_fp_cap_hits",
            "sccp_branch_prunes",
            "loop_edge_thread_prunes",
            "try_edge_thread_prunes",
            "unreachable_blocks_removed",
            "cfg_region_prunes",
            "label_prunes",
            "jump_noop_elisions",
            "licm_hoists",
            "guard_hoist_attempts",
            "guard_hoist_accepted",
            "guard_hoist_rejected",
            "phi_edge_trims",
            "gvn_hits",
            "dce_removed_total",
        ]
        rendered = " ".join(
            f"{key}={self.midend_stats.get(key, 0)}" for key in ordered_keys
        )
        print(
            f"molt midend stats: {rendered}",
            file=sys.stderr,
        )
        per_func = []
        for func_name in sorted(self.midend_stats_by_function):
            stats = self.midend_stats_by_function[func_name]
            per_func.append(
                f"{func_name}:"
                f"sccp={stats.get('sccp_accepted', 0)}/{stats.get('sccp_attempted', 0)},"
                f"sccp_cap={stats.get('sccp_iteration_cap_hits', 0)},"
                f"edge_thread={stats.get('edge_thread_accepted', 0)}/{stats.get('edge_thread_attempted', 0)}"
                f"(rej={stats.get('edge_thread_rejected', 0)}),"
                f"loop_rewrite={stats.get('loop_rewrite_accepted', 0)}/{stats.get('loop_rewrite_attempted', 0)}"
                f"(rej={stats.get('loop_rewrite_rejected', 0)}),"
                f"guard_hoist={stats.get('guard_hoist_accepted', 0)}/{stats.get('guard_hoist_attempted', 0)}"
                f"(rej={stats.get('guard_hoist_rejected', 0)}),"
                f"cse={stats.get('cse_accepted', 0)}/{stats.get('cse_attempted', 0)},"
                f"cse_readheap={stats.get('cse_readheap_accepted', 0)}/{stats.get('cse_readheap_attempted', 0)}"
                f"(rej={stats.get('cse_readheap_rejected', 0)}),"
                f"gvn={stats.get('gvn_accepted', 0)}/{stats.get('gvn_attempted', 0)},"
                f"licm={stats.get('licm_accepted', 0)}/{stats.get('licm_attempted', 0)}"
                f"(rej={stats.get('licm_rejected', 0)}),"
                f"dce={stats.get('dce_accepted', 0)}/{stats.get('dce_attempted', 0)},"
                f"dce_pure={stats.get('dce_pure_op_accepted', 0)}/{stats.get('dce_pure_op_attempted', 0)}"
                f"(rej={stats.get('dce_pure_op_rejected', 0)})"
            )
        if per_func:
            print(
                "molt midend function stats: " + " | ".join(per_func),
                file=sys.stderr,
            )
            hotspot_candidates: list[tuple[int, str, str, int, int]] = []
            tracked = [
                ("sccp_iteration_cap_hits", "sccp_cap"),
                ("edge_thread_rejected", "edge_thread"),
                ("loop_rewrite_rejected", "loop_rewrite"),
                ("cse_readheap_rejected", "cse_readheap"),
                ("dce_pure_op_rejected", "dce_pure_op"),
                ("guard_hoist_rejected", "guard_hoist"),
                ("licm_rejected", "licm"),
            ]
            for func_name, stats in self.midend_stats_by_function.items():
                for key, family in tracked:
                    rejected = int(stats.get(key, 0))
                    attempted = int(
                        stats.get(
                            {
                                "sccp_iteration_cap_hits": "sccp_attempted",
                                "edge_thread_rejected": "edge_thread_attempted",
                                "loop_rewrite_rejected": "loop_rewrite_attempted",
                                "cse_readheap_rejected": "cse_readheap_attempted",
                                "dce_pure_op_rejected": "dce_pure_op_attempted",
                                "guard_hoist_rejected": "guard_hoist_attempted",
                                "licm_rejected": "licm_attempted",
                            }[key],
                            0,
                        )
                    )
                    if rejected > 0:
                        hotspot_candidates.append(
                            (rejected, func_name, family, rejected, attempted)
                        )
            if hotspot_candidates:
                hotspot_candidates.sort(reverse=True)
                _score, func_name, family, rejected, attempted = hotspot_candidates[0]
                print(
                    "molt midend hotspot: "
                    f"{func_name} family={family} rejected={rejected} attempted={attempted}",
                    file=sys.stderr,
                )
        if self.midend_policy_outcomes_by_function:
            rendered_policy = []
            for func_name in sorted(self.midend_policy_outcomes_by_function):
                outcome = self.midend_policy_outcomes_by_function[func_name]
                rendered_policy.append(
                    f"{func_name}:profile={outcome.get('profile')},"
                    f"tier={outcome.get('tier')},"
                    f"spent_ms={outcome.get('spent_ms')},"
                    f"budget_ms={outcome.get('budget_ms')},"
                    f"degraded={outcome.get('degraded')}"
                )
            print(
                "molt midend policy outcomes: " + " | ".join(rendered_policy),
                file=sys.stderr,
            )
        pass_hotspots: list[tuple[float, str, str, float, float, int, int, int]] = []
        for func_name, per_pass in self.midend_pass_stats_by_function.items():
            for pass_name, stats in per_pass.items():
                samples = [
                    float(sample)
                    for sample in stats.get("samples_ms", [])
                    if isinstance(sample, (int, float))
                ]
                p95 = self._pass_stat_p95(samples)
                total_ms = float(stats.get("ms_total", 0.0))
                pass_hotspots.append(
                    (
                        total_ms,
                        func_name,
                        pass_name,
                        total_ms,
                        p95,
                        int(stats.get("attempted", 0)),
                        int(stats.get("accepted", 0)),
                        int(stats.get("degraded", 0)),
                    )
                )
        if pass_hotspots:
            pass_hotspots.sort(reverse=True)
            top_passes = []
            for (
                _score,
                func_name,
                pass_name,
                total_ms,
                p95,
                attempted,
                accepted,
                degraded,
            ) in pass_hotspots[:10]:
                top_passes.append(
                    f"{func_name}:{pass_name} total_ms={total_ms:.3f} "
                    f"p95_ms={p95:.3f} attempted={attempted} "
                    f"accepted={accepted} degraded={degraded}"
                )
            print(
                "molt midend pass hotspots: " + " | ".join(top_passes),
                file=sys.stderr,
            )

    def _resolve_alias_value(
        self, value: MoltValue, aliases: dict[str, MoltValue]
    ) -> MoltValue:
        current = value
        visited: set[str] = set()
        while current.name in aliases and current.name not in visited:
            visited.add(current.name)
            current = aliases[current.name]
        return current

    def _rewrite_aliases_in_arg(self, value: Any, aliases: dict[str, MoltValue]) -> Any:
        if isinstance(value, MoltValue):
            return self._resolve_alias_value(value, aliases)
        if isinstance(value, list):
            return [self._rewrite_aliases_in_arg(item, aliases) for item in value]
        if isinstance(value, tuple):
            return tuple(self._rewrite_aliases_in_arg(item, aliases) for item in value)
        if isinstance(value, dict):
            return {
                self._rewrite_aliases_in_arg(k, aliases): self._rewrite_aliases_in_arg(
                    v, aliases
                )
                for k, v in value.items()
            }
        return value

    def _is_canonicalization_barrier_op(self, op_kind: str) -> bool:
        if op_kind in {"RETURN", "RAISE", "RAISE_CAUSE", "RERAISE"}:
            return True
        if op_kind.startswith("EXCEPTION_"):
            return True
        if op_kind.startswith("STATE_"):
            return True
        return False

    def _const_type_tag(self, op: MoltOp) -> int | None:
        if op.kind == "CONST_BOOL":
            return BUILTIN_TYPE_TAGS["bool"]
        if op.kind == "CONST":
            value = op.args[0]
            if isinstance(value, int) and not isinstance(value, bool):
                return BUILTIN_TYPE_TAGS["int"]
        if op.kind == "CONST_BIGINT":
            return BUILTIN_TYPE_TAGS["int"]
        if op.kind == "CONST_FLOAT":
            return BUILTIN_TYPE_TAGS["float"]
        if op.kind == "CONST_STR":
            return BUILTIN_TYPE_TAGS["str"]
        if op.kind == "CONST_BYTES":
            return BUILTIN_TYPE_TAGS["bytes"]
        return None

    def _empty_canonicalization_state(self) -> CanonicalizationState:
        return {
            "aliases": {},
            "const_int_values": {},
            "value_type_tags": {},
            "available_values": {},
            "guard_dict_shapes": {},
            "alias_epochs": {},
            "object_epochs": {},
            "memory_epoch": 0,
        }

    def _clone_canonicalization_state(
        self, state: CanonicalizationState
    ) -> CanonicalizationState:
        cloned: CanonicalizationState = {
            "aliases": state["aliases"].copy(),
            "const_int_values": state["const_int_values"].copy(),
            "value_type_tags": state["value_type_tags"].copy(),
            "available_values": state["available_values"].copy(),
            "guard_dict_shapes": state["guard_dict_shapes"].copy(),
            "alias_epochs": state["alias_epochs"].copy(),
            "object_epochs": state["object_epochs"].copy(),
            "memory_epoch": state["memory_epoch"],
        }
        cached_signature = cast(Any, state).get(
            _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY
        )
        if cached_signature is not None:
            cast(Any, cloned)[_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY] = (
                cached_signature
            )
        return cloned

    def _invalidate_canonicalization_state_signature(
        self, state: CanonicalizationState
    ) -> None:
        cast(Any, state).pop(_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY, None)

    def _const_cache_key_for_op(self, op: MoltOp) -> tuple[Any, ...] | None:
        if op.kind in {"CONST_NONE", "CONST_NOT_IMPLEMENTED", "CONST_ELLIPSIS"}:
            return (op.kind,)
        if op.kind == "CONST_BYTES":
            return ("CONST_BYTES", bytes(op.args[0]))
        if op.kind in {"CONST_BOOL", "CONST_BIGINT", "CONST_FLOAT", "CONST_STR"}:
            value = op.args[0]
            try:
                hash(value)
                normalized = value
            except TypeError:
                normalized = repr(value)
            return (op.kind, normalized)
        if op.kind == "CONST":
            value = op.args[0]
            try:
                hash(value)
                normalized = value
            except TypeError:
                normalized = repr(value)
            return ("CONST", type(value).__name__, normalized)
        return None

    def _op_effect_class(self, op_kind: str) -> str:
        if op_kind in {
            "CONST",
            "CONST_BIGINT",
            "CONST_BOOL",
            "CONST_FLOAT",
            # CONST_STR is NOT pure: it allocates heap memory via
            # molt_string_from_bytes. LICM must not hoist it out of
            # loops — the Cranelift SSA variable for the string pointer
            # gets corrupted by loop-header phi merges if defined once
            # before the loop instead of on each iteration.
            "CONST_BYTES",
            "CONST_NONE",
            "CONST_NOT_IMPLEMENTED",
            "CONST_ELLIPSIS",
            "MISSING",
            "PHI",
            "NOT",
            "IS",
            "TYPE_OF",
            "ADD",
            "SUB",
            "MUL",
            "ABS",
            "AND",
            "OR",
            "EQ",
            "NE",
            "LT",
            "LE",
            "GT",
            "GE",
            "STRING_EQ",
        }:
            return "pure"
        if op_kind in {
            "LEN",
            "INDEX",
            "GET_ATTR",
            "GETATTR",
            "LOAD_ATTR",
            "GETATTR_NAME",
            "HASATTR_NAME",
            "GETATTR_SPECIAL_OBJ",
            "GETATTR_GENERIC_OBJ",
            "GETATTR_GENERIC_PTR",
            "GETATTR_NAME_DEFAULT",
            "GUARDED_GETATTR",
            "MODULE_GET_ATTR",
            "ISINSTANCE",
            "EXCEPTION_MATCH_BUILTIN",
            "CONTAINS",
        }:
            return "reads_heap"
        if op_kind in {
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "INVOKE_FFI",
            "STORE_ATTR",
            "SETATTR",
            "SET_ATTR",
            "STORE_INDEX",
            "SET_INDEX",
            "LIST_APPEND",
            "LIST_EXTEND",
            "LIST_POP",
            "LIST_REMOVE",
            "LIST_INSERT",
            "LIST_CLEAR",
            "LIST_REVERSE",
            "BYTEARRAY_FILL_RANGE",
            "DICT_SET",
            "DICT_STR_INT_INC",
            "DICT_SPLIT_COUNT_INT_INC",
            "DICT_SETDEFAULT",
            "DICT_POP",
            "DICT_POPITEM",
            "DICT_CLEAR",
            "DICT_UPDATE",
            "DICT_UPDATE_KWSTAR",
            "DICT_UPDATE_MISSING",
            "DEL_ATTR",
            "DELATTR",
            "SETATTR_NAME",
            "SETATTR_INIT",
            "SETATTR_GENERIC_OBJ",
            "SETATTR_GENERIC_PTR",
            "GUARDED_SETATTR",
            "GUARDED_SETATTR_INIT",
            "DELATTR_NAME",
            "DEL_INDEX",
            "STORE_VAR",
            "DELETE_VAR",
        }:
            return "writes_heap"
        if op_kind in {
            "LOAD_VAR",
        }:
            return "reads_heap"
        if op_kind.startswith("EXCEPTION_") or op_kind.startswith("STATE_"):
            return "control"
        if op_kind in {
            "IF",
            "ELSE",
            "END_IF",
            "LOOP_START",
            "LOOP_END",
            "LOOP_BREAK",
            "LOOP_BREAK_IF_TRUE",
            "LOOP_BREAK_IF_FALSE",
            "LOOP_BREAK_IF_EXCEPTION",
            "LOOP_CONTINUE",
            "TRY_START",
            "TRY_END",
            "JUMP",
            "RETURN",
            "RAISE",
            "RAISE_CAUSE",
            "RERAISE",
            "LABEL",
            "STATE_LABEL",
            "CHECK_EXCEPTION",
            "GUARD_TAG",
            "GUARD_TYPE",
            "GUARD_LAYOUT",
            "GUARD_DICT_SHAPE",
        }:
            return "control"
        return "unknown"

    def _is_pure_op_for_global_cse(self, op_kind: str) -> bool:
        return self._op_effect_class(op_kind) == "pure"

    def _is_cse_eligible_op(self, op_kind: str) -> bool:
        return self._op_effect_class(op_kind) in {"pure", "reads_heap"}

    def _normalize_value_operand_key(
        self, value: Any, const_int_values: dict[str, int]
    ) -> tuple[str, Any] | None:
        if not isinstance(value, MoltValue):
            return None
        const_value = const_int_values.get(value.name)
        if const_value is not None:
            return ("const_int", const_value)
        return ("ssa", value.name)

    def _normalize_operand_key_for_value_numbering(
        self, value: Any, const_int_values: dict[str, int]
    ) -> tuple[str, Any] | None:
        if isinstance(value, MoltValue):
            return self._normalize_value_operand_key(value, const_int_values)
        try:
            hash(value)
            return ("const", value)
        except TypeError:
            return ("const_repr", repr(value))

    def _const_type_tag_for_lattice_value(self, value: Any) -> int | None:
        if isinstance(value, bool):
            return BUILTIN_TYPE_TAGS["bool"]
        if isinstance(value, int):
            return BUILTIN_TYPE_TAGS["int"]
        if isinstance(value, float):
            return BUILTIN_TYPE_TAGS["float"]
        if isinstance(value, str):
            return BUILTIN_TYPE_TAGS["str"]
        if isinstance(value, bytes):
            return BUILTIN_TYPE_TAGS["bytes"]
        if isinstance(value, list):
            return BUILTIN_TYPE_TAGS["list"]
        if isinstance(value, tuple):
            return BUILTIN_TYPE_TAGS["tuple"]
        if isinstance(value, dict):
            return BUILTIN_TYPE_TAGS["dict"]
        if isinstance(value, set):
            return BUILTIN_TYPE_TAGS["set"]
        if isinstance(value, frozenset):
            return BUILTIN_TYPE_TAGS["frozenset"]
        if isinstance(value, range):
            return BUILTIN_TYPE_TAGS["range"]
        return None

    def _heap_alias_class_for_read_op(
        self, op: MoltOp, value_type_tags: dict[str, int]
    ) -> str | None:
        if not op.args:
            return None
        primary = op.args[0]
        if not isinstance(primary, MoltValue):
            return "indexable"
        type_tag = value_type_tags.get(primary.name)
        if op.kind == "LEN":
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["frozenset"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            return "indexable"
        if op.kind == "INDEX":
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            return "indexable"
        if op.kind == "CONTAINS":
            if type_tag in {
                BUILTIN_TYPE_TAGS["str"],
                BUILTIN_TYPE_TAGS["bytes"],
                BUILTIN_TYPE_TAGS["tuple"],
                BUILTIN_TYPE_TAGS["frozenset"],
                BUILTIN_TYPE_TAGS["range"],
            }:
                return "immutable_len"
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return "dict"
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return "list"
            return "indexable"
        if op.kind in {
            "GET_ATTR",
            "GETATTR",
            "LOAD_ATTR",
            "GETATTR_NAME",
            "HASATTR_NAME",
            "GETATTR_SPECIAL_OBJ",
            "GETATTR_GENERIC_OBJ",
            "GETATTR_GENERIC_PTR",
            "GETATTR_NAME_DEFAULT",
            "GUARDED_GETATTR",
            "MODULE_GET_ATTR",
        }:
            return "attr"
        return "indexable"

    def _is_uncertain_heap_boundary(self, op_kind: str) -> bool:
        return op_kind in {
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "INVOKE_FFI",
        }

    def _heap_alias_classes_for_write_op(
        self, op: MoltOp, value_type_tags: dict[str, int]
    ) -> set[str]:
        if op.kind in {
            "DICT_SET",
            "DICT_STR_INT_INC",
            "DICT_SPLIT_COUNT_INT_INC",
            "DICT_SETDEFAULT",
            "DICT_POP",
            "DICT_POPITEM",
            "DICT_CLEAR",
            "DICT_UPDATE",
            "DICT_UPDATE_KWSTAR",
        }:
            return {"dict", "indexable"}
        if op.kind in {
            "LIST_APPEND",
            "LIST_EXTEND",
            "LIST_POP",
            "LIST_REMOVE",
            "LIST_INSERT",
            "LIST_CLEAR",
            "LIST_REVERSE",
        }:
            return {"list", "indexable"}
        if op.kind in {
            "STORE_ATTR",
            "SET_ATTR",
            "SETATTR",
            "SETATTR_INIT",
            "SETATTR_GENERIC_OBJ",
            "SETATTR_GENERIC_PTR",
            "GUARDED_SETATTR",
            "GUARDED_SETATTR_INIT",
            "DEL_ATTR",
            "DELATTR",
            "SETATTR_NAME",
            "DELATTR_NAME",
        }:
            return {"attr"}
        if op.kind in {"STORE_INDEX", "SET_INDEX", "DEL_INDEX"}:
            if not op.args or not isinstance(op.args[0], MoltValue):
                return {"dict", "list", "indexable"}
            type_tag = value_type_tags.get(op.args[0].name)
            if type_tag == BUILTIN_TYPE_TAGS["dict"]:
                return {"dict", "indexable"}
            if type_tag == BUILTIN_TYPE_TAGS["list"]:
                return {"list", "indexable"}
            return {"dict", "list", "indexable"}
        return {"dict", "list", "indexable", "attr"}

    def _is_heap_read_key(self, key: tuple[Any, ...]) -> bool:
        return bool(key) and key[0] == "READ_HEAP_CLASS"

    def _heap_read_key_class(self, key: tuple[Any, ...]) -> str | None:
        if not self._is_heap_read_key(key):
            return None
        if len(key) < 2:
            return None
        read_class = key[1]
        if not isinstance(read_class, str):
            return None
        return read_class

    def _is_read_key_invalidated_by_alias_classes(
        self, key: tuple[Any, ...], alias_classes: set[str]
    ) -> bool:
        read_class = self._heap_read_key_class(key)
        if read_class is None:
            return False
        if read_class == "immutable_len":
            return False
        if read_class == "indexable":
            return bool(alias_classes.intersection({"indexable", "dict", "list"}))
        return read_class in alias_classes

    def _int_const_from_definition(
        self, name: str, definitions: dict[str, MoltOp]
    ) -> int | None:
        memo: dict[str, int | None] = {}
        visiting: set[str] = set()

        def resolve(value_name: str) -> int | None:
            if value_name in memo:
                return memo[value_name]
            if value_name in visiting:
                memo[value_name] = None
                return None
            visiting.add(value_name)
            op = definitions.get(value_name)
            resolved: int | None = None
            if op is not None:
                if op.kind in {"CONST", "CONST_BIGINT"} and op.args:
                    raw = op.args[0]
                    if isinstance(raw, int) and not isinstance(raw, bool):
                        resolved = raw
                elif op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
                    lhs = op.args[0]
                    rhs = op.args[1]
                    if isinstance(lhs, MoltValue) and isinstance(rhs, MoltValue):
                        lhs_const = resolve(lhs.name)
                        rhs_const = resolve(rhs.name)
                        if lhs_const is not None and rhs_const is not None:
                            if op.kind == "ADD":
                                resolved = lhs_const + rhs_const
                            elif op.kind == "SUB":
                                resolved = lhs_const - rhs_const
                            else:
                                resolved = lhs_const * rhs_const
                elif op.kind == "ABS" and len(op.args) == 1:
                    arg = op.args[0]
                    if isinstance(arg, MoltValue):
                        arg_const = resolve(arg.name)
                        if arg_const is not None:
                            resolved = abs(arg_const)
                elif op.kind == "PHI" and op.args:
                    phi_values: list[int] = []
                    for arg in op.args:
                        if not isinstance(arg, MoltValue):
                            phi_values = []
                            break
                        phi_const = resolve(arg.name)
                        if phi_const is None:
                            phi_values = []
                            break
                        phi_values.append(phi_const)
                    if phi_values and all(v == phi_values[0] for v in phi_values):
                        resolved = phi_values[0]
            visiting.discard(value_name)
            memo[value_name] = resolved
            return resolved

        return resolve(name)

    def _compare_int_truth(self, op_kind: str, lhs: int, rhs: int) -> bool | None:
        if op_kind == "EQ":
            return lhs == rhs
        if op_kind == "NE":
            return lhs != rhs
        if op_kind == "LT":
            return lhs < rhs
        if op_kind == "LE":
            return lhs <= rhs
        if op_kind == "GT":
            return lhs > rhs
        if op_kind == "GE":
            return lhs >= rhs
        return None

    def _detect_induction_step_from_recurrence(
        self, phi_name: str, recurrence: MoltOp, definitions: dict[str, MoltOp]
    ) -> int | None:
        if recurrence.kind not in {"ADD", "SUB"} or len(recurrence.args) != 2:
            return None
        lhs = recurrence.args[0]
        rhs = recurrence.args[1]
        if (
            isinstance(lhs, MoltValue)
            and lhs.name == phi_name
            and isinstance(rhs, MoltValue)
        ):
            rhs_const = self._int_const_from_definition(rhs.name, definitions)
            if rhs_const is None:
                return None
            if recurrence.kind == "ADD":
                return rhs_const
            return -rhs_const
        if (
            isinstance(rhs, MoltValue)
            and rhs.name == phi_name
            and isinstance(lhs, MoltValue)
            and recurrence.kind == "ADD"
        ):
            return self._int_const_from_definition(lhs.name, definitions)
        return None

    def _normalize_compare_for_induction(
        self, compare_op: str, lhs_is_iv: bool
    ) -> str | None:
        if lhs_is_iv:
            if compare_op in {"LT", "LE", "GT", "GE"}:
                return compare_op
            return None
        swapped = {
            "LT": "GT",
            "LE": "GE",
            "GT": "LT",
            "GE": "LE",
        }
        return swapped.get(compare_op)

    def _prove_monotonic_loop_compare(self, fact: LoopBoundFact) -> bool | None:
        start = fact.start
        step = fact.step
        bound = fact.bound
        compare_op = fact.compare_op

        if step == 0:
            return self._compare_int_truth(compare_op, start, bound)

        if step > 0:
            if compare_op == "LT" and start >= bound:
                return False
            if compare_op == "LE" and start > bound:
                return False
            if compare_op == "GT" and start > bound:
                return True
            if compare_op == "GE" and start >= bound:
                return True
            if compare_op == "EQ" and start > bound:
                return False
            if compare_op == "NE" and start > bound:
                return True
            return None

        if compare_op == "LT" and start < bound:
            return True
        if compare_op == "LE" and start <= bound:
            return True
        if compare_op == "GT" and start <= bound:
            return False
        if compare_op == "GE" and start < bound:
            return False
        if compare_op == "EQ" and start < bound:
            return False
        if compare_op == "NE" and start < bound:
            return True
        return None

    def _analyze_loop_bound_facts(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[int, LoopBoundFact]:
        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        loop_bound_facts: dict[int, LoopBoundFact] = {}

        def resolve_affine_iv_term(
            value: MoltValue,
            induction: dict[str, tuple[int, int]],
            *,
            seen: set[str] | None = None,
        ) -> tuple[str, int] | None:
            if value.name in induction:
                return value.name, 0
            if seen is None:
                seen = set()
            if value.name in seen:
                return None
            next_seen = set(seen)
            next_seen.add(value.name)
            def_op = definitions.get(value.name)
            if (
                def_op is None
                or def_op.kind not in {"ADD", "SUB"}
                or len(def_op.args) != 2
            ):
                return None
            lhs = def_op.args[0]
            rhs = def_op.args[1]
            if isinstance(lhs, MoltValue):
                lhs_term = resolve_affine_iv_term(lhs, induction, seen=next_seen)
            else:
                lhs_term = None
            if isinstance(rhs, MoltValue):
                rhs_term = resolve_affine_iv_term(rhs, induction, seen=next_seen)
            else:
                rhs_term = None
            if lhs_term is not None and isinstance(rhs, MoltValue):
                c = self._int_const_from_definition(rhs.name, definitions)
                if c is None:
                    return None
                if def_op.kind == "SUB":
                    c = -c
                return lhs_term[0], lhs_term[1] + c
            if (
                rhs_term is not None
                and isinstance(lhs, MoltValue)
                and def_op.kind == "ADD"
            ):
                c = self._int_const_from_definition(lhs.name, definitions)
                if c is None:
                    return None
                return rhs_term[0], rhs_term[1] + c
            return None

        for loop_start, loop_end in cfg.control.loop_start_to_end.items():
            if loop_end <= loop_start:
                continue

            induction_by_phi: dict[str, tuple[int, int]] = {}
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.kind != "PHI" or not op.args or op.result.name == "none":
                    continue
                phi_name = op.result.name
                start_value: int | None = None
                step_value: int | None = None
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        continue
                    recurrence = definitions.get(arg.name)
                    if recurrence is not None:
                        step = self._detect_induction_step_from_recurrence(
                            phi_name, recurrence, definitions
                        )
                        if step is not None:
                            step_value = step
                            continue
                    start_candidate = self._int_const_from_definition(
                        arg.name, definitions
                    )
                    if start_candidate is not None:
                        start_value = start_candidate
                if step_value is not None and start_value is not None:
                    induction_by_phi[phi_name] = (start_value, step_value)

            if not induction_by_phi:
                continue

            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if (
                    op.kind not in {"LT", "LE", "GT", "GE", "EQ", "NE"}
                    or len(op.args) != 2
                ):
                    continue
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    continue

                iv_name: str | None = None
                bound_value: int | None = None
                normalized_op: str | None = None
                lhs_term = resolve_affine_iv_term(lhs, induction_by_phi)
                rhs_term = resolve_affine_iv_term(rhs, induction_by_phi)
                if lhs_term is not None and rhs_term is None:
                    iv_name = lhs_term[0]
                    rhs_bound = self._int_const_from_definition(rhs.name, definitions)
                    if rhs_bound is None:
                        continue
                    bound_value = rhs_bound - lhs_term[1]
                    normalized_op = self._normalize_compare_for_induction(
                        op.kind, lhs_is_iv=True
                    )
                elif rhs_term is not None and lhs_term is None:
                    iv_name = rhs_term[0]
                    lhs_bound = self._int_const_from_definition(lhs.name, definitions)
                    if lhs_bound is None:
                        continue
                    bound_value = lhs_bound - rhs_term[1]
                    normalized_op = self._normalize_compare_for_induction(
                        op.kind, lhs_is_iv=False
                    )
                else:
                    continue

                if iv_name is None or bound_value is None or normalized_op is None:
                    continue
                start_value, step_value = induction_by_phi[iv_name]
                if op.result.name == "none":
                    continue
                loop_bound_facts[idx] = LoopBoundFact(
                    iv_name=iv_name,
                    start=start_value,
                    step=step_value,
                    bound=bound_value,
                    compare_op=normalized_op,
                    compare_index=idx,
                    compare_result=op.result.name,
                )

        return loop_bound_facts

    def _analyze_affine_loop_compare_truth(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[int, bool]:
        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        compare_truth: dict[int, bool] = {}

        def resolve_affine_iv_term(
            value: MoltValue,
            induction: dict[str, tuple[int, int]],
            *,
            seen: set[str] | None = None,
        ) -> tuple[str, int] | None:
            if value.name in induction:
                return value.name, 0
            if seen is None:
                seen = set()
            if value.name in seen:
                return None
            next_seen = set(seen)
            next_seen.add(value.name)
            def_op = definitions.get(value.name)
            if (
                def_op is None
                or def_op.kind not in {"ADD", "SUB"}
                or len(def_op.args) != 2
            ):
                return None
            lhs = def_op.args[0]
            rhs = def_op.args[1]
            lhs_term = (
                resolve_affine_iv_term(lhs, induction, seen=next_seen)
                if isinstance(lhs, MoltValue)
                else None
            )
            rhs_term = (
                resolve_affine_iv_term(rhs, induction, seen=next_seen)
                if isinstance(rhs, MoltValue)
                else None
            )
            if lhs_term is not None and isinstance(rhs, MoltValue):
                rhs_const = self._int_const_from_definition(rhs.name, definitions)
                if rhs_const is None:
                    return None
                if def_op.kind == "SUB":
                    rhs_const = -rhs_const
                return lhs_term[0], lhs_term[1] + rhs_const
            if (
                rhs_term is not None
                and isinstance(lhs, MoltValue)
                and def_op.kind == "ADD"
            ):
                lhs_const = self._int_const_from_definition(lhs.name, definitions)
                if lhs_const is None:
                    return None
                return rhs_term[0], rhs_term[1] + lhs_const
            return None

        for loop_start, loop_end in cfg.control.loop_start_to_end.items():
            if loop_end <= loop_start:
                continue

            induction_by_phi: dict[str, tuple[int, int]] = {}
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.kind != "PHI" or not op.args or op.result.name == "none":
                    continue
                phi_name = op.result.name
                start_value: int | None = None
                step_value: int | None = None
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        continue
                    recurrence = definitions.get(arg.name)
                    if recurrence is not None:
                        step = self._detect_induction_step_from_recurrence(
                            phi_name, recurrence, definitions
                        )
                        if step is not None:
                            step_value = step
                            continue
                    start_candidate = self._int_const_from_definition(
                        arg.name, definitions
                    )
                    if start_candidate is not None:
                        start_value = start_candidate
                if step_value is not None and start_value is not None:
                    induction_by_phi[phi_name] = (start_value, step_value)

            if not induction_by_phi:
                continue

            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if (
                    op.kind not in {"LT", "LE", "GT", "GE", "EQ", "NE"}
                    or len(op.args) != 2
                ):
                    continue
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    continue
                lhs_term = resolve_affine_iv_term(lhs, induction_by_phi)
                rhs_term = resolve_affine_iv_term(rhs, induction_by_phi)
                if lhs_term is None or rhs_term is None:
                    continue
                if lhs_term[0] != rhs_term[0]:
                    continue
                proven = self._compare_int_truth(op.kind, lhs_term[1], rhs_term[1])
                if isinstance(proven, bool):
                    compare_truth[idx] = proven

        return compare_truth

    def _analyze_loop_induction_steps(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> dict[str, int]:
        induction_steps: dict[str, int] = {}
        for fact in self._analyze_loop_bound_facts(ops, cfg).values():
            induction_steps.setdefault(fact.iv_name, fact.step)
        if induction_steps:
            return induction_steps

        definitions: dict[str, MoltOp] = {
            op.result.name: op for op in ops if op.result.name != "none"
        }
        for op in ops:
            if op.kind != "PHI" or not op.args or op.result.name == "none":
                continue
            phi_name = op.result.name
            for arg in op.args:
                if not isinstance(arg, MoltValue):
                    continue
                recurrence = definitions.get(arg.name)
                if recurrence is None:
                    continue
                step = self._detect_induction_step_from_recurrence(
                    phi_name, recurrence, definitions
                )
                if step is not None:
                    induction_steps[phi_name] = step
                    break
        return induction_steps

    def _value_number_key_for_op(
        self,
        op: MoltOp,
        const_int_values: dict[str, int],
        value_type_tags: dict[str, int],
        induction_steps: dict[str, int],
        *,
        alias_epochs: dict[str, int],
        object_epochs: dict[str, int],
        memory_epoch: int,
    ) -> tuple[Any, ...] | None:
        if not self._is_cse_eligible_op(op.kind):
            return None
        effect_class = self._op_effect_class(op.kind)

        const_key = self._const_cache_key_for_op(op)
        if const_key is not None:
            return ("CONST",) + const_key

        if op.kind == "IS" and len(op.args) == 2:
            lhs = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs is not None and rhs is not None:
                return ("IS", lhs, rhs)

        if op.kind == "TYPE_OF" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                if effect_class == "reads_heap":
                    return ("READ_HEAP", memory_epoch, "TYPE_OF", arg)
                return ("TYPE_OF", arg)

        if op.kind == "NOT" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                return ("NOT", arg)

        if op.kind == "ABS" and len(op.args) == 1:
            arg = self._normalize_value_operand_key(op.args[0], const_int_values)
            if arg is not None:
                return ("ABS", arg)

        if op.kind in {"AND", "OR"} and len(op.args) == 2:
            lhs = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs is not None and rhs is not None:
                return ("BOOL_BINOP", op.kind, lhs, rhs)

        if (
            op.kind in {"EQ", "NE", "LT", "LE", "GT", "GE", "STRING_EQ"}
            and len(op.args) == 2
        ):
            lhs_key = self._normalize_operand_key_for_value_numbering(
                op.args[0], const_int_values
            )
            rhs_key = self._normalize_operand_key_for_value_numbering(
                op.args[1], const_int_values
            )
            if lhs_key is None or rhs_key is None:
                return None
            if op.kind in {"EQ", "NE", "STRING_EQ"} and rhs_key < lhs_key:
                lhs_key, rhs_key = rhs_key, lhs_key
            return ("CMP_PURE", op.kind, lhs_key, rhs_key)

        if op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
            lhs_key = self._normalize_value_operand_key(op.args[0], const_int_values)
            rhs_key = self._normalize_value_operand_key(op.args[1], const_int_values)
            if lhs_key is None or rhs_key is None:
                return None

            if op.kind in {"ADD", "MUL"} and rhs_key < lhs_key:
                lhs_key, rhs_key = rhs_key, lhs_key

            lhs = op.args[0]
            rhs = op.args[1]
            if (
                op.kind in {"ADD", "SUB"}
                and isinstance(lhs, MoltValue)
                and isinstance(rhs, MoltValue)
                and lhs.name in induction_steps
                and rhs.name in const_int_values
            ):
                return (
                    "INDUCT_ARITH",
                    op.kind,
                    lhs.name,
                    induction_steps[lhs.name],
                    const_int_values[rhs.name],
                )

            return ("ARITH_PURE", op.kind, lhs_key, rhs_key)
        if effect_class == "reads_heap":
            normalized_args: list[tuple[str, Any]] = []
            for arg in op.args:
                key = self._normalize_operand_key_for_value_numbering(
                    arg, const_int_values
                )
                if key is None:
                    return None
                normalized_args.append(key)
            read_alias_class = self._heap_alias_class_for_read_op(op, value_type_tags)
            if read_alias_class is None:
                return None
            object_epoch = 0
            if op.args and isinstance(op.args[0], MoltValue):
                object_epoch = object_epochs.get(op.args[0].name, 0)
            if read_alias_class == "immutable_len":
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            class_epoch = alias_epochs.get(read_alias_class, 0)
            if read_alias_class in {"dict", "list"}:
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    class_epoch,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            if read_alias_class == "indexable":
                indexable_epoch = alias_epochs.get("indexable", 0)
                return (
                    "READ_HEAP_CLASS",
                    read_alias_class,
                    indexable_epoch,
                    object_epoch,
                    op.kind,
                    tuple(normalized_args),
                )
            return (
                "READ_HEAP_CLASS",
                read_alias_class,
                class_epoch,
                object_epoch,
                memory_epoch,
                op.kind,
                tuple(normalized_args),
            )
        return None

    def _kill_value_in_canonicalization_state(
        self, state: CanonicalizationState, name: str
    ) -> None:
        aliases: dict[str, MoltValue] = state["aliases"]
        aliases.pop(name, None)
        stale_aliases = [key for key, value in aliases.items() if value.name == name]
        for key in stale_aliases:
            aliases.pop(key, None)

        state["const_int_values"].pop(name, None)
        state["value_type_tags"].pop(name, None)

        available_values: dict[tuple[Any, ...], MoltValue] = state["available_values"]
        stale_values = [
            key for key, value in available_values.items() if value.name == name
        ]
        for key in stale_values:
            available_values.pop(key, None)

        guard_dict_shapes: dict[str, tuple[str, str]] = state["guard_dict_shapes"]
        guard_dict_shapes.pop(name, None)
        stale_dict_shapes = [
            key
            for key, (dict_type_name, version_name) in guard_dict_shapes.items()
            if dict_type_name == name or version_name == name
        ]
        for key in stale_dict_shapes:
            guard_dict_shapes.pop(key, None)
        object_epochs: dict[str, int] = state["object_epochs"]
        object_epochs.pop(name, None)
        self._invalidate_canonicalization_state_signature(state)

    def _intersect_canonicalization_state(
        self, left: CanonicalizationState, right: CanonicalizationState
    ) -> CanonicalizationState:
        aliases: dict[str, MoltValue] = {}
        for key, left_value in left["aliases"].items():
            right_value = right["aliases"].get(key)
            if (
                isinstance(right_value, MoltValue)
                and right_value.name == left_value.name
            ):
                aliases[key] = left_value

        const_int_values: dict[str, int] = {}
        for key, left_value in left["const_int_values"].items():
            right_value = right["const_int_values"].get(key)
            if isinstance(right_value, int) and right_value == left_value:
                const_int_values[key] = left_value

        value_type_tags: dict[str, int] = {}
        for key, left_value in left["value_type_tags"].items():
            right_value = right["value_type_tags"].get(key)
            if isinstance(right_value, int) and right_value == left_value:
                value_type_tags[key] = left_value

        available_values: dict[tuple[Any, ...], MoltValue] = {}
        for key, left_value in left["available_values"].items():
            right_value = right["available_values"].get(key)
            if (
                isinstance(right_value, MoltValue)
                and right_value.name == left_value.name
            ):
                available_values[key] = left_value

        guard_dict_shapes: dict[str, tuple[str, str]] = {}
        for key, left_value in left["guard_dict_shapes"].items():
            right_value = right["guard_dict_shapes"].get(key)
            if (
                isinstance(right_value, tuple)
                and len(right_value) == 2
                and right_value == left_value
            ):
                guard_dict_shapes[key] = left_value

        alias_epochs: dict[str, int] = {}
        left_alias_epochs = left["alias_epochs"]
        right_alias_epochs = right["alias_epochs"]
        for key in set(left_alias_epochs.keys()).union(right_alias_epochs.keys()):
            alias_epochs[key] = max(
                int(left_alias_epochs.get(key, 0)),
                int(right_alias_epochs.get(key, 0)),
            )

        object_epochs: dict[str, int] = {}
        left_object_epochs = left["object_epochs"]
        right_object_epochs = right["object_epochs"]
        for key in set(left_object_epochs.keys()).union(right_object_epochs.keys()):
            object_epochs[key] = max(
                int(left_object_epochs.get(key, 0)),
                int(right_object_epochs.get(key, 0)),
            )

        return {
            "aliases": aliases,
            "const_int_values": const_int_values,
            "value_type_tags": value_type_tags,
            "available_values": available_values,
            "guard_dict_shapes": guard_dict_shapes,
            "alias_epochs": alias_epochs,
            "object_epochs": object_epochs,
            "memory_epoch": max(left["memory_epoch"], right["memory_epoch"]),
        }

    def _intersect_canonicalization_states(
        self, states: list[CanonicalizationState]
    ) -> CanonicalizationState:
        if not states:
            return self._empty_canonicalization_state()
        merged = self._clone_canonicalization_state(states[0])
        for state in states[1:]:
            merged = self._intersect_canonicalization_state(merged, state)
        return merged

    def _canonicalization_state_signature(
        self, state: CanonicalizationState
    ) -> tuple[
        tuple[tuple[str, str], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[tuple[Any, ...], str], ...],
        tuple[tuple[str, tuple[str, str]], ...],
        tuple[tuple[str, int], ...],
        tuple[tuple[str, int], ...],
        int,
    ]:
        cached_signature = cast(Any, state).get(
            _CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY
        )
        if cached_signature is not None:
            return cast(
                tuple[
                    tuple[tuple[str, str], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[tuple[Any, ...], str], ...],
                    tuple[tuple[str, tuple[str, str]], ...],
                    tuple[tuple[str, int], ...],
                    tuple[tuple[str, int], ...],
                    int,
                ],
                cached_signature,
            )
        alias_items = tuple(
            sorted((key, value.name) for key, value in state["aliases"].items())
        )
        const_items = tuple(sorted(state["const_int_values"].items()))
        tag_items = tuple(sorted(state["value_type_tags"].items()))
        available_items = tuple(
            sorted(
                (key, value.name) for key, value in state["available_values"].items()
            )
        )
        dict_shape_items = tuple(sorted(state["guard_dict_shapes"].items()))
        alias_epoch_items = tuple(sorted(state["alias_epochs"].items()))
        object_epoch_items = tuple(sorted(state["object_epochs"].items()))
        memory_epoch = state["memory_epoch"]
        signature = (
            alias_items,
            const_items,
            tag_items,
            available_items,
            dict_shape_items,
            alias_epoch_items,
            object_epoch_items,
            memory_epoch,
        )
        cast(Any, state)[_CANONICALIZATION_STATE_SIGNATURE_CACHE_KEY] = signature
        return signature

    def _canonicalize_block_with_state(
        self,
        ops: list[MoltOp],
        in_state: CanonicalizationState,
        *,
        induction_steps: dict[str, int],
    ) -> tuple[list[MoltOp], CanonicalizationState]:
        func_stats = self._midend_function_stats()
        state = self._clone_canonicalization_state(in_state)
        aliases: dict[str, MoltValue] = state["aliases"]
        const_int_values: dict[str, int] = state["const_int_values"]
        value_type_tags: dict[str, int] = state["value_type_tags"]
        available_values: dict[tuple[Any, ...], MoltValue] = state["available_values"]
        guard_dict_shapes: dict[str, tuple[str, str]] = state["guard_dict_shapes"]
        alias_epochs: dict[str, int] = state["alias_epochs"]
        object_epochs: dict[str, int] = state["object_epochs"]
        memory_epoch = state["memory_epoch"]
        state_dirty = False

        out: list[MoltOp] = []
        for op in ops:
            canonical_args = [
                self._rewrite_aliases_in_arg(arg, aliases) for arg in op.args
            ]
            canonical_op = MoltOp(
                kind=op.kind,
                args=canonical_args,
                result=op.result,
                metadata=op.metadata,
                col_offset=op.col_offset,
                end_col_offset=op.end_col_offset,
            )

            result_name = canonical_op.result.name
            if result_name != "none":
                self._kill_value_in_canonicalization_state(state, result_name)

            if canonical_op.kind == "PHI" and canonical_op.args:
                phi_args = canonical_op.args
                if all(
                    isinstance(arg, MoltValue) and arg.name == phi_args[0].name
                    for arg in phi_args
                ):
                    shared = self._resolve_alias_value(phi_args[0], aliases)
                    aliases[result_name] = shared
                    state_dirty = True
                    if shared.name in const_int_values:
                        const_int_values[result_name] = const_int_values[shared.name]
                    if shared.name in value_type_tags:
                        value_type_tags[result_name] = value_type_tags[shared.name]
                    continue

            if canonical_op.kind == "GUARD_DICT_SHAPE" and len(canonical_op.args) == 3:
                guarded_obj = canonical_op.args[0]
                dict_type = canonical_op.args[1]
                version = canonical_op.args[2]
                if (
                    isinstance(guarded_obj, MoltValue)
                    and isinstance(dict_type, MoltValue)
                    and isinstance(version, MoltValue)
                ):
                    expected = (dict_type.name, version.name)
                    if guard_dict_shapes.get(guarded_obj.name) == expected:
                        continue

            if canonical_op.kind == "GUARD_TAG" and len(canonical_op.args) == 2:
                guarded = canonical_op.args[0]
                expected = canonical_op.args[1]
                if isinstance(guarded, MoltValue) and isinstance(expected, MoltValue):
                    actual_tag = value_type_tags.get(guarded.name)
                    expected_tag = const_int_values.get(expected.name)
                    if actual_tag is not None and expected_tag == actual_tag:
                        continue

            value_key = self._value_number_key_for_op(
                canonical_op,
                const_int_values,
                value_type_tags,
                induction_steps,
                alias_epochs=alias_epochs,
                object_epochs=object_epochs,
                memory_epoch=memory_epoch,
            )
            effect_class = self._op_effect_class(canonical_op.kind)
            if (
                effect_class == "reads_heap"
                and value_key is not None
                and result_name != "none"
            ):
                func_stats["cse_readheap_attempted"] += 1
            if value_key is not None and result_name != "none":
                cached = available_values.get(value_key)
                if cached is not None:
                    shared = self._resolve_alias_value(cached, aliases)
                    aliases[result_name] = shared
                    self.midend_stats["gvn_hits"] += 1
                    if effect_class == "reads_heap":
                        func_stats["cse_readheap_accepted"] += 1
                    if shared.name in const_int_values:
                        const_int_values[result_name] = const_int_values[shared.name]
                    if shared.name in value_type_tags:
                        value_type_tags[result_name] = value_type_tags[shared.name]
                    continue
                if effect_class == "reads_heap":
                    func_stats["cse_readheap_rejected"] += 1

            # Constant-fold arithmetic: when both operands are known
            # constants, compute the result.  If it overflows the
            # 47-bit signed inline range, replace with CONST_BIGINT
            # to prevent Cranelift 0.130 constant-folding miscompilation.
            _folded_to_bigint = False
            if (
                canonical_op.kind in {"ADD", "SUB", "MUL", "POW"}
                and len(canonical_op.args) == 2
            ):
                lhs, rhs = canonical_op.args
                if isinstance(lhs, MoltValue) and isinstance(rhs, MoltValue):
                    lhs_const = const_int_values.get(lhs.name)
                    rhs_const = const_int_values.get(rhs.name)
                    if lhs_const is not None and rhs_const is not None:
                        if canonical_op.kind == "ADD":
                            folded = lhs_const + rhs_const
                        elif canonical_op.kind == "SUB":
                            folded = lhs_const - rhs_const
                        elif canonical_op.kind == "MUL":
                            folded = lhs_const * rhs_const
                        else:
                            # POW – only fold for small non-negative exponents
                            # to avoid float results and unbounded computation.
                            if 0 <= rhs_const <= 64:
                                folded = lhs_const**rhs_const
                            else:
                                folded = None
                        if folded is None:
                            # Skip folding (e.g. negative exponent)
                            out.append(canonical_op)
                            continue
                        if not (_INLINE_INT_MIN <= folded <= _INLINE_INT_MAX):
                            canonical_op = MoltOp(
                                kind="CONST_BIGINT",
                                args=[str(folded)],
                                result=canonical_op.result,
                                col_offset=canonical_op.col_offset,
                                end_col_offset=canonical_op.end_col_offset,
                            )
                            _folded_to_bigint = True
                        elif canonical_op.kind == "POW":
                            canonical_op = MoltOp(
                                kind="CONST",
                                args=[folded],
                                result=canonical_op.result,
                                col_offset=canonical_op.col_offset,
                                end_col_offset=canonical_op.end_col_offset,
                            )
                        const_int_values[result_name] = folded
                        state_dirty = True

            # Constant-fold bitwise operations: when both operands are
            # known integer constants, compute the result at compile time.
            if (
                not _folded_to_bigint
                and canonical_op.kind
                in {
                    "BIT_AND",
                    "BIT_OR",
                    "BIT_XOR",
                    "LSHIFT",
                    "RSHIFT",
                    "INVERT",
                }
                and len(canonical_op.args) >= 1
            ):
                args = canonical_op.args
                if canonical_op.kind == "INVERT" and len(args) == 1:
                    arg = args[0]
                    if isinstance(arg, MoltValue):
                        arg_const = const_int_values.get(arg.name)
                        if arg_const is not None:
                            folded_bw = ~arg_const
                            if _INLINE_INT_MIN <= folded_bw <= _INLINE_INT_MAX:
                                const_int_values[result_name] = folded_bw
                                state_dirty = True
                elif len(args) == 2:
                    lhs_bw, rhs_bw = args
                    if isinstance(lhs_bw, MoltValue) and isinstance(rhs_bw, MoltValue):
                        lc = const_int_values.get(lhs_bw.name)
                        rc = const_int_values.get(rhs_bw.name)
                        if lc is not None and rc is not None:
                            if canonical_op.kind == "BIT_AND":
                                folded_bw = lc & rc
                            elif canonical_op.kind == "BIT_OR":
                                folded_bw = lc | rc
                            elif canonical_op.kind == "BIT_XOR":
                                folded_bw = lc ^ rc
                            elif canonical_op.kind == "LSHIFT":
                                folded_bw = lc << rc if 0 <= rc <= 128 else None
                            elif canonical_op.kind == "RSHIFT":
                                folded_bw = lc >> rc if 0 <= rc <= 128 else None
                            else:
                                folded_bw = None
                            if folded_bw is not None:
                                if not (
                                    _INLINE_INT_MIN <= folded_bw <= _INLINE_INT_MAX
                                ):
                                    canonical_op = MoltOp(
                                        kind="CONST_BIGINT",
                                        args=[str(folded_bw)],
                                        result=canonical_op.result,
                                        col_offset=canonical_op.col_offset,
                                        end_col_offset=canonical_op.end_col_offset,
                                    )
                                    _folded_to_bigint = True
                                const_int_values[result_name] = folded_bw
                                state_dirty = True

            out.append(canonical_op)

            if not _folded_to_bigint and canonical_op.kind == "CONST":
                value = canonical_op.args[0]
                if isinstance(value, int) and not isinstance(value, bool):
                    const_int_values[result_name] = value
                    state_dirty = True
            elif canonical_op.kind == "ABS" and len(canonical_op.args) == 1:
                arg = canonical_op.args[0]
                if isinstance(arg, MoltValue):
                    arg_const = const_int_values.get(arg.name)
                    if arg_const is not None:
                        const_int_values[result_name] = abs(arg_const)
                        state_dirty = True
            elif canonical_op.kind == "GUARD_TAG" and len(canonical_op.args) == 2:
                guarded, expected = canonical_op.args
                if isinstance(guarded, MoltValue) and isinstance(expected, MoltValue):
                    expected_tag = const_int_values.get(expected.name)
                    if expected_tag is not None:
                        value_type_tags[guarded.name] = expected_tag
                        state_dirty = True
            elif (
                canonical_op.kind == "GUARD_DICT_SHAPE" and len(canonical_op.args) == 3
            ):
                guarded_obj, dict_type, version = canonical_op.args
                if (
                    isinstance(guarded_obj, MoltValue)
                    and isinstance(dict_type, MoltValue)
                    and isinstance(version, MoltValue)
                ):
                    guard_dict_shapes[guarded_obj.name] = (dict_type.name, version.name)
                    state_dirty = True
            type_tag = self._const_type_tag(canonical_op)
            if type_tag is None and result_name != "none":
                if canonical_op.kind in {
                    "NOT",
                    "IS",
                    "AND",
                    "OR",
                    "EQ",
                    "NE",
                    "LT",
                    "LE",
                    "GT",
                    "GE",
                    "STRING_EQ",
                    "ISINSTANCE",
                    "EXCEPTION_MATCH_BUILTIN",
                }:
                    type_tag = BUILTIN_TYPE_TAGS["bool"]
                elif canonical_op.kind in {"LEN", "TYPE_OF"}:
                    type_tag = BUILTIN_TYPE_TAGS["int"]
                elif canonical_op.kind == "ABS" and len(canonical_op.args) == 1:
                    abs_arg = canonical_op.args[0]
                    if isinstance(abs_arg, MoltValue):
                        abs_arg_tag = value_type_tags.get(abs_arg.name)
                        if abs_arg_tag in {
                            BUILTIN_TYPE_TAGS["int"],
                            BUILTIN_TYPE_TAGS["float"],
                        }:
                            type_tag = abs_arg_tag
                elif canonical_op.kind == "DICT_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["dict"]
                elif canonical_op.kind == "LIST_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["list"]
                elif canonical_op.kind == "TUPLE_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["tuple"]
                elif canonical_op.kind == "SET_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["set"]
                elif canonical_op.kind == "FROZENSET_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["frozenset"]
                elif canonical_op.kind == "RANGE_NEW":
                    type_tag = BUILTIN_TYPE_TAGS["range"]
            if type_tag is not None and result_name != "none":
                value_type_tags[result_name] = type_tag
                state_dirty = True
            if canonical_op.kind == "IS" and result_name != "none":
                value_type_tags[result_name] = BUILTIN_TYPE_TAGS["bool"]
                state_dirty = True
            if value_key is not None and result_name != "none":
                available_values[value_key] = canonical_op.result
                state_dirty = True

            if self._is_canonicalization_barrier_op(canonical_op.kind):
                aliases.clear()
                const_int_values.clear()
                value_type_tags.clear()
                available_values.clear()
                guard_dict_shapes.clear()
                state_dirty = True

            if effect_class == "writes_heap":
                write_alias_classes = self._heap_alias_classes_for_write_op(
                    canonical_op, value_type_tags
                )
                if self._is_uncertain_heap_boundary(canonical_op.kind):
                    memory_epoch += 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_heap_read_key(key)
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                    for alias_class in sorted(alias_epochs):
                        alias_epochs[alias_class] = alias_epochs.get(alias_class, 0) + 1
                    guard_dict_shapes.clear()
                    state_dirty = True
                    continue
                if canonical_op.args and isinstance(canonical_op.args[0], MoltValue):
                    obj_name = canonical_op.args[0].name
                    object_epochs[obj_name] = object_epochs.get(obj_name, 0) + 1
                    state_dirty = True
                if write_alias_classes:
                    for alias_class in sorted(write_alias_classes):
                        alias_epochs[alias_class] = alias_epochs.get(alias_class, 0) + 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_read_key_invalidated_by_alias_classes(
                            key, write_alias_classes
                        )
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                else:
                    memory_epoch += 1
                    state_dirty = True
                    stale_read_keys = [
                        key
                        for key in list(available_values.keys())
                        if self._is_heap_read_key(key)
                    ]
                    for key in stale_read_keys:
                        available_values.pop(key, None)
                guard_dict_shapes.clear()
                state_dirty = True

        state["alias_epochs"] = alias_epochs
        state["object_epochs"] = object_epochs
        state["memory_epoch"] = memory_epoch
        if state_dirty:
            self._invalidate_canonicalization_state_signature(state)
        return out, state

    def _collect_arg_value_names(self, value: Any, out: set[str]) -> None:
        if isinstance(value, MoltValue):
            out.add(value.name)
            return
        if isinstance(value, list):
            for item in value:
                self._collect_arg_value_names(item, out)
            return
        if isinstance(value, tuple):
            for item in value:
                self._collect_arg_value_names(item, out)
            return
        if isinstance(value, dict):
            for key, item in value.items():
                self._collect_arg_value_names(key, out)
                self._collect_arg_value_names(item, out)

    def _compute_block_use_def(self, ops: list[MoltOp]) -> tuple[set[str], set[str]]:
        use: set[str] = set()
        defs: set[str] = set()
        for op in ops:
            arg_names: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, arg_names)
            use.update(name for name in arg_names if name not in defs)
            out_name = op.result.name
            if out_name != "none":
                defs.add(out_name)
        return use, defs

    def _collect_defined_value_names(self, ops: list[MoltOp]) -> set[str]:
        defined: set[str] = set()
        for op in ops:
            out_name = op.result.name
            if out_name != "none":
                defined.add(out_name)
        return defined

    def _find_unbound_value_uses(
        self, ops: list[MoltOp], *, params: Sequence[str] = ()
    ) -> list[tuple[int, str, str]]:
        defined: set[str] = set(params)
        defined.update(self._collect_defined_value_names(ops))
        missing: list[tuple[int, str, str]] = []
        for idx, op in enumerate(ops):
            used_names: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, used_names)
            for name in sorted(used_names):
                if name != "none" and name not in defined:
                    missing.append((idx, op.kind, name))
        return missing

    def _infer_predefined_value_names(self, ops: list[MoltOp]) -> set[str]:
        used: set[str] = set()
        for op in ops:
            for arg in op.args:
                self._collect_arg_value_names(arg, used)
        defined = self._collect_defined_value_names(ops)
        return used - defined

    def _verify_definite_assignment_in_ops(
        self,
        ops: list[MoltOp],
        *,
        predefined_value_names: set[str] | None = None,
    ) -> list[tuple[int, str, str]]:
        if not ops:
            return []

        predefined = set(predefined_value_names or set())
        cfg: CFGGraph = build_cfg(ops)
        if not cfg.blocks:
            return []
        all_defs = self._collect_defined_value_names(ops).union(predefined)

        # Track which value names are produced by MISSING ops so we can
        # verify they haven't been eliminated by a prior pass.
        missing_value_defs: set[str] = set()
        for op in ops:
            if op.kind == "MISSING" and op.result.name != "none":
                missing_value_defs.add(op.result.name)

        # Propagate MISSING taint transitively through PHI nodes: if every
        # input to a PHI is MISSING-tainted, the PHI result is also tainted.
        # This catches cases where branch pruning collapses a PHI to a single
        # MISSING-carrying input that escapes into CALL arg positions.
        missing_tainted: set[str] = set(missing_value_defs)
        _phi_changed = True
        while _phi_changed:
            _phi_changed = False
            for op in ops:
                if op.kind != "PHI" or not op.args:
                    continue
                out_name = op.result.name
                if out_name == "none" or out_name in missing_tainted:
                    continue
                phi_value_args = [arg for arg in op.args if isinstance(arg, MoltValue)]
                if phi_value_args and all(
                    arg.name in missing_tainted for arg in phi_value_args
                ):
                    missing_tainted.add(out_name)
                    _phi_changed = True

        block_defs: dict[int, set[str]] = {}
        for block in cfg.blocks:
            defs: set[str] = set()
            for op in ops[block.start : block.end]:
                out_name = op.result.name
                if out_name != "none":
                    defs.add(out_name)
            block_defs[block.id] = defs

        in_defs: dict[int, set[str]] = {}
        out_defs: dict[int, set[str]] = {}
        for block_id in range(len(cfg.blocks)):
            if block_id == 0:
                initial = set(predefined)
            elif block_id in cfg.reachable:
                initial = set(all_defs)
            else:
                initial = set()
            in_defs[block_id] = initial
            out_defs[block_id] = initial.union(block_defs[block_id])

        changed = True
        while changed:
            changed = False
            for block_id in range(1, len(cfg.blocks)):
                if block_id not in cfg.reachable:
                    continue
                preds = [
                    pred
                    for pred in cfg.predecessors.get(block_id, [])
                    if pred in cfg.reachable
                ]
                if not preds:
                    new_in = set(predefined)
                else:
                    new_in = set.intersection(*(out_defs[pred] for pred in preds))
                new_out = new_in.union(block_defs[block_id])
                if new_in != in_defs[block_id] or new_out != out_defs[block_id]:
                    in_defs[block_id] = new_in
                    out_defs[block_id] = new_out
                    changed = True

        failures: list[tuple[int, str, str]] = []
        definition_index: dict[str, int] = {}
        definition_block: dict[str, int] = {}
        for op_idx, op in enumerate(ops):
            out_name = op.result.name
            if out_name == "none":
                continue
            if out_name in definition_index:
                failures.append((op_idx, op.kind, out_name))
                continue
            definition_index[out_name] = op_idx
            definition_block[out_name] = cfg.index_to_block[op_idx]

        # Collect which value names are consumed by GETATTR/CALL/LOOKUP ops
        # as default or sentinel arguments — these are the critical consumers
        # of MISSING sentinels.
        _missing_sentinel_consumer_ops = {
            "GETATTR_NAME_DEFAULT",
            "CALL",
            "CALL_INDIRECT",
            "CALL_INTERNAL",
            "DICT_UPDATE_MISSING",
        }

        for block in cfg.blocks:
            block_id = block.id
            if block_id not in cfg.reachable:
                continue
            local_defs = set(in_defs[block_id])
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                used: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, used)
                missing = sorted(name for name in used if name not in local_defs)
                for name in missing:
                    failures.append((op_idx, op.kind, name))
                for name in sorted(used):
                    if name in predefined:
                        continue
                    def_idx = definition_index.get(name)
                    if def_idx is None:
                        # Value is used but has no definition at all — if it
                        # was originally a MISSING sentinel that got removed,
                        # flag this as a failure.
                        if name in missing_value_defs:
                            failures.append((op_idx, op.kind, name))
                        continue
                    def_block = definition_block[name]
                    if def_block not in cfg.dominators.get(block_id, set()):
                        failures.append((op_idx, op.kind, name))
                        continue
                    if def_block == block_id and def_idx >= op_idx:
                        failures.append((op_idx, op.kind, name))
                # Extra check: ops that consume MISSING-produced values
                # (sentinel consumers) must have those definitions still
                # present and dominating.
                if op.kind in _missing_sentinel_consumer_ops:
                    for arg in op.args:
                        if (
                            isinstance(arg, MoltValue)
                            and arg.name in missing_value_defs
                        ):
                            if arg.name not in local_defs:
                                failures.append((op_idx, op.kind, arg.name))
                # Transitive MISSING taint check: if a CALL/CALL_INDIRECT
                # arg is MISSING-tainted through a PHI collapse (not a direct
                # MISSING def), that means an uninitialized variable leaked
                # into a call site after branch pruning.
                if op.kind in {"CALL", "CALL_INDIRECT", "CALL_INTERNAL"}:
                    for arg in op.args:
                        if isinstance(arg, MoltValue) and (
                            arg.name in missing_tainted
                            and arg.name not in missing_value_defs
                        ):
                            failures.append((op_idx, op.kind, arg.name))
                out_name = op.result.name
                if out_name != "none":
                    local_defs.add(out_name)
        return failures

    def _dead_op_lattice_class(self, op_kind: str) -> str:
        effect = self._op_effect_class(op_kind)
        if effect == "control":
            return "protected"
        if effect == "pure":
            return "pure"
        if effect in {"reads_heap", "writes_heap"}:
            return effect
        return "unknown"

    def _eliminate_dead_trivial_consts(self, ops: list[MoltOp]) -> list[MoltOp]:
        if not ops:
            return []

        func_stats = self._midend_function_stats()
        cfg: CFGGraph = build_cfg(ops)
        if not cfg.blocks:
            return []

        def normalize_anchor_arg(value: Any) -> Any:
            if isinstance(value, MoltValue):
                return ("v", value.name)
            if isinstance(value, tuple):
                return ("t", tuple(normalize_anchor_arg(item) for item in value))
            if isinstance(value, list):
                return ("l", tuple(normalize_anchor_arg(item) for item in value))
            if isinstance(value, dict):
                return (
                    "d",
                    tuple(
                        sorted(
                            (
                                normalize_anchor_arg(key),
                                normalize_anchor_arg(item),
                            )
                            for key, item in value.items()
                        )
                    ),
                )
            try:
                hash(value)
                return ("c", value)
            except TypeError:
                return ("r", repr(value))

        def anchor_key(op: MoltOp) -> tuple[Any, ...] | None:
            out_name = op.result.name
            if out_name == "none":
                return None
            if self._dead_op_lattice_class(op.kind) != "pure":
                return None
            return (op.kind, tuple(normalize_anchor_arg(arg) for arg in op.args))

        anchor_first_result: dict[tuple[Any, ...], str] = {}
        anchor_counts: dict[tuple[Any, ...], int] = {}
        for op in ops:
            key = anchor_key(op)
            if key is None:
                continue
            anchor_counts[key] = anchor_counts.get(key, 0) + 1
            anchor_first_result.setdefault(key, op.result.name)
        preserve_anchor_results: set[str] = {
            anchor_first_result[key]
            for key, count in anchor_counts.items()
            if count > 1 and key in anchor_first_result
        }

        pure_attempted = 0
        uses_by_index: dict[int, set[str]] = {}
        defs_by_name: dict[str, list[int]] = {}
        removable_indices: set[int] = set()
        required_values: set[str] = set()
        worklist: list[str] = []

        def require_value(name: str) -> None:
            if name == "none" or name in required_values:
                return
            required_values.add(name)
            worklist.append(name)

        for idx, op in enumerate(ops):
            out_name = op.result.name
            uses: set[str] = set()
            for arg in op.args:
                self._collect_arg_value_names(arg, uses)
            uses_by_index[idx] = uses

            lattice_class = self._dead_op_lattice_class(op.kind)
            if out_name != "none":
                defs_by_name.setdefault(out_name, []).append(idx)
                if lattice_class == "pure":
                    pure_attempted += 1
                    # MISSING ops are runtime sentinels (uninitialized locals,
                    # optional defaults) that downstream GETATTR/CALL sites
                    # depend on — never eliminate them.
                    if out_name not in preserve_anchor_results and op.kind != "MISSING":
                        removable_indices.add(idx)

        for idx, op in enumerate(ops):
            if idx in removable_indices:
                continue
            for name in uses_by_index[idx]:
                require_value(name)

        required_removable_indices: set[int] = set()
        while worklist:
            value_name = worklist.pop()
            for producer_idx in defs_by_name.get(value_name, []):
                if producer_idx not in removable_indices:
                    continue
                if producer_idx in required_removable_indices:
                    continue
                required_removable_indices.add(producer_idx)
                for dependency_name in uses_by_index[producer_idx]:
                    require_value(dependency_name)

        remove_indices = removable_indices - required_removable_indices
        pure_removed = len(remove_indices)
        removed_count = pure_removed
        out = [op for idx, op in enumerate(ops) if idx not in remove_indices]
        self.midend_stats["dce_removed_total"] += removed_count
        func_stats["dce_pure_op_attempted"] += pure_attempted
        func_stats["dce_pure_op_accepted"] += pure_removed
        func_stats["dce_pure_op_rejected"] += max(0, pure_attempted - pure_removed)
        return out

    def _op_may_raise_for_sccp(self, op_kind: str) -> bool:
        non_raising = {
            "LINE",
            "IF",
            "ELSE",
            "END_IF",
            "LOOP_START",
            "LOOP_END",
            "LOOP_BREAK",
            "LOOP_BREAK_IF_TRUE",
            "LOOP_BREAK_IF_FALSE",
            "LOOP_BREAK_IF_EXCEPTION",
            "LOOP_CONTINUE",
            "TRY_START",
            "TRY_END",
            "JUMP",
            "LABEL",
            "STATE_LABEL",
            "PHI",
            "CONST",
            "CONST_BIGINT",
            "CONST_BOOL",
            "CONST_FLOAT",
            "CONST_STR",
            "CONST_BYTES",
            "CONST_NONE",
            "CONST_NOT_IMPLEMENTED",
            "CONST_ELLIPSIS",
            "MISSING",
            "ADD",
            "SUB",
            "MUL",
            "NOT",
            "IS",
            "TYPE_OF",
            "LEN",
            "EXCEPTION_NEW_BUILTIN",
            "EXCEPTION_NEW_BUILTIN_EMPTY",
            "EXCEPTION_NEW_BUILTIN_ONE",
            "EXCEPTION_MATCH_BUILTIN",
            "STORE_VAR",
            "DELETE_VAR",
            "LOAD_VAR",
        }
        if op_kind in non_raising:
            return False
        if op_kind.startswith("STATE_"):
            return False
        return True

    def _compute_sccp(
        self,
        ops: list[MoltOp],
        cfg: CFGGraph,
        *,
        max_iters_override: int | None = None,
    ) -> SCCPResult:
        # Current contract: SCCP tracks executable edges and supplies facts for
        # conservative loop/try marker rewrites only; broader LOOP_END and
        # exceptional-handler CFG rewrites remain roadmap work and must preserve
        # dominance/post-dominance invariants.
        in_values: dict[int, dict[str, Any]] = {block.id: {} for block in cfg.blocks}
        out_values: dict[int, dict[str, Any]] = {block.id: {} for block in cfg.blocks}
        executable_blocks: set[int] = {0} if cfg.blocks else set()
        executable_edges: set[tuple[int, int]] = set()
        branch_choice_by_if_index: dict[int, bool] = {}
        loop_break_choice_by_index: dict[int, bool] = {}
        try_exception_possible_by_start: dict[int, bool] = {}
        try_normal_possible_by_start: dict[int, bool] = {}
        guard_fail_indices: set[int] = set()
        loop_bound_facts = self._analyze_loop_bound_facts(ops, cfg)
        loop_compare_truth = self._analyze_affine_loop_compare_truth(ops, cfg)
        type_of_origin: dict[str, str] = {}
        for op in ops:
            if (
                op.kind == "TYPE_OF"
                and len(op.args) == 1
                and isinstance(op.args[0], MoltValue)
                and op.result.name != "none"
            ):
                type_of_origin[op.result.name] = op.args[0].name

        def type_fact_key(name: str) -> str:
            return f"__tag__:{name}"

        def dict_shape_fact_key(name: str) -> str:
            return f"__dict_shape__:{name}"

        def is_overdefined(value: Any) -> bool:
            return value is _SCCP_OVERDEFINED

        def is_missing_sentinel(value: Any) -> bool:
            return value is _SCCP_MISSING

        def merge_lattice(left: Any, right: Any) -> Any:
            # MISSING sentinels must never fold: if either side is MISSING,
            # the merge is overdefined so downstream operations cannot
            # constant-fold through a MISSING value.
            if is_missing_sentinel(left) or is_missing_sentinel(right):
                return _SCCP_OVERDEFINED
            if left is _SCCP_UNKNOWN:
                return right
            if right is _SCCP_UNKNOWN:
                return left
            if is_overdefined(left) or is_overdefined(right):
                return _SCCP_OVERDEFINED
            if left == right:
                return left
            return _SCCP_OVERDEFINED

        def merge_states(states: list[dict[str, Any]]) -> dict[str, Any]:
            if not states:
                return {}
            merged: dict[str, Any] = {}
            all_keys: set[str] = set()
            for state in states:
                all_keys.update(state.keys())
            for key in all_keys:
                current: Any = _SCCP_UNKNOWN
                for state in states:
                    current = merge_lattice(current, state.get(key, _SCCP_UNKNOWN))
                    if is_overdefined(current):
                        break
                if current is not _SCCP_UNKNOWN:
                    merged[key] = current
            return merged

        def value_lattice(name: str, known: dict[str, Any]) -> Any:
            return known.get(name, _SCCP_UNKNOWN)

        def value_type_tag(name: str, known: dict[str, Any]) -> int | None:
            fact = known.get(type_fact_key(name))
            if isinstance(fact, int):
                return fact
            value = value_lattice(name, known)
            if (
                value is _SCCP_UNKNOWN
                or is_overdefined(value)
                or is_missing_sentinel(value)
            ):
                return None
            return self._const_type_tag_for_lattice_value(value)

        def scalar_cmp_supported(value: Any) -> bool:
            if value is None:
                return True
            if isinstance(value, bool):
                return True
            if isinstance(value, int):
                return True
            if isinstance(value, float):
                return True
            if isinstance(value, str):
                return True
            if isinstance(value, bytes):
                return True
            return False

        def eval_lattice_value(op: MoltOp, known: dict[str, Any], op_index: int) -> Any:
            # MISSING ops produce runtime sentinel values that must never be
            # constant-folded or propagated.  Return _SCCP_MISSING so that
            # any downstream consumer goes to overdefined via merge_lattice.
            if op.kind == "MISSING":
                return _SCCP_MISSING
            if op.kind == "CONST":
                return op.args[0]
            if op.kind == "CONST_BOOL":
                return bool(op.args[0])
            if op.kind == "CONST_BIGINT":
                return int(op.args[0])
            if op.kind == "CONST_FLOAT":
                return float(op.args[0])
            if op.kind == "CONST_STR":
                return str(op.args[0])
            if op.kind == "CONST_BYTES":
                return bytes(op.args[0])
            if op.kind == "CONST_NONE":
                return None
            if op.kind == "CONST_NOT_IMPLEMENTED":
                return NotImplemented
            if op.kind == "CONST_ELLIPSIS":
                return Ellipsis
            if op.kind == "PHI" and op.args:
                block_id = cfg.index_to_block.get(op_index)
                if block_id is not None:
                    block_preds = cfg.predecessors.get(block_id, [])
                    if len(block_preds) == len(op.args):
                        merged: Any = _SCCP_UNKNOWN
                        seen_exec = False
                        for arg, pred in zip(op.args, block_preds):
                            if (pred, block_id) not in executable_edges:
                                continue
                            if not isinstance(arg, MoltValue):
                                return _SCCP_OVERDEFINED
                            seen_exec = True
                            merged = merge_lattice(
                                merged, value_lattice(arg.name, known)
                            )
                            if is_overdefined(merged):
                                return _SCCP_OVERDEFINED
                        if seen_exec:
                            return merged
                        return _SCCP_UNKNOWN
                merged = _SCCP_UNKNOWN
                for arg in op.args:
                    if not isinstance(arg, MoltValue):
                        return _SCCP_OVERDEFINED
                    merged = merge_lattice(merged, value_lattice(arg.name, known))
                    if is_overdefined(merged):
                        return _SCCP_OVERDEFINED
                return merged
            if op.kind in {"ADD", "SUB", "MUL"} and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels must never fold through arithmetic.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if (
                    isinstance(lhs_value, int)
                    and not isinstance(lhs_value, bool)
                    and isinstance(rhs_value, int)
                    and not isinstance(rhs_value, bool)
                ):
                    if op.kind == "ADD":
                        return lhs_value + rhs_value
                    if op.kind == "SUB":
                        return lhs_value - rhs_value
                    return lhs_value * rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "NOT" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value) or is_missing_sentinel(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(arg_value, bool):
                    return not arg_value
                return _SCCP_OVERDEFINED
            if op.kind == "ABS" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value) or is_missing_sentinel(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(arg_value, (int, float)):
                    return abs(arg_value)
                return _SCCP_OVERDEFINED
            if op.kind in {"AND", "OR"} and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels must never fold through boolean ops.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if isinstance(lhs_value, bool) and isinstance(rhs_value, bool):
                    if op.kind == "AND":
                        return lhs_value and rhs_value
                    return lhs_value or rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "IS" and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                # MISSING sentinels are singleton objects in the lattice but
                # represent distinct runtime values — never fold identity
                # comparisons through them.
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                return lhs_value is rhs_value
            if op.kind in {"EQ", "NE", "LT", "LE", "GT", "GE"} and len(op.args) == 2:
                proven_static = loop_compare_truth.get(op_index)
                if isinstance(proven_static, bool):
                    return proven_static
                loop_fact = loop_bound_facts.get(op_index)
                if loop_fact is not None:
                    proven = self._prove_monotonic_loop_compare(loop_fact)
                    if isinstance(proven, bool):
                        return proven
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if not scalar_cmp_supported(lhs_value) or not scalar_cmp_supported(
                    rhs_value
                ):
                    return _SCCP_OVERDEFINED
                try:
                    if op.kind == "EQ":
                        return lhs_value == rhs_value
                    if op.kind == "NE":
                        return lhs_value != rhs_value
                    if op.kind == "LT":
                        return lhs_value < rhs_value
                    if op.kind == "LE":
                        return lhs_value <= rhs_value
                    if op.kind == "GT":
                        return lhs_value > rhs_value
                    return lhs_value >= rhs_value
                except Exception:
                    return _SCCP_OVERDEFINED
            if op.kind == "STRING_EQ" and len(op.args) == 2:
                lhs = op.args[0]
                rhs = op.args[1]
                if not isinstance(lhs, MoltValue) or not isinstance(rhs, MoltValue):
                    return _SCCP_OVERDEFINED
                lhs_value = value_lattice(lhs.name, known)
                rhs_value = value_lattice(rhs.name, known)
                if lhs_value is _SCCP_UNKNOWN or rhs_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(lhs_value) or is_overdefined(rhs_value):
                    return _SCCP_OVERDEFINED
                if is_missing_sentinel(lhs_value) or is_missing_sentinel(rhs_value):
                    return _SCCP_OVERDEFINED
                if isinstance(lhs_value, str) and isinstance(rhs_value, str):
                    return lhs_value == rhs_value
                return _SCCP_OVERDEFINED
            if op.kind == "TYPE_OF" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                tag = value_type_tag(arg.name, known)
                if tag is not None:
                    return tag
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value):
                    return _SCCP_OVERDEFINED
                return _SCCP_OVERDEFINED
            if op.kind == "ISINSTANCE" and len(op.args) == 2:
                obj = op.args[0]
                classinfo = op.args[1]
                if not isinstance(obj, MoltValue) or not isinstance(
                    classinfo, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                class_value = value_lattice(classinfo.name, known)
                if class_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(class_value):
                    return _SCCP_OVERDEFINED
                obj_tag = value_type_tag(obj.name, known)
                if obj_tag is None:
                    return _SCCP_UNKNOWN
                if isinstance(class_value, int):
                    return obj_tag == class_value
                if isinstance(class_value, tuple) and all(
                    isinstance(item, int) for item in class_value
                ):
                    return obj_tag in class_value
                return _SCCP_OVERDEFINED
            if op.kind == "LEN" and len(op.args) == 1:
                arg = op.args[0]
                if not isinstance(arg, MoltValue):
                    return _SCCP_OVERDEFINED
                arg_value = value_lattice(arg.name, known)
                if arg_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(arg_value):
                    return _SCCP_OVERDEFINED
                if isinstance(
                    arg_value, (str, bytes, tuple, list, dict, set, frozenset, range)
                ):
                    return len(arg_value)
                return _SCCP_OVERDEFINED
            if op.kind == "CONTAINS" and len(op.args) == 2:
                container = op.args[0]
                item = op.args[1]
                if not isinstance(container, MoltValue) or not isinstance(
                    item, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                container_value = value_lattice(container.name, known)
                item_value = value_lattice(item.name, known)
                if container_value is _SCCP_UNKNOWN or item_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(container_value) or is_overdefined(item_value):
                    return _SCCP_OVERDEFINED
                if not isinstance(
                    container_value,
                    (str, bytes, tuple, list, dict, set, frozenset, range),
                ):
                    return _SCCP_OVERDEFINED
                try:
                    return item_value in container_value
                except Exception:
                    return _SCCP_OVERDEFINED
            if op.kind == "INDEX" and len(op.args) == 2:
                container = op.args[0]
                index = op.args[1]
                if not isinstance(container, MoltValue) or not isinstance(
                    index, MoltValue
                ):
                    return _SCCP_OVERDEFINED
                container_value = value_lattice(container.name, known)
                index_value = value_lattice(index.name, known)
                if container_value is _SCCP_UNKNOWN or index_value is _SCCP_UNKNOWN:
                    return _SCCP_UNKNOWN
                if is_overdefined(container_value) or is_overdefined(index_value):
                    return _SCCP_OVERDEFINED
                if isinstance(container_value, (tuple, list, str, bytes, range)):
                    if isinstance(index_value, int) and not isinstance(
                        index_value, bool
                    ):
                        try:
                            return container_value[index_value]
                        except Exception:
                            return _SCCP_OVERDEFINED
                    return _SCCP_OVERDEFINED
                if isinstance(container_value, dict):
                    try:
                        if index_value in container_value:
                            return container_value[index_value]
                    except Exception:
                        return _SCCP_OVERDEFINED
                return _SCCP_OVERDEFINED
            return _SCCP_OVERDEFINED

        def evaluate_try_behavior(start_idx: int, end_idx: int) -> tuple[bool, bool]:
            known: dict[str, Any] = {}
            may_raise = False
            may_complete_normally = True
            if end_idx <= start_idx + 1:
                return False, True
            for op_idx in range(start_idx + 1, end_idx):
                op = ops[op_idx]
                if op.kind in {
                    "IF",
                    "ELSE",
                    "END_IF",
                    "LOOP_START",
                    "LOOP_END",
                    "LOOP_BREAK",
                    "LOOP_BREAK_IF_TRUE",
                    "LOOP_BREAK_IF_FALSE",
                    "LOOP_BREAK_IF_EXCEPTION",
                    "LOOP_CONTINUE",
                    "TRY_START",
                    "TRY_END",
                    "JUMP",
                    "LABEL",
                    "STATE_LABEL",
                }:
                    return True, True
                if op.kind in {"GUARD_TAG", "GUARD_TYPE"} and len(op.args) == 2:
                    guarded = op.args[0]
                    expected = op.args[1]
                    if isinstance(guarded, MoltValue) and isinstance(
                        expected, MoltValue
                    ):
                        expected_value = value_lattice(expected.name, known)
                        guarded_tag = value_type_tag(guarded.name, known)
                        if (
                            isinstance(expected_value, int)
                            and guarded_tag is not None
                            and guarded_tag == expected_value
                        ):
                            known[type_fact_key(guarded.name)] = expected_value
                            continue
                    return True, False
                if op.kind == "GUARD_DICT_SHAPE" and len(op.args) == 3:
                    guarded = op.args[0]
                    dict_type = op.args[1]
                    version = op.args[2]
                    if (
                        isinstance(guarded, MoltValue)
                        and isinstance(dict_type, MoltValue)
                        and isinstance(version, MoltValue)
                    ):
                        shape_key = dict_shape_fact_key(guarded.name)
                        expected = (dict_type.name, version.name)
                        known_shape = known.get(shape_key)
                        if isinstance(known_shape, tuple):
                            if known_shape == expected:
                                continue
                            return True, False
                        known[shape_key] = expected
                        continue
                    return True, True
                if op.kind in {"RAISE", "RAISE_CAUSE", "RERAISE"}:
                    return True, False
                out_name = op.result.name
                lattice_value: Any = _SCCP_UNKNOWN
                if out_name != "none":
                    known.pop(out_name, None)
                    known.pop(type_fact_key(out_name), None)
                    known.pop(dict_shape_fact_key(out_name), None)
                    lattice_value = eval_lattice_value(op, known, op_idx)
                    # Promote MISSING sentinels to overdefined in try analysis too.
                    if is_missing_sentinel(lattice_value):
                        lattice_value = _SCCP_OVERDEFINED
                    if (
                        lattice_value is not _SCCP_UNKNOWN
                        and lattice_value is not _SCCP_OVERDEFINED
                    ):
                        known[out_name] = lattice_value
                        tag = self._const_type_tag_for_lattice_value(lattice_value)
                        if tag is not None:
                            known[type_fact_key(out_name)] = tag
                if self._op_may_raise_for_sccp(op.kind):
                    if (
                        lattice_value is _SCCP_OVERDEFINED
                        or lattice_value is _SCCP_UNKNOWN
                    ):
                        may_raise = True
            return may_raise, may_complete_normally

        for try_start_idx, try_end_idx in cfg.control.try_start_to_end.items():
            may_raise, may_complete_normally = evaluate_try_behavior(
                try_start_idx, try_end_idx
            )
            try_exception_possible_by_start[try_start_idx] = may_raise
            try_normal_possible_by_start[try_start_idx] = may_complete_normally
        check_exception_try_owner: dict[int, int] = {}
        for try_start_idx, try_end_idx in cfg.control.try_start_to_end.items():
            for op_idx in range(try_start_idx + 1, try_end_idx):
                if op_idx >= len(ops) or ops[op_idx].kind != "CHECK_EXCEPTION":
                    continue
                owner = check_exception_try_owner.get(op_idx)
                if owner is None or try_start_idx > owner:
                    check_exception_try_owner[op_idx] = try_start_idx

        value_users: dict[str, set[int]] = {}
        for op_idx, op in enumerate(ops):
            block_id = cfg.index_to_block.get(op_idx)
            if block_id is None:
                continue
            for arg in op.args:
                if isinstance(arg, MoltValue):
                    value_users.setdefault(arg.name, set()).add(block_id)

        iterations = 0
        ssa_defs = sum(1 for op in ops if op.result.name != "none")
        if max_iters_override is not None and max_iters_override > 0:
            max_iterations = max_iters_override
        elif self.midend_env.sccp_iter_cap_override is not None:
            max_iterations = self.midend_env.sccp_iter_cap_override
        else:
            # Dynamic cap keeps compile-time bounded while scaling with function/CFG size.
            # Keep the default ceiling conservative so wasm builds cannot stall for
            # minutes in pathological SCCP worklists.
            cfg_edge_count = sum(len(succs) for succs in cfg.successors.values())
            max_iterations = max(
                2048,
                min(
                    131072,
                    (len(cfg.blocks) * 96) + (cfg_edge_count * 48) + (ssa_defs * 24),
                ),
            )
        func_stats = self._midend_function_stats()

        block_queue: deque[int] = deque()
        queued_blocks: set[int] = set()
        edge_queue: deque[tuple[int, int]] = deque()
        queued_edges: set[tuple[int, int]] = set()
        value_queue: deque[str] = deque()
        queued_values: set[str] = set()

        def enqueue_block(block_id: int) -> None:
            if block_id in queued_blocks:
                return
            queued_blocks.add(block_id)
            block_queue.append(block_id)

        def enqueue_edge(src: int, dst: int) -> None:
            edge = (src, dst)
            if edge in executable_edges or edge in queued_edges:
                return
            queued_edges.add(edge)
            edge_queue.append(edge)

        def enqueue_value(name: str) -> None:
            if name in queued_values:
                return
            queued_values.add(name)
            value_queue.append(name)

        if cfg.blocks:
            enqueue_block(0)

        while block_queue or edge_queue or value_queue:
            if edge_queue:
                src, dst = edge_queue.popleft()
                queued_edges.discard((src, dst))
                if (src, dst) in executable_edges:
                    continue
                executable_edges.add((src, dst))
                if dst not in executable_blocks:
                    executable_blocks.add(dst)
                enqueue_block(dst)
                continue

            if value_queue:
                value_name = value_queue.popleft()
                queued_values.discard(value_name)
                for block_id in value_users.get(value_name, ()):
                    if block_id in executable_blocks:
                        enqueue_block(block_id)
                continue

            iterations += 1
            if iterations > max_iterations:
                self.midend_stats["sccp_iteration_cap_hits"] = (
                    self.midend_stats.get("sccp_iteration_cap_hits", 0) + 1
                )
                func_stats["sccp_iteration_cap_hits"] += 1
                all_blocks = {block.id for block in cfg.blocks}
                all_edges = {
                    (src, dst) for src, succs in cfg.successors.items() for dst in succs
                }
                conservative_try = {
                    start_idx: True for start_idx in cfg.control.try_start_to_end
                }
                return SCCPResult(
                    in_values={block.id: {} for block in cfg.blocks},
                    out_values={block.id: {} for block in cfg.blocks},
                    executable_blocks=all_blocks,
                    executable_edges=all_edges,
                    branch_choice_by_if_index={},
                    loop_break_choice_by_index={},
                    try_exception_possible_by_start=conservative_try,
                    try_normal_possible_by_start=dict(conservative_try),
                    guard_fail_indices=set(),
                )

            block_id = block_queue.popleft()
            queued_blocks.discard(block_id)
            if block_id not in executable_blocks:
                continue
            block = cfg.blocks[block_id]

            if block_id == 0:
                new_in: dict[str, Any] = {}
            else:
                exec_preds = [
                    pred
                    for pred in cfg.predecessors.get(block_id, [])
                    if (pred, block_id) in executable_edges
                ]
                pred_states = [out_values[pred] for pred in exec_preds]
                new_in = merge_states(pred_states)

            if new_in != in_values[block_id]:
                in_values[block_id] = new_in

            known = dict(new_in)
            block_traps = False
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if op.kind in {"GUARD_TAG", "GUARD_TYPE"} and len(op.args) == 2:
                    guarded = op.args[0]
                    expected = op.args[1]
                    if isinstance(guarded, MoltValue) and isinstance(
                        expected, MoltValue
                    ):
                        expected_value = known.get(expected.name, _SCCP_UNKNOWN)
                        if isinstance(expected_value, int):
                            guarded_tag = value_type_tag(guarded.name, known)
                            if (
                                guarded_tag is not None
                                and guarded_tag != expected_value
                            ):
                                guard_fail_indices.add(op_idx)
                                block_traps = True
                                break
                            known[type_fact_key(guarded.name)] = expected_value
                    continue
                if op.kind == "GUARD_DICT_SHAPE" and len(op.args) == 3:
                    guarded = op.args[0]
                    dict_type = op.args[1]
                    version = op.args[2]
                    if (
                        isinstance(guarded, MoltValue)
                        and isinstance(dict_type, MoltValue)
                        and isinstance(version, MoltValue)
                    ):
                        shape_key = dict_shape_fact_key(guarded.name)
                        expected_shape = (dict_type.name, version.name)
                        known_shape = known.get(shape_key)
                        if isinstance(known_shape, tuple):
                            if known_shape != expected_shape:
                                guard_fail_indices.add(op_idx)
                                block_traps = True
                                break
                        else:
                            known[shape_key] = expected_shape
                    continue
                out_name = op.result.name
                if out_name == "none":
                    continue
                known.pop(out_name, None)
                known.pop(type_fact_key(out_name), None)
                known.pop(dict_shape_fact_key(out_name), None)
                lattice_value = eval_lattice_value(op, known, op_idx)
                if lattice_value is _SCCP_UNKNOWN:
                    continue
                # MISSING sentinels must not propagate as constants through
                # the lattice — promote to overdefined so no downstream op
                # can constant-fold through a MISSING value.
                if is_missing_sentinel(lattice_value):
                    lattice_value = _SCCP_OVERDEFINED
                known[out_name] = lattice_value
                tag = self._const_type_tag_for_lattice_value(lattice_value)
                if tag is not None:
                    known[type_fact_key(out_name)] = tag
                if (
                    op.kind in {"EQ", "NE"}
                    and isinstance(lattice_value, bool)
                    and len(op.args) == 2
                ):
                    lhs = op.args[0]
                    rhs = op.args[1]
                    for type_side, tag_side in ((lhs, rhs), (rhs, lhs)):
                        if not isinstance(type_side, MoltValue) or not isinstance(
                            tag_side, MoltValue
                        ):
                            continue
                        guarded_name = type_of_origin.get(type_side.name)
                        if guarded_name is None:
                            continue
                        expected_tag = known.get(tag_side.name, _SCCP_UNKNOWN)
                        if not isinstance(expected_tag, int):
                            continue
                        implies_equal = (
                            lattice_value if op.kind == "EQ" else not lattice_value
                        )
                        if implies_equal:
                            known[type_fact_key(guarded_name)] = expected_tag
                if (
                    op.kind == "ISINSTANCE"
                    and lattice_value is True
                    and len(op.args) == 2
                ):
                    guarded_obj = op.args[0]
                    classinfo = op.args[1]
                    if isinstance(guarded_obj, MoltValue) and isinstance(
                        classinfo, MoltValue
                    ):
                        class_value = known.get(classinfo.name, _SCCP_UNKNOWN)
                        if isinstance(class_value, int):
                            known[type_fact_key(guarded_obj.name)] = class_value
                        elif isinstance(class_value, tuple):
                            tags = [
                                item for item in class_value if isinstance(item, int)
                            ]
                            if len(tags) == 1:
                                known[type_fact_key(guarded_obj.name)] = tags[0]

            prior_out = out_values[block_id]
            out_changed_keys: list[str] = []
            if known != prior_out:
                # DETERMINISM (#73, #34 bug class): `out_changed_keys` drives the
                # order values are pushed onto the SCCP `value_queue` (see the
                # `enqueue_value(key)` loop below), which in turn dictates the
                # block-processing schedule of this worklist fixed point.  Built
                # from a `set[str]` union, its iteration order is
                # PYTHONHASHSEED-dependent — and while the SCCP lattice *result*
                # is order-independent (monotone), the NUMBER of node re-visits
                # to reach the fixed point is not.  For a function near the
                # `max_iterations` cap, a worse schedule can exceed the cap and
                # bail to the conservative empty-facts result, whereas a better
                # schedule converges with full const facts.  That flips
                # downstream CSE/const-dedup on or off, so the emitted IR
                # silently diverged across hash seeds.  Sort the changed keys at
                # this construction site so the worklist schedule — and thus the
                # cap behaviour and the compiled IR — is byte-stable.
                all_keys = set(prior_out.keys()) | set(known.keys())
                out_changed_keys = sorted(
                    key
                    for key in all_keys
                    if prior_out.get(key, _SCCP_UNKNOWN)
                    != known.get(key, _SCCP_UNKNOWN)
                )
                out_values[block_id] = known

            succs = cfg.successors.get(block_id, [])
            chosen_succs = succs
            if block_traps:
                chosen_succs = []
            elif block.start < block.end:
                terminator_idx = block.end - 1
                terminator = ops[terminator_idx]
                if terminator.kind == "IF" and len(terminator.args) == 1:
                    cond = terminator.args[0]
                    cond_value: Any = _SCCP_UNKNOWN
                    if isinstance(cond, MoltValue):
                        cond_value = known.get(cond.name, _SCCP_UNKNOWN)
                    if isinstance(cond_value, bool):
                        branch_choice_by_if_index[terminator_idx] = cond_value
                        if cond_value and succs:
                            chosen_succs = [succs[0]]
                        elif not cond_value and len(succs) >= 2:
                            chosen_succs = [succs[1]]
                    else:
                        branch_choice_by_if_index.pop(terminator_idx, None)
                elif (
                    terminator.kind in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE"}
                    and len(terminator.args) == 1
                ):
                    cond = terminator.args[0]
                    cond_value: Any = _SCCP_UNKNOWN
                    if isinstance(cond, MoltValue):
                        cond_value = known.get(cond.name, _SCCP_UNKNOWN)
                    if isinstance(cond_value, bool) and len(succs) >= 2:
                        if terminator.kind == "LOOP_BREAK_IF_TRUE":
                            break_taken = bool(cond_value)
                        else:
                            break_taken = not bool(cond_value)
                        loop_break_choice_by_index[terminator_idx] = break_taken
                        chosen_succs = [succs[1] if break_taken else succs[0]]
                    else:
                        loop_break_choice_by_index.pop(terminator_idx, None)
                elif terminator.kind == "TRY_START":
                    can_raise = try_exception_possible_by_start.get(
                        terminator_idx, True
                    )
                    if not can_raise and succs:
                        chosen_succs = [succs[0]]
                elif terminator.kind == "CHECK_EXCEPTION":
                    owner_start = check_exception_try_owner.get(terminator_idx)
                    if owner_start is not None:
                        can_raise = try_exception_possible_by_start.get(
                            owner_start, True
                        )
                        if not can_raise and succs:
                            chosen_succs = [succs[0]]
                elif terminator.kind == "LOOP_END" and len(succs) >= 2:
                    loop_start_idx = cfg.control.loop_end_to_start.get(terminator_idx)
                    back_succ = (
                        None
                        if loop_start_idx is None
                        else cfg.index_to_block.get(loop_start_idx)
                    )
                    if back_succ is not None:
                        exit_succ = next(
                            (succ for succ in succs if succ != back_succ),
                            None,
                        )
                        back_exec = back_succ in executable_blocks
                        exit_exec = (
                            exit_succ in executable_blocks
                            if exit_succ is not None
                            else False
                        )
                        if back_exec and not exit_exec:
                            chosen_succs = [back_succ]
                        elif exit_succ is not None and exit_exec and not back_exec:
                            chosen_succs = [exit_succ]

            for succ in chosen_succs:
                enqueue_edge(block_id, succ)

            if out_changed_keys:
                for key in out_changed_keys:
                    if not key.startswith("__"):
                        enqueue_value(key)
                for succ in cfg.successors.get(block_id, []):
                    if succ in executable_blocks:
                        enqueue_block(succ)

        return SCCPResult(
            in_values=in_values,
            out_values=out_values,
            executable_blocks=executable_blocks,
            executable_edges=executable_edges,
            branch_choice_by_if_index=branch_choice_by_if_index,
            loop_break_choice_by_index=loop_break_choice_by_index,
            try_exception_possible_by_start=try_exception_possible_by_start,
            try_normal_possible_by_start=try_normal_possible_by_start,
            guard_fail_indices=guard_fail_indices,
        )

    def _sccp_in_const_int_values(self, sccp: SCCPResult) -> dict[int, dict[str, int]]:
        in_int_values: dict[int, dict[str, int]] = {}
        for block_id, known in sccp.in_values.items():
            in_int_values[block_id] = {
                key: value
                for key, value in known.items()
                if (
                    not str(key).startswith("__tag__:")
                    and value is not _SCCP_OVERDEFINED
                    and isinstance(value, int)
                    and not isinstance(value, bool)
                )
            }
        return in_int_values

    def _trim_phi_args_by_executable_edges(
        self,
        ops: list[MoltOp],
        cfg: CFGGraph,
        executable_edges: set[tuple[int, int]],
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        trimmed = 0
        out: list[MoltOp] = []
        for block in cfg.blocks:
            block_preds = cfg.predecessors.get(block.id, [])
            # Look through single-predecessor post-merge blocks: if this
            # block has exactly one predecessor that is itself a merge point
            # (multiple predecessors), the PHI args correspond to the merge
            # block's predecessors, not the direct predecessor.
            effective_preds = block_preds
            edge_target = block.id
            if (
                len(block_preds) == 1
                and len(cfg.predecessors.get(block_preds[0], [])) > 1
            ):
                effective_preds = cfg.predecessors.get(block_preds[0], [])
                edge_target = block_preds[0]
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if (
                    op.kind == "PHI"
                    and op.args
                    and len(op.args) == len(effective_preds)
                    and len(effective_preds) > 1
                ):
                    kept_args = [
                        arg
                        for arg, pred in zip(op.args, effective_preds)
                        if (pred, edge_target) in executable_edges
                    ]
                    normalized_args = kept_args
                    if kept_args and all(
                        isinstance(arg, MoltValue)
                        and isinstance(kept_args[0], MoltValue)
                        and arg.name == kept_args[0].name
                        for arg in kept_args
                    ):
                        normalized_args = [kept_args[0]]
                    if 0 < len(normalized_args) < len(op.args):
                        out.append(
                            MoltOp(
                                kind=op.kind,
                                args=normalized_args,
                                result=op.result,
                                metadata=op.metadata,
                            )
                        )
                        trimmed += len(op.args) - len(normalized_args)
                        continue
                out.append(op)
        return out, trimmed

    def _align_phi_args_to_cfg_predecessors(
        self, ops: list[MoltOp], cfg: CFGGraph
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        rewrites = 0
        out: list[MoltOp] = []
        for block in cfg.blocks:
            block_preds = cfg.predecessors.get(block.id, [])
            # Look through single-predecessor post-merge blocks to find
            # the effective predecessor count that PHI args should match.
            effective_preds = block_preds
            if (
                len(block_preds) == 1
                and len(cfg.predecessors.get(block_preds[0], [])) > 1
            ):
                effective_preds = cfg.predecessors.get(block_preds[0], [])
            expected = len(effective_preds)
            for op_idx in range(block.start, block.end):
                op = ops[op_idx]
                if op.kind != "PHI" or not op.args:
                    out.append(op)
                    continue
                if expected == 0:
                    out.append(op)
                    continue
                args = list(op.args)
                if len(args) == expected:
                    out.append(op)
                    continue
                if not all(isinstance(arg, MoltValue) for arg in args):
                    out.append(op)
                    continue
                first = cast(MoltValue, args[0])
                all_same = all(
                    isinstance(arg, MoltValue) and arg.name == first.name
                    for arg in args
                )
                if not all_same:
                    out.append(op)
                    continue
                if expected > 0:
                    # Expand to match effective predecessor count, then
                    # collapse identical args back down.
                    expanded = [first for _ in range(expected)]
                    if all(
                        isinstance(a, MoltValue) and a.name == first.name
                        for a in expanded
                    ):
                        normalized = [first]
                    else:
                        normalized = expanded
                    out.append(
                        MoltOp(
                            kind=op.kind,
                            args=normalized,
                            result=op.result,
                            metadata=op.metadata,
                        )
                    )
                    rewrites += abs(len(args) - expected)
                    continue
                out.append(op)
        return out, rewrites

    def _canonicalize_cfg_before_optimization(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        current = ops
        total_rewrites = 0
        for _ in range(8):
            round_rewrites = 0
            round_cfg = build_cfg(current)
            if not round_cfg.blocks:
                break

            step_ops, phi_align = self._align_phi_args_to_cfg_predecessors(
                current, round_cfg
            )
            round_rewrites += phi_align

            step_cfg = build_cfg(step_ops)
            if step_cfg.blocks:
                step_ops, ladder_threads = self._normalize_try_except_join_labels(
                    step_ops, cfg=step_cfg
                )
                round_rewrites += ladder_threads

            step_ops, label_prunes, jump_noops = self._prune_dead_labels_and_noop_jumps(
                step_ops
            )
            round_rewrites += label_prunes + jump_noops

            step_ops, structural_prunes = (
                self._canonicalize_structured_regions_pre_sccp(step_ops)
            )
            round_rewrites += structural_prunes

            if step_ops == current:
                break
            total_rewrites += round_rewrites
            current = step_ops

        return current, total_rewrites

    def _can_hoist_guard_pair(self, first: MoltOp, second: MoltOp) -> bool:
        if first.kind != second.kind:
            return False
        if first.kind not in {"GUARD_TAG", "GUARD_TYPE", "GUARD_DICT_SHAPE"}:
            return False
        if first.result.name != "none" or second.result.name != "none":
            return False
        if len(first.args) != len(second.args):
            return False
        for left, right in zip(first.args, second.args):
            if isinstance(left, MoltValue) and isinstance(right, MoltValue):
                if left.name != right.name:
                    return False
                continue
            if left != right:
                return False
        return True

    def _guard_signature(self, op: MoltOp) -> tuple[Any, ...] | None:
        if op.kind not in {"GUARD_TAG", "GUARD_TYPE", "GUARD_DICT_SHAPE"}:
            return None
        if op.result.name != "none":
            return None
        normalized_args: list[Any] = []
        for arg in op.args:
            if isinstance(arg, MoltValue):
                normalized_args.append(("v", arg.name))
            else:
                normalized_args.append(("c", arg))
        return (op.kind, tuple(normalized_args))

    def _collect_branch_defined_names(self, ops: list[MoltOp]) -> set[str]:
        out: set[str] = set()
        for op in ops:
            if op.result.name != "none":
                out.add(op.result.name)
        return out

    def _collect_movable_common_guards(
        self, then_ops: list[MoltOp], else_ops: list[MoltOp]
    ) -> list[MoltOp]:
        then_defined = self._collect_branch_defined_names(then_ops)
        else_defined = self._collect_branch_defined_names(else_ops)
        branch_defined = then_defined.union(else_defined)

        def candidates(ops: list[MoltOp]) -> dict[tuple[Any, ...], MoltOp]:
            found: dict[tuple[Any, ...], MoltOp] = {}
            for op in ops:
                sig = self._guard_signature(op)
                if sig is None:
                    continue
                arg_names: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, arg_names)
                if arg_names.intersection(branch_defined):
                    continue
                found.setdefault(sig, op)
            return found

        then_guards = candidates(then_ops)
        else_guards = candidates(else_ops)
        common_sigs = sorted(set(then_guards.keys()).intersection(else_guards.keys()))
        hoisted: list[MoltOp] = []
        for sig in common_sigs:
            source = then_guards[sig]
            hoisted.append(
                MoltOp(
                    kind=source.kind,
                    args=list(source.args),
                    result=MoltValue("none"),
                    metadata=source.metadata,
                )
            )
        return hoisted

    def _clear_invalidated_guard_signatures(
        self, available: set[tuple[Any, ...]], op: MoltOp
    ) -> None:
        if not available:
            return
        effect_class = self._op_effect_class(op.kind)
        if self._is_uncertain_heap_boundary(op.kind):
            available.clear()
            return
        if effect_class == "writes_heap":
            stale = [
                sig
                for sig in available
                if sig and isinstance(sig, tuple) and sig[0] == "GUARD_DICT_SHAPE"
            ]
            for sig in stale:
                available.discard(sig)

    def _eliminate_redundant_fused_dict_increment_guards(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        use_counts: dict[str, int] = {}
        users_by_value: dict[str, set[int]] = {}
        removable_guard_producer_kinds = {
            "BUILTIN_TYPE",
            "CLASS_LAYOUT_VERSION",
            "CLASS_VERSION",
            "CONST",
            "CONST_BOOL",
            "CONST_STR",
            "MISSING",
        }
        guard_consumer_skip_kinds = {"CHECK_EXCEPTION", "LINE"}
        for op_index, op in enumerate(ops):
            for arg in op.args:
                if isinstance(arg, MoltValue):
                    use_counts[arg.name] = use_counts.get(arg.name, 0) + 1
                    users_by_value.setdefault(arg.name, set()).add(op_index)

        fused_dict_operand_index = {
            "DICT_STR_INT_INC": 0,
            "STRING_SPLIT_WS_DICT_INC": 1,
            "STRING_SPLIT_SEP_DICT_INC": 2,
        }

        remove_indices: set[int] = set()
        removed_guards = 0
        for idx, op in enumerate(ops):
            op = ops[idx]
            if (
                op.kind == "GUARD_DICT_SHAPE"
                and len(op.args) == 3
                and op.result.name != "none"
                and use_counts.get(op.result.name, 0) == 0
                and idx + 1 < len(ops)
            ):
                next_idx = idx + 1
                while (
                    next_idx < len(ops)
                    and ops[next_idx].kind in guard_consumer_skip_kinds
                ):
                    next_idx += 1
                if next_idx >= len(ops):
                    continue
                next_op = ops[next_idx]
                dict_operand_index = fused_dict_operand_index.get(next_op.kind)
                guarded = op.args[0]
                if (
                    dict_operand_index is not None
                    and len(next_op.args) > dict_operand_index
                    and isinstance(guarded, MoltValue)
                    and isinstance(next_op.args[dict_operand_index], MoltValue)
                    and guarded.name == next_op.args[dict_operand_index].name
                ):
                    remove_indices.add(idx)
                    removed_guards += 1

        if remove_indices:
            changed = True
            while changed:
                changed = False
                for idx, op in enumerate(ops):
                    if (
                        idx in remove_indices
                        or op.kind not in removable_guard_producer_kinds
                    ):
                        continue
                    if op.result.name == "none":
                        continue
                    users = users_by_value.get(op.result.name, set())
                    if users and users.issubset(remove_indices):
                        remove_indices.add(idx)
                        changed = True

        if not remove_indices:
            return ops, 0

        out = [op for idx, op in enumerate(ops) if idx not in remove_indices]
        return out, removed_guards

    def _eliminate_redundant_guards_cfg(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int, int, int]:
        if not ops:
            return ops, 0, 0, 0
        cfg = build_cfg(ops)
        control = cfg.control
        if_to_else = control.if_to_else
        if_to_end = control.if_to_end
        loop_start_to_end = control.loop_start_to_end
        try_start_to_end = control.try_start_to_end

        def process_range(
            start: int,
            end: int,
            in_guards: set[tuple[Any, ...]],
        ) -> tuple[list[MoltOp], set[tuple[Any, ...]], int, int]:
            out: list[MoltOp] = []
            available = set(in_guards)
            attempted = 0
            accepted = 0
            i = start
            while i < end:
                op = ops[i]
                if op.kind == "IF" and i in if_to_end:
                    else_idx = if_to_else.get(i)
                    end_if_idx = if_to_end[i]
                    then_start = i + 1
                    then_end = else_idx if else_idx is not None else end_if_idx
                    then_ops, then_out, then_attempts, then_accepted = process_range(
                        then_start,
                        then_end,
                        set(available),
                    )
                    if else_idx is not None:
                        else_ops, else_out, else_attempts, else_accepted = (
                            process_range(
                                else_idx + 1,
                                end_if_idx,
                                set(available),
                            )
                        )
                    else:
                        else_ops, else_out, else_attempts, else_accepted = (
                            [],
                            set(available),
                            0,
                            0,
                        )
                    attempted += then_attempts + else_attempts
                    accepted += then_accepted + else_accepted
                    out.append(op)
                    out.extend(then_ops)
                    if else_idx is not None:
                        out.append(ops[else_idx])
                        out.extend(else_ops)
                    out.append(ops[end_if_idx])
                    available = then_out.intersection(else_out)
                    i = end_if_idx + 1
                    continue

                if op.kind == "LOOP_START" and i in loop_start_to_end:
                    loop_end = loop_start_to_end[i]
                    body_ops, body_out, body_attempts, body_accepted = process_range(
                        i + 1,
                        loop_end,
                        set(available),
                    )
                    attempted += body_attempts
                    accepted += body_accepted
                    out.append(op)
                    out.extend(body_ops)
                    out.append(ops[loop_end])
                    # Loop may execute zero times, so only guards guaranteed on both
                    # paths remain available after the loop region.
                    available = available.intersection(body_out)
                    i = loop_end + 1
                    continue

                if op.kind == "TRY_START" and i in try_start_to_end:
                    try_end = try_start_to_end[i]
                    body_ops, body_out, body_attempts, body_accepted = process_range(
                        i + 1,
                        try_end,
                        set(available),
                    )
                    attempted += body_attempts
                    accepted += body_accepted
                    out.append(op)
                    out.extend(body_ops)
                    out.append(ops[try_end])
                    # Try body may exit via exceptional edge, so preserve only
                    # guards guaranteed on both normal and exceptional paths.
                    available = available.intersection(body_out)
                    i = try_end + 1
                    continue

                sig = self._guard_signature(op)
                if sig is not None:
                    attempted += 1
                    if sig in available:
                        accepted += 1
                        i += 1
                        continue
                    available.add(sig)
                    out.append(op)
                    i += 1
                    continue

                self._clear_invalidated_guard_signatures(available, op)
                out.append(op)
                i += 1

            return out, available, attempted, accepted

        rewritten, _out_guards, attempted, accepted = process_range(0, len(ops), set())
        rejected = max(0, attempted - accepted)
        return rewritten, attempted, accepted, rejected

    def _op_equal_for_tail_merge(self, left: MoltOp, right: MoltOp) -> bool:
        return (
            left.kind == right.kind
            and left.result.name == right.result.name
            and left.args == right.args
            and left.metadata == right.metadata
        )

    def _can_tail_merge_op(self, op: MoltOp) -> bool:
        if op.result.name != "none":
            return False
        if op.kind in {
            "IF",
            "ELSE",
            "END_IF",
            "LOOP_START",
            "LOOP_END",
            "LOOP_BREAK",
            "LOOP_BREAK_IF_TRUE",
            "LOOP_BREAK_IF_FALSE",
            "LOOP_BREAK_IF_EXCEPTION",
            "LOOP_CONTINUE",
            "TRY_START",
            "TRY_END",
            "JUMP",
            "RETURN",
            "RAISE",
            "RAISE_CAUSE",
            "RERAISE",
            "LABEL",
            "STATE_LABEL",
        }:
            return False
        return True

    def _rewrite_structured_if_regions(
        self,
        ops: list[MoltOp],
        *,
        control: ControlMaps,
        branch_choice_by_if_index: dict[int, bool],
    ) -> tuple[list[MoltOp], int]:
        if_to_else = control.if_to_else
        if_to_end = control.if_to_end

        branch_prunes = 0

        def rewrite_range(start: int, end: int) -> list[MoltOp]:
            nonlocal branch_prunes
            out: list[MoltOp] = []
            i = start
            while i < end:
                op = ops[i]
                if op.kind != "IF" or i not in if_to_end:
                    out.append(op)
                    i += 1
                    continue

                else_idx = if_to_else.get(i)
                end_if_idx = if_to_end[i]
                then_start = i + 1
                then_end = else_idx if else_idx is not None else end_if_idx
                then_ops = rewrite_range(then_start, then_end)
                else_ops = (
                    rewrite_range(else_idx + 1, end_if_idx)
                    if else_idx is not None
                    else []
                )

                branch_choice = branch_choice_by_if_index.get(i)
                if branch_choice is True:
                    out.extend(then_ops)
                    branch_prunes += 1
                    i = end_if_idx + 1
                    continue
                if branch_choice is False:
                    out.extend(else_ops)
                    branch_prunes += 1
                    i = end_if_idx + 1
                    continue

                if else_idx is not None and then_ops and else_ops:
                    hoisted_guards = self._collect_movable_common_guards(
                        then_ops, else_ops
                    )
                    self.midend_stats["guard_hoist_attempts"] += max(
                        1, len(hoisted_guards)
                    )
                    if hoisted_guards:
                        self.midend_stats["guard_hoist_accepted"] += len(hoisted_guards)
                        for hoisted in hoisted_guards:
                            sig = self._guard_signature(hoisted)
                            if sig is None:
                                continue
                            then_ops = [
                                op
                                for op in then_ops
                                if self._guard_signature(op) != sig
                            ]
                            else_ops = [
                                op
                                for op in else_ops
                                if self._guard_signature(op) != sig
                            ]
                        out.extend(hoisted_guards)
                    else:
                        self.midend_stats["guard_hoist_rejected"] += 1

                shared_tail: list[MoltOp] = []
                while then_ops and else_ops:
                    tail_then = then_ops[-1]
                    tail_else = else_ops[-1]
                    if not self._op_equal_for_tail_merge(tail_then, tail_else):
                        break
                    if not self._can_tail_merge_op(tail_then):
                        break
                    shared_tail.append(tail_then)
                    then_ops = then_ops[:-1]
                    else_ops = else_ops[:-1]
                shared_tail.reverse()

                if not then_ops and not else_ops:
                    out.extend(shared_tail)
                    i = end_if_idx + 1
                    continue

                out.append(op)
                out.extend(then_ops)
                if else_idx is not None and else_ops:
                    out.append(ops[else_idx])
                    out.extend(else_ops)
                out.append(ops[end_if_idx])
                out.extend(shared_tail)
                i = end_if_idx + 1
            return out

        rewritten = rewrite_range(0, len(ops))
        return rewritten, branch_prunes

    def _canonicalize_structured_regions_pre_sccp(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0
        cfg = build_cfg(ops)
        control = cfg.control
        if_to_else = control.if_to_else
        if_to_end = control.if_to_end
        loop_start_to_end = control.loop_start_to_end
        try_start_to_end = control.try_start_to_end

        structural_prunes = 0

        def rewrite_range(start: int, end: int) -> list[MoltOp]:
            nonlocal structural_prunes
            out: list[MoltOp] = []
            i = start
            while i < end:
                op = ops[i]
                if op.kind == "IF" and i in if_to_end:
                    else_idx = if_to_else.get(i)
                    end_if_idx = if_to_end[i]
                    then_start = i + 1
                    then_end = else_idx if else_idx is not None else end_if_idx
                    then_ops = rewrite_range(then_start, then_end)
                    else_ops = (
                        rewrite_range(else_idx + 1, end_if_idx)
                        if else_idx is not None
                        else []
                    )
                    if not then_ops and not else_ops:
                        structural_prunes += 1
                        i = end_if_idx + 1
                        continue
                    if else_idx is not None and then_ops == else_ops:
                        structural_prunes += 1
                        out.extend(then_ops)
                        i = end_if_idx + 1
                        continue
                    out.append(op)
                    out.extend(then_ops)
                    if else_idx is not None and else_ops:
                        out.append(ops[else_idx])
                        out.extend(else_ops)
                    out.append(ops[end_if_idx])
                    i = end_if_idx + 1
                    continue
                if op.kind == "LOOP_START" and i in loop_start_to_end:
                    loop_end = loop_start_to_end[i]
                    body = rewrite_range(i + 1, loop_end)
                    if not body:
                        structural_prunes += 1
                        i = loop_end + 1
                        continue
                    out.append(op)
                    out.extend(body)
                    out.append(ops[loop_end])
                    i = loop_end + 1
                    continue
                if op.kind == "TRY_START" and i in try_start_to_end:
                    try_end = try_start_to_end[i]
                    body = rewrite_range(i + 1, try_end)
                    if not body:
                        structural_prunes += 1
                        i = try_end + 1
                        continue
                    out.append(op)
                    out.extend(body)
                    out.append(ops[try_end])
                    i = try_end + 1
                    continue
                out.append(op)
                i += 1
            return out

        rewritten = rewrite_range(0, len(ops))
        return rewritten, structural_prunes

    def _compute_postdominators_for_cfg(self, cfg: CFGGraph) -> dict[int, set[int]]:
        block_count = len(cfg.blocks)
        if block_count == 0:
            return {}
        reachable = set(cfg.reachable)
        postdom: dict[int, set[int]] = {}
        for block_id in range(block_count):
            if block_id in reachable:
                postdom[block_id] = set(reachable)
            else:
                postdom[block_id] = {block_id}

        exits = [
            block_id
            for block_id in reachable
            if not any(succ in reachable for succ in cfg.successors.get(block_id, []))
        ]
        if not exits and reachable:
            exits = [max(reachable)]
        for exit_block in exits:
            postdom[exit_block] = {exit_block}

        changed = True
        while changed:
            changed = False
            for block_id in reversed(range(block_count)):
                if block_id not in reachable or block_id in exits:
                    continue
                succs = [s for s in cfg.successors.get(block_id, []) if s in reachable]
                if not succs:
                    new_set = {block_id}
                else:
                    new_set = set.intersection(*(postdom[s] for s in succs))
                    new_set.add(block_id)
                if new_set != postdom[block_id]:
                    postdom[block_id] = new_set
                    changed = True
        return postdom

    def _rewrite_loop_try_edge_threading(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
        control: ControlMaps,
        executable_edges: set[tuple[int, int]],
        loop_break_choice_by_index: dict[int, bool],
        try_exception_possible_by_start: dict[int, bool],
        try_normal_possible_by_start: dict[int, bool],
        guard_fail_indices: set[int],
    ) -> tuple[list[MoltOp], int, int, int, int, int, int]:
        single_exec_succ_by_block: dict[int, int] = {}
        executable_blocks: set[int] = {0} if cfg.blocks else set()
        postdominators = self._compute_postdominators_for_cfg(cfg)
        for block in cfg.blocks:
            succs = cfg.successors.get(block.id, [])
            chosen = [succ for succ in succs if (block.id, succ) in executable_edges]
            for succ in chosen:
                executable_blocks.add(block.id)
                executable_blocks.add(succ)
            if len(chosen) == 1:
                single_exec_succ_by_block[block.id] = chosen[0]

        label_alias: dict[str, str] = {}

        def collect_label_aliases() -> None:
            def alias_target_from_body(body_ops: list[MoltOp]) -> str | None:
                if (
                    len(body_ops) == 1
                    and body_ops[0].kind == "JUMP"
                    and body_ops[0].args
                ):
                    return self._control_label_key(body_ops[0].args[0])
                if (
                    len(body_ops) == 2
                    and body_ops[0].kind == "CHECK_EXCEPTION"
                    and body_ops[0].args
                    and body_ops[1].kind == "JUMP"
                    and body_ops[1].args
                ):
                    check_key = self._control_label_key(body_ops[0].args[0])
                    jump_key = self._control_label_key(body_ops[1].args[0])
                    if check_key is not None and check_key == jump_key:
                        return jump_key
                return None

            for block in cfg.blocks:
                if block.start >= block.end:
                    continue
                head = ops[block.start]
                if head.kind not in {"LABEL", "STATE_LABEL"} or not head.args:
                    continue
                head_key = self._control_label_key(head.args[0])
                if head_key is None:
                    continue
                body_ops = [
                    ops[idx]
                    for idx in range(block.start + 1, block.end)
                    if ops[idx].kind != "LINE"
                ]
                target_key = alias_target_from_body(body_ops)
                if target_key is None and not body_ops:
                    succs = cfg.successors.get(block.id, [])
                    if len(succs) == 1:
                        succ_block = cfg.blocks[succs[0]]
                        succ_body = [
                            ops[idx]
                            for idx in range(succ_block.start, succ_block.end)
                            if ops[idx].kind != "LINE"
                        ]
                        target_key = alias_target_from_body(succ_body)
                if target_key is None or target_key == head_key:
                    continue
                if cfg.label_to_block.get(target_key) is None:
                    continue
                label_alias[head_key] = target_key

        def resolve_label_alias(label_key: str) -> str:
            resolved = label_key
            seen: set[str] = set()
            while resolved in label_alias and resolved not in seen:
                seen.add(resolved)
                resolved = label_alias[resolved]
            return resolved

        collect_label_aliases()

        try_remove_starts = {
            start
            for start, can_raise in try_exception_possible_by_start.items()
            if not can_raise
        }
        for start in control.try_start_to_end:
            block_id = cfg.index_to_block.get(start)
            if block_id is None:
                continue
            chosen = single_exec_succ_by_block.get(block_id)
            succs = cfg.successors.get(block_id, [])
            if chosen is not None and succs and chosen == succs[0]:
                try_remove_starts.add(start)
        try_remove_ends = {
            control.try_start_to_end[start]
            for start in try_remove_starts
            if start in control.try_start_to_end
        }

        try_unreachable_body_indices: set[int] = set()
        threaded_check_exception_jumps: dict[int, Any] = {}
        check_exception_elisions: set[int] = set()
        check_try_owner: dict[int, int] = {}
        for start, end in control.try_start_to_end.items():
            for idx in range(start + 1, end):
                if idx >= len(ops) or ops[idx].kind != "CHECK_EXCEPTION":
                    continue
                owner = check_try_owner.get(idx)
                if owner is None or start > owner:
                    check_try_owner[idx] = start
        for idx, start in check_try_owner.items():
            if not try_exception_possible_by_start.get(start, True):
                check_exception_elisions.add(idx)

        for start, end in control.try_start_to_end.items():
            if try_normal_possible_by_start.get(start, True):
                continue
            stop_idx: int | None = None
            for idx in range(start + 1, end):
                if idx in guard_fail_indices:
                    stop_idx = idx
                    break
                if ops[idx].kind in {"RAISE", "RAISE_CAUSE", "RERAISE"}:
                    stop_idx = idx
                    break
            if stop_idx is None:
                continue
            start_block = cfg.index_to_block.get(start)
            stop_block = cfg.index_to_block.get(stop_idx)
            end_block = cfg.index_to_block.get(end)
            if start_block is None or stop_block is None or end_block is None:
                continue
            if stop_block not in cfg.dominators.get(end_block, {end_block}):
                continue
            stop_postdominates_start = stop_block in postdominators.get(
                start_block, {start_block}
            )

            threaded_check_idx: int | None = None
            for check_idx in range(stop_idx + 1, end):
                check_op = ops[check_idx]
                if check_op.kind != "CHECK_EXCEPTION" or not check_op.args:
                    continue
                if any(
                    ops[mid].kind not in {"LINE", "LABEL", "STATE_LABEL"}
                    for mid in range(stop_idx + 1, check_idx)
                ):
                    continue
                check_block = cfg.index_to_block.get(check_idx)
                if check_block is None:
                    continue
                if stop_block not in cfg.dominators.get(check_block, {check_block}):
                    continue
                target_label = str(check_op.args[0])
                target_block = cfg.label_to_block.get(target_label)
                if target_block is None:
                    continue
                if target_block not in cfg.successors.get(check_block, []):
                    continue
                threaded_check_idx = check_idx
                target_key = self._control_label_key(check_op.args[0])
                if target_key is None:
                    threaded_check_exception_jumps[check_idx] = check_op.args[0]
                else:
                    resolved_key = resolve_label_alias(target_key)
                    threaded_check_exception_jumps[check_idx] = (
                        self._coerce_control_label_like(check_op.args[0], resolved_key)
                    )
                break

            if threaded_check_idx is not None:
                for idx in range(stop_idx + 1, threaded_check_idx):
                    try_unreachable_body_indices.add(idx)
                for idx in range(threaded_check_idx + 1, end):
                    try_unreachable_body_indices.add(idx)
            else:
                if not stop_postdominates_start:
                    continue
                for idx in range(stop_idx + 1, end):
                    try_unreachable_body_indices.add(idx)
            # Only remove try markers for exceptional-only lanes when we can
            # prove no in-region CHECK_EXCEPTION dispatch depends on marker
            # structure before the guaranteed trap point.
            has_pretrap_check_exception = any(
                ops[idx].kind == "CHECK_EXCEPTION"
                for idx in range(start + 1, stop_idx + 1)
            )
            if not has_pretrap_check_exception and (
                stop_postdominates_start or threaded_check_idx is not None
            ):
                try_remove_starts.add(start)
                try_remove_ends.add(end)

        loop_remove_markers: set[int] = set()
        for loop_start, loop_end in control.loop_start_to_end.items():
            end_block = cfg.index_to_block.get(loop_end)
            start_block = cfg.index_to_block.get(loop_start)
            if end_block is None or start_block is None:
                continue
            if (end_block, start_block) in executable_edges:
                continue
            # Keep loop markers whenever dynamic loop-control ops are present
            # anywhere in the loop body. Restricting this to only currently
            # executable blocks can invalidate structure after later rewrites.
            body_has_dynamic_loop_control = any(
                ops[idx].kind
                in {
                    "LOOP_BREAK",
                    "LOOP_BREAK_IF_TRUE",
                    "LOOP_BREAK_IF_FALSE",
                    "LOOP_BREAK_IF_EXCEPTION",
                    "LOOP_CONTINUE",
                }
                for idx in range(loop_start + 1, loop_end)
            )
            if body_has_dynamic_loop_control:
                continue
            loop_remove_markers.add(loop_start)
            loop_remove_markers.add(loop_end)

        out: list[MoltOp] = []
        loop_rewrites = 0
        try_marker_prunes = 0
        loop_marker_prunes = 0
        try_body_prunes = 0
        check_exception_threads = 0
        check_exception_elisions_count = 0
        block_jump_label_arg: dict[int, Any] = {}
        for block_id, label in cfg.block_entry_label.items():
            label_key = self._control_label_key(label)
            if label_key is None:
                block_jump_label_arg[block_id] = label
                continue
            resolved_label = resolve_label_alias(label_key)
            block_jump_label_arg[block_id] = self._coerce_control_label_like(
                label, resolved_label
            )

        for idx, op in enumerate(ops):
            if op.kind == "CHECK_EXCEPTION":
                target = threaded_check_exception_jumps.get(idx)
                if target is not None:
                    out.append(
                        MoltOp(
                            kind="JUMP",
                            args=[target],
                            result=MoltValue("none"),
                            metadata=op.metadata,
                        )
                    )
                    check_exception_threads += 1
                    continue
                if idx in check_exception_elisions:
                    check_exception_elisions_count += 1
                    continue
                if op.args:
                    original_key = self._control_label_key(op.args[0])
                    if original_key is not None:
                        resolved_key = resolve_label_alias(original_key)
                        if resolved_key != original_key:
                            out.append(
                                MoltOp(
                                    kind=op.kind,
                                    args=[
                                        self._coerce_control_label_like(
                                            op.args[0], resolved_key
                                        ),
                                        *op.args[1:],
                                    ],
                                    result=op.result,
                                    metadata=op.metadata,
                                )
                            )
                            check_exception_threads += 1
                            continue
            if idx in try_unreachable_body_indices:
                try_body_prunes += 1
                continue
            if idx in loop_remove_markers and op.kind in {"LOOP_START", "LOOP_END"}:
                loop_marker_prunes += 1
                continue
            if op.kind == "LOOP_END":
                block_id = cfg.index_to_block.get(idx)
                if block_id is not None:
                    chosen = single_exec_succ_by_block.get(block_id)
                    succs = cfg.successors.get(block_id, [])
                    loop_start_idx = control.loop_end_to_start.get(idx)
                    back_succ = (
                        None
                        if loop_start_idx is None
                        else cfg.index_to_block.get(loop_start_idx)
                    )
                    if chosen is not None and len(succs) >= 2 and back_succ is not None:
                        exit_succ = next(
                            (succ for succ in succs if succ != back_succ), None
                        )
                        if chosen == back_succ and exit_succ is not None:
                            loop_rewrites += 1
                            back_label = block_jump_label_arg.get(back_succ)
                            if back_label is not None:
                                out.append(
                                    MoltOp(
                                        kind="JUMP",
                                        args=[back_label],
                                        result=MoltValue("none"),
                                        metadata=op.metadata,
                                    )
                                )
                                continue
                            out.append(
                                MoltOp(
                                    kind="LOOP_CONTINUE",
                                    args=[],
                                    result=MoltValue("none"),
                                    metadata=op.metadata,
                                )
                            )
                            continue
                        if chosen == exit_succ:
                            loop_rewrites += 1
                            exit_label = (
                                None
                                if exit_succ is None
                                else block_jump_label_arg.get(exit_succ)
                            )
                            if exit_label is not None:
                                out.append(
                                    MoltOp(
                                        kind="JUMP",
                                        args=[exit_label],
                                        result=MoltValue("none"),
                                        metadata=op.metadata,
                                    )
                                )
                                continue
                            out.append(
                                MoltOp(
                                    kind="LOOP_BREAK",
                                    args=[],
                                    result=MoltValue("none"),
                                    metadata=op.metadata,
                                )
                            )
                            continue
            if op.kind in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE"}:
                break_taken = loop_break_choice_by_index.get(idx)
                if break_taken is None:
                    block_id = cfg.index_to_block.get(idx)
                    if block_id is not None:
                        chosen = single_exec_succ_by_block.get(block_id)
                        succs = cfg.successors.get(block_id, [])
                        if chosen is not None and len(succs) >= 2:
                            break_taken = chosen == succs[1]
                if break_taken is True:
                    loop_rewrites += 1
                    block_id = cfg.index_to_block.get(idx)
                    succs = [] if block_id is None else cfg.successors.get(block_id, [])
                    break_succ = succs[1] if len(succs) >= 2 else None
                    break_label = (
                        None
                        if break_succ is None
                        else block_jump_label_arg.get(break_succ)
                    )
                    if break_label is not None:
                        out.append(
                            MoltOp(
                                kind="JUMP",
                                args=[break_label],
                                result=MoltValue("none"),
                                metadata=op.metadata,
                            )
                        )
                        continue
                    out.append(
                        MoltOp(
                            kind="LOOP_BREAK",
                            args=[],
                            result=MoltValue("none"),
                            metadata=op.metadata,
                        )
                    )
                    continue
                if break_taken is False:
                    loop_rewrites += 1
                    continue
            if idx in try_remove_starts and op.kind == "TRY_START":
                try_marker_prunes += 1
                continue
            if idx in try_remove_ends and op.kind == "TRY_END":
                try_marker_prunes += 1
                continue
            out.append(op)

        return (
            out,
            loop_rewrites,
            try_marker_prunes,
            loop_marker_prunes,
            try_body_prunes,
            check_exception_threads,
            check_exception_elisions_count,
        )

    def _range_overlaps_executable_blocks(
        self,
        cfg: CFGGraph,
        *,
        start: int,
        end_inclusive: int,
        executable_blocks: set[int],
    ) -> bool:
        for block in cfg.blocks:
            if block.id not in executable_blocks:
                continue
            if block.start <= end_inclusive and block.end > start:
                return True
        return False

    def _prune_unreachable_cfg_regions(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
        executable_blocks: set[int],
    ) -> tuple[list[MoltOp], int, int]:
        if not cfg.blocks:
            return ops, 0, 0

        keep = [True] * len(ops)
        region_ranges: list[tuple[int, int]] = []

        control = cfg.control
        region_maps = [
            control.if_to_end,
            control.loop_start_to_end,
            control.try_start_to_end,
        ]
        for mapping in region_maps:
            for start, end in mapping.items():
                if start < 0 or end < start or end >= len(ops):
                    continue
                if not self._range_overlaps_executable_blocks(
                    cfg,
                    start=start,
                    end_inclusive=end,
                    executable_blocks=executable_blocks,
                ):
                    region_ranges.append((start, end))

        region_ranges.sort()
        merged_ranges: list[tuple[int, int]] = []
        for start, end in region_ranges:
            if not merged_ranges:
                merged_ranges.append((start, end))
                continue
            prev_start, prev_end = merged_ranges[-1]
            if start <= prev_end + 1:
                merged_ranges[-1] = (prev_start, max(prev_end, end))
            else:
                merged_ranges.append((start, end))

        for start, end in merged_ranges:
            for idx in range(start, end + 1):
                keep[idx] = False

        structural_keep = {
            "IF",
            "ELSE",
            "END_IF",
            "LOOP_START",
            "LOOP_END",
            "TRY_START",
            "TRY_END",
            "LABEL",
            "STATE_LABEL",
        }
        removed_blocks = 0
        for block in cfg.blocks:
            if block.id in executable_blocks:
                continue
            removed_any = False
            for idx in range(block.start, block.end):
                if not keep[idx]:
                    removed_any = True
                    continue
                op = ops[idx]
                if op.kind in structural_keep:
                    continue
                keep[idx] = False
                removed_any = True
            if removed_any:
                removed_blocks += 1

        out = [op for idx, op in enumerate(ops) if keep[idx]]
        if out == ops:
            return ops, 0, 0
        return out, len(merged_ranges), removed_blocks

    def _control_label_key(self, value: Any) -> str | None:
        if isinstance(value, bool):
            return None
        if isinstance(value, int):
            return str(value)
        if isinstance(value, str):
            text = value.strip()
            if not text:
                return None
            return text
        return None

    def _coerce_control_label_like(self, exemplar: Any, key: str) -> Any:
        if isinstance(exemplar, bool):
            return exemplar
        if isinstance(exemplar, int):
            if key.startswith(("+", "-")):
                sign = key[0]
                digits = key[1:]
                if digits.isdigit():
                    return int(f"{sign}{digits}")
            elif key.isdigit():
                return int(key)
            return exemplar
        if isinstance(exemplar, str):
            return key
        return key

    def _ensure_structural_cfg_validity(
        self, ops: list[MoltOp], *, stage: str
    ) -> tuple[list[MoltOp], int]:
        if not ops:
            return ops, 0

        close_for_open = {
            "IF": "END_IF",
            "LOOP_START": "LOOP_END",
            "TRY_START": "TRY_END",
        }
        open_for_close = {close: open_ for open_, close in close_for_open.items()}
        # Stack entries are (kind, aux). For "IF", `aux` is the bool `seen_else`.
        # For "TRY_START", `aux` carries the region id (handler label) so that the
        # DIVERGENT `TRY_END`s a `with`/`try` legitimately emits — one on the
        # protected-body exit path and one on the exception-handler path, sharing a
        # `try_region_id` — pair to the SAME open frame instead of being treated as
        # a single bracket. For "LOOP_START", `aux` is unused (None).
        control_stack: list[tuple[str, Any]] = []
        rewritten: list[MoltOp] = []
        rewrites = 0

        def try_region_id(op: MoltOp) -> Any:
            # The region id is the try's handler label. `visit_Try`/finally carry
            # it in `args[0]`; `with`/`async with` carry it in
            # `metadata["try_region_id"]` (their TRY_START/TRY_END have empty args).
            if op.metadata is not None and "try_region_id" in op.metadata:
                return op.metadata["try_region_id"]
            if op.args:
                return op.args[0]
            return None

        def fail(message: str) -> NoReturn:
            self.midend_stats["cfg_structural_failures"] += 1
            raise RuntimeError(
                f"Malformed control flow after {stage} in "
                f"{self._active_midend_function_name}: {message}"
            )

        def append_synthetic_close(open_kind: str) -> None:
            nonlocal rewrites
            close_kind = close_for_open[open_kind]
            rewritten.append(
                MoltOp(
                    kind=close_kind,
                    args=[],
                    result=MoltValue("none"),
                    metadata={
                        "synthetic": "cfg_structural_canonicalizer",
                        "stage": stage,
                    },
                )
            )
            rewrites += 1

        for idx, op in enumerate(ops):
            kind = op.kind
            if kind in {"IF", "LOOP_START", "TRY_START"}:
                if kind == "IF":
                    aux: Any = False  # seen_else
                elif kind == "TRY_START":
                    aux = try_region_id(op)  # handler-label region id
                else:
                    aux = None
                control_stack.append((kind, aux))
                rewritten.append(op)
                continue

            if kind == "TRY_END":
                # `TRY_END` is a DIVERGENT-PATH close, not a strict bracket: a
                # `with`/`try` emits ONE `TRY_START` but a `TRY_END` on the normal
                # protected-body exit AND on the exception-handler entry (after
                # `LABEL try_exc`). When the body cannot fall through (returns /
                # raises) only the handler-path `TRY_END` is emitted, so a region
                # has ONE or TWO textual closes. Pairing by region id makes this
                # exact: the FIRST `TRY_END` for a region closes its frame; any
                # LATER `TRY_END` with the same id is a redundant divergent close
                # and is elided WITHOUT disturbing other open frames.
                #
                # This is what fixes the P45 `for`-in-`with` miscompile: the inner
                # `with`'s second (handler) `TRY_END` arrives while the enclosing
                # `LOOP_START` is still open. The generic close logic below would
                # synth-close that `LOOP_START` to reach the outer `TRY_START`,
                # then elide the loop's real `LOOP_CONTINUE`/`LOOP_END` — orphaning
                # the back-edge so the loop runs once. Region-id pairing leaves the
                # loop untouched.
                region_id = try_region_id(op)
                frame_idx = None
                for i in range(len(control_stack) - 1, -1, -1):
                    open_kind, open_aux = control_stack[i]
                    if open_kind == "TRY_START" and (
                        region_id is None or open_aux == region_id
                    ):
                        frame_idx = i
                        break
                if frame_idx is None:
                    # No open try frame for this region: a redundant divergent
                    # close (its frame was already closed on the body path) or a
                    # stray close. Elide it; never tear down other open frames.
                    rewrites += 1
                    continue
                # Close this try frame. Any frames ABOVE it are genuinely dangling
                # (their own close never appeared inside the try body) — repair
                # them with synthetic closes, mirroring the END_IF/LOOP_END path.
                while len(control_stack) - 1 > frame_idx:
                    dangling_kind, _ = control_stack.pop()
                    append_synthetic_close(dangling_kind)
                control_stack.pop()
                rewritten.append(op)
                continue

            if kind == "ELSE":
                if_indices = [
                    i
                    for i, (open_kind, _seen_else) in enumerate(control_stack)
                    if open_kind == "IF"
                ]
                if not if_indices:
                    rewrites += 1
                    continue
                while control_stack and control_stack[-1][0] != "IF":
                    dangling_kind, _ = control_stack.pop()
                    append_synthetic_close(dangling_kind)
                if not control_stack:
                    rewrites += 1
                    continue
                open_kind, seen_else = control_stack[-1]
                if open_kind != "IF":
                    rewrites += 1
                    continue
                if seen_else:
                    rewrites += 1
                    continue
                control_stack[-1] = ("IF", True)
                rewritten.append(op)
                continue

            if kind in open_for_close:
                required_open = open_for_close[kind]
                open_indices = [
                    i
                    for i, (open_kind, _seen_else) in enumerate(control_stack)
                    if open_kind == required_open
                ]
                if not open_indices:
                    rewrites += 1
                    continue
                while control_stack and control_stack[-1][0] != required_open:
                    dangling_kind, _ = control_stack.pop()
                    append_synthetic_close(dangling_kind)
                if control_stack:
                    control_stack.pop()
                rewritten.append(op)
                continue

            if kind in {
                "LOOP_BREAK",
                "LOOP_BREAK_IF_TRUE",
                "LOOP_BREAK_IF_FALSE",
                "LOOP_BREAK_IF_EXCEPTION",
                "LOOP_CONTINUE",
            }:
                if not any(open_kind == "LOOP_START" for open_kind, _ in control_stack):
                    # Structural repairs should be fail-closed for malformed
                    # labels/targets, but loop-control ops outside loop scope
                    # can be safely elided as no-ops to keep IR canonical.
                    rewrites += 1
                    continue
                rewritten.append(op)
                continue

            rewritten.append(op)

        while control_stack:
            dangling_kind, _ = control_stack.pop()
            append_synthetic_close(dangling_kind)

        labels: dict[str, int] = {}
        for idx, op in enumerate(rewritten):
            if op.kind not in {"LABEL", "STATE_LABEL"}:
                continue
            if not op.args:
                fail(f"{op.kind} at op index {idx} is missing label argument")
            label_key = self._control_label_key(op.args[0])
            if label_key is None:
                fail(f"{op.kind} at op index {idx} has invalid label {op.args[0]!r}")
            assert label_key is not None
            if label_key in labels:
                prior = labels[label_key]
                fail(
                    f"duplicate label {label_key!r} at op index {idx}; "
                    f"already defined at {prior}"
                )
            labels[label_key] = idx

        for idx, op in enumerate(rewritten):
            if op.kind not in {"JUMP", "CHECK_EXCEPTION"}:
                continue
            if not op.args:
                fail(f"{op.kind} at op index {idx} is missing target label")
            label_key = self._control_label_key(op.args[0])
            if label_key is None:
                fail(f"{op.kind} at op index {idx} has invalid target {op.args[0]!r}")
            assert label_key is not None
            if label_key not in labels:
                fail(f"{op.kind} at op index {idx} targets unknown label {label_key!r}")

        return rewritten, rewrites

    def _normalize_try_except_join_labels(
        self,
        ops: list[MoltOp],
        *,
        cfg: CFGGraph,
    ) -> tuple[list[MoltOp], int]:
        if not ops or not cfg.blocks:
            return ops, 0

        def collect_alias_labels(
            local_ops: list[MoltOp], local_cfg: CFGGraph
        ) -> dict[str, str]:
            alias_label: dict[str, str] = {}

            def extract_alias_target(body_ops: list[MoltOp]) -> str | None:
                if (
                    len(body_ops) == 1
                    and body_ops[0].kind == "JUMP"
                    and body_ops[0].args
                ):
                    return self._control_label_key(body_ops[0].args[0])
                if (
                    len(body_ops) == 2
                    and body_ops[0].kind == "CHECK_EXCEPTION"
                    and body_ops[0].args
                    and body_ops[1].kind == "JUMP"
                    and body_ops[1].args
                ):
                    exc_target = self._control_label_key(body_ops[0].args[0])
                    normal_target = self._control_label_key(body_ops[1].args[0])
                    if exc_target is not None and exc_target == normal_target:
                        return exc_target
                return None

            for block in local_cfg.blocks:
                if block.start >= block.end:
                    continue
                head = local_ops[block.start]
                if head.kind not in {"LABEL", "STATE_LABEL"} or not head.args:
                    continue
                label_key = self._control_label_key(head.args[0])
                if label_key is None:
                    continue

                body_ops = [
                    local_ops[idx]
                    for idx in range(block.start + 1, block.end)
                    if local_ops[idx].kind != "LINE"
                ]
                target_key = extract_alias_target(body_ops)
                if target_key is None and not body_ops:
                    succs = local_cfg.successors.get(block.id, [])
                    if len(succs) == 1:
                        succ_block = local_cfg.blocks[succs[0]]
                        succ_body = [
                            local_ops[idx]
                            for idx in range(succ_block.start, succ_block.end)
                            if local_ops[idx].kind != "LINE"
                        ]
                        target_key = extract_alias_target(succ_body)
                if target_key is None or target_key == label_key:
                    continue
                if local_cfg.label_to_block.get(target_key) is None:
                    continue
                alias_label[label_key] = target_key
            return alias_label

        total_rewrites = 0
        current = ops
        for _ in range(6):
            local_cfg = build_cfg(current)
            if not local_cfg.blocks:
                break
            alias_label = collect_alias_labels(current, local_cfg)

            def resolve_alias(label: str) -> str:
                resolved = label
                seen: set[str] = set()
                while resolved in alias_label and resolved not in seen:
                    seen.add(resolved)
                    resolved = alias_label[resolved]
                return resolved

            round_rewrites = 0
            skip_indices: set[int] = set()
            out: list[MoltOp] = []
            i = 0
            while i < len(current):
                if i in skip_indices:
                    i += 1
                    continue
                op = current[i]
                rewritten = op
                if op.kind in {"JUMP", "CHECK_EXCEPTION"} and op.args:
                    first = op.args[0]
                    label_key = self._control_label_key(first)
                    if label_key is not None:
                        resolved = resolve_alias(label_key)
                        if resolved != label_key:
                            new_first = self._coerce_control_label_like(first, resolved)
                            rewritten = MoltOp(
                                kind=op.kind,
                                args=[new_first, *op.args[1:]],
                                result=op.result,
                                metadata=op.metadata,
                            )
                            round_rewrites += 1

                if rewritten.kind == "CHECK_EXCEPTION" and rewritten.args:
                    check_target_key = self._control_label_key(rewritten.args[0])
                    if check_target_key is not None:
                        j = i + 1
                        while j < len(current) and current[j].kind == "LINE":
                            j += 1
                        if (
                            j < len(current)
                            and current[j].kind == "JUMP"
                            and current[j].args
                        ):
                            jump_target_key = self._control_label_key(
                                current[j].args[0]
                            )
                            if jump_target_key is not None:
                                resolved_check = resolve_alias(check_target_key)
                                resolved_jump = resolve_alias(jump_target_key)
                                if resolved_check == resolved_jump:
                                    out.append(
                                        MoltOp(
                                            kind="JUMP",
                                            args=[
                                                self._coerce_control_label_like(
                                                    rewritten.args[0], resolved_check
                                                )
                                            ],
                                            result=MoltValue("none"),
                                            metadata=rewritten.metadata,
                                        )
                                    )
                                    skip_indices.add(j)
                                    round_rewrites += 1
                                    i += 1
                                    continue

                out.append(rewritten)
                i += 1

            total_rewrites += round_rewrites
            if out == current:
                break
            current = out

        return current, total_rewrites

    def _prune_dead_labels_and_noop_jumps(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int, int]:
        if not ops:
            return ops, 0, 0

        current = ops
        total_label_prunes = 0
        total_jump_elisions = 0

        for _ in range(6):
            jump_elisions = 0
            no_noop_jumps: list[MoltOp] = []
            i = 0
            while i < len(current):
                op = current[i]
                if op.kind == "JUMP" and op.args:
                    target = str(op.args[0])
                    j = i + 1
                    while j < len(current) and current[j].kind == "LINE":
                        j += 1
                    if (
                        j < len(current)
                        and current[j].kind == "LABEL"
                        and current[j].args
                        and str(current[j].args[0]) == target
                    ):
                        jump_elisions += 1
                        i += 1
                        continue
                no_noop_jumps.append(op)
                i += 1

            referenced_labels: set[str] = set()
            for op in no_noop_jumps:
                if op.kind == "JUMP" and op.args:
                    referenced_labels.add(str(op.args[0]))
                elif op.kind == "CHECK_EXCEPTION" and op.args:
                    referenced_labels.add(str(op.args[0]))

            label_prunes = 0
            cleaned: list[MoltOp] = []
            for idx, op in enumerate(no_noop_jumps):
                if op.kind == "LABEL" and op.args:
                    name = str(op.args[0])
                    if name not in referenced_labels:
                        label_prunes += 1
                        continue
                cleaned.append(op)

            total_label_prunes += label_prunes
            total_jump_elisions += jump_elisions
            if cleaned == current:
                break
            current = cleaned

        return current, total_label_prunes, total_jump_elisions

    def _hoist_loop_invariant_pure_ops(
        self, ops: list[MoltOp]
    ) -> tuple[list[MoltOp], int]:
        cfg = build_cfg(ops)
        if not cfg.blocks:
            return ops, 0

        control = cfg.control
        target_start_by_index: dict[int, int] = {}
        loop_ranges = sorted(
            (
                (start, end)
                for start, end in control.loop_start_to_end.items()
                if end > start
            ),
            key=lambda item: (item[1] - item[0], item[0]),
        )

        for loop_start, loop_end in loop_ranges:
            if loop_end is None or loop_end <= loop_start:
                continue
            # In generators, loops containing state_yield create resume points
            # inside the loop body.  Hoisting definitions before the loop would
            # leave them undefined when the generator is resumed at that point.
            has_yield = any(
                ops[i].kind == "STATE_YIELD" for i in range(loop_start + 1, loop_end)
            )
            if has_yield:
                continue
            pre_defs = self._collect_defined_value_names(ops[:loop_start])
            hoisted_defs: set[str] = set()
            for idx in range(loop_start + 1, loop_end):
                op = ops[idx]
                if op.result.name == "none":
                    continue
                if op.kind == "PHI":
                    continue
                if self._op_effect_class(op.kind) != "pure":
                    continue
                uses: set[str] = set()
                for arg in op.args:
                    self._collect_arg_value_names(arg, uses)
                if uses.issubset(pre_defs.union(hoisted_defs)):
                    target_start_by_index.setdefault(idx, loop_start)
                    hoisted_defs.add(op.result.name)

        if not target_start_by_index:
            return ops, 0

        out: list[MoltOp] = []
        hoisted_count = 0
        for idx, op in enumerate(ops):
            if op.kind == "LOOP_START":
                hoisted_here = [
                    ops[candidate_idx]
                    for candidate_idx, target_start in sorted(
                        target_start_by_index.items()
                    )
                    if target_start == idx
                ]
                out.extend(hoisted_here)
                hoisted_count += len(hoisted_here)
            if idx in target_start_by_index:
                continue
            out.append(op)

        return out, hoisted_count

    def _run_cse_canonicalization_round(
        self,
        ops: list[MoltOp],
        *,
        allow_cross_block_const_dedupe: bool,
        max_cse_iterations_override: int | None = None,
        sccp_iter_cap_override: int | None = None,
    ) -> tuple[list[MoltOp], int]:
        round_cfg = build_cfg(ops)
        if not round_cfg.blocks:
            return ops, 0
        sccp = self._compute_sccp(
            ops, round_cfg, max_iters_override=sccp_iter_cap_override
        )
        working_ops, phi_trims = self._trim_phi_args_by_executable_edges(
            ops, round_cfg, sccp.executable_edges
        )
        if phi_trims > 0:
            round_cfg = build_cfg(working_ops)
            if not round_cfg.blocks:
                return working_ops, phi_trims
            sccp = self._compute_sccp(
                working_ops,
                round_cfg,
                max_iters_override=sccp_iter_cap_override,
            )
        sccp_in_consts = self._sccp_in_const_int_values(sccp)
        induction_steps = self._analyze_loop_induction_steps(working_ops, round_cfg)

        # Build value-name -> defining-block-id map so we can filter cross-block
        # aliases to only those whose targets are defined in dominating blocks.
        _value_def_block: dict[str, int] = {}
        for _blk in round_cfg.blocks:
            for _op_idx in range(_blk.start, _blk.end):
                _def_name = working_ops[_op_idx].result.name
                if _def_name != "none" and _def_name not in _value_def_block:
                    _value_def_block[_def_name] = _blk.id

        block_inputs: dict[int, CanonicalizationState] = {
            block.id: self._empty_canonicalization_state() for block in round_cfg.blocks
        }
        block_outputs: dict[int, CanonicalizationState] = {
            block.id: self._empty_canonicalization_state() for block in round_cfg.blocks
        }
        block_canonical_ops: dict[int, list[MoltOp]] = {
            block.id: [] for block in round_cfg.blocks
        }

        changed = True
        iterations = 0
        if max_cse_iterations_override is not None and max_cse_iterations_override > 0:
            max_cse_iterations = max_cse_iterations_override
        else:
            max_cse_iterations = (
                self.midend_env.cse_iter_cap_override
                if self.midend_env.cse_iter_cap_override is not None
                else 20
            )
        while changed and iterations < max_cse_iterations:
            iterations += 1
            changed = False
            for block in round_cfg.blocks:
                block_id = block.id
                if block_id == 0 or block_id not in round_cfg.reachable:
                    in_state = self._empty_canonicalization_state()
                else:
                    pred_states = [
                        block_outputs[pred]
                        for pred in round_cfg.predecessors.get(block_id, [])
                        if pred in round_cfg.reachable
                    ]
                    in_state = self._intersect_canonicalization_states(pred_states)
                    if not allow_cross_block_const_dedupe:
                        # Keep cross-block propagation limited to must-facts only.
                        # Alias/value-reuse state remains block-local to avoid
                        # rewriting gaps at control joins.
                        in_state["aliases"] = {}
                        in_state["available_values"] = {}
                    else:
                        # Filter aliases and available_values to only those
                        # whose target values are defined in blocks that
                        # dominate the current block.  Without this guard,
                        # CSE can rewrite an operand (e.g. a STORE_INDEX
                        # value arg) to reference a variable from a non-
                        # dominating block, producing invalid IR — the
                        # "return-buffer" bug.
                        block_doms = round_cfg.dominators.get(block_id, {block_id})
                        filtered_aliases: dict[str, MoltValue] = {}
                        for _ak, _av in in_state["aliases"].items():
                            _target_block = _value_def_block.get(_av.name)
                            if _target_block is None or _target_block in block_doms:
                                filtered_aliases[_ak] = _av
                        in_state["aliases"] = filtered_aliases
                        filtered_avail: dict[tuple[Any, ...], MoltValue] = {}
                        for _vk, _vv in in_state["available_values"].items():
                            _target_block = _value_def_block.get(_vv.name)
                            if _target_block is None or _target_block in block_doms:
                                filtered_avail[_vk] = _vv
                        in_state["available_values"] = filtered_avail
                        self._invalidate_canonicalization_state_signature(in_state)

                for name, value in sccp_in_consts.get(block_id, {}).items():
                    in_state["const_int_values"][name] = value
                    in_state["value_type_tags"][name] = BUILTIN_TYPE_TAGS["int"]

                if self._canonicalization_state_signature(
                    in_state
                ) != self._canonicalization_state_signature(block_inputs[block_id]):
                    block_inputs[block_id] = self._clone_canonicalization_state(
                        in_state
                    )
                    changed = True

                canonical_ops, out_state = self._canonicalize_block_with_state(
                    working_ops[block.start : block.end],
                    in_state,
                    induction_steps=induction_steps,
                )
                if self._canonicalization_state_signature(
                    out_state
                ) != self._canonicalization_state_signature(block_outputs[block_id]):
                    block_outputs[block_id] = self._clone_canonicalization_state(
                        out_state
                    )
                    changed = True
                if canonical_ops != block_canonical_ops[block_id]:
                    block_canonical_ops[block_id] = canonical_ops
                    changed = True

        if changed:
            self.midend_stats["cse_iteration_cap_hits"] = (
                self.midend_stats.get("cse_iteration_cap_hits", 0) + 1
            )
            return working_ops, phi_trims

        canonicalized_ops: list[MoltOp] = []
        for block_id in range(len(round_cfg.blocks)):
            canonicalized_ops.extend(block_canonical_ops[block_id])

        # ── Global alias resolution ─────────────────────────────────────
        # CSE creates aliases when merging duplicate ops within a block.
        # When allow_cross_block_const_dedupe is False, these aliases are
        # NOT propagated to successor blocks — a variable eliminated in
        # block A can still be referenced in block B.  Collect the union
        # of all aliases from every block's output state and apply them
        # to the reassembled ops so that no dangling references remain.
        global_aliases: dict[str, MoltValue] = {}
        for block_id in range(len(round_cfg.blocks)):
            for alias_name, alias_target in block_outputs[block_id]["aliases"].items():
                if alias_name not in global_aliases:
                    global_aliases[alias_name] = alias_target
        if global_aliases:
            resolved_ops: list[MoltOp] = []
            for op in canonicalized_ops:
                new_args = [
                    self._rewrite_aliases_in_arg(arg, global_aliases) for arg in op.args
                ]
                if new_args != op.args:
                    resolved_ops.append(
                        MoltOp(
                            kind=op.kind,
                            args=new_args,
                            result=op.result,
                            metadata=op.metadata,
                        )
                    )
                else:
                    resolved_ops.append(op)
            canonicalized_ops = resolved_ops

        return canonicalized_ops, phi_trims

    def _canonicalize_control_aware_ops_impl(
        self,
        ops: list[MoltOp],
        *,
        allow_cross_block_const_dedupe: bool,
    ) -> list[MoltOp]:
        self._refresh_midend_env_config_if_needed()
        # Current contract: sparse SCCP covers arithmetic/boolean/comparison/type
        # families plus bounded loop facts used by today’s fixed-point passes;
        # heap/call-specialization widening and stronger cross-iteration solvers
        # remain roadmap work and are intentionally not inferred here.
        validated_ops, preflight_rewrites = self._ensure_structural_cfg_validity(
            ops, stage="midend_fixed_point_entry"
        )
        self.midend_stats["cfg_structural_canonicalizations"] += preflight_rewrites
        cfg: CFGGraph = build_cfg(validated_ops)
        if not cfg.blocks:
            return validated_ops

        func_stats = self._midend_function_stats()
        func_stats["sccp_attempted"] += 1
        func_stats["edge_thread_attempted"] += 1
        func_stats["gvn_attempted"] += 1
        func_stats["cse_attempted"] += 1
        func_stats["licm_attempted"] += 1
        func_stats["dce_attempted"] += 1

        policy = self._resolve_midend_function_policy(
            validated_ops,
            function_name=self._active_midend_function_name,
            block_count=len(cfg.blocks),
        )
        pass_start = time.perf_counter()
        rewritten_ops, pre_cfg_rewrites = self._canonicalize_cfg_before_optimization(
            validated_ops
        )
        self._record_midend_pass_sample(
            "cfg_precanonicalize",
            elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
            accepted=pre_cfg_rewrites > 0,
            degraded=False,
        )

        # `midend_start` measures wall-clock for TELEMETRY ONLY (logged in
        # degrade/pass events).  It MUST NOT feed any pass-selection decision —
        # the degrade ladder gates on the deterministic `work_units_spent`
        # accumulator below so the emitted IR is a pure function of the input
        # (#73; the old wall-clock gate made IR depend on machine speed).
        midend_start = time.perf_counter()
        # Deterministic degrade-ladder accumulator: charged the live op count at
        # each inter-pass checkpoint via `charge_work(...)`.  Compared against
        # the deterministic `policy.work_budget` to decide degradation.
        work_units_spent = 0.0
        degrade_events: list[dict[str, Any]] = []
        degraded = False
        enable_deep_edge_thread = policy.enable_deep_edge_thread
        enable_cse = policy.enable_cse
        enable_licm = policy.enable_licm
        enable_guard_hoist = policy.enable_guard_hoist
        max_rounds = max(2, policy.max_rounds)
        sccp_iter_cap = max(1, policy.sccp_iter_cap)
        cse_iter_cap = max(1, policy.cse_iter_cap)

        # --- Per-function DETERMINISTIC work budget, 3-level degrade ladder ---
        # (formerly a wall-time budget, MOL-27; made deterministic for #73).
        per_func_work_budget = max(0.0, float(policy.work_budget))
        degrade_level: int = 0
        degrade_level_reasons: list[str] = []

        if self.midend_env.max_rounds_override is not None:
            max_rounds = max(2, self.midend_env.max_rounds_override)
        if self.midend_env.sccp_iter_cap_override is not None:
            sccp_iter_cap = self.midend_env.sccp_iter_cap_override
        if self.midend_env.cse_iter_cap_override is not None:
            cse_iter_cap = self.midend_env.cse_iter_cap_override

        def spent_midend_ms() -> float:
            # Telemetry only — never feeds a pass-selection decision (#73).
            return (time.perf_counter() - midend_start) * 1000.0

        def charge_work(units: float) -> None:
            # Deterministic work accounting for the degrade ladder.  `units` is
            # an op-count-derived (hence input-determined) cost.
            nonlocal work_units_spent
            if units > 0.0:
                work_units_spent += float(units)

        def add_degrade_event(
            reason: str,
            stage: str,
            action: str,
            *,
            value: Any | None = None,
        ) -> None:
            event: dict[str, Any] = {
                "reason": reason,
                "stage": stage,
                "action": action,
                "spent_ms": round(max(0.0, spent_midend_ms()), 3),
            }
            if value is not None:
                event["value"] = value
            degrade_events.append(event)

        if not enable_deep_edge_thread:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_deep_edge_thread",
            )
        if not enable_cse:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_cse",
            )
        if not enable_guard_hoist:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_guard_hoist",
            )
        if not enable_licm:
            add_degrade_event(
                "policy_tier_limit",
                "policy_init",
                "disable_licm",
            )

        def maybe_apply_budget_degrade(
            stage: str,
            round_index: int,
            *,
            ops_now: int,
            upcoming_pass: str | None = None,
        ) -> None:
            """Deterministically degrade the optimisation pipeline when the
            accumulated DETERMINISTIC work exceeds the per-function work budget.

            `ops_now` is the live op count at this checkpoint; it is charged to
            the work accumulator before the budget is evaluated.  Charging the
            op count makes the total work scale with how much IR each pass had
            to process — a deterministic proxy for compile cost — so the
            resulting pass selection (and thus the emitted IR) depends only on
            the input, never on wall-clock timing (#73).
            """
            nonlocal degraded
            nonlocal degrade_level
            nonlocal enable_deep_edge_thread
            nonlocal enable_cse
            nonlocal enable_guard_hoist
            nonlocal enable_licm
            nonlocal max_rounds
            nonlocal sccp_iter_cap
            nonlocal cse_iter_cap
            if per_func_work_budget < 0:
                return
            charge_work(max(1, ops_now))
            while work_units_spent > per_func_work_budget:
                action: str | None = None
                proof_floor = round_index + 2
                if max_rounds > proof_floor:
                    max_rounds = proof_floor
                    action = f"cap_rounds_to_{max_rounds}"
                elif sccp_iter_cap > 8:
                    sccp_iter_cap = max(8, sccp_iter_cap // 2)
                    action = f"shrink_sccp_iter_cap_to_{sccp_iter_cap}"
                elif cse_iter_cap > 4:
                    cse_iter_cap = max(4, cse_iter_cap // 2)
                    action = f"shrink_cse_iter_cap_to_{cse_iter_cap}"
                elif enable_cse:
                    enable_cse = False
                    action = "disable_cse"
                elif enable_deep_edge_thread:
                    enable_deep_edge_thread = False
                    action = "disable_deep_edge_thread"
                elif enable_guard_hoist:
                    enable_guard_hoist = False
                    action = "disable_guard_hoist"
                elif enable_licm:
                    enable_licm = False
                    action = "disable_licm"
                if action is None:
                    break
                degraded = True
                degrade_level = min(3, degrade_level + 1)
                degrade_level_reasons.append(
                    f"work_budget_exceeded at {stage}: "
                    f"work={work_units_spent:.0f} > budget="
                    f"{per_func_work_budget:.0f}; action={action}"
                )
                extra_value: dict[str, Any] = {
                    "work_units": round(work_units_spent, 1),
                    "work_budget": round(per_func_work_budget, 1),
                }
                if upcoming_pass is not None:
                    extra_value["upcoming_pass"] = upcoming_pass
                add_degrade_event(
                    "work_budget_exceeded", stage, action, value=extra_value
                )

        total_branch_prunes = 0
        total_loop_edge_prunes = 0
        total_try_edge_prunes = 0
        total_loop_marker_prunes = 0
        total_unreachable_blocks = 0
        total_region_prunes = pre_cfg_rewrites
        total_label_prunes = 0
        total_jump_noops = 0
        total_try_join_threads = 0
        total_licm_hoists = 0
        total_phi_edge_trims = 0
        total_loop_rewrite_attempts = 0
        total_loop_rewrite_accepted = 0

        gvn_hits_before = self.midend_stats.get("gvn_hits", 0)
        dce_removed_before = self.midend_stats.get("dce_removed_total", 0)
        guard_hoist_attempts_before = self.midend_stats.get("guard_hoist_attempts", 0)
        guard_hoist_accepted_before = self.midend_stats.get("guard_hoist_accepted", 0)
        guard_hoist_rejected_before = self.midend_stats.get("guard_hoist_rejected", 0)

        converged = False
        round_index = 0
        round_snapshots: list[dict[str, Any]] = []
        cse_dce_closure_failed = False
        while round_index < max_rounds:
            round_index += 1
            maybe_apply_budget_degrade(
                f"round_{round_index}_start",
                round_index - 1,
                ops_now=len(rewritten_ops),
                upcoming_pass="simplify",
            )
            step_before = rewritten_ops
            step_ops = rewritten_ops
            post_cse_dce_ran = False

            # 1) simplify
            pass_start = time.perf_counter()
            step_ops, structural_prunes = (
                self._canonicalize_structured_regions_pre_sccp(step_ops)
            )
            self._record_midend_pass_sample(
                "simplify",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=structural_prunes > 0,
                degraded=degraded,
            )
            total_region_prunes += structural_prunes
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_simplify",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="sccp_edge_thread",
            )

            # 2) SCCP/edge-thread
            iter_cfg = build_cfg(step_ops)
            if iter_cfg.blocks:
                pass_start = time.perf_counter()
                iter_sccp = self._compute_sccp(
                    step_ops,
                    iter_cfg,
                    max_iters_override=sccp_iter_cap,
                )
                step_ops, phi_trims = self._trim_phi_args_by_executable_edges(
                    step_ops, iter_cfg, iter_sccp.executable_edges
                )
                total_phi_edge_trims += phi_trims
                if phi_trims > 0:
                    iter_cfg = build_cfg(step_ops)
                    if iter_cfg.blocks:
                        iter_sccp = self._compute_sccp(
                            step_ops,
                            iter_cfg,
                            max_iters_override=sccp_iter_cap,
                        )
                if iter_cfg.blocks:
                    step_ops, branch_prunes = self._rewrite_structured_if_regions(
                        step_ops,
                        control=iter_cfg.control,
                        branch_choice_by_if_index=iter_sccp.branch_choice_by_if_index,
                    )
                    total_branch_prunes += branch_prunes
                else:
                    branch_prunes = 0

                threaded_cfg = build_cfg(step_ops)
                loop_rewrite_attempts = sum(
                    1
                    for op in step_ops
                    if op.kind
                    in {"LOOP_BREAK_IF_TRUE", "LOOP_BREAK_IF_FALSE", "LOOP_END"}
                )
                total_loop_rewrite_attempts += loop_rewrite_attempts
                if threaded_cfg.blocks and enable_deep_edge_thread:
                    threaded_sccp = self._compute_sccp(
                        step_ops,
                        threaded_cfg,
                        max_iters_override=sccp_iter_cap,
                    )
                    (
                        step_ops,
                        loop_rewrites,
                        try_marker_prunes,
                        loop_marker_prunes,
                        try_body_prunes,
                        check_exception_threads,
                        check_exception_elisions,
                    ) = self._rewrite_loop_try_edge_threading(
                        step_ops,
                        cfg=threaded_cfg,
                        control=threaded_cfg.control,
                        executable_edges=threaded_sccp.executable_edges,
                        loop_break_choice_by_index=threaded_sccp.loop_break_choice_by_index,
                        try_exception_possible_by_start=threaded_sccp.try_exception_possible_by_start,
                        try_normal_possible_by_start=threaded_sccp.try_normal_possible_by_start,
                        guard_fail_indices=threaded_sccp.guard_fail_indices,
                    )
                else:
                    (
                        loop_rewrites,
                        try_marker_prunes,
                        loop_marker_prunes,
                        try_body_prunes,
                        check_exception_threads,
                        check_exception_elisions,
                    ) = (0, 0, 0, 0, 0, 0)

                total_loop_edge_prunes += loop_rewrites
                total_try_edge_prunes += (
                    try_marker_prunes
                    + try_body_prunes
                    + check_exception_threads
                    + check_exception_elisions
                )
                total_loop_marker_prunes += loop_marker_prunes
                total_loop_rewrite_accepted += (
                    loop_rewrites + loop_marker_prunes + try_body_prunes
                )
                self._record_midend_pass_sample(
                    "sccp_edge_thread",
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=(
                        branch_prunes
                        + loop_rewrites
                        + loop_marker_prunes
                        + try_marker_prunes
                        + try_body_prunes
                        + check_exception_threads
                        + check_exception_elisions
                        + phi_trims
                    )
                    > 0,
                    degraded=degraded
                    or (not enable_deep_edge_thread and loop_rewrite_attempts > 0),
                )
            else:
                self._record_midend_pass_sample(
                    "sccp_edge_thread",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=degraded,
                )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_sccp",
                round_index - 1,
                ops_now=len(step_ops),
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_pre_join",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="join_canonicalize",
            )

            # 3) join canonicalize
            pass_start = time.perf_counter()
            join_cfg = build_cfg(step_ops)
            if join_cfg.blocks:
                step_ops, try_join_threads = self._normalize_try_except_join_labels(
                    step_ops, cfg=join_cfg
                )
            else:
                try_join_threads = 0
            total_try_join_threads += try_join_threads
            total_try_edge_prunes += try_join_threads
            self._record_midend_pass_sample(
                "join_canonicalize",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=try_join_threads > 0,
                degraded=degraded,
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_join",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="guard_hoist",
            )

            pass_start = time.perf_counter()
            guard_prune_input = step_ops
            step_ops, fused_dict_guard_prunes = (
                self._eliminate_redundant_fused_dict_increment_guards(step_ops)
            )
            if fused_dict_guard_prunes:
                self.midend_stats["fused_dict_guard_prunes"] = (
                    self.midend_stats.get("fused_dict_guard_prunes", 0)
                    + fused_dict_guard_prunes
                )
                func_stats["fused_dict_guard_prunes"] += fused_dict_guard_prunes
            self._record_midend_pass_sample(
                "fused_dict_guard_prune",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=step_ops != guard_prune_input,
                degraded=degraded,
            )

            if enable_guard_hoist:
                pass_start = time.perf_counter()
                step_ops, guard_attempts, guard_accepts, guard_rejects = (
                    self._eliminate_redundant_guards_cfg(step_ops)
                )
                self.midend_stats["guard_hoist_attempts"] += guard_attempts
                self.midend_stats["guard_hoist_accepted"] += guard_accepts
                self.midend_stats["guard_hoist_rejected"] += guard_rejects
                self._record_midend_pass_sample(
                    "guard_hoist",
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=guard_accepts > 0,
                    degraded=degraded,
                )
            else:
                self._record_midend_pass_sample(
                    "guard_hoist",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_guard_hoist",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="licm",
            )

            # Auxiliary: LICM/loop hoists in same deterministic round.
            if enable_licm:
                pass_start = time.perf_counter()
                step_ops, licm_hoists = self._hoist_loop_invariant_pure_ops(step_ops)
                self._record_midend_pass_sample(
                    "licm",
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=licm_hoists > 0,
                    degraded=degraded,
                )
            else:
                licm_hoists = 0
                self._record_midend_pass_sample(
                    "licm",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
            total_licm_hoists += licm_hoists
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_hoists",
                round_index - 1,
                ops_now=len(step_ops),
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_pre_prune",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="prune",
            )

            # 4) prune
            pass_start = time.perf_counter()
            prune_cfg = build_cfg(step_ops)
            if prune_cfg.blocks:
                prune_sccp = self._compute_sccp(
                    step_ops,
                    prune_cfg,
                    max_iters_override=sccp_iter_cap,
                )
                step_ops, region_prunes, unreachable_blocks = (
                    self._prune_unreachable_cfg_regions(
                        step_ops,
                        cfg=prune_cfg,
                        executable_blocks=prune_sccp.executable_blocks,
                    )
                )
            else:
                region_prunes, unreachable_blocks = 0, 0
            total_region_prunes += region_prunes
            total_unreachable_blocks += unreachable_blocks

            step_ops, label_prunes, jump_noops = self._prune_dead_labels_and_noop_jumps(
                step_ops
            )
            total_label_prunes += label_prunes
            total_jump_noops += jump_noops
            step_ops, round_structural_rewrites = self._ensure_structural_cfg_validity(
                step_ops,
                stage=f"midend_fixed_point_round_{round_index}",
            )
            total_region_prunes += round_structural_rewrites
            self.midend_stats["cfg_structural_canonicalizations"] += (
                round_structural_rewrites
            )
            self._record_midend_pass_sample(
                "prune",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=(
                    region_prunes
                    + unreachable_blocks
                    + label_prunes
                    + jump_noops
                    + round_structural_rewrites
                )
                > 0,
                degraded=degraded,
            )
            maybe_apply_budget_degrade(
                f"round_{round_index}_post_prune",
                round_index - 1,
                ops_now=len(step_ops),
                upcoming_pass="verifier",
            )

            # 5) verifier
            pass_start = time.perf_counter()
            # Compute predefined from the ORIGINAL ops at round start, not the
            # current ops.  If LICM+CSE eliminated a variable's definition,
            # that variable is NOT predefined — it's a dangling reference that
            # must be caught.  Using step_ops here masks the bug because
            # _infer_predefined_value_names treats "used but not defined" as
            # predefined (assumed to be a function parameter).
            round_predefined = self._infer_predefined_value_names(step_before)
            round_failures = self._verify_definite_assignment_in_ops(
                step_ops, predefined_value_names=round_predefined
            )
            self._record_midend_pass_sample(
                "verifier",
                elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                accepted=not round_failures,
                degraded=degraded,
            )

            def run_verified_dce(
                dce_input: list[MoltOp],
                *,
                pass_name: str,
            ) -> tuple[list[MoltOp], bool]:
                pass_start = time.perf_counter()
                dce_candidate = self._eliminate_dead_trivial_consts(dce_input)
                dce_failures = self._verify_definite_assignment_in_ops(
                    dce_candidate, predefined_value_names=round_predefined
                )
                accepted = (not dce_failures) and dce_candidate != dce_input
                self._record_midend_pass_sample(
                    pass_name,
                    elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                    accepted=accepted,
                    degraded=degraded,
                )
                if dce_failures:
                    return dce_input, False
                return dce_candidate, accepted

            if round_failures:
                step_ops = step_before
                self._record_midend_pass_sample(
                    "dce",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
                self._record_midend_pass_sample(
                    "cse",
                    elapsed_ms=0.0,
                    accepted=False,
                    degraded=True,
                )
            else:
                # 6) DCE
                step_ops, _dce_accepted = run_verified_dce(step_ops, pass_name="dce")
                maybe_apply_budget_degrade(
                    f"round_{round_index}_post_dce",
                    round_index - 1,
                    ops_now=len(step_ops),
                    upcoming_pass="cse",
                )

                # 7) CSE
                if enable_cse:
                    cse_dce_closure_converged = False
                    cse_dce_fp_max_iters = max(1, self.midend_env.cse_fp_max_iters)
                    for cse_dce_fp_iter in range(1, cse_dce_fp_max_iters + 1):
                        pass_start = time.perf_counter()
                        cse_input = step_ops
                        cse_candidate, cse_phi_trims = (
                            self._run_cse_canonicalization_round(
                                step_ops,
                                allow_cross_block_const_dedupe=(
                                    allow_cross_block_const_dedupe
                                ),
                                max_cse_iterations_override=cse_iter_cap,
                                sccp_iter_cap_override=sccp_iter_cap,
                            )
                        )
                        total_phi_edge_trims += cse_phi_trims
                        cse_failures = self._verify_definite_assignment_in_ops(
                            cse_candidate, predefined_value_names=round_predefined
                        )
                        cse_accepted = (not cse_failures) and (
                            cse_candidate != cse_input or cse_phi_trims > 0
                        )
                        self._record_midend_pass_sample(
                            "cse",
                            elapsed_ms=(time.perf_counter() - pass_start) * 1000.0,
                            accepted=cse_accepted,
                            degraded=degraded,
                        )
                        if cse_failures:
                            cse_dce_closure_converged = True
                            break
                        step_ops = cse_candidate
                        step_ops, post_cse_dce_accepted = run_verified_dce(
                            step_ops, pass_name="post_cse_dce"
                        )
                        post_cse_dce_ran = True
                        if cse_dce_fp_iter > 1:
                            charge_work(max(1, len(step_ops)))
                        if not cse_accepted and not post_cse_dce_accepted:
                            cse_dce_closure_converged = True
                            break
                    if not cse_dce_closure_converged:
                        cse_dce_closure_failed = True
                        self.midend_stats["cse_dce_fp_cap_hits"] = (
                            self.midend_stats.get("cse_dce_fp_cap_hits", 0) + 1
                        )
                        add_degrade_event(
                            "cse_dce_fixed_point_cap",
                            "cse_dce_closure",
                            "fail_closed_non_convergence",
                            value=cse_dce_fp_max_iters,
                        )
                        degraded = True
                        self._record_midend_pass_sample(
                            "cse_dce_closure",
                            elapsed_ms=0.0,
                            accepted=False,
                            degraded=True,
                        )
                else:
                    self._record_midend_pass_sample(
                        "cse",
                        elapsed_ms=0.0,
                        accepted=False,
                        degraded=True,
                    )
                maybe_apply_budget_degrade(
                    f"round_{round_index}_post_cse",
                    round_index - 1,
                    ops_now=len(step_ops),
                )

            rewritten_ops = step_ops
            round_passes_run: list[str] = [
                "simplify",
                "sccp_edge_thread",
                "join_canonicalize",
            ]
            if enable_guard_hoist:
                round_passes_run.append("guard_hoist")
            if enable_licm:
                round_passes_run.append("licm")
            round_passes_run.append("prune")
            round_passes_run.append("verifier")
            if not round_failures:
                round_passes_run.append("dce")
                if enable_cse:
                    round_passes_run.append("cse")
                if post_cse_dce_ran:
                    round_passes_run.append("post_cse_dce")
            round_changed = rewritten_ops != step_before
            round_snapshots.append(
                {
                    "round": round_index,
                    "spent_ms": round(max(0.0, spent_midend_ms()), 3),
                    "passes_run": round_passes_run,
                    "changed": round_changed,
                }
            )
            if cse_dce_closure_failed:
                break
            if not round_changed:
                converged = True
                break

        if not converged:
            self.midend_stats["fixed_point_fail_fast"] += 1
            add_degrade_event(
                "fixed_point_round_cap",
                "fixed_point_exit",
                "fail_closed_non_convergence",
                value=max_rounds,
            )
            degraded = True
            self._record_midend_policy_outcome(
                policy=policy,
                spent_ms=spent_midend_ms(),
                work_units_spent=work_units_spent,
                degraded=degraded,
                degrade_events=degrade_events,
                round_snapshots=round_snapshots,
            )
            raise RuntimeError(
                "midend deterministic fixed-point failed to converge within "
                f"{max_rounds} rounds for {self._active_midend_function_name}"
            )

        if converged:
            probe_ops = rewritten_ops
            probe_ops, _probe_cfg_rewrites = self._canonicalize_cfg_before_optimization(
                probe_ops
            )
            probe_ops, _probe_region_prunes = (
                self._canonicalize_structured_regions_pre_sccp(probe_ops)
            )
            probe_ops, _probe_label_prunes, _probe_jump_noops = (
                self._prune_dead_labels_and_noop_jumps(probe_ops)
            )
            probe_ops, _probe_validity_rewrites = self._ensure_structural_cfg_validity(
                probe_ops, stage="midend_idempotence_probe"
            )
            if probe_ops != rewritten_ops:
                self.midend_stats["fixed_point_fail_fast"] += 1
                add_degrade_event(
                    "idempotence_probe_mismatch",
                    "idempotence_probe",
                    "fail_closed_idempotence_probe",
                )
                degraded = True
                self._record_midend_policy_outcome(
                    policy=policy,
                    spent_ms=spent_midend_ms(),
                    work_units_spent=work_units_spent,
                    degraded=degraded,
                    degrade_events=degrade_events,
                    round_snapshots=round_snapshots,
                )
                raise RuntimeError(
                    "midend idempotence check failed after convergence for "
                    f"{self._active_midend_function_name}"
                )

        final_guard_prune_input = rewritten_ops
        rewritten_ops, final_fused_dict_guard_prunes = (
            self._eliminate_redundant_fused_dict_increment_guards(rewritten_ops)
        )
        if final_fused_dict_guard_prunes:
            self.midend_stats["fused_dict_guard_prunes"] = (
                self.midend_stats.get("fused_dict_guard_prunes", 0)
                + final_fused_dict_guard_prunes
            )
            func_stats["fused_dict_guard_prunes"] += final_fused_dict_guard_prunes
            final_predefined = self._infer_predefined_value_names(
                final_guard_prune_input
            )
            final_failures = self._verify_definite_assignment_in_ops(
                rewritten_ops, predefined_value_names=final_predefined
            )
            if final_failures:
                rewritten_ops = final_guard_prune_input
                self.midend_stats["fused_dict_guard_prunes"] -= (
                    final_fused_dict_guard_prunes
                )
                func_stats["fused_dict_guard_prunes"] -= final_fused_dict_guard_prunes

        self.midend_stats["sccp_branch_prunes"] += total_branch_prunes
        self.midend_stats["loop_edge_thread_prunes"] += (
            total_loop_edge_prunes + total_loop_marker_prunes
        )
        self.midend_stats["try_edge_thread_prunes"] += total_try_edge_prunes
        self.midend_stats["licm_hoists"] += total_licm_hoists
        self.midend_stats["unreachable_blocks_removed"] += total_unreachable_blocks
        self.midend_stats["cfg_region_prunes"] += total_region_prunes
        self.midend_stats["label_prunes"] += total_label_prunes
        self.midend_stats["jump_noop_elisions"] += total_jump_noops
        self.midend_stats["phi_edge_trims"] += total_phi_edge_trims

        sccp_applied = (
            total_branch_prunes
            + total_loop_edge_prunes
            + total_try_edge_prunes
            + total_loop_marker_prunes
            + total_region_prunes
            + total_unreachable_blocks
            + total_label_prunes
            + total_jump_noops
            + total_try_join_threads
            + total_phi_edge_trims
            + total_licm_hoists
        )
        if sccp_applied > 0:
            func_stats["sccp_accepted"] += 1

        edge_thread_applied = (
            total_branch_prunes
            + total_loop_edge_prunes
            + total_try_edge_prunes
            + total_loop_marker_prunes
            + total_region_prunes
            + total_unreachable_blocks
            + total_label_prunes
            + total_jump_noops
            + total_try_join_threads
        )
        if edge_thread_applied > 0:
            func_stats["edge_thread_accepted"] += 1
        else:
            func_stats["edge_thread_rejected"] += 1

        func_stats["loop_rewrite_attempted"] += total_loop_rewrite_attempts
        func_stats["loop_rewrite_accepted"] += total_loop_rewrite_accepted
        func_stats["loop_rewrite_rejected"] += max(
            0, total_loop_rewrite_attempts - total_loop_rewrite_accepted
        )

        guard_hoist_attempt_delta = (
            self.midend_stats.get("guard_hoist_attempts", 0)
            - guard_hoist_attempts_before
        )
        guard_hoist_accept_delta = (
            self.midend_stats.get("guard_hoist_accepted", 0)
            - guard_hoist_accepted_before
        )
        guard_hoist_reject_delta = (
            self.midend_stats.get("guard_hoist_rejected", 0)
            - guard_hoist_rejected_before
        )
        func_stats["guard_hoist_attempted"] += max(0, guard_hoist_attempt_delta)
        func_stats["guard_hoist_accepted"] += max(0, guard_hoist_accept_delta)
        func_stats["guard_hoist_rejected"] += max(0, guard_hoist_reject_delta)

        if self.midend_stats.get("gvn_hits", 0) > gvn_hits_before:
            func_stats["gvn_accepted"] += 1
            func_stats["cse_accepted"] += 1
        if total_licm_hoists > 0:
            func_stats["licm_accepted"] += 1
        else:
            func_stats["licm_rejected"] += 1
        if self.midend_stats.get("dce_removed_total", 0) > dce_removed_before:
            func_stats["dce_accepted"] += 1

        self._record_midend_policy_outcome(
            policy=policy,
            spent_ms=spent_midend_ms(),
            work_units_spent=work_units_spent,
            degraded=degraded,
            degrade_events=degrade_events,
            round_snapshots=round_snapshots,
        )
        return rewritten_ops

    def _canonicalize_control_aware_ops(self, ops: list[MoltOp]) -> list[MoltOp]:
        predefined = self._infer_predefined_value_names(ops)
        self.midend_stats["expanded_attempts"] += 1

        expanded_ops = self._canonicalize_control_aware_ops_impl(
            ops, allow_cross_block_const_dedupe=True
        )
        expanded_failures = self._verify_definite_assignment_in_ops(
            expanded_ops, predefined_value_names=predefined
        )
        if not expanded_failures:
            self.midend_stats["expanded_accepted"] += 1
            return expanded_ops

        self.midend_stats["expanded_fallbacks"] += 1
        # Diagnostic: log which variables caused the verification failure so
        # that the cross-block CSE issue can be traced.  Each failure is a
        # tuple (op_index, op_kind, value_name).
        if os.getenv("MOLT_MIDEND_STATS"):
            failed_vars = sorted({name for _, _, name in expanded_failures})
            failed_ops = sorted({kind for _, kind, _ in expanded_failures})
            print(
                f"molt midend cross-block CSE fallback:"
                f" func={self._active_midend_function_name!r}"
                f" failed_vars={failed_vars}"
                f" failed_ops={failed_ops}"
                f" failure_count={len(expanded_failures)}",
                file=sys.stderr,
            )
        safe_ops = self._canonicalize_control_aware_ops_impl(
            ops, allow_cross_block_const_dedupe=False
        )
        return safe_ops

    def _coalesce_check_exception_ops(self, ops: list[MoltOp]) -> list[MoltOp]:
        # Keep coalescing conservative: moving checks across value-producing ops can
        # expose uninitialized/missing operands when an exception is already pending.
        # `LINE` is metadata-only and safe to commute with `CHECK_EXCEPTION`.
        safe_after_check = {"LINE"}
        out: list[MoltOp] = []
        pending_check: MoltOp | None = None
        for op in ops:
            if op.kind == "CHECK_EXCEPTION":
                pending_check = op
                continue
            if pending_check is not None and op.kind not in safe_after_check:
                out.append(pending_check)
                pending_check = None
            out.append(op)
        if pending_check is not None:
            out.append(pending_check)
        return out

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
