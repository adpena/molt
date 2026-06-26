"""CallNamedBuiltinScalarDispatchMixin: named builtin call lowering authority."""

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

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object

from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED


class CallNamedBuiltinScalarDispatchMixin(_MixinBase):
    def _try_emit_named_builtin_scalar_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id == "type":
            if node.keywords or len(node.args) != 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            arg = self.visit(node.args[0])
            res = MoltValue(self.next_var(), type_hint="type")
            self.emit(MoltOp(kind="TYPE_OF", args=[arg], result=res))
            return res
        if func_id == "isinstance":
            if len(node.args) != 2:
                raise NotImplementedError("isinstance expects 2 arguments")
            obj = self.visit(node.args[0])
            clsinfo = self.visit(node.args[1])
            if obj is None or clsinfo is None:
                raise NotImplementedError("Unsupported isinstance arguments")
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISINSTANCE", args=[obj, clsinfo], result=res))
            return res
        if func_id == "issubclass":
            if len(node.args) != 2:
                raise NotImplementedError("issubclass expects 2 arguments")
            sub = self.visit(node.args[0])
            clsinfo = self.visit(node.args[1])
            if sub is None or clsinfo is None:
                raise NotImplementedError("Unsupported issubclass arguments")
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISSUBCLASS", args=[sub, clsinfo], result=res))
            return res
        if func_id == "object":
            if node.args:
                raise NotImplementedError("object expects 0 arguments")
            res = MoltValue(self.next_var(), type_hint="object")
            self.emit(MoltOp(kind="OBJECT_NEW", args=[], result=res))
            return res
        if func_id == "len":
            if node.keywords:
                raise NotImplementedError("len does not support keywords")
            if len(node.args) != 1:
                from molt.compat import CompatibilityIssue

                issue = CompatibilityIssue(
                    feature="len() argument count",
                    tier="unsupported",
                    impact="high",
                    location=f"line {node.lineno}",
                    detail=f"len() takes exactly one argument ({len(node.args)} given)",
                )
                raise NotImplementedError(issue.format_error())
            # Constant-fold len() on string/bytes literals and
            # list/tuple literals with all-constant elements.
            raw_arg = node.args[0]
            if isinstance(raw_arg, ast.Constant) and isinstance(
                raw_arg.value, (str, bytes)
            ):
                folded_len = len(raw_arg.value)
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[folded_len], result=res))
                return res
            if isinstance(raw_arg, (ast.List, ast.Tuple)) and all(
                isinstance(e, ast.Constant) for e in raw_arg.elts
            ):
                folded_len = len(raw_arg.elts)
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[folded_len], result=res))
                return res
            arg = self.visit(node.args[0])
            spec = self._intrinsic_handle_class_spec_for_value(arg)
            if spec is not None and spec.len_intrinsic is not None:
                return self._emit_intrinsic_handle_class_call(
                    arg,
                    spec,
                    spec.len_intrinsic,
                    [],
                    result_hint="int",
                )
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[arg], result=res))
            return res
        if func_id == "id":
            if node.keywords or len(node.args) != 1:
                raise NotImplementedError("id expects 1 argument")
            arg = self.visit(node.args[0])
            if arg is None:
                raise NotImplementedError("Unsupported id argument")
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ID", args=[arg], result=res))
            return res
        if func_id == "bool":
            if node.keywords or len(node.args) > 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if not node.args:
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=res))
                return res
            arg = self.visit(node.args[0])
            if arg is None:
                raise NotImplementedError("Unsupported bool argument")
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="BOOL", args=[arg], result=res))
            return res
        if func_id == "ord":
            if node.keywords or len(node.args) != 1:
                raise NotImplementedError("ord expects 1 argument")
            raw_arg = node.args[0]
            if isinstance(raw_arg, ast.Subscript) and not isinstance(
                raw_arg.slice, ast.Slice
            ):
                target = self.visit(raw_arg.value)
                index_val = self.visit(raw_arg.slice)
                if target is None or index_val is None:
                    raise NotImplementedError("Unsupported ord subscript argument")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ORD_AT", args=[target, index_val], result=res))
                return res
            arg = self.visit(node.args[0])
            if arg is None:
                raise NotImplementedError("Unsupported ord argument")
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ORD", args=[arg], result=res))
            return res
        if func_id == "chr":
            if node.keywords or len(node.args) != 1:
                raise NotImplementedError("chr expects 1 argument")
            arg = self.visit(node.args[0])
            if arg is None:
                raise NotImplementedError("Unsupported chr argument")
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CHR", args=[arg], result=res))
            return res
        if func_id == "repr":
            if node.keywords or len(node.args) != 1:
                raise NotImplementedError("repr expects 1 argument")
            arg = self.visit(node.args[0])
            if arg is None:
                raise NotImplementedError("Unsupported repr argument")
            return self._emit_repr_from_obj(arg)
        if func_id == "callable":
            if node.keywords or len(node.args) != 1:
                raise NotImplementedError("callable expects 1 argument")
            arg = self.visit(node.args[0])
            if arg is None:
                raise NotImplementedError("Unsupported callable argument")
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS_CALLABLE", args=[arg], result=res))
            return res
        if func_id == "str":
            # CPython str() signatures:
            #   str() → ''
            #   str(object) → str(object)
            #   str(object=x) → str(x)
            #   str(bytes, encoding) → decoded str
            #   str(bytes, encoding, errors) → decoded str
            #   str(bytes, encoding=..., errors=...) → decoded str
            kw_object = next(
                (kw.value for kw in node.keywords if kw.arg == "object"), None
            )
            kw_encoding = next(
                (kw.value for kw in node.keywords if kw.arg == "encoding"), None
            )
            known_kw = {"object", "encoding", "errors"}
            has_unsupported_kw = any(
                kw.arg not in known_kw for kw in node.keywords if kw.arg is not None
            )
            has_star_kw = any(kw.arg is None for kw in node.keywords)
            if has_unsupported_kw or has_star_kw or len(node.args) > 3:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            # str(bytes_obj, encoding[, errors]) — decode bytes to str
            # Fall through to dynamic call which the runtime handles via
            # the str() builtin's multi-arg path.
            has_encoding = len(node.args) >= 2 or kw_encoding is not None
            if has_encoding:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            # str() → ''
            if not node.args and kw_object is None:
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
                return res
            # str(object) or str(object=x)
            if node.args:
                arg = self.visit(node.args[0])
            elif kw_object is not None:
                arg = self.visit(kw_object)
            else:
                arg = None
            if arg is None:
                arg = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
            return self._emit_str_from_obj(arg)
        return CALL_NOT_HANDLED
