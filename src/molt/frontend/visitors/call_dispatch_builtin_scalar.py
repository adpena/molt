"""CallNamedBuiltinScalarDispatchMixin: named builtin call lowering authority."""

from __future__ import annotations

import ast
from typing import (
    Any,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection


from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend._mixin_base import GeneratorMixinBase


class CallNamedBuiltinScalarDispatchMixin(GeneratorMixinBase):
    def _try_emit_named_builtin_scalar_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id == "type":
            if node.keywords or len(node.args) != 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported call target"
                    )
                return self._emit_dynamic_call(node, callee)
            arg = self.visit(node.args[0])
            res = MoltValue(self.next_var(), type_hint="type")
            self.emit(MoltOp(kind="TYPE_OF", args=[arg], result=res))
            return res
        if func_id == "isinstance":
            if len(node.args) != 2:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "isinstance expects 2 arguments"
                )
            obj = self.visit(node.args[0])
            clsinfo = self.visit(node.args[1])
            if obj is None or clsinfo is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported isinstance arguments"
                )
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISINSTANCE", args=[obj, clsinfo], result=res))
            return res
        if func_id == "issubclass":
            if len(node.args) != 2:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "issubclass expects 2 arguments"
                )
            sub = self.visit(node.args[0])
            clsinfo = self.visit(node.args[1])
            if sub is None or clsinfo is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported issubclass arguments"
                )
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISSUBCLASS", args=[sub, clsinfo], result=res))
            return res
        if func_id == "object":
            if node.args:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "object expects 0 arguments"
                )
            res = MoltValue(self.next_var(), type_hint="object")
            self.emit(MoltOp(kind="OBJECT_NEW", args=[], result=res))
            return res
        if func_id == "id":
            if node.keywords or len(node.args) != 1:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "id expects 1 argument"
                )
            arg = self.visit(node.args[0])
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported id argument"
                )
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ID", args=[arg], result=res))
            return res
        if func_id == "ord":
            if node.keywords or len(node.args) != 1:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "ord expects 1 argument"
                )
            raw_arg = node.args[0]
            if isinstance(raw_arg, ast.Subscript) and not isinstance(
                raw_arg.slice, ast.Slice
            ):
                target = self.visit(raw_arg.value)
                index_val = self.visit(raw_arg.slice)
                if target is None or index_val is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE,
                        "Unsupported ord subscript argument",
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ORD_AT", args=[target, index_val], result=res))
                return res
            arg = self.visit(node.args[0])
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported ord argument"
                )
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ORD", args=[arg], result=res))
            return res
        if func_id == "chr":
            if node.keywords or len(node.args) != 1:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "chr expects 1 argument"
                )
            arg = self.visit(node.args[0])
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported chr argument"
                )
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CHR", args=[arg], result=res))
            return res
        if func_id == "repr":
            if node.keywords or len(node.args) != 1:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "repr expects 1 argument"
                )
            arg = self.visit(node.args[0])
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported repr argument"
                )
            return self._emit_repr_from_obj(arg)
        if func_id == "callable":
            if node.keywords or len(node.args) != 1:
                raise FrontendRejection(
                    Diagnostic.CALL_SIGNATURE, "callable expects 1 argument"
                )
            arg = self.visit(node.args[0])
            if arg is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported callable argument"
                )
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS_CALLABLE", args=[arg], result=res))
            return res
        return CALL_NOT_HANDLED
