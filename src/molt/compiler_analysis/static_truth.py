"""Shared expression-result authority for binding, closure and code emission.

A known truth value is not an exact Python value and never, by itself, permits
discarding evaluation. Source-point facts own names and members; spelling does
not establish a constant or a target-platform binding.
"""

from __future__ import annotations

import ast
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field, replace
import math
from typing import Literal, TypeAlias, TypedDict, cast

from molt.compiler_analysis.literal_identity import (
    literal_identity_key,
    same_literal_value,
)

from molt.compiler_analysis.python_value_identity import (
    IdentityMask,
    OTHER_IDENTITY,
    PythonIdentity,
)

from molt.compiler_analysis.python_source_keys import PythonSourceKey


@dataclass(frozen=True, slots=True)
class DeferredExecution:
    source: PythonSourceKey
    phase: Literal["call", "factory", "class", "resume", "escape"]


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
_EXACT_SCALAR_EXPRESSION_KINDS: frozenset[ExpressionKind] = frozenset(
    {
        "NoneType",
        "bool",
        "int",
        "float",
        "complex",
        "str",
        "bytes",
    }
)
_STABLE_LEAF_EXPRESSION_KINDS: frozenset[ExpressionKind] = frozenset(
    {*_EXACT_SCALAR_EXPRESSION_KINDS, "range"}
)
_PUBLICATION_RELEASE_STABLE_LEAF_KINDS: frozenset[ExpressionKind] = frozenset(
    {*_STABLE_LEAF_EXPRESSION_KINDS, "bytearray"}
)


def _expression_result_semantic_key(
    truth: bool | None,
    value: ScalarValue,
    value_known: bool,
    kind: ExpressionKind,
    scalar_kinds: frozenset[ExpressionKind],
    evaluation_required: bool,
    items: tuple[ExpressionSequenceItem, ...] | None,
    release_may_call: bool,
    fresh_container: bool,
    length: int | None,
    element_result: StaticExpressionResult | None,
    publication_release_stable: bool,
    identities: IdentityMask,
    exposes_module_globals: bool,
    deferred: frozenset[DeferredExecution],
    exposed_deferred: frozenset[DeferredExecution],
    deferred_complete: bool,
    attribute_hooks: frozenset[DeferredExecution] | None,
    instance_attribute_hooks: frozenset[DeferredExecution] | None,
    class_instantiation_inert: bool,
) -> tuple[object, ...]:
    """Scalar portion of the one result identity; graph edges compare separately."""
    return (
        truth,
        literal_identity_key(value) if value_known else None,
        value_known,
        kind,
        scalar_kinds,
        evaluation_required,
        items is None,
        release_may_call,
        fresh_container,
        length,
        element_result is None,
        publication_release_stable,
        identities,
        exposes_module_globals,
        deferred,
        exposed_deferred,
        deferred_complete,
        attribute_hooks,
        instance_attribute_hooks,
        class_instantiation_inert,
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
    # binding publication; writes/callbacks expire shape and retain only
    # possible canonical identities alongside the unknown alternative.
    element_result: StaticExpressionResult | None = None
    # Source-point value provenance is independent of normal-result shape.
    # Selected aliases carry the same identities even when their kind is unknown.
    identities: IdentityMask = OTHER_IDENTITY
    # Value exposure follows stored/selected mappings and their containers.
    # Evaluation observation is a separate source event, not result provenance.
    # Neither fact proves ownership or release safety.
    exposes_module_globals: bool = False
    # Direct execution and contained escape are distinct. Copying (generator,)
    # does not resume generator; passing the aggregate to foreign code may.
    deferred: frozenset[DeferredExecution] = frozenset()
    exposed_deferred: frozenset[DeferredExecution] = frozenset()
    # An unknown alternative can replace a source callable at a callback.
    # Presence of surviving candidates alone never seals callable provenance.
    deferred_complete: bool = False
    # None is unknown lookup dispatch; an empty set proves ordinary storage.
    # Class creation transports instance hooks separately from metaclass hooks.
    attribute_hooks: frozenset[DeferredExecution] | None = None
    instance_attribute_hooks: frozenset[DeferredExecution] | None = None
    # Sealed class namespace with the default type call and object constructor.
    # Callback invalidation drops this independently of candidate provenance.
    class_instantiation_inert: bool = False
    # Finite exact-builtin alternatives extend the singleton kind proof at
    # joins and value-dependent operations. Empty means an unknown/non-scalar
    # alternative is possible. __post_init__ keeps singleton kind canonical.
    scalar_kinds: frozenset[ExpressionKind] = frozenset()
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
        scalar_kinds = self.scalar_kinds
        if not scalar_kinds <= _EXACT_SCALAR_EXPRESSION_KINDS:
            raise ValueError("scalar alternatives must be exact builtin scalar kinds")
        if self.kind in _EXACT_SCALAR_EXPRESSION_KINDS:
            if scalar_kinds and scalar_kinds != {self.kind}:
                raise ValueError("singleton kind conflicts with scalar alternatives")
            scalar_kinds = frozenset({self.kind})
        elif self.kind != "unknown" and scalar_kinds:
            raise ValueError("non-scalar kind cannot carry scalar alternatives")
        elif len(scalar_kinds) == 1:
            object.__setattr__(self, "kind", next(iter(scalar_kinds)))
        object.__setattr__(self, "scalar_kinds", scalar_kinds)
        exposed = self.exposed_deferred | self.deferred
        for item in self.items or ():
            exposed |= item.result.exposed_deferred
        if self.element_result is not None:
            exposed |= self.element_result.exposed_deferred
        object.__setattr__(self, "exposed_deferred", exposed)
        exposes_globals = (
            self.exposes_module_globals
            or bool(
                self.identities
                & int(PythonIdentity.CURRENT_GLOBALS | PythonIdentity.CURRENT_MODULE)
            )
            or any(item.result.exposes_module_globals for item in self.items or ())
            or self.element_result is not None
            and self.element_result.exposes_module_globals
        )
        object.__setattr__(self, "exposes_module_globals", exposes_globals)
        object.__setattr__(
            self,
            "_recursively_stable",
            bool(scalar_kinds)
            or self.kind in _STABLE_LEAF_EXPRESSION_KINDS
            or self.kind in {"tuple", "frozenset"}
            and self.items is not None
            and all(item.result._recursively_stable for item in self.items),
        )
        publication_release_stable = self._publication_release_stable
        if publication_release_stable is None:
            publication_release_stable = not self.release_may_call and (
                bool(scalar_kinds)
                or self.kind in _PUBLICATION_RELEASE_STABLE_LEAF_KINDS
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
            scalar_kinds,
            self.evaluation_required,
            self.items,
            self.release_may_call,
            self.fresh_container,
            self.length,
            self.element_result,
            publication_release_stable,
            self.identities,
            self.exposes_module_globals,
            self.deferred,
            self.exposed_deferred,
            self.deferred_complete,
            self.attribute_hooks,
            self.instance_attribute_hooks,
            self.class_instantiation_inert,
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

    @property
    def is_exact_scalar(self) -> bool:
        """Builtin representation proof, independent of a known constant value."""
        return bool(self.scalar_kinds)

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
            identities=int(
                PythonIdentity.STATIC_FALSE
                if scalar is False
                else PythonIdentity.INERT_VALUE
            ),
        )


UNKNOWN_EXPRESSION_RESULT = StaticExpressionResult()
ExpressionResultLookup = Callable[[ast.expr], StaticExpressionResult | None]


def expression_result_without_value_facts(
    result: StaticExpressionResult,
) -> StaticExpressionResult:
    """Expire exact facts while retaining possible canonical value provenance.

    A callback can replace a binding or element, but may also leave its old
    value in place. Carry those candidates beside OTHER through the existing
    element graph. Scalar classes and absence alone add nothing to an unknown
    value; omit empty element projections so widening does not retain dead shape.
    """
    completed: dict[int, StaticExpressionResult] = {}
    pending: list[
        tuple[StaticExpressionResult, StaticExpressionResult | None, bool]
    ] = [(result, None, False)]
    while pending:
        current, child, expanded = pending.pop()
        identity = id(current)
        if identity in completed:
            continue
        if not expanded:
            child = current.element_result
            if child is None and current.items is not None:
                child = sequence_element_result(current.items)
        if child is not None and id(child) not in completed and not expanded:
            pending.append((current, child, True))
            pending.append((child, None, False))
            continue
        projected = None if child is None else completed[id(child)]
        if projected == UNKNOWN_EXPRESSION_RESULT:
            projected = None
        symbols = current.identities & ~int(
            PythonIdentity.OTHER
            | PythonIdentity.UNBOUND
            | PythonIdentity.INERT_VALUE
            | PythonIdentity.STATIC_FALSE
        )
        completed[identity] = (
            UNKNOWN_EXPRESSION_RESULT
            if not symbols
            and not current.exposes_module_globals
            and projected is None
            and not current.exposed_deferred
            else StaticExpressionResult(
                identities=OTHER_IDENTITY | symbols,
                exposes_module_globals=current.exposes_module_globals,
                deferred=current.deferred,
                exposed_deferred=current.exposed_deferred,
                element_result=projected,
            )
        )
    return completed[id(result)]


class StaticTruthKwargs(TypedDict, total=False):
    fact_result: ExpressionResultLookup


# Bounds are checked before allocation. Exceeding a fold budget forgets the
# constant while retaining its exact result kind and mandatory evaluation.
_BINARY_FOLD_MAX_SEQUENCE_LENGTH = 4096
_BINARY_FOLD_MAX_INTEGER_BITS = 4096


def _scalar_binary_result_kinds(
    left: StaticExpressionResult,
    operator: ast.operator,
    right: StaticExpressionResult,
    left_kind: ExpressionKind,
    right_kind: ExpressionKind,
) -> frozenset[ExpressionKind]:
    """Normal-result transfer only; errors and overflow remain runtime work."""
    integers = {"bool", "int"}
    numeric = {*integers, "float", "complex"}
    if left_kind in numeric and right_kind in numeric:
        promoted: ExpressionKind = (
            "complex"
            if "complex" in {left_kind, right_kind}
            else "float"
            if "float" in {left_kind, right_kind}
            else "int"
        )
        if isinstance(operator, (ast.Add, ast.Sub, ast.Mult)):
            return frozenset({promoted})
        if isinstance(operator, ast.Div):
            return frozenset({"complex" if promoted == "complex" else "float"})
        if isinstance(operator, (ast.FloorDiv, ast.Mod)):
            return frozenset({promoted}) if promoted != "complex" else frozenset()
        if isinstance(operator, ast.Pow):
            if promoted == "complex":
                return frozenset({"complex"})
            if right_kind in integers:
                if left_kind == "float":
                    return frozenset({"float"})
                if right.value_known:
                    return frozenset(
                        {"int" if cast(int, right.value) >= 0 else "float"}
                    )
                return frozenset({"int", "float"})
            if left.value_known and cast(int | float, left.value) >= 0:
                return frozenset({"float"})
            # Negative-base fractional powers can change the exact kind. Keep
            # the finite alternatives without evaluating the power.
            return frozenset({"float", "complex"})
        if left_kind in integers and right_kind in integers:
            if isinstance(operator, (ast.BitAnd, ast.BitOr, ast.BitXor)):
                return frozenset(
                    {"bool" if left_kind == right_kind == "bool" else "int"}
                )
            if isinstance(operator, (ast.LShift, ast.RShift)):
                return frozenset({"int"})
    if isinstance(operator, ast.Add) and left_kind == right_kind:
        if left_kind in {"str", "bytes"}:
            return frozenset({left_kind})
    if isinstance(operator, ast.Mult):
        if left_kind in {"str", "bytes"} and right_kind in integers:
            return frozenset({left_kind})
        if right_kind in {"str", "bytes"} and left_kind in integers:
            return frozenset({right_kind})
    if isinstance(operator, ast.Mod) and left_kind in {"str", "bytes"}:
        return frozenset({left_kind})
    return frozenset()


def static_binary_result(
    left: StaticExpressionResult,
    operator: ast.operator,
    right: StaticExpressionResult,
) -> StaticExpressionResult:
    """One exact-scalar transfer for syntax, ordinary and augmented operators.

    Kinds describe successful results without speculative arithmetic. Folding
    is optional and bounded; neither a folded value nor a known kind permits
    eliding evaluation, allocation errors, overflow or operand retirement.
    """
    if left.is_exact_scalar and right.is_exact_scalar:
        if isinstance(operator, ast.Add) and left.value_known and right.value_known:
            if type(left.value) is str and type(right.value) is str:
                if (
                    len(left.value) + len(right.value)
                    <= _BINARY_FOLD_MAX_SEQUENCE_LENGTH
                ):
                    return StaticExpressionResult.scalar(
                        left.value + right.value, evaluation_required=True
                    )
            elif type(left.value) is bytes and type(right.value) is bytes:
                if (
                    len(left.value) + len(right.value)
                    <= _BINARY_FOLD_MAX_SEQUENCE_LENGTH
                ):
                    return StaticExpressionResult.scalar(
                        left.value + right.value, evaluation_required=True
                    )
            elif type(left.value) in {bool, int} and type(right.value) in {bool, int}:
                left_int, right_int = cast(int, left.value), cast(int, right.value)
                if (
                    max(left_int.bit_length(), right_int.bit_length())
                    < _BINARY_FOLD_MAX_INTEGER_BITS
                ):
                    return StaticExpressionResult.scalar(
                        left_int + right_int, evaluation_required=True
                    )
        return StaticExpressionResult(
            scalar_kinds=frozenset().union(
                *(
                    _scalar_binary_result_kinds(
                        left, operator, right, left_kind, right_kind
                    )
                    for left_kind in left.scalar_kinds
                    for right_kind in right.scalar_kinds
                )
            ),
            evaluation_required=True,
            release_may_call=False,
            identities=int(PythonIdentity.INERT_VALUE),
        )
    return expression_result_without_value_facts(
        join_static_expression_results((left, right))
    )


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
    if isinstance(expr, ast.BinOp):
        return static_binary_result(child(expr.left), expr.op, child(expr.right))
    if isinstance(expr, ast.NamedExpr):
        value = child(expr.value)
        # The binding write publishes an alias of the original value.
        return replace(value, evaluation_required=True, fresh_container=False)
    if isinstance(expr, ast.Subscript):
        # The owner read precedes index evaluation. Its child fact may be stale
        # after a callback; the provider owns the completed subscription fact.
        if fact_result is not None:
            recorded = fact_result(expr)
            if recorded is not None:
                return recorded
        index_parts = (
            (expr.slice.lower, expr.slice.upper, expr.slice.step)
            if isinstance(expr.slice, ast.Slice)
            else (expr.slice,)
        )
        return static_subscription_shape(
            child(expr.value),
            child(expr.slice),
            index_may_mutate=any(
                part is not None and child(part).evaluation_required
                for part in index_parts
            ),
            slice_parts=(
                tuple(
                    child(part)
                    if part is not None
                    else StaticExpressionResult.scalar(None)
                    for part in (expr.slice.lower, expr.slice.upper, expr.slice.step)
                )
                if isinstance(expr.slice, ast.Slice)
                else None
            ),
        ).result
    if isinstance(expr, (ast.Name, ast.Attribute)):
        if fact_result is not None:
            return fact_result(expr) or UNKNOWN_EXPRESSION_RESULT
        return UNKNOWN_EXPRESSION_RESULT
    if isinstance(expr, (ast.Tuple, ast.List, ast.Set, ast.Dict)):
        members: list[ExpressionSequenceItem] = []
        uncertain = False
        nonempty = False
        release_may_call = False
        exposes_module_globals = False
        exposed_deferred: frozenset[DeferredExecution] = frozenset()
        ordered_length = 0
        ordered_length_known = True
        if isinstance(expr, ast.Dict):
            entries = zip(expr.keys, expr.values, strict=True)
            for key, value in entries:
                value_result = child(value)
                release_may_call |= value_result.release_may_call
                exposes_module_globals |= value_result.exposes_module_globals
                exposed_deferred |= value_result.exposed_deferred
                if key is not None:
                    key_result = child(key)
                    release_may_call |= key_result.release_may_call
                    exposes_module_globals |= key_result.exposes_module_globals
                    members.append(ExpressionSequenceItem(key_result))
                    nonempty = True
                else:
                    expansion = child(value)
                    members.append(ExpressionSequenceItem(expansion, expanded=True))
                    if expansion.kind != "dict" or expansion.items is None:
                        uncertain = True
                    else:
                        nonempty |= expansion.truth is True
            kind: ExpressionKind = "dict"
        else:
            kind = cast(ExpressionKind, type(expr).__name__.lower())
            for element in expr.elts:
                if not isinstance(element, ast.Starred):
                    element_result = child(element)
                    release_may_call |= element_result.release_may_call
                    exposes_module_globals |= element_result.exposes_module_globals
                    members.append(ExpressionSequenceItem(element_result))
                    nonempty = True
                    ordered_length += 1
                    continue
                expansion = child(element.value)
                release_may_call |= expansion.release_may_call
                exposes_module_globals |= expansion.exposes_module_globals
                members.append(ExpressionSequenceItem(expansion, expanded=True))
                if expansion.items is not None:
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
            identities=int(PythonIdentity.INERT_VALUE),
            exposes_module_globals=exposes_module_globals,
            exposed_deferred=exposed_deferred,
            evaluation_required=True,
            items=None if uncertain else tuple(members),
            release_may_call=release_may_call,
            fresh_container=True,
            length=length,
            element_result=sequence_element_result(tuple(members)),
        )
    if isinstance(expr, ast.GeneratorExp):
        # The body has not executed. Retain possible yielded namespace exposure,
        # never its creation-time identity, shape, cardinality, or release proof.
        yielded = replace(
            UNKNOWN_EXPRESSION_RESULT,
            exposes_module_globals=child(expr.elt).exposes_module_globals,
            deferred=child(expr.elt).deferred,
            exposed_deferred=child(expr.elt).exposed_deferred,
        )
        return (
            StaticExpressionResult(element_result=yielded)
            if yielded.exposes_module_globals or yielded.exposed_deferred
            else UNKNOWN_EXPRESSION_RESULT
        )
    if isinstance(expr, (ast.ListComp, ast.SetComp, ast.DictComp)):
        # Comprehension execution can call arbitrary iteration, filtering, and
        # payload code, but every normal completion still constructs the exact
        # builtin outer container. Contents and cardinality remain unknown.
        kind = cast(ExpressionKind, type(expr).__name__.removesuffix("Comp").lower())
        return StaticExpressionResult(
            kind=kind,
            identities=int(PythonIdentity.INERT_VALUE),
            exposes_module_globals=(
                child(expr.value).exposes_module_globals
                if isinstance(expr, ast.DictComp)
                else False
            ),
            exposed_deferred=(
                child(expr.value).exposed_deferred
                if isinstance(expr, ast.DictComp)
                else frozenset()
            ),
            evaluation_required=True,
            release_may_call=True,
            fresh_container=True,
            element_result=(
                child(expr.key) if isinstance(expr, ast.DictComp) else child(expr.elt)
            ),
        )
    if isinstance(expr, ast.UnaryOp):
        operand = child(expr.operand)
        if isinstance(expr.op, ast.Not):
            if operand.truth is None:
                return StaticExpressionResult(
                    identities=int(PythonIdentity.INERT_VALUE), kind="bool"
                )
            return StaticExpressionResult.scalar(
                not operand.truth, evaluation_required=operand.evaluation_required
            )
        # Transfer only normal scalar outcomes: unsupported alternatives raise.
        # Bool inversion stays conservative: its version-dependent warning
        # callback belongs to unary_operation_effects and never runs here.
        supported_kinds = (
            operand.scalar_kinds & {"bool", "int", "float", "complex"}
            if isinstance(expr.op, (ast.UAdd, ast.USub))
            else operand.scalar_kinds & {"int"}
            if isinstance(expr.op, ast.Invert)
            else frozenset()
        )
        value = operand.value
        if (
            supported_kinds
            and operand.value_known
            and (
                type(value) is int
                or type(value) is bool
                or type(value) is float
                or type(value) is complex
            )
        ):
            if isinstance(expr.op, ast.UAdd):
                value = +value
            elif isinstance(expr.op, ast.USub):
                value = -value
            elif isinstance(expr.op, ast.Invert) and type(value) is int:
                value = ~value
            else:
                return UNKNOWN_EXPRESSION_RESULT
            return StaticExpressionResult.scalar(
                value, evaluation_required=operand.evaluation_required
            )
        if supported_kinds:
            return StaticExpressionResult(
                scalar_kinds=frozenset(
                    "int" if kind == "bool" else kind for kind in supported_kinds
                ),
                identities=int(PythonIdentity.INERT_VALUE),
                evaluation_required=True,
                release_may_call=False,
            )
        return UNKNOWN_EXPRESSION_RESULT
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
        return replace(
            join_static_expression_results(tuple(exits)), evaluation_required=required
        )
    if isinstance(expr, ast.IfExp):
        test = child(expr.test)
        if test.truth is not None:
            selected = child(expr.body if test.truth else expr.orelse)
            return replace(
                selected,
                evaluation_required=test.evaluation_required
                or selected.evaluation_required,
            )
        alternatives = join_static_expression_results(
            (child(expr.body), child(expr.orelse))
        )
        return replace(
            alternatives,
            evaluation_required=test.evaluation_required
            or alternatives.evaluation_required,
        )
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


@dataclass(frozen=True, slots=True)
class StaticSubscriptionShape:
    result: StaticExpressionResult = UNKNOWN_EXPRESSION_RESULT
    invokes_python: bool = True


def _sequence_item_at(
    owner: StaticExpressionResult, index: int
) -> StaticExpressionResult | None:
    """Select from compact ordered display DAGs without expanding their size."""
    current = owner
    while True:
        if current.value_known and current.kind in {"str", "bytes"}:
            value = current.value
            assert isinstance(value, (str, bytes))
            return StaticExpressionResult.scalar(value[index])
        if current.kind not in {"tuple", "list"} or current.items is None:
            return None
        for member in current.items:
            length = member.result.length if member.expanded else 1
            if length is None:
                return None
            if index >= length:
                index -= length
                continue
            if not member.expanded:
                return member.result
            current = member.result
            break
        else:
            return None


def static_subscription_shape(
    owner: StaticExpressionResult,
    index: StaticExpressionResult,
    *,
    slice_parts: tuple[StaticExpressionResult, ...] | None = None,
    index_may_mutate: bool = False,
) -> StaticSubscriptionShape:
    """One normal-result/protocol authority for exact builtin subscriptions.

    Evaluation and release of operands remain the caller's responsibility.
    Pass index_may_mutate when the owner fact precedes an effectful index;
    a caller with the completed source-point result should use that directly.
    Exact builtin sequence types cannot override __getitem__; only an unknown
    index (or slice component) can dispatch __index__. Dictionary lookup has a
    different key hash/equality protocol and stays callback-conservative here.
    """
    if index_may_mutate:
        owner = expression_result_without_mutable_contents(owner)
    unknown = replace(
        UNKNOWN_EXPRESSION_RESULT,
        exposes_module_globals=owner.exposes_module_globals,
        deferred=owner.exposed_deferred,
    )
    if owner.kind == "unknown":
        # An expired sequence may still return a formerly observed element.
        # Retain only that possible provenance, plus OTHER; neither subscription
        # dispatch nor its result shape becomes exact again after invalidation.
        if slice_parts is not None:
            unknown = expression_result_without_value_facts(owner)
        elif owner.element_result is not None:
            unknown = replace(
                expression_result_without_value_facts(owner.element_result),
                exposes_module_globals=owner.exposes_module_globals,
                deferred=owner.exposed_deferred | unknown.deferred,
            )
        return StaticSubscriptionShape(unknown)
    if owner.kind == "dict":
        # A mapping's element_result describes iteration keys, not its values.
        return StaticSubscriptionShape(unknown)
    if owner.kind not in {"tuple", "list", "str", "bytes", "bytearray", "range"}:
        return StaticSubscriptionShape(
            invokes_python=owner.kind in {"file_text", "file_bytes"}
        )
    if slice_parts is not None:
        if any(
            part.kind in {"unknown", "file_text", "file_bytes"} for part in slice_parts
        ):
            return StaticSubscriptionShape(expression_result_without_value_facts(owner))
        if any(part.kind not in {"NoneType", "bool", "int"} for part in slice_parts):
            return StaticSubscriptionShape(invokes_python=False)
        if all(part.value_known for part in slice_parts):
            bounds = tuple(cast(int | None, part.value) for part in slice_parts)
            if bounds[2] == 0:
                return StaticSubscriptionShape(invokes_python=False)
            if owner.value_known and owner.kind in {"str", "bytes"}:
                value = owner.value
                assert isinstance(value, (str, bytes))
                return StaticSubscriptionShape(
                    StaticExpressionResult.scalar(
                        value[slice(*bounds)], evaluation_required=True
                    ),
                    False,
                )
        return StaticSubscriptionShape(
            StaticExpressionResult(
                kind=owner.kind,
                identities=int(PythonIdentity.INERT_VALUE),
                exposes_module_globals=owner.exposes_module_globals,
                release_may_call=owner.release_may_call,
                fresh_container=owner.kind in {"list", "bytearray"},
                element_result=iterable_element_result(owner),
            ),
            False,
        )
    if index.kind in {"unknown", "file_text", "file_bytes"}:
        element = iterable_element_result(owner)
        if element is not None:
            unknown = replace(
                expression_result_without_value_facts(element),
                exposes_module_globals=owner.exposes_module_globals,
                deferred=owner.exposed_deferred | unknown.deferred,
            )
        return StaticSubscriptionShape(unknown)
    if index.kind not in {"bool", "int"}:
        return StaticSubscriptionShape(invokes_python=False)
    selected = None
    if index.value_known and owner.length is not None:
        assert isinstance(index.value, int)
        offset = int(index.value)
        if offset < 0:
            offset += owner.length
        if not 0 <= offset < owner.length:
            return StaticSubscriptionShape(invokes_python=False)
        selected = _sequence_item_at(owner, offset)
    if selected is None:
        selected = iterable_element_result(owner)
    if selected is not None:
        selected = replace(
            expression_result_for_publication(selected), evaluation_required=True
        )
    return StaticSubscriptionShape(selected or unknown, False)


def static_unpack_results(
    owner: StaticExpressionResult, target_count: int, starred_index: int | None = None
) -> tuple[StaticExpressionResult, ...] | None:
    """Normal builtin unpack results, including compact starred remainder facts.

    This transports selected objects; it does not establish new alias ownership.
    Exact builtin iteration does not invoke user iteration methods. Unknown
    cardinality retains possible element identities on the successful path.
    """
    if owner.kind == "unknown" and owner.element_result is not None:
        # A successful unpack can yield a formerly observed element. This
        # transports candidates only; callers still own iteration effects.
        element = expression_result_without_value_facts(owner.element_result)
        return tuple(
            StaticExpressionResult(element_result=element)
            if position == starred_index
            else element
            for position in range(target_count)
        )
    if owner.kind not in {
        "tuple",
        "list",
        "str",
        "bytes",
        "bytearray",
        "range",
        "set",
        "frozenset",
        "dict",
    }:
        return None
    element = iterable_element_result(owner)
    selected: list[StaticExpressionResult] = []
    for position in range(target_count):
        if position == starred_index:
            length = (
                max(0, owner.length - target_count + 1)
                if owner.length is not None
                else None
            )
            selected.append(
                StaticExpressionResult(
                    kind="list",
                    identities=int(PythonIdentity.INERT_VALUE),
                    fresh_container=True,
                    length=length,
                    truth=bool(length) if length is not None else None,
                    element_result=element,
                    exposes_module_globals=owner.exposes_module_globals,
                )
            )
            continue
        offset = (
            position - target_count
            if starred_index is not None and position > starred_index
            else position
        )
        result = (
            element or UNKNOWN_EXPRESSION_RESULT
            if owner.kind in {"set", "frozenset", "dict"}
            else static_subscription_shape(
                owner, StaticExpressionResult.scalar(offset)
            ).result
        )
        selected.append(expression_result_for_publication(result))
    return tuple(selected)


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
            elements.append(
                iterable_element_result(item.result) or UNKNOWN_EXPRESSION_RESULT
            )
        else:
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
        return StaticExpressionResult(
            kind=intrinsic_kind,
            release_may_call=False,
            identities=int(PythonIdentity.INERT_VALUE),
        )
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
    published. A subsequent object-write or callback expires element shape,
    retaining only possible canonical identities beside OTHER. Immutable owners retain
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
    return replace(
        result,
        truth=truth,
        value=None,
        value_known=False,
        items=None,
        release_may_call=release_may_call,
        fresh_container=False,
        length=length,
        element_result=iterable_element_result(result),
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
    published_results: dict[tuple[int, bool], StaticExpressionResult] = {}
    pending = [(result, preserve_owner, False)]
    while pending:
        current, retain_outer, expanded = pending.pop()
        identity = (id(current), retain_outer)
        if identity in completed:
            continue
        published = published_results.get(identity)
        if published is None:
            published = (
                expression_result_for_owned_binding(current)
                if retain_outer
                else expression_result_for_publication(current)
            )
            # Keep normalized item edges alive and identical across revisits.
            published_results[identity] = published
        if published._recursively_stable:
            completed[identity] = published
            continue
        child = published.element_result
        if published.kind in _MUTABLE_EXPRESSION_KINDS and not retain_outer:
            child = (
                expression_result_without_value_facts(child)
                if child is not None
                else None
            )
            if child == UNKNOWN_EXPRESSION_RESULT:
                child = None
            # The projection has already expired every descendant. Complete
            # this node directly; visiting it again must not rebuild a child
            # with a different object identity from the pending DAG edge.
            completed[identity] = (
                published
                if child is published.element_result
                else replace(published, element_result=child)
            )
            continue
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


def _join_known_deferred_sets(
    values: Iterable[frozenset[DeferredExecution] | None],
) -> frozenset[DeferredExecution] | None:
    """Unknown dominates; a known empty set remains a complete fact."""
    merged: set[DeferredExecution] = set()
    for value in values:
        if value is None:
            return None
        merged.update(value)
    return frozenset(merged)


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
                result.element_result or UNKNOWN_EXPRESSION_RESULT for result in current
            )
            if any(result.element_result is not None for result in current)
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
        scalar_kinds = (
            frozenset().union(*(result.scalar_kinds for result in current))
            if all(result.is_exact_scalar for result in current)
            else frozenset()
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
        identities = 0
        for result in current:
            identities |= result.identities
        exposes_module_globals = any(
            result.exposes_module_globals for result in current
        )
        deferred = frozenset().union(*(result.deferred for result in current))
        exposed_deferred = frozenset().union(
            *(result.exposed_deferred for result in current)
        )
        deferred_complete = all(result.deferred_complete for result in current)
        attribute_hooks = _join_known_deferred_sets(
            result.attribute_hooks for result in current
        )
        instance_attribute_hooks = _join_known_deferred_sets(
            result.instance_attribute_hooks for result in current
        )
        class_instantiation_inert = all(
            result.class_instantiation_inert for result in current
        )
        publication_release_stable = all(
            bool(result._publication_release_stable) for result in current
        )
        semantic_key = _expression_result_semantic_key(
            truth,
            value,
            exact,
            kind,
            scalar_kinds,
            evaluation_required,
            items,
            release_may_call,
            fresh_container,
            length,
            element_result,
            publication_release_stable,
            identities,
            exposes_module_globals,
            deferred,
            exposed_deferred,
            deferred_complete,
            attribute_hooks,
            instance_attribute_hooks,
            class_instantiation_inert,
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
                scalar_kinds=scalar_kinds,
                evaluation_required=evaluation_required,
                items=items,
                release_may_call=release_may_call,
                fresh_container=fresh_container,
                length=length,
                element_result=element_result,
                _publication_release_stable=publication_release_stable,
                identities=identities,
                exposes_module_globals=exposes_module_globals,
                deferred=deferred,
                exposed_deferred=exposed_deferred,
                deferred_complete=deferred_complete,
                attribute_hooks=attribute_hooks,
                instance_attribute_hooks=instance_attribute_hooks,
                class_instantiation_inert=class_instantiation_inert,
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
        return StaticExpressionResult(
            identities=int(PythonIdentity.INERT_VALUE),
            kind="bool",
            evaluation_required=required,
        )
    if isinstance(operator, (ast.In, ast.NotIn)):
        return StaticExpressionResult(
            identities=int(PythonIdentity.INERT_VALUE), kind="bool"
        )
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


def statically_executed_boolop_values(
    node: ast.BoolOp,
    *,
    fact_result: ExpressionResultLookup | None = None,
) -> tuple[ast.expr, ...]:
    values: list[ast.expr] = []
    if isinstance(node.op, ast.And):
        for idx, value in enumerate(node.values):
            values.append(value)
            value_truth = static_test_truthiness(
                value,
                fact_result=fact_result,
            )
            if value_truth is False:
                return tuple(values)
            if value_truth is None:
                values.extend(node.values[idx + 1 :])
                return tuple(values)
        return tuple(values)
    if isinstance(node.op, ast.Or):
        for idx, value in enumerate(node.values):
            values.append(value)
            value_truth = static_test_truthiness(
                value,
                fact_result=fact_result,
            )
            if value_truth is True:
                return tuple(values)
            if value_truth is None:
                values.extend(node.values[idx + 1 :])
                return tuple(values)
        return tuple(values)
    return tuple(node.values)
