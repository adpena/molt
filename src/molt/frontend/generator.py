"""The assembled frontend generator: ``SimpleTIRGenerator`` and ``compile_to_tir``.

The generator is composed from visitor/lowering mixins (F1 decomposition).
Per-family method bodies live under ``molt.frontend.visitors`` and
``molt.frontend.lowering`` and are mixed into the class below via MRO.
Importing this module loads the whole generator; ``molt.frontend`` imports
it only when a caller first asks for one of its names.
"""

from __future__ import annotations

import ast
from typing import Any, Literal

from molt.frontend.lowering.analysis_collect_static import AnalysisCollectStaticMixin
from molt.frontend.lowering.analysis_patterns import AnalysisPatternMixin
from molt.frontend.lowering.app_bindings import AppBindingAuthorityMixin
from molt.frontend.lowering.attribute_access import AttributeAccessMixin
from molt.frontend.lowering.class_resolution import ClassResolutionMixin
from molt.frontend.lowering.compile_warnings import CompileWarningMixin
from molt.frontend.lowering.condition_flow import ConditionFlowMixin
from molt.frontend.lowering.emission_core import EmissionCoreMixin
from molt.frontend.lowering.exception_lowering import ExceptionLoweringMixin
from molt.frontend.lowering.expression_primitives import ExpressionPrimitivesMixin
from molt.frontend.lowering.function_lifecycle import FunctionLifecycleMixin
from molt.frontend.lowering.function_metadata import FunctionMetadataMixin
from molt.frontend.lowering.generator_state import GeneratorStateMixin
from molt.frontend.lowering.import_lowering import ImportLoweringMixin
from molt.frontend.lowering.local_bindings import LocalBindingMixin
from molt.frontend.lowering.loop_lowering import LoopLoweringMixin
from molt.frontend.lowering.builtin_guards import BuiltinGuardMixin
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


class SimpleTIRGenerator(
    GeneratorStateMixin,
    AppBindingAuthorityMixin,
    LocalBindingMixin,
    SerializationMixin,
    OwnershipLoweringMixin,
    StringFormattingMixin,
    RuntimeReferenceMixin,
    CompileWarningMixin,
    EmissionCoreMixin,
    ExpressionPrimitivesMixin,
    ConditionFlowMixin,
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
    BuiltinGuardMixin,
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
    type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
) -> dict[str, Any]:
    from molt.host_python import require_frontend_python

    require_frontend_python()
    tree = ast.parse(source)
    gen = SimpleTIRGenerator(
        type_hint_policy=type_hint_policy,
    )
    gen.visit(tree)
    return gen.to_json()
