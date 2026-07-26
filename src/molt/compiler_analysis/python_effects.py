"""Python AST effect projection backed by the generated effect-mask authority."""

from __future__ import annotations

import ast
from collections.abc import Collection

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


def _constant_hash_is_closed(node: ast.expr) -> bool:
    if isinstance(node, ast.Constant):
        return isinstance(node.value, (str, bytes, int, float, complex, bool, type(None)))
    if isinstance(node, ast.Tuple):
        return all(_constant_hash_is_closed(element) for element in node.elts)
    return False


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
        return (
            expression_effect_mask(node.value, proven_pure_calls=proven_pure_calls)
            | INVOKES_ITERATION_CALLBACK
            | EXECUTES_ARBITRARY_PYTHON
            | RAISES
        )
    if isinstance(node, (ast.Tuple, ast.List)):
        return ALLOCATES | _joined_child_effects(
            node, proven_pure_calls=proven_pure_calls
        )
    if isinstance(node, ast.Dict):
        mask = ALLOCATES
        for key, value in zip(node.keys, node.values):
            if key is None:
                mask |= (
                    expression_effect_mask(value, proven_pure_calls=proven_pure_calls)
                    | EXECUTES_ARBITRARY_PYTHON
                    | INVOKES_ITERATION_CALLBACK
                    | READS_OBJECT_STATE
                    | RAISES
                )
                continue
            mask |= expression_effect_mask(key, proven_pure_calls=proven_pure_calls)
            mask |= expression_effect_mask(value, proven_pure_calls=proven_pure_calls)
            if not _constant_hash_is_closed(key):
                mask |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
        return mask
    if isinstance(node, ast.Set):
        mask = ALLOCATES | _joined_child_effects(
            node, proven_pure_calls=proven_pure_calls
        )
        if any(not _constant_hash_is_closed(element) for element in node.elts):
            mask |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
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
        for argument in (*node.args, *(keyword.value for keyword in node.keywords)):
            arguments |= expression_effect_mask(
                argument, proven_pure_calls=proven_pure_calls
            )
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
