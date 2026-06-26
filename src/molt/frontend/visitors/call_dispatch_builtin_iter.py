"""CallNamedBuiltinIterDispatchMixin: named builtin call lowering authority."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    BUILTIN_FUNC_SPECS,
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


class CallNamedBuiltinIterDispatchMixin(_MixinBase):
    def _try_emit_named_builtin_iter_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id == "range":
            if node.keywords:
                for keyword in node.keywords:
                    val = self.visit(keyword.value)
                    if val is None:
                        raise NotImplementedError("Unsupported range keyword")
                return self._emit_type_error_value(
                    "range() takes no keyword arguments", "range"
                )
            range_args = self._parse_range_call(node)
            if range_args is None:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            start, stop, step, _lowerable = range_args
            res = MoltValue(self.next_var(), type_hint="range")
            self.emit(MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res))
            return res
        if func_id == "enumerate":
            if len(node.args) > 2:
                raise NotImplementedError("enumerate expects 1 or 2 arguments")
            if node.keywords:
                for keyword in node.keywords:
                    if keyword.arg is None:
                        raise NotImplementedError("enumerate does not support **kwargs")
                    if keyword.arg != "start":
                        raise NotImplementedError(
                            f"enumerate got unexpected keyword {keyword.arg}"
                        )
            iterable = self.visit(node.args[0]) if node.args else None
            if iterable is None:
                raise NotImplementedError("Unsupported enumerate iterable")
            start_val = None
            has_start = False
            if len(node.args) == 2:
                start_val = self.visit(node.args[1])
                has_start = True
            for keyword in node.keywords:
                if keyword.arg == "start":
                    if has_start:
                        raise NotImplementedError(
                            "enumerate got multiple values for start"
                        )
                    start_val = self.visit(keyword.value)
                    has_start = True
            if start_val is None:
                start_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
            has_start_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[has_start], result=has_start_val))
            res = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="ENUMERATE",
                    args=[iterable, start_val, has_start_val],
                    result=res,
                )
            )
            return res
        if func_id == "slice":
            if len(node.args) not in (1, 2, 3):
                raise NotImplementedError("slice expects 1-3 arguments")
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
                raise NotImplementedError("aiter expects 1 argument")
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError("Unsupported iterable in aiter()")
            return self._emit_aiter(iterable)
        if func_id == "anext":
            if node.keywords or len(node.args) not in (1, 2):
                raise NotImplementedError("anext expects 1 or 2 positional arguments")
            iter_obj = self.visit(node.args[0])
            if iter_obj is None:
                raise NotImplementedError("Unsupported iterator in anext()")
            if len(node.args) == 2:
                default_val = self.visit(node.args[1])
                if default_val is None:
                    raise NotImplementedError("Unsupported default in anext()")
                placeholder = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="CALL_ASYNC",
                        args=[
                            "molt_anext_default_poll",
                            iter_obj,
                            default_val,
                            placeholder,
                        ],
                        result=res,
                    )
                )
                return res
            res = MoltValue(self.next_var(), type_hint="Future")
            self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=res))
            return res
        if func_id in {"any", "all"}:
            return self._emit_any_all_call(func_id, node, needs_bind)
        if func_id == "sum":
            return self._emit_sum_call(func_id, node, needs_bind)
        if func_id == "map":
            if any(isinstance(arg, ast.Starred) for arg in node.args) or node.keywords:
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
                return res
            if len(node.args) < 2:
                return self._emit_type_error_value(
                    "map() must have at least two arguments"
                )
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            callargs = self._emit_call_args_builder(node)
            self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            return res
        if func_id == "zip":
            if any(isinstance(arg, ast.Starred) for arg in node.args) or node.keywords:
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
                return res
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            callargs = self._emit_call_args_builder(node)
            self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            return res
        if func_id in {"min", "max"}:
            if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                kw.arg is None for kw in node.keywords
            ):
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res
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
                callee = self._emit_runtime_function(runtime_name, 3)
                arg_vals: list[MoltValue] = []
                for expr in node.args:
                    arg_val = self.visit(expr)
                    if arg_val is None:
                        raise NotImplementedError(
                            f"Unsupported {func_id} positional argument"
                        )
                    arg_vals.append(arg_val)
                args_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=arg_vals, result=args_tuple))
                key_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
                default_val = self._emit_missing_value()
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, args_tuple, key_val, default_val],
                        result=res,
                    )
                )
            return res
        if func_id == "sorted":
            if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                kw.arg is None for kw in node.keywords
            ):
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res
            if not node.args:
                return self._emit_type_error_value("sorted expected 1 argument, got 0")
            if len(node.args) > 1:
                return self._emit_type_error_value(
                    f"sorted expected 1 argument, got {len(node.args)}"
                )
            key_expr = None
            reverse_expr = None
            for keyword in node.keywords:
                if keyword.arg not in {"key", "reverse"}:
                    msg = f"sorted() got an unexpected keyword argument '{keyword.arg}'"
                    return self._emit_type_error_value(msg)
                if keyword.arg == "key":
                    if key_expr is not None:
                        return self._emit_type_error_value(
                            "sorted() got multiple values for argument 'key'"
                        )
                    key_expr = keyword.value
                else:
                    if reverse_expr is not None:
                        return self._emit_type_error_value(
                            "sorted() got multiple values for argument 'reverse'"
                        )
                    reverse_expr = keyword.value
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError("Unsupported sorted iterable")
            # Emit key argument (default: None)
            if key_expr is not None:
                key_val = self.visit(key_expr)
                if key_val is None:
                    raise NotImplementedError("Unsupported sorted key expression")
            else:
                key_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
            # Emit reverse argument (default: False)
            if reverse_expr is not None:
                reverse_val = self.visit(reverse_expr)
                if reverse_val is None:
                    raise NotImplementedError("Unsupported sorted reverse expression")
            else:
                reverse_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=reverse_val))
            self.emit(
                MoltOp(
                    kind="CALL_FUNC",
                    args=[callee, iterable, key_val, reverse_val],
                    result=res,
                )
            )
            return res
        if func_id == "iter":
            if node.keywords:
                return self._emit_type_error_value(
                    "iter() takes no keyword arguments", "iter"
                )
            if len(node.args) == 1:
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported iterable in iter()")
                return self._emit_iter_new(iterable)
            if len(node.args) == 2:
                callable_val = self.visit(node.args[0])
                sentinel_val = self.visit(node.args[1])
                if callable_val is None or sentinel_val is None:
                    raise NotImplementedError("Unsupported iter() arguments")
                callee = MoltValue(self.next_var(), type_hint="function")
                self.emit(
                    MoltOp(
                        kind="BUILTIN_FUNC",
                        args=["molt_iter_sentinel", 2],
                        result=callee,
                    )
                )
                self._emit_function_metadata(
                    callee,
                    name="iter",
                    qualname="iter",
                    posonly_params=["callable", "sentinel"],
                    pos_or_kw_params=[],
                    kwonly_params=[],
                    vararg=None,
                    varkw=None,
                    default_exprs=[],
                    kw_default_exprs=[],
                    docstring=None,
                    module_override="builtins",
                )
                res = MoltValue(self.next_var(), type_hint="iter")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, callable_val, sentinel_val],
                        result=res,
                    )
                )
                return res
            if not node.args:
                return self._emit_type_error_value(
                    "iter expected 1 argument, got 0", "iter"
                )
            msg = f"iter expected at most 2 arguments, got {len(node.args)}"
            return self._emit_type_error_value(msg, "iter")
        return CALL_NOT_HANDLED
