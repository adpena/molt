"""ExpressionPrimitivesMixin: shared expression and call emission helpers.

Move-only extraction from frontend/__init__.py. This lowering authority owns
cross-consumer expression-list evaluation, primitive bool/compare/containment
emission, int-array iterable adaptation, molt_buffer call parsing, any/all
genexpr shape checks, and bound/function call normalization.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING

from molt.frontend._types import MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class ExpressionPrimitivesMixin(_MixinBase):
    def _emit_expr_list(self, exprs: list[ast.expr]) -> list[MoltValue]:
        if not exprs:
            return []
        if not self.is_async():
            values: list[MoltValue] = []
            for expr in exprs:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported expression")
                values.append(val)
            return values
        yield_flags = [self._expr_may_yield(expr) for expr in exprs]
        if not any(yield_flags):
            values = []
            for expr in exprs:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported expression")
                values.append(val)
            return values
        values = []
        spills: list[tuple[int, int, str]] = []
        for idx, expr in enumerate(exprs):
            val = self.visit(expr)
            if val is None:
                raise NotImplementedError("Unsupported expression")
            values.append(val)
            if any(yield_flags[idx + 1 :]):
                slot = self._spill_async_value(
                    val, f"__expr_spill_{len(self.async_locals)}"
                )
                spills.append((idx, slot, val.type_hint))
        for idx, slot, hint in spills:
            values[idx] = self._reload_async_value(slot, hint)
        return values

    def _emit_intarray_from_seq(self, seq: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="intarray")
        self.emit(MoltOp(kind="INTARRAY_FROM_SEQ", args=[seq], result=res))
        self.container_elem_hints[res.name] = "int"
        return res

    def _is_flat_list_int_container(self, value: MoltValue) -> bool:
        return value.name in getattr(self, "_list_int_containers", set())

    def _emit_not(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[value], result=res))
        return res

    def _emit_contains(self, container: MoltValue, item: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONTAINS", args=[container, item], result=res))
        return res

    def _emit_compare_op(
        self, op: ast.cmpop, left: MoltValue, right: MoltValue
    ) -> MoltValue:
        if isinstance(op, ast.Eq):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="EQ", args=[left, right], result=res))
            return res
        if isinstance(op, ast.NotEq):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Lt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Gt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="GT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.LtE):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.GtE):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="GE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Is):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[left, right], result=res))
            return res
        if isinstance(op, ast.IsNot):
            is_val = self._emit_compare_op(ast.Is(), left, right)
            return self._emit_not(is_val)
        if isinstance(op, ast.In):
            return self._emit_contains(right, left)
        if isinstance(op, ast.NotIn):
            in_val = self._emit_contains(right, left)
            return self._emit_not(in_val)
        raise NotImplementedError("Comparison operator not supported")

    def _parse_molt_buffer_call(
        self, node: ast.Call, name: str
    ) -> list[ast.expr] | None:
        if (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "molt_buffer"
            and node.func.attr == name
        ):
            return node.args
        return None

    @staticmethod
    def _can_inline_any_all_genexpr(node: ast.GeneratorExp) -> bool:
        return (
            len(node.generators) == 1
            and not node.generators[0].is_async
            and isinstance(node.generators[0].target, ast.Name)
        )

    def _emit_call_bound_or_func(
        self, callee: MoltValue, args: list[MoltValue]
    ) -> MoltValue:
        # Use CALL_FUNC to centralize bound-method handling and keep async IR linear.
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
        return res
