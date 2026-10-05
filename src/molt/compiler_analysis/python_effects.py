"""Python AST effect projection backed by the generated effect-mask authority."""

from __future__ import annotations

import ast


from molt.compiler_analysis.static_truth import (
    ExpressionResultLookup,
    StaticExpressionResult,
    static_expression_result,
    static_subscription_shape,
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
    RELEASES_REFERENCE,
    RUNS_FINALIZER,
    RUNS_WEAKREF_CALLBACK,
    WRITES_OBJECT_STATE,
    EffectMask,
    effect_mask_satisfies_capability,
)


def binary_operation_effects(
    left: StaticExpressionResult,
    right: StaticExpressionResult,
    *,
    inplace: bool = False,
) -> EffectMask:
    """Project dispatch and operand retirement from retained value facts.

    Exact scalar kinds in the result authority exclude subclasses even when
    their values are unknown. Their protocols cannot invoke Python, including
    unsupported operand/operator pairs that raise. Unknown operands retain
    reflected/in-place dispatch and post-callback retirement conservatively.
    """
    effects = ALLOCATES | RAISES | RELEASES_REFERENCE
    if not (left.is_exact_scalar and right.is_exact_scalar):
        effects |= EXECUTES_ARBITRARY_PYTHON
        if inplace:
            effects |= WRITES_OBJECT_STATE
        if left.release_may_call or right.release_may_call:
            effects |= RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
    return effects


def unary_operation_effects(
    operand: StaticExpressionResult, operator: ast.unaryop
) -> EffectMask:
    """Exact scalar protocols are inert except bool inversion's warning path."""
    effects = ALLOCATES | RAISES | RELEASES_REFERENCE
    if not operand.is_exact_scalar or (
        isinstance(operator, ast.Invert) and "bool" in operand.scalar_kinds
    ):
        effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK
        if operand.release_may_call:
            effects |= RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
    return effects


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
        owner = static_expression_result(node.value)
        index = static_expression_result(node.slice)
        parts = (
            tuple(
                static_expression_result(part)
                if part is not None
                else StaticExpressionResult.scalar(None)
                for part in (node.slice.lower, node.slice.upper, node.slice.step)
            )
            if isinstance(node.slice, ast.Slice)
            else None
        )
        shape = static_subscription_shape(owner, index, slice_parts=parts)
        mask = (
            _joined_child_effects(node)
            | READS_OBJECT_STATE
            | RAISES
            | RELEASES_REFERENCE
        )
        if shape.invokes_python:
            mask |= EXECUTES_ARBITRARY_PYTHON
        if owner.release_may_call or any(
            part.release_may_call for part in (parts or (index,))
        ):
            mask |= RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
        return mask
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
    if isinstance(node, ast.BinOp):
        return _joined_child_effects(node) | binary_operation_effects(
            static_expression_result(node.left), static_expression_result(node.right)
        )
    if isinstance(node, ast.UnaryOp):
        return _joined_child_effects(node) | unary_operation_effects(
            static_expression_result(node.operand), node.op
        )
    if isinstance(node, (ast.FormattedValue, ast.JoinedStr)):
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
