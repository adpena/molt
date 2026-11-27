"""Exact builtin split dispatch with a retained, source-ordered call target.

An unknown receiver is not retained after generic descriptor lookup. The captured
target is either an exact builtin receiver or the actual descriptor result, with
an inert tag distinguishing them. Arguments are evaluated once after capture.
"""

from __future__ import annotations

from molt.frontend.lowering.op_kinds_generated import FrontendOpKind

import ast
from collections.abc import Callable
from typing import Any

from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import MoltOp, MoltValue
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED


# This is an emission map, not a source-type classifier. Admission is exact
# source-point authority or runtime class identity, never storage tags/hints.
_SPLIT_OPS: dict[str, FrontendOpKind] = {
    "str": "STRING_SPLIT",
    "bytes": "BYTES_SPLIT",
    "bytearray": "BYTEARRAY_SPLIT",
}
_SPLIT_MAX_OPS: dict[str, FrontendOpKind] = {
    "str": "STRING_SPLIT_MAX",
    "bytes": "BYTES_SPLIT_MAX",
    "bytearray": "BYTEARRAY_SPLIT_MAX",
}
_SPLIT_KINDS = tuple(_SPLIT_OPS)


class CallSplitDispatchMixin(GeneratorMixinBase):
    def _try_emit_split_call(self, node: ast.Call) -> Any:
        if not isinstance(node.func, ast.Attribute) or node.func.attr != "split":
            return CALL_NOT_HANDLED
        parameters = ("sep", "maxsplit")
        if len(node.args) > len(parameters) or any(
            isinstance(argument, ast.Starred) for argument in node.args
        ):
            return CALL_NOT_HANDLED
        supplied = set(parameters[: len(node.args)])
        keyword_names: list[str] = []
        for keyword in node.keywords:
            name = keyword.arg
            if name is None or name not in parameters or name in supplied:
                # The real callable owns signature errors, after evaluation of
                # all supplied arguments. This also covers arbitrary **mapping.
                return CALL_NOT_HANDLED
            supplied.add(name)
            keyword_names.append(name)

        receiver = self.visit(node.func.value)
        if receiver is None:
            raise FrontendRejection(
                Diagnostic.CALL_TARGET, "Unsupported attribute call receiver"
            )
        expressions = [*node.args, *(keyword.value for keyword in node.keywords)]

        def evaluate_arguments() -> tuple[list[MoltValue], MoltValue, MoltValue | None]:
            values = self._emit_call_args(expressions)
            arguments = dict(zip(parameters, values[: len(node.args)]))
            arguments.update(zip(keyword_names, values[len(node.args) :], strict=True))
            separator = arguments.get("sep")
            if separator is None:
                separator = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=separator))
            return values, separator, arguments.get("maxsplit")

        exact_kind = self._builtin_exact_type_from_expr(node.func.value)
        if exact_kind in _SPLIT_KINDS:
            assert exact_kind is not None
            _, separator, maxsplit = evaluate_arguments()
            return self._emit_split_intrinsic(
                _SPLIT_KINDS.index(exact_kind), receiver, separator, maxsplit
            )

        receiver = MoltValue(receiver.name, type_hint="Any")
        actual_type = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="TYPE_OF", args=[receiver], result=actual_type))
        # This finite set lowers to immutable code constants. Their evaluation
        # executes no Python code, so directly invoke each selected target
        # without carrying a discriminator between selection and invocation.
        if all(
            isinstance(expression, ast.Constant)
            and type(expression.value) in {str, bytes, int, bool, float, type(None)}
            for expression in expressions
        ):
            values, separator, maxsplit = evaluate_arguments()

            def invoke(index: int, target: MoltValue) -> tuple[MoltValue, ...]:
                if index == len(_SPLIT_KINDS):
                    return (self._emit_generic_split_call(node, target, values),)
                return (self._emit_split_intrinsic(index, target, separator, maxsplit),)

            return self._select_split_target(
                node.func, receiver, actual_type, invoke, 1
            )[0]

        def capture(index: int, target: MoltValue) -> tuple[MoltValue, ...]:
            tag = MoltValue(self.next_var(), type_hint="int")
            discriminator = 0 if index == len(_SPLIT_KINDS) else index + 1
            self.emit(MoltOp(kind="CONST", args=[discriminator], result=tag))
            return tag, target

        tag, target = self._select_split_target(
            node.func, receiver, actual_type, capture, 2
        )
        values, separator, maxsplit = evaluate_arguments()
        return self._dispatch_split_target(
            node, tag, target, values, separator, maxsplit
        )

    def _select_split_target(
        self,
        attribute: ast.Attribute,
        receiver: MoltValue,
        actual_type: MoltValue,
        select: Callable[[int, MoltValue], tuple[MoltValue, ...]],
        result_count: int,
        index: int = 0,
    ) -> tuple[MoltValue, ...]:
        if index == len(_SPLIT_KINDS):
            # No annotation, spelling, or stale SSA hint can replace lookup.
            target = self._emit_attribute_load(
                attribute, receiver, None, None, generic=True
            )
            return select(index, target)
        expected_type = self._emit_builtin_type_value(_SPLIT_KINDS[index])
        matches = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[actual_type, expected_type], result=matches))
        merge = self._new_condition_merge(result_count, ())
        self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
        direct = self._store_condition_branch(merge, select(index, receiver))
        self._condition_else(merge)
        fallback = self._store_condition_branch(
            merge,
            self._select_split_target(
                attribute, receiver, actual_type, select, result_count, index + 1
            ),
        )
        return self._finish_condition_merge(merge, direct, fallback)

    def _emit_generic_split_call(
        self, node: ast.Call, target: MoltValue, values: list[MoltValue]
    ) -> MoltValue:
        result = MoltValue(self.next_var(), type_hint="Any")
        if node.keywords:
            callargs = self._emit_call_args_builder(node, evaluated=tuple(values))
            self.emit(
                MoltOp(kind="CALL_INDIRECT", args=[target, callargs], result=result)
            )
        else:
            self.emit(MoltOp(kind="CALL_FUNC", args=[target, *values], result=result))
        return result

    def _emit_split_intrinsic(
        self,
        index: int,
        receiver: MoltValue,
        separator: MoltValue,
        maxsplit: MoltValue | None,
    ) -> MoltValue:
        arguments = [receiver, separator]
        receiver_kind = _SPLIT_KINDS[index]
        if maxsplit is None:
            opcode = _SPLIT_OPS[receiver_kind]
        else:
            arguments.append(maxsplit)
            opcode = _SPLIT_MAX_OPS[receiver_kind]
        result = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind=opcode, args=arguments, result=result))
        self._record_container_elem_hint(result, _SPLIT_KINDS[index])
        return result

    def _dispatch_split_target(
        self,
        node: ast.Call,
        tag: MoltValue,
        target: MoltValue,
        values: list[MoltValue],
        separator: MoltValue,
        maxsplit: MoltValue | None,
        index: int = 0,
    ) -> MoltValue:
        if index == len(_SPLIT_KINDS):
            return self._emit_generic_split_call(node, target, values)
        expected = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[index + 1], result=expected))
        matches = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[tag, expected], result=matches))
        merge = self._new_condition_merge(1, ())
        self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
        direct = self._store_condition_branch(
            merge, (self._emit_split_intrinsic(index, target, separator, maxsplit),)
        )
        self._condition_else(merge)
        fallback = self._store_condition_branch(
            merge,
            (
                self._dispatch_split_target(
                    node, tag, target, values, separator, maxsplit, index + 1
                ),
            ),
        )
        return self._finish_condition_merge(merge, direct, fallback)[0]
