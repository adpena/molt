"""Exact builtin split dispatch with a retained, source-ordered call target.

An unknown receiver is not retained after generic descriptor lookup. The captured
target is either an exact builtin receiver or the actual descriptor result, with
an inert tag distinguishing them. Arguments are evaluated once after capture.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING, Any

from molt.frontend._types import MoltOp, MoltValue
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


# This is an emission map, not a source-type classifier. Admission is exact
# source-point authority or runtime class identity, never storage tags/hints.
_SPLIT_OPS = {
    "str": "STRING_SPLIT",
    "bytes": "BYTES_SPLIT",
    "bytearray": "BYTEARRAY_SPLIT",
}
_SPLIT_KINDS = tuple(_SPLIT_OPS)


class CallSplitDispatchMixin(_MixinBase):
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
        exact_kind = self._builtin_exact_type_from_expr(node.func.value)
        if exact_kind in _SPLIT_KINDS:
            tag = None
            target = receiver
        else:
            receiver = MoltValue(receiver.name, type_hint="Any")
            actual_type = MoltValue(self.next_var(), type_hint="type")
            self.emit(MoltOp(kind="TYPE_OF", args=[receiver], result=actual_type))
            tag, target = self._capture_split_target(node.func, receiver, actual_type)

        expressions = [*node.args, *(keyword.value for keyword in node.keywords)]
        suspends = self.is_async() and any(
            self._expr_may_yield(expression) for expression in expressions
        )
        target_cell = (
            self._new_scratch_cell(target, type_hint=target.type_hint)
            if suspends
            else None
        )
        tag_cell = (
            self._new_scratch_cell(tag, type_hint="int")
            if suspends and tag is not None
            else None
        )
        values = self._emit_call_args(expressions)
        if target_cell is not None:
            target = self._consume_scratch_cell(target_cell)
        if tag_cell is not None:
            tag = self._consume_scratch_cell(tag_cell)

        arguments = dict(zip(parameters, values[: len(node.args)]))
        arguments.update(zip(keyword_names, values[len(node.args) :], strict=True))
        separator = arguments.get("sep")
        if separator is None:
            separator = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=separator))
        maxsplit = arguments.get("maxsplit")
        if tag is None:
            assert exact_kind is not None
            return self._emit_split_intrinsic(
                _SPLIT_KINDS.index(exact_kind), target, separator, maxsplit
            )
        return self._dispatch_split_target(
            node, tag, target, values, separator, maxsplit
        )

    def _capture_split_target(
        self,
        attribute: ast.Attribute,
        receiver: MoltValue,
        actual_type: MoltValue,
        index: int = 0,
    ) -> tuple[MoltValue, MoltValue]:
        if index == len(_SPLIT_KINDS):
            # No annotation, spelling, or stale SSA hint can replace lookup.
            target = self._emit_attribute_load(
                attribute, receiver, None, None, generic=True
            )
            tag = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=tag))
            return tag, target
        expected_type = self._emit_builtin_type_value(_SPLIT_KINDS[index])
        matches = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[actual_type, expected_type], result=matches))
        merge = self._new_condition_merge(2, ())
        self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
        tag = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[index + 1], result=tag))
        direct = self._store_condition_branch(merge, (tag, receiver))
        self._condition_else(merge)
        fallback = self._store_condition_branch(
            merge,
            self._capture_split_target(attribute, receiver, actual_type, index + 1),
        )
        merged = self._finish_condition_merge(merge, direct, fallback)
        return merged[0], merged[1]

    def _emit_split_intrinsic(
        self,
        index: int,
        receiver: MoltValue,
        separator: MoltValue,
        maxsplit: MoltValue | None,
    ) -> MoltValue:
        arguments = [receiver, separator]
        opcode = _SPLIT_OPS[_SPLIT_KINDS[index]]
        if maxsplit is not None:
            arguments.append(maxsplit)
            opcode += "_MAX"
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
            result = MoltValue(self.next_var(), type_hint="Any")
            if node.keywords:
                callargs = self._emit_call_args_builder(node, evaluated=tuple(values))
                self.emit(
                    MoltOp(kind="CALL_INDIRECT", args=[target, callargs], result=result)
                )
            else:
                self.emit(
                    MoltOp(kind="CALL_FUNC", args=[target, *values], result=result)
                )
            return result
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
