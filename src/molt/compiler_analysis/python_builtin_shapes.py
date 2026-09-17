"""Canonical normal-result shapes and invocation effects for exact builtins.

This module owns the finite builtin catalog consumed by binding analysis and
frontend specialization. It describes a call's normal result independently
from whether invoking it is callback-free; exact output kind is not permission
to elide evaluation or dispatch.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import Final, cast

from molt.compiler_analysis.python_effects_generated import (
    ALLOCATES,
    EXECUTES_ARBITRARY_PYTHON,
    INVOKES_COMPARISON_CALLBACK,
    INVOKES_ITERATION_CALLBACK,
    READS_OBJECT_STATE,
    RAISES,
    RELEASES_REFERENCE,
    RUNS_FINALIZER,
    RUNS_WEAKREF_CALLBACK,
    WRITES_OBJECT_STATE,
    EffectMask,
)
from molt.compiler_analysis.static_truth import (
    ExpressionKind,
    ExpressionSequenceItem,
    StaticExpressionResult,
    UNKNOWN_EXPRESSION_RESULT,
    iterable_element_result,
    join_static_expression_results,
)


BUILTIN_SHAPE_NAMES: Final[frozenset[str]] = frozenset(
    {
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
        "len",
        "open",
    }
)

_SCALAR_KINDS: Final[frozenset[ExpressionKind]] = frozenset(
    {"NoneType", "bool", "int", "float", "complex", "str", "bytes"}
)
_REFERENCE_FREE_KINDS: Final[frozenset[ExpressionKind]] = _SCALAR_KINDS | {
    "bytearray",
    "range",
}
_BUILTIN_CONTAINER_KINDS: Final[frozenset[ExpressionKind]] = frozenset(
    {"bytearray", "tuple", "list", "set", "frozenset", "dict", "range"}
)
_METHODS_BY_KIND: Final[dict[ExpressionKind, frozenset[str]]] = {
    "list": frozenset(
        {
            "append",
            "insert",
            "extend",
            "remove",
            "count",
            "index",
            "pop",
            "clear",
            "reverse",
        }
    ),
    "dict": frozenset({"get", "pop", "setdefault", "keys", "values", "items"}),
    "tuple": frozenset({"count", "index"}),
    "str": frozenset(
        {"find", "split", "replace", "startswith", "endswith", "count", "join"}
    ),
    "bytes": frozenset(
        {"find", "split", "replace", "startswith", "endswith", "count", "join"}
    ),
    "bytearray": frozenset(
        {"find", "split", "replace", "startswith", "endswith", "count", "join"}
    ),
}


@dataclass(frozen=True, slots=True)
class BuiltinCallShape:
    result: StaticExpressionResult
    invocation_effects: EffectMask
    specialization_valid: bool


@dataclass(frozen=True, slots=True)
class BuiltinMethodCallShape:
    """Normal result and lifetime transitions for an exact builtin method."""

    result: StaticExpressionResult
    invocation_effects: EffectMask
    argument_effects: EffectMask
    receiver_after: StaticExpressionResult | None
    specialization_valid: bool
    retained_argument_indices: frozenset[int]


def builtin_method_descriptor_known(kind: ExpressionKind, method: str) -> bool:
    return method in _METHODS_BY_KIND.get(kind, ())


def _normal_result(
    kind: ExpressionKind,
    *,
    truth: bool | None = None,
    items: tuple[ExpressionSequenceItem, ...] | None = None,
    release_may_call: bool = True,
    fresh_container: bool = False,
    length: int | None = None,
    element_result: StaticExpressionResult | None = None,
    publication_release_stable: bool | None = None,
) -> StaticExpressionResult:
    return StaticExpressionResult(
        truth=truth,
        kind=kind,
        evaluation_required=True,
        items=items,
        release_may_call=release_may_call,
        fresh_container=fresh_container,
        length=length,
        element_result=element_result,
        _publication_release_stable=publication_release_stable,
    )


def _empty_container(kind: ExpressionKind) -> StaticExpressionResult:
    return _normal_result(
        kind,
        truth=False,
        items=(),
        release_may_call=False,
        fresh_container=kind in {"bytearray", "list", "set", "dict"},
        length=0,
    )


def _known_scalar_conversion(
    name: str, argument: StaticExpressionResult
) -> StaticExpressionResult | None:
    if not argument.value_known or argument.kind != name:
        return None
    if name not in {"bool", "int", "float", "complex", "str", "bytes"}:
        return None
    # Same-kind constructors preserve the exact builtin scalar. Do not invoke
    # host parsing/formatting/allocation as a target-language fact authority.
    return StaticExpressionResult.scalar(argument.value, evaluation_required=True)


def _plain_arguments(
    node: ast.Call, argument_results: tuple[StaticExpressionResult, ...]
) -> tuple[StaticExpressionResult, ...] | None:
    if any(isinstance(argument, ast.Starred) for argument in node.args):
        return None
    if any(keyword.arg is None for keyword in node.keywords):
        return None
    if len(argument_results) != len(node.args) + len(node.keywords):
        return None
    return argument_results


def _range_result(
    node: ast.Call, arguments: tuple[StaticExpressionResult, ...]
) -> StaticExpressionResult:
    if node.keywords or not 1 <= len(arguments) <= 3:
        return _normal_result("range", release_may_call=False)
    values: list[int] = []
    for argument in arguments:
        if argument.kind not in {"bool", "int"} or not argument.value_known:
            return _normal_result("range", release_may_call=False)
        assert isinstance(argument.value, (bool, int))
        values.append(int(argument.value))
    start, stop, step = (
        (0, values[0], 1)
        if len(values) == 1
        else (values[0], values[1], 1)
        if len(values) == 2
        else (values[0], values[1], values[2])
    )
    if step == 0:
        return _normal_result("range", release_may_call=False)
    nonempty = start < stop if step > 0 else start > stop
    cardinality = (
        0
        if not nonempty
        else 1 + (stop - 1 - start) // step
        if step > 0
        else 1 + (start - 1 - stop) // -step
    )
    return _normal_result(
        "range",
        truth=nonempty,
        release_may_call=False,
        # ``len`` is constrained by Py_ssize_t. Keep only the portable range
        # across supported 32- and 64-bit targets while retaining exact truth.
        length=cardinality if cardinality <= (1 << 31) - 1 else None,
    )


def _result_shape(
    name: str,
    node: ast.Call,
    arguments: tuple[StaticExpressionResult, ...] | None,
) -> StaticExpressionResult:
    kind = cast(ExpressionKind, "int" if name == "len" else name)
    if arguments is None:
        return _normal_result(
            kind,
            release_may_call=kind not in _REFERENCE_FREE_KINDS,
            fresh_container=kind in {"bytearray", "list", "set", "dict"},
        )
    positional = arguments[: len(node.args)]
    if name == "len":
        if len(positional) == 1 and not node.keywords:
            length = positional[0].length
            if length is not None:
                return StaticExpressionResult.scalar(length, evaluation_required=True)
        return _normal_result("int", release_may_call=False)
    if name == "range":
        return _range_result(node, positional)
    if not positional and not node.keywords:
        if name in {"bool", "int"}:
            return StaticExpressionResult.scalar(
                False if name == "bool" else 0, evaluation_required=True
            )
        if name == "float":
            return StaticExpressionResult.scalar(0.0, evaluation_required=True)
        if name == "complex":
            return StaticExpressionResult.scalar(0j, evaluation_required=True)
        if name == "str":
            return StaticExpressionResult.scalar("", evaluation_required=True)
        if name == "bytes":
            return StaticExpressionResult.scalar(b"", evaluation_required=True)
        return _empty_container(kind)
    if len(positional) == 1 and not node.keywords:
        converted = _known_scalar_conversion(name, positional[0])
        if converted is not None:
            return converted
        if name == "bool" and positional[0].truth is not None:
            return StaticExpressionResult.scalar(
                positional[0].truth, evaluation_required=True
            )
        if name in {"tuple", "list"} and positional[0].kind in {"tuple", "list"}:
            return _normal_result(
                kind,
                truth=positional[0].truth,
                items=positional[0].items,
                release_may_call=positional[0].release_may_call,
                fresh_container=name == "list",
                length=positional[0].length,
                element_result=positional[0].element_result,
                publication_release_stable=(
                    bool(positional[0]._publication_release_stable)
                    if name == "tuple"
                    and positional[0].kind == "tuple"
                    and positional[0].items is None
                    else None
                ),
            )
    if (
        name == "dict"
        and not positional
        and all(keyword.arg is not None for keyword in node.keywords)
    ):
        return _normal_result(
            "dict",
            truth=bool(node.keywords),
            release_may_call=any(result.release_may_call for result in arguments),
            fresh_container=True,
            length=len(node.keywords),
        )
    return _normal_result(
        kind,
        release_may_call=kind not in _REFERENCE_FREE_KINDS,
        fresh_container=kind in {"bytearray", "list", "set", "dict"},
    )


def _callback_free_invocation(
    name: str,
    node: ast.Call,
    arguments: tuple[StaticExpressionResult, ...] | None,
) -> bool:
    if arguments is None:
        return False
    positional = arguments[: len(node.args)]
    if name == "len":
        return (
            len(positional) == 1
            and not node.keywords
            and positional[0].kind in _SCALAR_KINDS | _BUILTIN_CONTAINER_KINDS
        )
    if name == "range":
        return (
            not node.keywords
            and 1 <= len(positional) <= 3
            and all(argument.kind in {"bool", "int"} for argument in positional)
        )
    if not positional and not node.keywords:
        return True
    if (
        name == "dict"
        and not positional
        and all(keyword.arg is not None for keyword in node.keywords)
    ):
        return True
    if (
        name == "dict"
        and len(positional) == 1
        and not node.keywords
        and positional[0].kind == "dict"
    ):
        return True
    if (
        name == "int"
        and len(positional) == 2
        and not node.keywords
        and positional[0].kind in {"str", "bytes", "bytearray"}
        and positional[1].kind in {"bool", "int"}
    ):
        return True
    if (
        name == "int"
        and len(positional) == 1
        and len(node.keywords) == 1
        and node.keywords[0].arg == "base"
        and positional[0].kind in {"str", "bytes", "bytearray"}
        and arguments[-1].kind in {"bool", "int"}
    ):
        return True
    if (
        name == "complex"
        and len(positional) == 2
        and not node.keywords
        and all(argument.kind in {"bool", "int", "float"} for argument in positional)
    ):
        return True
    if len(positional) == 1 and not node.keywords:
        argument = positional[0]
        if name == "bool" and argument.kind in _SCALAR_KINDS | _BUILTIN_CONTAINER_KINDS:
            return True
        if name in {"int", "float", "complex"} and argument.kind in {
            "bool",
            "int",
            "float",
            "complex",
            "str",
            "bytes",
            "bytearray",
        }:
            return True
        if name == "str" and argument.kind in _SCALAR_KINDS | {"bytearray", "range"}:
            return True
        if name == "bytes" and argument.kind in {
            "bool",
            "int",
            "bytes",
            "bytearray",
            "range",
        }:
            return True
        if name == "bytearray" and argument.kind in {
            "bool",
            "int",
            "bytes",
            "bytearray",
            "range",
        }:
            return True
        if (
            name in {"tuple", "list"}
            and argument.kind in _SCALAR_KINDS | _BUILTIN_CONTAINER_KINDS
        ):
            return True
        if (
            name in {"set", "frozenset"}
            and argument.items is not None
            and all(item.result.kind in _SCALAR_KINDS for item in argument.items)
        ):
            return True
    return False


def _invocation_effects(
    name: str,
    node: ast.Call,
    arguments: tuple[StaticExpressionResult, ...] | None,
) -> EffectMask:
    effects = RAISES
    if name != "len":
        effects |= ALLOCATES
    if _callback_free_invocation(name, node, arguments):
        return effects
    return effects | EXECUTES_ARBITRARY_PYTHON | INVOKES_ITERATION_CALLBACK


def _text_result_may_be_strict_subclass(
    node: ast.Call,
    arguments: tuple[StaticExpressionResult, ...] | None,
) -> bool:
    if arguments is None:
        return True
    positional = arguments[: len(node.args)]
    if len(positional) == 1 and not node.keywords:
        return positional[0].kind == "unknown"
    if len(positional) in {2, 3} and not node.keywords:
        # Dynamic codecs can return strict str/bytes subclasses.
        return True
    return bool(positional) and any(
        keyword.arg in {"encoding", "errors"} for keyword in node.keywords
    )


def builtin_call_shape(
    name: str,
    node: ast.Call,
    argument_results: tuple[StaticExpressionResult, ...],
) -> BuiltinCallShape:
    """Return normal-result shape and newly executed invocation effects."""

    if name not in BUILTIN_SHAPE_NAMES:
        raise ValueError(f"unsupported builtin shape name: {name}")
    if name == "open":
        return _builtin_open_shape(node, argument_results)
    arguments = _plain_arguments(node, argument_results)
    arities = {
        "bool": (0, 1),
        "int": (0, 2),
        "float": (0, 1),
        "complex": (0, 2),
        "str": (0, 3),
        "bytes": (0, 3),
        "bytearray": (0, 3),
        "tuple": (0, 1),
        "list": (0, 1),
        "set": (0, 1),
        "frozenset": (0, 1),
        "dict": (0, 1),
        "range": (1, 3),
        "len": (1, 1),
    }
    minimum, maximum = arities[name]
    specialization_valid = (
        arguments is not None
        and not node.keywords
        and minimum <= len(node.args) <= maximum
    )
    invocation_effects = _invocation_effects(name, node, arguments)
    result = _result_shape(name, node, arguments)
    if name in {"str", "bytes"} and _text_result_may_be_strict_subclass(
        node, arguments
    ):
        # __str__ and __bytes__ may legally return strict subclasses. Unlike
        # numeric constructors, these calls do not normalize every hook result
        # to the exact builtin type.
        result = UNKNOWN_EXPRESSION_RESULT
    return BuiltinCallShape(result, invocation_effects, specialization_valid)


def _builtin_open_shape(
    node: ast.Call, argument_results: tuple[StaticExpressionResult, ...]
) -> BuiltinCallShape:
    """Return exact builtin ``open`` mode/result facts without spelling authority."""

    effects = ALLOCATES | READS_OBJECT_STATE | EXECUTES_ARBITRARY_PYTHON | RAISES
    arguments = _plain_arguments(node, argument_results)
    if arguments is None:
        return BuiltinCallShape(UNKNOWN_EXPRESSION_RESULT, effects, False)
    positional = arguments[: len(node.args)]
    keyword_results = {
        keyword.arg: result
        for keyword, result in zip(node.keywords, arguments[len(node.args) :])
        if keyword.arg is not None
    }
    file_result = positional[0] if positional else keyword_results.get("file")
    if file_result is None:
        return BuiltinCallShape(UNKNOWN_EXPRESSION_RESULT, effects, False)
    mode_result = positional[1] if len(positional) > 1 else keyword_results.get("mode")
    mode = "r" if mode_result is None else None
    if (
        mode_result is not None
        and mode_result.kind == "str"
        and mode_result.value_known
    ):
        mode = cast(str, mode_result.value)
    result = (
        _normal_result(
            "file_bytes" if "b" in mode else "file_text",
            release_may_call=True,
        )
        if mode is not None
        else UNKNOWN_EXPRESSION_RESULT
    )
    # ``open`` always emits the ``open`` audit event. Registered audit hooks
    # can execute Python even for literal paths, default encodings, and the
    # builtin opener, so exact normal-result shape is independent of callback
    # freedom here.
    return BuiltinCallShape(result, effects, False)


def _joined_element_result(
    receiver: StaticExpressionResult,
    incoming: StaticExpressionResult | None,
) -> StaticExpressionResult | None:
    current = iterable_element_result(receiver)
    if receiver.truth is False:
        return incoming
    if current is None or incoming is None:
        return None
    joined = join_static_expression_results((current, incoming))
    return None if joined.kind == "unknown" else joined


def _receiver_with_element(
    receiver: StaticExpressionResult,
    element: StaticExpressionResult | None,
    *,
    release_may_call: bool,
) -> StaticExpressionResult:
    return StaticExpressionResult(
        kind=receiver.kind,
        evaluation_required=receiver.evaluation_required,
        release_may_call=release_may_call,
        element_result=element,
        _publication_release_stable=receiver._publication_release_stable,
    )


def _comparison_callback_free(
    receiver: StaticExpressionResult, argument: StaticExpressionResult | None
) -> bool:
    element = iterable_element_result(receiver)
    safe = _SCALAR_KINDS | {"range"}
    return (
        element is not None
        and argument is not None
        and element.kind in safe
        and argument.kind in safe
    )


def _dict_lookup_callback_free(
    receiver: StaticExpressionResult, key: StaticExpressionResult | None
) -> bool:
    if key is None or key.kind not in _SCALAR_KINDS | {"range"}:
        return False
    element = iterable_element_result(receiver)
    return receiver.truth is False or (
        element is not None and element.kind in _SCALAR_KINDS | {"range"}
    )


def _index_callback_free(result: StaticExpressionResult | None) -> bool:
    return result is not None and result.kind in {"bool", "int"}


def _iteration_callback_free(result: StaticExpressionResult) -> bool:
    return result.kind in {
        "str",
        "bytes",
        "bytearray",
        "tuple",
        "list",
        "set",
        "frozenset",
        "dict",
        "range",
    }


def _buffer_argument_callback_free(
    result: StaticExpressionResult, *, tuple_items: bool = False
) -> bool:
    """Whether buffer acquisition cannot reach PEP 688 Python hooks."""

    if result.kind == "unknown":
        return False
    if not tuple_items or result.kind != "tuple":
        return True
    element = iterable_element_result(result)
    return result.truth is False or (element is not None and element.kind != "unknown")


_RELEASE_CALLBACK_EFFECTS: Final[EffectMask] = (
    RELEASES_REFERENCE | RUNS_FINALIZER | RUNS_WEAKREF_CALLBACK
)


def builtin_method_call_shape(
    receiver: StaticExpressionResult,
    method: str,
    node: ast.Call,
    argument_results: tuple[StaticExpressionResult, ...],
) -> BuiltinMethodCallShape | None:
    """Classify exact builtin methods and independent receiver-content lifetime."""

    if any(isinstance(argument, ast.Starred) for argument in node.args) or any(
        keyword.arg is None for keyword in node.keywords
    ):
        return None
    positional = argument_results[: len(node.args)]
    keyword_results = {
        keyword.arg: result
        for keyword, result in zip(
            node.keywords,
            argument_results[len(node.args) :],
        )
        if keyword.arg is not None
    }
    arguments_complete = len(argument_results) == len(node.args) + len(node.keywords)
    inert_none = StaticExpressionResult.scalar(None, evaluation_required=True)
    inert_int = _normal_result("int", release_may_call=False)
    inert_bool = _normal_result("bool", release_may_call=False)
    base_effects = READS_OBJECT_STATE | RAISES

    def shape(
        result: StaticExpressionResult,
        effects: EffectMask,
        receiver_after: StaticExpressionResult | None,
        valid: bool,
        *,
        publishes_arguments: bool = False,
        retained_argument_indices: frozenset[int] = frozenset(),
    ) -> BuiltinMethodCallShape:
        if not valid:
            return BuiltinMethodCallShape(
                UNKNOWN_EXPRESSION_RESULT,
                base_effects,
                base_effects,
                None,
                False,
                frozenset(),
            )
        # Receiver mutation is publication only for methods that may store
        # arguments. Exact positions stored by the normal result have separate
        # retained-owner custody; callback/release effects remain shared.
        argument_effects = (
            effects if publishes_arguments else effects & ~WRITES_OBJECT_STATE
        )
        return BuiltinMethodCallShape(
            result,
            effects,
            argument_effects,
            receiver_after,
            True,
            retained_argument_indices,
        )

    if receiver.kind == "list":
        if method == "append":
            valid = arguments_complete and len(positional) == 1 and not node.keywords
            incoming = positional[0] if positional else None
            return shape(
                inert_none,
                base_effects | ALLOCATES | WRITES_OBJECT_STATE,
                _receiver_with_element(
                    receiver,
                    _joined_element_result(receiver, incoming),
                    release_may_call=(
                        receiver.release_may_call or incoming.release_may_call
                    ),
                )
                if incoming is not None
                else None,
                valid,
                publishes_arguments=True,
                retained_argument_indices=frozenset((0,)),
            )
        if method == "insert":
            valid = arguments_complete and len(positional) == 2 and not node.keywords
            callback_free = bool(positional) and _index_callback_free(positional[0])
            effects = base_effects | WRITES_OBJECT_STATE
            effects |= ALLOCATES
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON
            incoming = positional[1] if len(positional) > 1 else None
            return shape(
                inert_none,
                effects,
                _receiver_with_element(
                    receiver,
                    _joined_element_result(receiver, incoming),
                    release_may_call=(
                        receiver.release_may_call or incoming.release_may_call
                    ),
                )
                if callback_free and incoming is not None
                else None,
                valid,
                publishes_arguments=True,
                retained_argument_indices=frozenset((1,)),
            )
        if method == "extend":
            valid = arguments_complete and len(positional) == 1 and not node.keywords
            source = positional[0] if positional else UNKNOWN_EXPRESSION_RESULT
            incoming = iterable_element_result(source)
            callback_free = _iteration_callback_free(source)
            effects = base_effects | ALLOCATES | WRITES_OBJECT_STATE
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_ITERATION_CALLBACK
            return shape(
                inert_none,
                effects,
                _receiver_with_element(
                    receiver,
                    _joined_element_result(receiver, incoming),
                    release_may_call=(
                        receiver.release_may_call
                        or source.truth is not False
                        and (incoming is None or incoming.release_may_call)
                    ),
                )
                if callback_free
                else None,
                valid,
            )
        if method == "remove":
            valid = arguments_complete and len(positional) == 1 and not node.keywords
            safe = bool(positional) and _comparison_callback_free(
                receiver, positional[0]
            )
            element = iterable_element_result(receiver)
            release_safe = receiver.truth is False or (
                element is not None and not element.release_may_call
            )
            effects = base_effects | WRITES_OBJECT_STATE
            if not safe:
                effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK
            if not release_safe:
                effects |= _RELEASE_CALLBACK_EFFECTS
            return shape(
                inert_none,
                effects,
                _receiver_with_element(
                    receiver,
                    element,
                    release_may_call=receiver.release_may_call,
                )
                if safe and release_safe
                else None,
                valid,
            )
        if method in {"count", "index"}:
            valid = (
                arguments_complete
                and not node.keywords
                and (
                    len(positional) == 1
                    if method == "count"
                    else 1 <= len(positional) <= 3
                )
            )
            safe = bool(positional) and _comparison_callback_free(
                receiver, positional[0]
            )
            if method == "index":
                safe &= all(_index_callback_free(bound) for bound in positional[1:])
            effects = base_effects
            if not safe:
                effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK
            return shape(inert_int, effects, None, valid)
        if method == "pop":
            valid = arguments_complete and len(positional) <= 1 and not node.keywords
            callback_free = not positional or _index_callback_free(positional[0])
            element = iterable_element_result(receiver)
            effects = base_effects | WRITES_OBJECT_STATE
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON
            return shape(
                element or UNKNOWN_EXPRESSION_RESULT
                if callback_free
                else UNKNOWN_EXPRESSION_RESULT,
                effects,
                _receiver_with_element(
                    receiver,
                    element,
                    release_may_call=receiver.release_may_call,
                )
                if callback_free
                else None,
                valid,
            )
        if method == "clear":
            valid = arguments_complete and not positional and not node.keywords
            element = iterable_element_result(receiver)
            release_safe = receiver.truth is False or (
                element is not None and not element.release_may_call
            )
            effects = base_effects | WRITES_OBJECT_STATE
            if not release_safe:
                effects |= _RELEASE_CALLBACK_EFFECTS
            return shape(
                inert_none,
                effects,
                _receiver_with_element(receiver, None, release_may_call=False)
                if release_safe
                else None,
                valid,
            )
        if method == "reverse":
            valid = arguments_complete and not positional and not node.keywords
            return shape(
                inert_none,
                base_effects | WRITES_OBJECT_STATE,
                _receiver_with_element(
                    receiver,
                    iterable_element_result(receiver),
                    release_may_call=receiver.release_may_call,
                ),
                valid,
            )

    if receiver.kind == "dict":
        key = positional[0] if positional else None
        safe = _dict_lookup_callback_free(receiver, key)
        if method in {"get", "pop", "setdefault"} and key is not None:
            arities = {"get": (1, 2), "pop": (1, 2), "setdefault": (1, 2)}
            minimum, maximum = arities[method]
            valid = (
                arguments_complete
                and not node.keywords
                and minimum <= len(positional) <= maximum
            )
            effects = base_effects
            mutates = method in {"pop", "setdefault"}
            if mutates:
                effects |= WRITES_OBJECT_STATE
            if not safe:
                effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK
            element = iterable_element_result(receiver)
            if (
                method == "pop"
                and receiver.truth is not False
                and (element is None or element.release_may_call)
            ):
                effects |= _RELEASE_CALLBACK_EFFECTS
            default = positional[1] if len(positional) > 1 else inert_none
            result = (
                default
                if receiver.truth is False and safe
                else UNKNOWN_EXPRESSION_RESULT
            )
            receiver_after = None
            if mutates and safe and not effects & _RELEASE_CALLBACK_EFFECTS:
                receiver_after = _receiver_with_element(
                    receiver,
                    _joined_element_result(receiver, key)
                    if method == "setdefault"
                    else element,
                    release_may_call=(
                        receiver.release_may_call
                        or method == "setdefault"
                        and (key.release_may_call or default.release_may_call)
                    ),
                )
            return shape(
                result,
                effects,
                receiver_after,
                valid,
                publishes_arguments=method == "setdefault",
                retained_argument_indices=(
                    frozenset(range(len(positional)))
                    if method == "setdefault" and receiver.truth is False and safe
                    else frozenset()
                ),
            )
        if method in {"keys", "values", "items"}:
            valid = arguments_complete and not positional and not node.keywords
            return shape(UNKNOWN_EXPRESSION_RESULT, base_effects, None, valid)

    if receiver.kind == "tuple" and method in {"count", "index"}:
        valid = (
            arguments_complete
            and not node.keywords
            and (
                len(positional) == 1 if method == "count" else 1 <= len(positional) <= 3
            )
        )
        effects = base_effects
        callback_free = bool(positional) and _comparison_callback_free(
            receiver, positional[0]
        )
        if method == "index":
            callback_free &= all(
                _index_callback_free(bound) for bound in positional[1:]
            )
        if not callback_free:
            effects |= EXECUTES_ARBITRARY_PYTHON | INVOKES_COMPARISON_CALLBACK
        return shape(inert_int, effects, None, valid)

    if receiver.kind in {"str", "bytes", "bytearray"}:
        text_kind = receiver.kind
        if method == "split":
            valid_keywords = set(keyword_results) <= {"sep", "maxsplit"}
            duplicates = (
                bool(positional)
                and "sep" in keyword_results
                or (len(positional) > 1 and "maxsplit" in keyword_results)
            )
            valid = (
                arguments_complete
                and len(positional) <= 2
                and valid_keywords
                and not duplicates
            )
            maxsplit = (
                positional[1]
                if len(positional) > 1
                else keyword_results.get("maxsplit")
            )
            effects = base_effects | ALLOCATES
            separator = positional[0] if positional else keyword_results.get("sep")
            if (
                receiver.kind in {"bytes", "bytearray"}
                and separator is not None
                and not _buffer_argument_callback_free(separator)
            ):
                effects |= EXECUTES_ARBITRARY_PYTHON
            if maxsplit is not None and not _index_callback_free(maxsplit):
                effects |= EXECUTES_ARBITRARY_PYTHON
            element = _normal_result(text_kind, release_may_call=False)
            return shape(
                _normal_result(
                    "list",
                    # split constructs an exact list containing only exact
                    # str/bytes/bytearray values. Retiring that normal result
                    # cannot invoke element finalizers or weakref callbacks.
                    release_may_call=element.release_may_call,
                    fresh_container=True,
                    element_result=element,
                ),
                effects,
                None,
                valid,
            )
        if method in {"find", "count"}:
            valid = (
                arguments_complete and not node.keywords and 1 <= len(positional) <= 3
            )
            callback_free = bool(positional) and all(
                _index_callback_free(bound) for bound in positional[1:]
            )
            if (
                receiver.kind in {"bytes", "bytearray"}
                and positional
                and not _buffer_argument_callback_free(positional[0])
            ):
                callback_free = False
            effects = base_effects
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON
            return shape(inert_int, effects, None, valid)
        if method in {"startswith", "endswith"}:
            valid = (
                arguments_complete and not node.keywords and 1 <= len(positional) <= 3
            )
            callback_free = bool(positional) and all(
                _index_callback_free(bound) for bound in positional[1:]
            )
            if (
                receiver.kind in {"bytes", "bytearray"}
                and positional
                and not _buffer_argument_callback_free(positional[0], tuple_items=True)
            ):
                callback_free = False
            effects = base_effects
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON
            return shape(inert_bool, effects, None, valid)
        if method == "replace":
            valid = (
                arguments_complete and not node.keywords and 2 <= len(positional) <= 3
            )
            callback_free = len(positional) < 3 or _index_callback_free(positional[2])
            if receiver.kind in {"bytes", "bytearray"} and any(
                not _buffer_argument_callback_free(argument)
                for argument in positional[:2]
            ):
                callback_free = False
            effects = base_effects | ALLOCATES
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON
            return shape(
                _normal_result(receiver.kind, release_may_call=False),
                effects,
                None,
                valid,
            )
        if method == "join":
            valid = arguments_complete and len(positional) == 1 and not node.keywords
            source = positional[0] if positional else UNKNOWN_EXPRESSION_RESULT
            incoming = iterable_element_result(source)
            expected = {"str"} if receiver.kind == "str" else {"bytes", "bytearray"}
            iteration_callback_free = _iteration_callback_free(source)
            element_callback_free = incoming is not None and (
                incoming.kind in expected
                if receiver.kind == "str"
                else _buffer_argument_callback_free(incoming)
            )
            callback_free = iteration_callback_free and element_callback_free
            effects = base_effects | ALLOCATES
            if not callback_free:
                effects |= EXECUTES_ARBITRARY_PYTHON
                if not iteration_callback_free:
                    effects |= INVOKES_ITERATION_CALLBACK
            return shape(
                _normal_result(receiver.kind, release_may_call=False),
                effects,
                None,
                valid,
            )
    return None


__all__ = [
    "BUILTIN_SHAPE_NAMES",
    "BuiltinCallShape",
    "BuiltinMethodCallShape",
    "builtin_call_shape",
    "builtin_method_descriptor_known",
    "builtin_method_call_shape",
]
