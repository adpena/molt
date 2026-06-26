"""Frontend Bind/Sema phase (doc 44 §F2b).

The semantic-analysis package: free functions over the immutable ``ast.Module``
that compute :class:`SemaResult` — the immutable annotation tables Lower will
read instead of recomputing inline.  This follows the in-package existence proof
``cfg_analysis.py`` (free functions over frozen dataclasses, zero ``self``, zero
god-object state) named by doc 44 §0.

F2b is the **additive-shim** phase: ``analyze_module`` is computed once at
generator construction time, and the existing ``SimpleTIRGenerator`` state dicts
are populated *from* its result (see ``_populate_sema_state`` in
``frontend/__init__.py``), leaving the lowering walk byte-identical.  F2c rewires
the walk to read these tables directly at the use sites; this phase only relocates
the analysis and introduces the contract.
"""

from __future__ import annotations

import ast

from molt.frontend.sema.classgraph import (
    build_class_facts,
    build_class_graph,
    c3_merge,
    class_facts_with_super_fold_sound_methods,
    class_body_needs_block_exec,
    reachable_base_names,
    static_class_bases,
    static_method_owner_after,
    static_mro_names,
    super_fold_is_sound,
    visible_subclasses_of,
)
from molt.frontend.sema.constenv import collect_module_const_dicts
from molt.frontend.sema.funcmeta import (
    FUNCTION_KIND_VALUES,
    STATEFUL_FUNCTION_KINDS,
    STATEFUL_FUNCTION_TAGS,
    FunctionKind,
    StatefulFunctionFramePlan,
    StatefulFunctionTypeHint,
    async_generator_contains_return_value,
    async_generator_contains_yield_from,
    collect_module_class_names,
    collect_module_func_defaults,
    collect_module_func_kinds,
    expression_contains_yield,
    function_contains_yield,
    normalize_function_kind,
    parse_stateful_function_type_hint,
    signature_contains_yield,
    stateful_function_frame_plan,
    stateful_function_result_type_hint,
    stateful_function_tag,
    stateful_function_task_kind,
)
from molt.frontend.sema.result import (
    ClassFacts,
    ClassGraph,
    FunctionMeta,
    SemaResult,
)

__all__ = [
    "ClassGraph",
    "ClassFacts",
    "FUNCTION_KIND_VALUES",
    "FunctionMeta",
    "FunctionKind",
    "STATEFUL_FUNCTION_KINDS",
    "STATEFUL_FUNCTION_TAGS",
    "SemaResult",
    "StatefulFunctionFramePlan",
    "StatefulFunctionTypeHint",
    "analyze_module",
    "async_generator_contains_return_value",
    "async_generator_contains_yield_from",
    "build_class_facts",
    "build_class_graph",
    "c3_merge",
    "class_facts_with_super_fold_sound_methods",
    "class_body_needs_block_exec",
    "collect_module_class_names",
    "collect_module_const_dicts",
    "collect_module_func_defaults",
    "collect_module_func_kinds",
    "expression_contains_yield",
    "function_contains_yield",
    "normalize_function_kind",
    "parse_stateful_function_type_hint",
    "reachable_base_names",
    "signature_contains_yield",
    "stateful_function_frame_plan",
    "stateful_function_result_type_hint",
    "stateful_function_tag",
    "stateful_function_task_kind",
    "static_class_bases",
    "static_method_owner_after",
    "static_mro_names",
    "super_fold_is_sound",
    "visible_subclasses_of",
]


def analyze_module(node: ast.Module) -> SemaResult:
    """Compute the full :class:`SemaResult` for a module, pre-walk.

    Every field is a pure function of *node*; the result is immutable.  An
    externally-supplied ``known_func_defaults`` override (a runtime input, not an
    AST fact) is applied downstream by the populate-shim, not here.
    """
    return SemaResult(
        class_graph=build_class_graph(node),
        class_facts=build_class_facts(node),
        const_dicts=collect_module_const_dicts(node),
        function_meta=FunctionMeta(
            declared_funcs=collect_module_func_kinds(node),
            declared_classes=collect_module_class_names(node),
            defaults=collect_module_func_defaults(node),
        ),
    )
