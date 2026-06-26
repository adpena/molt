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
# computing the SemaResult — immutable semantic facts (static class graph,
# local class-member facts, const environment, top-level function metadata) —
# once, pre-walk, in the cfg_analysis.py house shape. F2b is additive:
# visit_Module populates the existing pre-walk state dicts FROM SemaResult (the
# _populate_sema_state shim) so the walk is byte-identical; F2c rewires the
# read-sites onto SemaResult and deletes the mutable god-object dicts.
from molt.frontend.sema import (
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
from molt.frontend.lowering.generator_state import GeneratorStateMixin
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
    GeneratorStateMixin,
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
    pass


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
