"""CallNamedBuiltinConstructorDispatchMixin: named builtin call lowering authority."""

from __future__ import annotations

import ast
from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object

from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED


class CallNamedBuiltinConstructorDispatchMixin(_MixinBase):
    def _try_emit_named_builtin_constructor_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id == "pow":
            if node.keywords:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "pow does not support keywords"
                )
            if len(node.args) not in (2, 3):
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "pow expects 2 or 3 arguments"
                )
            base = self.visit(node.args[0])
            exp = self.visit(node.args[1])
            if base is None or exp is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported pow inputs"
                )
            if len(node.args) == 2:
                if "complex" in {base.type_hint, exp.type_hint}:
                    res_type = "complex"
                elif "float" in {base.type_hint, exp.type_hint}:
                    res_type = "float"
                else:
                    res_type = "Unknown"
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(MoltOp(kind="POW", args=[base, exp], result=res))
                return res
            mod = self.visit(node.args[2])
            if mod is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported pow mod input"
                )
            int_like = {"int", "bool"}
            res_type = (
                "int"
                if {
                    base.type_hint,
                    exp.type_hint,
                    mod.type_hint,
                }.issubset(int_like)
                else "Unknown"
            )
            res = MoltValue(self.next_var(), type_hint=res_type)
            self.emit(MoltOp(kind="POW_MOD", args=[base, exp, mod], result=res))
            return res
        if func_id == "round":
            if node.keywords:
                callee = self.visit(node.func)
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported call target"
                    )
                return self._emit_dynamic_call(node, callee)
            if len(node.args) not in (1, 2):
                callee = self.visit(node.func)
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported call target"
                    )
                return self._emit_dynamic_call(node, callee)
            value = self.visit(node.args[0])
            if value is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported round input"
                )
            if len(node.args) == 2:
                ndigits = self.visit(node.args[1])
                if ndigits is None:
                    ndigits = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=ndigits))
                has_ndigits = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_ndigits))
                if value.type_hint == "float":
                    res_type = "float"
                elif value.type_hint in {"int", "bool"}:
                    res_type = "int"
                else:
                    res_type = "Unknown"
            else:
                ndigits = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=ndigits))
                has_ndigits = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_ndigits))
                res_type = (
                    "int" if value.type_hint in {"int", "bool", "float"} else "Unknown"
                )
            res = MoltValue(self.next_var(), type_hint=res_type)
            self.emit(
                MoltOp(kind="ROUND", args=[value, ndigits, has_ndigits], result=res)
            )
            return res
        if func_id == "memoryview":
            if len(node.args) != 1:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "memoryview expects 1 argument"
                )
            arg = self.visit(node.args[0])
            res = MoltValue(self.next_var(), type_hint="memoryview")
            self.emit(MoltOp(kind="MEMORYVIEW_NEW", args=[arg], result=res))
            return res
        return CALL_NOT_HANDLED
