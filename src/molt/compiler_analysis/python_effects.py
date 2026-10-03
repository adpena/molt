"""Python AST effect projection backed by the generated effect-mask authority."""

from __future__ import annotations

import ast


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
) -> EffectMask:
    mask = NO_EFFECTS
    for child in expression_evaluation_children(node):
        mask |= expression_effect_mask(child)
    return mask


def _key_effects(result: StaticExpressionResult, *, expanded: bool) -> EffectMask:
    """Reduce key effects once per shared shape node and interpretation.

    Expanded bytearrays yield inert integers; a bytearray used as a key raises.
    Keep that distinction while retaining compact DAGs such as ``(*x, *x)``.
    """
    mask = NO_EFFECTS
    pending = [(result, expanded)]
    seen: set[tuple[int, bool]] = set()
    callbacks = EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK | RAISES
    while pending:
        current, as_sequence = pending.pop()
        key = (id(current), as_sequence)
        if key in seen:
            continue
        seen.add(key)
        if not as_sequence:
            if current.kind == "unknown":
                return mask | callbacks
            if current.kind in {"bytearray", "list", "set", "dict"}:
                mask |= RAISES
                continue
            if current.kind not in {"tuple", "frozenset"}:
                continue
        if current.items is None:
            if current.kind not in {"str", "bytes", "bytearray", "range"}:
                return mask | callbacks
            continue
        pending.extend((item.result, item.expanded) for item in current.items)
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
        return self._insert(_key_effects(result, expanded=False))

    def extend(self, result: StaticExpressionResult) -> EffectMask:
        return self._insert(
            _key_effects(result, expanded=True),
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
        if result.kind
        in {
            "tuple",
            "list",
            "set",
            "frozenset",
            "dict",
            "str",
            "bytes",
            "bytearray",
            "range",
        }
        else EXECUTES_ARBITRARY_PYTHON | INVOKES_ITERATION_CALLBACK | RAISES
    )


def mapping_unpack_effects(
    node: ast.expr,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> EffectMask:
    result = static_expression_result(node, fact_result=fact_result)
    if result.kind == "dict":
        return _key_effects(result, expanded=True)
    return (
        EXECUTES_ARBITRARY_PYTHON
        | INVOKES_ITERATION_CALLBACK
        | READS_OBJECT_STATE
        | RAISES
    )


def expression_effect_mask(
    node: ast.AST,
) -> EffectMask:
    """Return a fail-closed mask for evaluating one expression.

    This syntax-only projection never establishes callable identity. Consumers
    with binding facts use their canonical call-site effects. Unknown forms and
    calls are top, so spelling cannot manufacture purity or import stability.
    """

    if isinstance(node, (ast.Constant, ast.Name)):
        return NO_EFFECTS
    if isinstance(node, ast.Lambda):
        return ALLOCATES | _joined_child_effects(node)
    if isinstance(node, ast.NamedExpr):
        return expression_effect_mask(node.value)
    if isinstance(node, ast.Starred):
        return expression_effect_mask(node.value) | iterable_unpack_effects(node.value)
    if isinstance(node, (ast.Tuple, ast.List)):
        return ALLOCATES | _joined_child_effects(node)
    if isinstance(node, ast.Dict):
        mask = ALLOCATES
        keys = AccumulatedKeyEffects()
        for key, value in zip(node.keys, node.values):
            if key is None:
                mask |= (
                    expression_effect_mask(value)
                    | mapping_unpack_effects(value)
                    | keys.extend(static_expression_result(value))
                )
                continue
            mask |= expression_effect_mask(key)
            mask |= expression_effect_mask(value)
            mask |= keys.add(static_expression_result(key))
        return mask
    if isinstance(node, ast.Set):
        keys = AccumulatedKeyEffects()
        mask = ALLOCATES | _joined_child_effects(node)
        for element in node.elts:
            mask |= (
                keys.extend(static_expression_result(element.value))
                if isinstance(element, ast.Starred)
                else keys.add(static_expression_result(element))
            )
        return mask
    if isinstance(node, ast.Slice):
        return ALLOCATES | _joined_child_effects(node)
    if isinstance(node, ast.Attribute):
        return (
            expression_effect_mask(node.value)
            | EXECUTES_ARBITRARY_PYTHON
            | INVOKES_DESCRIPTOR
            | READS_OBJECT_STATE
            | RAISES
        )
    if isinstance(node, ast.Subscript):
        return (
            _joined_child_effects(node)
            | EXECUTES_ARBITRARY_PYTHON
            | READS_OBJECT_STATE
            | RAISES
        )
    if isinstance(node, ast.Call):
        return UNKNOWN_EFFECTS
    if isinstance(node, (ast.Await, ast.Yield, ast.YieldFrom)):
        return (
            _joined_child_effects(node) | EXECUTES_ARBITRARY_PYTHON | SUSPENDS | RAISES
        )
    if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
        return (
            _joined_child_effects(node)
            | ALLOCATES
            | EXECUTES_ARBITRARY_PYTHON
            | INVOKES_ITERATION_CALLBACK
            | RAISES
        )
    if isinstance(node, (ast.BoolOp, ast.Compare, ast.IfExp)):
        return (
            _joined_child_effects(node)
            | EXECUTES_ARBITRARY_PYTHON
            | INVOKES_COMPARISON_CALLBACK
            | RAISES
        )
    if isinstance(node, (ast.BinOp, ast.UnaryOp, ast.FormattedValue, ast.JoinedStr)):
        return _joined_child_effects(node) | EXECUTES_ARBITRARY_PYTHON | RAISES
    return UNKNOWN_EFFECTS


def expression_may_execute_python(
    node: ast.AST,
) -> bool:
    mask = expression_effect_mask(node)
    return not effect_mask_satisfies_capability(
        mask, NO_PYTHON_CALLBACKS_FORBIDDEN_EFFECTS
    )


def expression_preserves_import_state(
    node: ast.AST,
) -> bool:
    mask = expression_effect_mask(node)
    return effect_mask_satisfies_capability(
        mask, PRESERVES_IMPORT_STATE_FORBIDDEN_EFFECTS
    )
