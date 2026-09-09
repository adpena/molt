"""Python AST effect projection backed by the generated effect-mask authority."""

from __future__ import annotations

import ast
from collections.abc import Collection

from molt.compiler_analysis.python_call_arguments import call_argument_schedule

from molt.compiler_analysis.static_truth import (
    ExpressionResultLookup,
    StaticExpressionResult,
    static_expression_result,
)

from molt.compiler_analysis.python_effects_generated import (
    ALLOCATES,
    EXECUTES_ARBITRARY_PYTHON,
    INVOKES_COMPARISON_CALLBACK,
    INVOKES_DESCRIPTOR,
    INVOKES_ITERATION_CALLBACK,
    NO_EFFECTS,
    NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS,
    PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS,
    RAISES,
    READS_OBJECT_STATE,
    SUSPENDS,
    UNKNOWN_EFFECTS,
    EffectMask,
    effect_mask_satisfies_capability,
)


def dotted_expression_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        owner = dotted_expression_name(node.value)
        return f"{owner}.{node.attr}" if owner is not None else None
    return None


def expression_evaluation_children(node: ast.AST) -> tuple[ast.expr, ...]:
    """Return child expressions in CPython evaluation order."""

    if isinstance(node, ast.Call):
        return (node.func, *node.args, *(keyword.value for keyword in node.keywords))
    if isinstance(node, ast.Lambda):
        return (
            *node.args.defaults,
            *(default for default in node.args.kw_defaults if default is not None),
        )
    if isinstance(node, ast.Dict):
        return tuple(
            expression
            for key, value in zip(node.keys, node.values)
            for expression in (key, value)
            if expression is not None
        )
    if isinstance(node, ast.NamedExpr):
        return (node.value,)
    return tuple(
        child for child in ast.iter_child_nodes(node) if isinstance(child, ast.expr)
    )


def _joined_child_effects(
    node: ast.AST,
    *,
    proven_pure_calls: Collection[str],
) -> EffectMask:
    mask = NO_EFFECTS
    for child in expression_evaluation_children(node):
        mask |= expression_effect_mask(child, proven_pure_calls=proven_pure_calls)
    return mask


def _result_hash_effects(result: StaticExpressionResult) -> EffectMask:
    if result.kind == "unknown":
        return EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
    if result.kind in {"list", "set", "dict"}:
        return RAISES  # Exact builtin containers are unhashable, not callbacks.
    return _sequence_hash_effects(result) if result.kind == "tuple" else NO_EFFECTS


def _sequence_hash_effects(result: StaticExpressionResult) -> EffectMask:
    if result.items is None:
        if result.kind in {"str", "bytes"}:
            return NO_EFFECTS
        return EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
    mask = NO_EFFECTS
    for item in result.items:
        mask |= (
            _sequence_hash_effects(item.result)
            if item.expanded
            else _result_hash_effects(item.result)
        )
    return mask


class AccumulatedKeyEffects:
    """Insertion effects include equality on keys already in the destination.

    Retain callback capability, not source ASTs: later evaluation may rebind a
    key expression, but cannot replace the objects already held by the table.
    The same authority governs keyword assembly and dict/set displays.
    """

    def __init__(self) -> None:
        self._existing_callbacks = NO_EFFECTS

    def _insert(self, incoming: EffectMask, *, empty: bool = False) -> EffectMask:
        if empty:
            return incoming
        boundary = incoming | self._existing_callbacks
        if incoming & INVOKES_COMPARISON_CALLBACK:
            self._existing_callbacks = (
                EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
            )
        return boundary

    def add(self, result: StaticExpressionResult) -> EffectMask:
        return self._insert(_result_hash_effects(result))

    def extend(self, result: StaticExpressionResult) -> EffectMask:
        return self._insert(
            _sequence_hash_effects(result),
            empty=result.items == () or result.truth is False,
        )


def iterable_unpack_effects(
    node: ast.expr,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> EffectMask:
    result = static_expression_result(node, fact_result=fact_result)
    return (
        NO_EFFECTS
        if result.kind in {"tuple", "list", "set", "dict", "str", "bytes"}
        else EXECUTES_ARBITRARY_PYTHON | INVOKES_ITERATION_CALLBACK | RAISES
    )


def mapping_unpack_effects(
    node: ast.expr,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> EffectMask:
    result = static_expression_result(node, fact_result=fact_result)
    if result.kind == "dict":
        return _sequence_hash_effects(result)
    return (
        EXECUTES_ARBITRARY_PYTHON
        | INVOKES_ITERATION_CALLBACK
        | READS_OBJECT_STATE
        | RAISES
    )


def expression_effect_mask(
    node: ast.AST,
    *,
    proven_pure_calls: Collection[str] = (),
) -> EffectMask:
    """Return a fail-closed mask for evaluating one expression.

    Exact calls are admitted only through the caller's binding-proven identity
    set. Unknown AST forms are top so adding syntax cannot silently manufacture
    purity or import-state stability.
    """

    if isinstance(node, (ast.Constant, ast.Name)):
        return NO_EFFECTS
    if isinstance(node, ast.Lambda):
        return ALLOCATES | _joined_child_effects(
            node, proven_pure_calls=proven_pure_calls
        )
    if isinstance(node, ast.NamedExpr):
        return expression_effect_mask(node.value, proven_pure_calls=proven_pure_calls)
    if isinstance(node, ast.Starred):
        return expression_effect_mask(
            node.value, proven_pure_calls=proven_pure_calls
        ) | iterable_unpack_effects(node.value)
    if isinstance(node, (ast.Tuple, ast.List)):
        return ALLOCATES | _joined_child_effects(
            node, proven_pure_calls=proven_pure_calls
        )
    if isinstance(node, ast.Dict):
        mask = ALLOCATES
        keys = AccumulatedKeyEffects()
        for key, value in zip(node.keys, node.values):
            if key is None:
                mask |= (
                    expression_effect_mask(value, proven_pure_calls=proven_pure_calls)
                    | mapping_unpack_effects(value)
                    | keys.extend(static_expression_result(value))
                )
                continue
            mask |= expression_effect_mask(key, proven_pure_calls=proven_pure_calls)
            mask |= expression_effect_mask(value, proven_pure_calls=proven_pure_calls)
            mask |= keys.add(static_expression_result(key))
        return mask
    if isinstance(node, ast.Set):
        keys = AccumulatedKeyEffects()
        mask = ALLOCATES | _joined_child_effects(
            node, proven_pure_calls=proven_pure_calls
        )
        for element in node.elts:
            mask |= (
                keys.extend(static_expression_result(element.value))
                if isinstance(element, ast.Starred)
                else keys.add(static_expression_result(element))
            )
        return mask
    if isinstance(node, ast.Slice):
        return ALLOCATES | _joined_child_effects(
            node, proven_pure_calls=proven_pure_calls
        )
    if isinstance(node, ast.Attribute):
        return (
            expression_effect_mask(node.value, proven_pure_calls=proven_pure_calls)
            | EXECUTES_ARBITRARY_PYTHON
            | INVOKES_DESCRIPTOR
            | READS_OBJECT_STATE
            | RAISES
        )
    if isinstance(node, ast.Subscript):
        return (
            _joined_child_effects(node, proven_pure_calls=proven_pure_calls)
            | EXECUTES_ARBITRARY_PYTHON
            | READS_OBJECT_STATE
            | RAISES
        )
    if isinstance(node, ast.Call):
        arguments = NO_EFFECTS
        keys = AccumulatedKeyEffects()
        for step in call_argument_schedule(node):
            if step.action == "evaluate":
                arguments |= expression_effect_mask(
                    step.expression, proven_pure_calls=proven_pure_calls
                )
            elif step.action == "star":
                arguments |= iterable_unpack_effects(step.expression)
            elif step.action == "kw":
                arguments |= keys.add(StaticExpressionResult.scalar(step.name))
            elif step.action == "kwstar":
                arguments |= mapping_unpack_effects(step.expression)
                arguments |= keys.extend(static_expression_result(step.expression))
        name = dotted_expression_name(node.func)
        if name in proven_pure_calls:
            return arguments | ALLOCATES | RAISES
        return UNKNOWN_EFFECTS
    if isinstance(node, (ast.Await, ast.Yield, ast.YieldFrom)):
        return (
            _joined_child_effects(node, proven_pure_calls=proven_pure_calls)
            | EXECUTES_ARBITRARY_PYTHON
            | SUSPENDS
            | RAISES
        )
    if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
        return (
            _joined_child_effects(node, proven_pure_calls=proven_pure_calls)
            | ALLOCATES
            | EXECUTES_ARBITRARY_PYTHON
            | INVOKES_ITERATION_CALLBACK
            | RAISES
        )
    if isinstance(node, (ast.BoolOp, ast.Compare, ast.IfExp)):
        return (
            _joined_child_effects(node, proven_pure_calls=proven_pure_calls)
            | EXECUTES_ARBITRARY_PYTHON
            | INVOKES_COMPARISON_CALLBACK
            | RAISES
        )
    if isinstance(node, (ast.BinOp, ast.UnaryOp, ast.FormattedValue, ast.JoinedStr)):
        return (
            _joined_child_effects(node, proven_pure_calls=proven_pure_calls)
            | EXECUTES_ARBITRARY_PYTHON
            | RAISES
        )
    return UNKNOWN_EFFECTS


def expression_may_execute_python(
    node: ast.AST,
    *,
    proven_pure_calls: Collection[str] = (),
) -> bool:
    mask = expression_effect_mask(node, proven_pure_calls=proven_pure_calls)
    return not effect_mask_satisfies_capability(
        mask, NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    )


def expression_preserves_import_state(
    node: ast.AST,
    *,
    proven_pure_calls: Collection[str] = (),
) -> bool:
    mask = expression_effect_mask(node, proven_pure_calls=proven_pure_calls)
    return effect_mask_satisfies_capability(
        mask, PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS
    )
