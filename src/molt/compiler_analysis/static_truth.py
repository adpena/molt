"""Shared expression-result authority for binding, closure and code emission.

A known truth value is not an exact Python value and never, by itself, permits
discarding evaluation. Source-point facts own names and members; spelling does
not establish a constant or a target-platform binding.
"""

from __future__ import annotations

import ast
from collections.abc import Callable
from dataclasses import dataclass
import math
from typing import Literal, TypeAlias, TypedDict, cast

from molt.compiler_analysis.literal_identity import same_literal_value

ScalarValue: TypeAlias = None | bool | int | float | complex | str | bytes
ExpressionKind: TypeAlias = Literal[
    "unknown",
    "NoneType",
    "bool",
    "int",
    "float",
    "complex",
    "str",
    "bytes",
    "tuple",
    "list",
    "set",
    "dict",
]


@dataclass(frozen=True, slots=True)
class ExpressionSequenceItem:
    result: StaticExpressionResult
    expanded: bool = False


@dataclass(frozen=True, slots=True)
class StaticExpressionResult:
    truth: bool | None = None
    value: ScalarValue = None
    value_known: bool = False
    kind: ExpressionKind = "unknown"
    evaluation_required: bool = True
    items: tuple[ExpressionSequenceItem, ...] | None = None
    release_may_call: bool = True
    # A fresh container has not been published through a name/alias. Mutable
    # iteration may use its contents only until a callback can expose it.
    fresh_container: bool = False

    @classmethod
    def scalar(
        cls, value: object, *, evaluation_required: bool = False
    ) -> StaticExpressionResult:
        if type(value) not in {type(None), bool, int, float, complex, str, bytes}:
            return cls()
        scalar = cast(ScalarValue, value)
        return cls(
            bool(scalar),
            scalar,
            True,
            cast(ExpressionKind, type(value).__name__),
            evaluation_required,
            release_may_call=False,
        )


UNKNOWN_EXPRESSION_RESULT = StaticExpressionResult()
ExpressionResultLookup = Callable[[ast.expr], StaticExpressionResult | None]


class StaticTruthKwargs(TypedDict, total=False):
    fact_result: ExpressionResultLookup


def static_expression_result(
    expr: ast.expr,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> StaticExpressionResult:
    """Project a result without executing user code or inventing binding facts.

    A supplied unknown fact is authoritative. Recursion retains the same
    source-point provider, including through negation and short circuiting.
    """

    def child(node: ast.expr) -> StaticExpressionResult:
        if fact_result is not None:
            fact = fact_result(node)
            if fact is not None:
                return fact
        return static_expression_result(node, fact_result=fact_result)

    if isinstance(expr, ast.Constant):
        return StaticExpressionResult.scalar(expr.value)
    if isinstance(expr, ast.NamedExpr):
        value = child(expr.value)
        return StaticExpressionResult(
            value.truth,
            value.value,
            value.value_known,
            value.kind,
            True,  # The binding write is required even for an exact value.
            value.items,
            value.release_may_call,
        )
    if isinstance(expr, (ast.Name, ast.Attribute)):
        if fact_result is not None:
            return fact_result(expr) or UNKNOWN_EXPRESSION_RESULT
        return UNKNOWN_EXPRESSION_RESULT
    if isinstance(expr, (ast.Tuple, ast.List, ast.Set, ast.Dict)):
        members: list[ExpressionSequenceItem] = []
        uncertain = False
        nonempty = False
        release_may_call = False
        if isinstance(expr, ast.Dict):
            entries = zip(expr.keys, expr.values, strict=True)
            for key, value in entries:
                release_may_call |= child(value).release_may_call
                if key is not None:
                    key_result = child(key)
                    release_may_call |= key_result.release_may_call
                    members.append(ExpressionSequenceItem(key_result))
                    nonempty = True
                else:
                    expansion = child(value)
                    if expansion.kind != "dict" or expansion.items is None:
                        uncertain = True
                    else:
                        members.append(ExpressionSequenceItem(expansion, expanded=True))
                        nonempty |= expansion.truth is True
            kind: ExpressionKind = "dict"
        else:
            kind = cast(ExpressionKind, type(expr).__name__.lower())
            for element in expr.elts:
                if not isinstance(element, ast.Starred):
                    element_result = child(element)
                    release_may_call |= element_result.release_may_call
                    members.append(ExpressionSequenceItem(element_result))
                    nonempty = True
                    continue
                expansion = child(element.value)
                release_may_call |= expansion.release_may_call
                if expansion.items is not None:
                    members.append(ExpressionSequenceItem(expansion, expanded=True))
                    nonempty |= expansion.truth is True
                elif expansion.value_known and expansion.kind in {"str", "bytes"}:
                    nonempty |= bool(expansion.value)
                    # Cardinality is known, but do not materialize arbitrary
                    # strings or manufacture element identity facts.
                    uncertain |= bool(expansion.value)
                else:
                    uncertain = True
        truth = True if nonempty else None if uncertain else False
        return StaticExpressionResult(
            truth=truth,
            kind=kind,
            evaluation_required=True,
            items=None if uncertain else tuple(members),
            release_may_call=release_may_call,
            fresh_container=True,
        )
    if isinstance(expr, ast.UnaryOp) and isinstance(expr.op, ast.Not):
        operand = child(expr.operand)
        if operand.truth is None:
            return StaticExpressionResult(kind="bool")
        return StaticExpressionResult.scalar(
            not operand.truth, evaluation_required=operand.evaluation_required
        )
    if isinstance(expr, ast.BoolOp):
        exits: list[StaticExpressionResult] = []
        required = False
        for index, value in enumerate(expr.values):
            result = child(value)
            required |= result.evaluation_required
            final = index == len(expr.values) - 1
            stops = final or (
                result.truth is False
                if isinstance(expr.op, ast.And)
                else result.truth is True
            )
            if stops or result.truth is None:
                exits.append(result)
            if stops:
                break
        if not exits:
            return UNKNOWN_EXPRESSION_RESULT
        first = exits[0]
        truth = (
            first.truth if all(item.truth is first.truth for item in exits) else None
        )
        exact = first.value_known and all(
            item.value_known
            and item.kind == first.kind
            and _same_scalar_value(item.value, first.value)
            for item in exits
        )
        kind = (
            first.kind if all(item.kind == first.kind for item in exits) else "unknown"
        )
        return StaticExpressionResult(
            truth,
            first.value if exact else None,
            exact,
            kind,
            required,
            first.items if all(item.items == first.items for item in exits) else None,
            release_may_call=any(item.release_may_call for item in exits),
        )
    if isinstance(expr, ast.IfExp):
        test = child(expr.test)
        if test.truth is not None:
            selected = child(expr.body if test.truth else expr.orelse)
            return StaticExpressionResult(
                selected.truth,
                selected.value,
                selected.value_known,
                selected.kind,
                test.evaluation_required or selected.evaluation_required,
                selected.items,
                selected.release_may_call,
            )
        return UNKNOWN_EXPRESSION_RESULT
    if isinstance(expr, ast.Compare):
        left = child(expr.left)
        required = left.evaluation_required
        unknown = False
        for operator, comparator in zip(expr.ops, expr.comparators, strict=True):
            right = child(comparator)
            comparison = static_comparison_result(left, operator, right)
            required |= comparison.evaluation_required
            truth = comparison.truth
            if truth is False:
                if unknown:
                    # A prior rich comparison can short circuit with an
                    # arbitrary falsy object, not the bool singleton False.
                    return UNKNOWN_EXPRESSION_RESULT
                return StaticExpressionResult.scalar(
                    False, evaluation_required=required
                )
            unknown |= truth is None
            required |= truth is None
            left = right
        if not unknown:
            return StaticExpressionResult.scalar(True, evaluation_required=required)
        return UNKNOWN_EXPRESSION_RESULT
    if (
        isinstance(expr, ast.Call)
        and isinstance(expr.func, ast.Attribute)
        and expr.func.attr == "startswith"
        and len(expr.args) == 1
        and not expr.keywords
    ):
        owner, prefix = child(expr.func.value), child(expr.args[0])
        if (
            owner.value_known
            and prefix.value_known
            and owner.kind == prefix.kind == "str"
        ):
            assert isinstance(owner.value, str) and isinstance(prefix.value, str)
            return StaticExpressionResult.scalar(
                owner.value.startswith(prefix.value), evaluation_required=True
            )
    return UNKNOWN_EXPRESSION_RESULT


def _same_scalar_value(left: ScalarValue, right: ScalarValue) -> bool:
    """Exact merge identity, distinct from Python numeric equality."""
    # Expression truth inference deliberately declines NaN merges. The shared
    # key still preserves NaN payloads for dataflow convergence and literal CSE.
    return not (_has_nan(left) or _has_nan(right)) and same_literal_value(left, right)


def _has_nan(value: ScalarValue) -> bool:
    if isinstance(value, float):
        return math.isnan(value)
    if isinstance(value, complex):
        return math.isnan(value.real) or math.isnan(value.imag)
    return False


def static_comparison_result(
    left: StaticExpressionResult, operator: ast.cmpop, right: StaticExpressionResult
) -> StaticExpressionResult:
    """One comparison's value facts, shared with source-ordered execution."""
    truth = _comparison_truth(left, operator, right)
    required = left.evaluation_required or right.evaluation_required
    if truth is not None:
        return StaticExpressionResult.scalar(truth, evaluation_required=required)
    if isinstance(operator, (ast.Is, ast.IsNot)):
        return StaticExpressionResult(kind="bool", evaluation_required=required)
    if isinstance(operator, (ast.In, ast.NotIn)):
        return StaticExpressionResult(kind="bool")
    return UNKNOWN_EXPRESSION_RESULT


def _comparison_truth(
    left: StaticExpressionResult, operator: ast.cmpop, right: StaticExpressionResult
) -> bool | None:
    if isinstance(operator, (ast.Is, ast.IsNot)):
        # Only singleton identity and provably different builtin types are
        # portable. Integer/string interning must never become semantic input.
        if (
            left.value_known
            and right.value_known
            and (
                left.kind in {"bool", "NoneType"} or right.kind in {"bool", "NoneType"}
            )
        ):
            same = left.kind == right.kind and left.value == right.value
            return same if isinstance(operator, ast.Is) else not same
        return None
    if isinstance(operator, (ast.Eq, ast.NotEq)):
        if left.value_known and right.value_known:
            equal = left.value == right.value
        elif (
            left.kind in {"tuple", "list", "set", "dict"}
            and right.kind == "bool"
            or right.kind in {"tuple", "list", "set", "dict"}
            and left.kind == "bool"
        ):
            equal = False
        else:
            return None
        return equal if isinstance(operator, ast.Eq) else not equal
    if isinstance(operator, (ast.In, ast.NotIn)) and left.value_known:
        if right.items is not None:
            # Container membership is identity-or-equality. NaN identity is
            # not represented by scalar values, so do not fold that case.
            if _has_nan(left.value):
                return None
            pending = list(reversed(right.items))
            present = False
            while pending:
                member = pending.pop()
                if member.expanded:
                    if member.result.items is None:
                        return None
                    pending.extend(reversed(member.result.items))
                elif member.result.value_known:
                    if _has_nan(member.result.value):
                        return None
                    present |= left.value == member.result.value
                else:
                    return None
        elif (
            left.kind == right.kind
            and left.kind in {"str", "bytes"}
            and right.value_known
        ):
            if isinstance(left.value, str) and isinstance(right.value, str):
                present = left.value in right.value
            elif isinstance(left.value, bytes) and isinstance(right.value, bytes):
                present = left.value in right.value
            else:
                return None
        else:
            return None
        return present if isinstance(operator, ast.In) else not present
    return None


def static_test_truthiness(
    expr: ast.expr,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> bool | None:
    return static_expression_result(expr, fact_result=fact_result).truth


def static_if_live_branch(
    node: ast.If,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> list[ast.stmt] | None:
    """Select reachable statements; emission must separately preserve evaluation."""
    truth = static_test_truthiness(node.test, fact_result=fact_result)
    return None if truth is None else node.body if truth else node.orelse
