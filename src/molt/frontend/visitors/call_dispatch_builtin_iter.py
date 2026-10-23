"""CallNamedBuiltinIterDispatchMixin: named builtin call lowering authority."""

from __future__ import annotations

import ast
from typing import (
    Any,
)

from molt.builtin_callables import BUILTIN_FUNC_SPECS
from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection


from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend._mixin_base import GeneratorMixinBase


class CallNamedBuiltinIterDispatchMixin(GeneratorMixinBase):
    def _try_emit_named_builtin_iter_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id == "slice":
            if len(node.args) not in (1, 2, 3):
                return CALL_NOT_HANDLED
            if len(node.args) == 1:
                start = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                stop = self.visit(node.args[0])
                step = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
            elif len(node.args) == 2:
                start = self.visit(node.args[0])
                stop = self.visit(node.args[1])
                step = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
            else:
                start = self.visit(node.args[0])
                stop = self.visit(node.args[1])
                step = self.visit(node.args[2])
            res = MoltValue(self.next_var(), type_hint="slice")
            self.emit(MoltOp(kind="SLICE_NEW", args=[start, stop, step], result=res))
            return res
        if func_id == "aiter":
            if len(node.args) != 1:
                return CALL_NOT_HANDLED
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported iterable in aiter()"
                )
            return self._emit_aiter(iterable)
        if func_id == "anext":
            if node.keywords or len(node.args) not in (1, 2):
                return CALL_NOT_HANDLED
            iter_obj = self.visit(node.args[0])
            if iter_obj is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported iterator in anext()"
                )
            if len(node.args) == 2:
                default_val = self.visit(node.args[1])
                if default_val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "Unsupported default in anext()",
                    )
                return self._emit_runtime_call(
                    BUILTIN_FUNC_SPECS[func_id].runtime,
                    [iter_obj, default_val],
                    type_hint="Future",
                )
            res = MoltValue(self.next_var(), type_hint="Future")
            self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=res))
            return res
        if func_id in {"any", "all"}:
            return self._emit_any_all_call(func_id, node, needs_bind)
        if func_id == "sum":
            return self._emit_sum_call(func_id, node, needs_bind)
        if func_id in {"map", "zip"}:
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            callargs = self._emit_call_args_builder(node)
            self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            return res
        if func_id in {"min", "max"}:
            if not node.args:
                return self._emit_type_error_value(
                    f"{func_id} expected at least 1 argument, got 0"
                )
            key_expr = None
            default_expr = None
            for keyword in node.keywords:
                if keyword.arg not in {"key", "default"}:
                    msg = (
                        f"{func_id}() got an unexpected keyword argument "
                        f"'{keyword.arg}'"
                    )
                    return self._emit_type_error_value(msg)
                if keyword.arg == "key":
                    if key_expr is not None:
                        return self._emit_type_error_value(
                            f"{func_id}() got multiple values for argument 'key'"
                        )
                    key_expr = keyword.value
                else:
                    if default_expr is not None:
                        return self._emit_type_error_value(
                            f"{func_id}() got multiple values for argument 'default'"
                        )
                    default_expr = keyword.value
            if len(node.args) > 1 and default_expr is not None:
                msg = (
                    f"Cannot specify a default for {func_id}() with "
                    "multiple positional arguments"
                )
                return self._emit_type_error_value(msg)
            res = MoltValue(self.next_var(), type_hint="Any")
            if node.keywords:
                callee = self._emit_builtin_function(func_id)
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            else:
                runtime_name = BUILTIN_FUNC_SPECS[func_id].runtime
                arg_vals: list[MoltValue] = []
                for expr in node.args:
                    arg_val = self.visit(expr)
                    if arg_val is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            f"Unsupported {func_id} positional argument",
                        )
                    arg_vals.append(arg_val)
                args_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=arg_vals, result=args_tuple))
                key_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
                default_val = self._emit_missing_value()
                res = self._emit_runtime_call(
                    runtime_name,
                    [args_tuple, key_val, default_val],
                )
            return res
        if func_id == "iter":
            if node.keywords or len(node.args) not in (1, 2):
                # The generic binder evaluates every argument before rejecting
                # an invalid shape, preserving effects and raised exceptions.
                return CALL_NOT_HANDLED
            if len(node.args) == 1:
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "Unsupported iterable in iter()",
                    )
                return self._emit_iter_new(iterable)
            if len(node.args) == 2:
                callable_val = self.visit(node.args[0])
                sentinel_val = self.visit(node.args[1])
                if callable_val is None or sentinel_val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "Unsupported iter() arguments"
                    )
                return self._emit_runtime_call(
                    BUILTIN_FUNC_SPECS[func_id].runtime,
                    [callable_val, sentinel_val],
                    type_hint="iter",
                )
        return CALL_NOT_HANDLED
