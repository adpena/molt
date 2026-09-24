"""Shared expression-result authority for binding, closure and code emission.

A known truth value is not an exact Python value and never, by itself, permits
discarding evaluation. Source-point facts own names and members; spelling does
not establish a constant or a target-platform binding.
"""

from __future__ import annotations

import ast
from collections.abc import Callable
from dataclasses import dataclass, field, replace
import math
from typing import Literal, TypeAlias, TypedDict, cast

from molt.compiler_analysis.literal_identity import (
    literal_identity_key,
    same_literal_value,
)

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
    "bytearray",
    "tuple",
    "list",
    "set",
    "frozenset",
    "dict",
    "range",
    "file_text",
    "file_bytes",
]
_MUTABLE_EXPRESSION_KINDS: frozenset[ExpressionKind] = frozenset(
    {"bytearray", "list", "set", "dict"}
)
_STABLE_LEAF_EXPRESSION_KINDS: frozenset[ExpressionKind] = frozenset(
    {
        "NoneType",
        "bool",
        "int",
        "float",
        "complex",
        "str",
        "bytes",
        "range",
    }
)
_PUBLICATION_RELEASE_STABLE_LEAF_KINDS: frozenset[ExpressionKind] = frozenset(
    {*_STABLE_LEAF_EXPRESSION_KINDS, "bytearray"}
)


def _expression_result_semantic_key(
    truth: bool | None,
    value: ScalarValue,
    value_known: bool,
    kind: ExpressionKind,
    evaluation_required: bool,
    items: tuple[ExpressionSequenceItem, ...] | None,
    release_may_call: bool,
    fresh_container: bool,
    length: int | None,
    element_result: StaticExpressionResult | None,
    publication_release_stable: bool,
) -> tuple[object, ...]:
    """Scalar portion of the one result identity; graph edges compare separately."""
    return (
        truth,
        literal_identity_key(value) if value_known else None,
        value_known,
        kind,
        evaluation_required,
        items is None,
        release_may_call,
        fresh_container,
        length,
        element_result is None,
        publication_release_stable,
    )


@dataclass(frozen=True, slots=True)
class ExpressionSequenceItem:
    result: StaticExpressionResult
    expanded: bool = False


@dataclass(frozen=True, slots=True, eq=False)
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
    # Exact cardinality without materializing potentially enormous contents
    # (notably ``range``). ``items`` remains the content/provenance authority.
    length: int | None = None
    # Homogeneous yielded-item authority. Unlike ``items``, this does not claim
    # cardinality or concrete contents. Mutable producers may retain it after
    # binding publication until an object-write or callback boundary expires it.
    element_result: StaticExpressionResult | None = None
    _semantic_key: tuple[object, ...] = field(
        init=False, repr=False, compare=False, hash=False
    )
    _semantic_hash: int = field(init=False, repr=False, compare=False, hash=False)
    _recursively_stable: bool = field(init=False, repr=False, compare=False, hash=False)
    # Canonical cached proof transported when publication erases descendant
    # shape. ``None`` means derive it from the still-visible result graph.
    _publication_release_stable: bool | None = field(
        default=None, repr=False, compare=False, hash=False
    )

    def __post_init__(self) -> None:
        object.__setattr__(
            self,
            "_recursively_stable",
            self.kind in _STABLE_LEAF_EXPRESSION_KINDS
            or self.kind in {"tuple", "frozenset"}
            and self.items is not None
            and all(item.result._recursively_stable for item in self.items),
        )
        publication_release_stable = self._publication_release_stable
        if publication_release_stable is None:
            publication_release_stable = not self.release_may_call and (
                self.kind in _PUBLICATION_RELEASE_STABLE_LEAF_KINDS
                or self.kind in {"tuple", "frozenset"}
                and self.items is not None
                and all(
                    bool(item.result._publication_release_stable) for item in self.items
                )
            )
        publication_release_stable = bool(publication_release_stable)
        object.__setattr__(
            self, "_publication_release_stable", publication_release_stable
        )
        key = _expression_result_semantic_key(
            self.truth,
            self.value,
            self.value_known,
            self.kind,
            self.evaluation_required,
            self.items,
            self.release_may_call,
            self.fresh_container,
            self.length,
            self.element_result,
            publication_release_stable,
        )
        object.__setattr__(self, "_semantic_key", key)
        edges = (
            None
            if self.items is None
            else tuple(
                (item.result._semantic_hash, item.expanded) for item in self.items
            )
        )
        element_edge = (
            None if self.element_result is None else self.element_result._semantic_hash
        )
        object.__setattr__(self, "_semantic_hash", hash((key, edges, element_edge)))

    def __hash__(self) -> int:
        return self._semantic_hash

    def __eq__(self, other: object) -> bool:
        if self is other:
            return True
        if not isinstance(other, StaticExpressionResult):
            return False
        pending: list[tuple[StaticExpressionResult, StaticExpressionResult]] = [
            (self, other)
        ]
        compared: set[tuple[int, int]] = set()
        while pending:
            left, right = pending.pop()
            if left is right:
                continue
            pair = (id(left), id(right))
            if pair in compared:
                continue
            if (
                left._semantic_hash != right._semantic_hash
                or left._semantic_key != right._semantic_key
            ):
                return False
            left_element, right_element = left.element_result, right.element_result
            if left_element is None or right_element is None:
                if left_element is not right_element:
                    return False
            else:
                pending.append((left_element, right_element))
            left_items, right_items = left.items, right.items
            if left_items is None or right_items is None:
                if left_items is not right_items:
                    return False
                compared.add(pair)
                continue
            if len(left_items) != len(right_items):
                return False
            compared.add(pair)
            for left_item, right_item in zip(left_items, right_items, strict=True):
                if left_item.expanded is not right_item.expanded:
                    return False
                pending.append((left_item.result, right_item.result))
        return True

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
            length=len(scalar) if isinstance(scalar, (str, bytes)) else None,
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
            truth=value.truth,
            value=value.value,
            value_known=value.value_known,
            kind=value.kind,
            evaluation_required=True,  # The binding write is required.
            items=value.items,
            release_may_call=value.release_may_call,
            fresh_container=value.fresh_container,
            length=value.length,
            element_result=value.element_result,
            _publication_release_stable=value._publication_release_stable,
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
        ordered_length = 0
        ordered_length_known = True
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
                    ordered_length += 1
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
                if expansion.length is None:
                    ordered_length_known = False
                else:
                    ordered_length += expansion.length
        truth = True if nonempty else None if uncertain else False
        length = (
            ordered_length
            if kind in {"tuple", "list"} and ordered_length_known
            else 0
            if truth is False
            else None
        )
        return StaticExpressionResult(
            truth=truth,
            kind=kind,
            evaluation_required=True,
            items=None if uncertain else tuple(members),
            release_may_call=release_may_call,
            fresh_container=True,
            length=length,
            element_result=(
                sequence_element_result(tuple(members)) if not uncertain else None
            ),
        )
    if isinstance(expr, (ast.ListComp, ast.SetComp, ast.DictComp)):
        # Comprehension execution can call arbitrary iteration, filtering, and
        # payload code, but every normal completion still constructs the exact
        # builtin outer container. Contents and cardinality remain unknown.
        kind = cast(ExpressionKind, type(expr).__name__.removesuffix("Comp").lower())
        return StaticExpressionResult(
            kind=kind,
            evaluation_required=True,
            release_may_call=True,
            fresh_container=True,
            element_result=(
                child(expr.key) if isinstance(expr, ast.DictComp) else child(expr.elt)
            ),
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
            fresh_container=all(item.fresh_container for item in exits),
            length=(
                first.length
                if all(item.length == first.length for item in exits)
                else None
            ),
            element_result=(
                join_static_expression_results(
                    tuple(
                        item.element_result
                        for item in exits
                        if item.element_result is not None
                    )
                )
                if all(item.element_result is not None for item in exits)
                else None
            ),
            _publication_release_stable=all(
                bool(item._publication_release_stable) for item in exits
            ),
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
                selected.fresh_container,
                selected.length,
                selected.element_result,
                _publication_release_stable=selected._publication_release_stable,
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


def _recursively_stable_result(result: StaticExpressionResult) -> bool:
    return result._recursively_stable


def _release_stable_after_publication(result: StaticExpressionResult) -> bool:
    return bool(result._publication_release_stable)


def sequence_element_result(
    items: tuple[ExpressionSequenceItem, ...],
) -> StaticExpressionResult | None:
    """Join finite sequence members without claiming a concrete cardinality."""

    pending = list(items)
    expanded_seen: set[int] = set()
    elements: list[StaticExpressionResult] = []
    while pending:
        item = pending.pop()
        if not item.expanded:
            elements.append(item.result)
            continue
        identity = id(item.result)
        if identity in expanded_seen:
            continue
        expanded_seen.add(identity)
        if item.result.items is None:
            return None
        pending.extend(item.result.items)
    return join_static_expression_results(tuple(elements)) if elements else None


def iterable_element_result(
    result: StaticExpressionResult,
) -> StaticExpressionResult | None:
    """Project the yielded-item result from the canonical expression graph."""

    if result.element_result is not None:
        return result.element_result
    # File mode is not an exact iterator protocol. Text decoders may return a
    # str subclass, and unbuffered FileIO dispatches the instance's readline.
    # A stronger producer may transport an explicit element_result above.
    intrinsic_kind: ExpressionKind | None = (
        "str"
        if result.kind == "str"
        else "int"
        if result.kind in {"bytes", "bytearray", "range"}
        else None
    )
    if intrinsic_kind is not None:
        return StaticExpressionResult(kind=intrinsic_kind, release_may_call=False)
    if result.items is None:
        return None
    return sequence_element_result(result.items)


def expression_result_for_publication(
    result: StaticExpressionResult,
) -> StaticExpressionResult:
    """Publish a value into a binding without inventing heap/alias custody.

    Mutable values keep their exact normal-result kind and homogeneous element
    result, but concrete contents, cardinality, truth, freshness, and
    reference-bearing release safety cease to be stable once an alias is
    published. The element result has its own shorter lifetime and is erased by
    a subsequent object-write or callback boundary. Immutable owners retain
    their own truth and cardinality while unsafe descendant shape is erased.
    Bytearrays remain release-safe because their payload cannot contain object
    references.
    """

    unstable_immutable_container = result.kind in {
        "tuple",
        "frozenset",
    } and not _recursively_stable_result(result)
    if (
        result.kind not in _MUTABLE_EXPRESSION_KINDS
        and not unstable_immutable_container
    ):
        return result
    immutable_owner = result.kind in {"tuple", "frozenset"}
    truth = result.truth if immutable_owner else None
    length = result.length if immutable_owner else None
    release_may_call = not _release_stable_after_publication(result)
    if (
        result.truth is truth
        and result.value is None
        and not result.value_known
        and result.items is None
        and result.release_may_call is release_may_call
        and not result.fresh_container
        and result.length == length
    ):
        return result
    return StaticExpressionResult(
        truth=truth,
        kind=result.kind,
        evaluation_required=result.evaluation_required,
        release_may_call=release_may_call,
        length=length,
        element_result=result.element_result,
        _publication_release_stable=result._publication_release_stable,
    )


def expression_result_for_owned_binding(
    result: StaticExpressionResult,
) -> StaticExpressionResult:
    """Publish into tracked allocation storage without inventing exposure.

    The caller must hold a nonzero evaluated-allocation token and invalidate
    contents of every alias at object-write/callback boundaries. This retains
    current contents, not unexposed freshness or lifetime-long immutability.
    Unknown owners use ``expression_result_for_publication`` instead.
    """

    return replace(result, fresh_container=False) if result.fresh_container else result


def expression_result_without_mutable_contents(
    result: StaticExpressionResult, *, preserve_owner: bool = False
) -> StaticExpressionResult:
    """Expire alias-sensitive mutable contents while retaining normal kind."""

    # A proven nonalias outer allocation can survive a write to another owner,
    # but any mutable object reachable through it can still be that receiver.
    # Transform both per-member and homogeneous element facts in one DAG walk.
    completed: dict[tuple[int, bool], StaticExpressionResult] = {}
    pending = [(result, preserve_owner, False)]
    while pending:
        current, retain_outer, expanded = pending.pop()
        identity = (id(current), retain_outer)
        if identity in completed:
            continue
        published = (
            expression_result_for_owned_binding(current)
            if retain_outer
            else expression_result_for_publication(current)
        )
        if published._recursively_stable:
            completed[identity] = published
            continue
        child = published.element_result
        if published.kind in _MUTABLE_EXPRESSION_KINDS and not retain_outer:
            child = None
        members = published.items
        if not expanded and (child is not None or members):
            pending.append((current, retain_outer, True))
            if child is not None:
                pending.append((child, False, False))
            if members:
                pending.extend((item.result, False, False) for item in members)
            continue
        if child is not None:
            child = completed[(id(child), False)]
        if members is not None and any(
            completed[(id(item.result), False)] is not item.result for item in members
        ):
            members = tuple(
                ExpressionSequenceItem(
                    completed[(id(item.result), False)], expanded=item.expanded
                )
                for item in members
            )
        descendants_changed = (
            child is not published.element_result or members is not published.items
        )
        release_may_call = published.release_may_call or (
            retain_outer
            and descendants_changed
            and (
                child is not None
                and child.release_may_call
                or members is not None
                and any(item.result.release_may_call for item in members)
            )
        )
        completed[identity] = (
            published
            if not descendants_changed
            else replace(
                published,
                items=members,
                element_result=child,
                release_may_call=release_may_call,
                _publication_release_stable=(
                    False if release_may_call else published._publication_release_stable
                ),
            )
        )
    return completed[(id(result), preserve_owner)]


def join_static_expression_results(
    results: tuple[StaticExpressionResult, ...],
) -> StaticExpressionResult:
    """Join control-flow alternatives in the canonical result algebra.

    Exact equality and absorption retain the first matching representative in
    input order. A computed NaN join still declines exact value identity; only
    the established all-equal path retains an already equal exact NaN fact.
    """
    if not results:
        return UNKNOWN_EXPRESSION_RESULT
    first = results[0]
    if results.count(first) == len(results):
        return first
    completed: dict[tuple[int, ...], StaticExpressionResult] = {}
    pending = [(results, False)]
    while pending:
        current, expanded = pending.pop()
        key = tuple(id(result) for result in current)
        if key in completed:
            continue
        first = current[0]
        if current.count(first) == len(current):
            completed[key] = first
            continue
        child_results = (
            tuple(
                result.element_result
                for result in current
                if result.element_result is not None
            )
            if all(result.element_result is not None for result in current)
            else None
        )
        child_key = (
            None
            if child_results is None
            else tuple(id(result) for result in child_results)
        )
        if child_results is not None and child_key not in completed and not expanded:
            pending.append((current, True))
            pending.append((child_results, False))
            continue
        kind: ExpressionKind = (
            first.kind
            if all(result.kind == first.kind for result in current)
            else "unknown"
        )
        exact = first.value_known and all(
            result.value_known
            and result.kind == first.kind
            and _same_scalar_value(result.value, first.value)
            for result in current[1:]
        )
        truth = (
            first.truth
            if all(result.truth is first.truth for result in current[1:])
            else None
        )
        value = first.value if exact else None
        evaluation_required = any(result.evaluation_required for result in current)
        items = (
            first.items
            if all(result.items == first.items for result in current[1:])
            else None
        )
        release_may_call = any(result.release_may_call for result in current)
        fresh_container = all(result.fresh_container for result in current)
        length = (
            first.length
            if all(result.length == first.length for result in current[1:])
            else None
        )
        element_result = None if child_key is None else completed[child_key]
        publication_release_stable = all(
            bool(result._publication_release_stable) for result in current
        )
        semantic_key = _expression_result_semantic_key(
            truth,
            value,
            exact,
            kind,
            evaluation_required,
            items,
            release_may_call,
            fresh_container,
            length,
            element_result,
            publication_release_stable,
        )
        for candidate in current:
            if (
                candidate._semantic_key == semantic_key
                and candidate.items == items
                and candidate.element_result == element_result
            ):
                completed[key] = candidate
                break
        else:
            completed[key] = StaticExpressionResult(
                truth=truth,
                value=value,
                value_known=exact,
                kind=kind,
                evaluation_required=evaluation_required,
                items=items,
                release_may_call=release_may_call,
                fresh_container=fresh_container,
                length=length,
                element_result=element_result,
                _publication_release_stable=publication_release_stable,
            )
    return completed[tuple(id(result) for result in results)]


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
            left.kind
            in {"bytearray", "tuple", "list", "set", "frozenset", "dict", "range"}
            and right.kind == "bool"
            or right.kind
            in {"bytearray", "tuple", "list", "set", "frozenset", "dict", "range"}
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
            expanded_seen: set[int] = set()
            present = False
            while pending:
                member = pending.pop()
                if member.expanded:
                    identity = id(member.result)
                    if identity in expanded_seen:
                        continue
                    expanded_seen.add(identity)
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
