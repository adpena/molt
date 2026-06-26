"""CallNamedBuiltinDispatchMixin: named builtin call lowering."""

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


class CallNamedBuiltinDispatchMixin(_MixinBase):
    def _try_emit_named_builtin_call(
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
        if func_id == "list":
            if node.keywords or len(node.args) > 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if not node.args:
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
                return res
            range_args = self._parse_range_call(node.args[0])
            if range_args is not None:
                start, stop, step, lowerable = range_args
                if lowerable:
                    return self._emit_range_list(start, stop, step)
                range_obj = self._emit_range_obj_from_args(start, stop, step)
                return self._emit_list_from_iter(range_obj)
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError("Unsupported list input")
            return self._emit_list_from_iter(iterable)
        if func_id == "tuple":
            if node.keywords or len(node.args) > 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if not node.args:
                res = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=res))
                return res
            range_args = self._parse_range_call(node.args[0])
            if range_args is not None:
                start, stop, step, _lowerable = range_args
                range_obj = self._emit_range_obj_from_args(start, stop, step)
                return self._emit_tuple_from_iter(range_obj)
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError("Unsupported tuple input")
            if iterable.type_hint == "tuple":
                return iterable
            if iterable.type_hint == "list":
                res = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_FROM_LIST", args=[iterable], result=res))
                return res
            return self._emit_tuple_from_iter(iterable)
        if func_id == "dict":
            has_starargs = len(node.args) > 1 or any(
                isinstance(a, ast.Starred) for a in node.args
            )
            if has_starargs:
                # dict(*args, ...) must unpack star-args into positional
                # arguments at runtime, so route through CALL_BIND.
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            res = MoltValue(self.next_var(), type_hint="dict")
            if not node.args:
                self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
            else:
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported dict input")
                self.emit(MoltOp(kind="DICT_FROM_OBJ", args=[iterable], result=res))
            for kw in node.keywords:
                if kw.arg is None:
                    mapping = self.visit(kw.value)
                    if mapping is None:
                        raise NotImplementedError("Unsupported dict ** input")
                    self.emit(
                        MoltOp(
                            kind="DICT_UPDATE_KWSTAR",
                            args=[res, mapping],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    key = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[kw.arg], result=key))
                    val = self.visit(kw.value)
                    if val is None:
                        raise NotImplementedError("Unsupported dict kw value")
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[res, key, val],
                            result=MoltValue("none"),
                        )
                    )
            return res
        if func_id == "float":
            if node.keywords or len(node.args) > 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if not node.args:
                res = MoltValue(self.next_var(), type_hint="float")
                self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=res))
                return res
            value = self.visit(node.args[0])
            if value is None:
                raise NotImplementedError("Unsupported float input")
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="FLOAT_FROM_OBJ", args=[value], result=res))
            return res
        if func_id == "complex":
            if any(kw.arg is None for kw in node.keywords) or len(node.args) > 2:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            kw_real = 0
            kw_imag = 0
            invalid_kw = False
            for kw in node.keywords:
                if kw.arg == "real":
                    kw_real += 1
                elif kw.arg == "imag":
                    kw_imag += 1
                else:
                    invalid_kw = True
                    break
            if (
                invalid_kw
                or kw_real > 1
                or kw_imag > 1
                or (kw_real > 0 and len(node.args) >= 1)
                or (kw_imag > 0 and len(node.args) >= 2)
            ):
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            real_val: MoltValue | None = None
            imag_val: MoltValue | None = None
            if node.args:
                real_val = self.visit(node.args[0])
                if real_val is None:
                    raise NotImplementedError("Unsupported complex real input")
            if len(node.args) == 2:
                imag_val = self.visit(node.args[1])
                if imag_val is None:
                    raise NotImplementedError("Unsupported complex imag input")
            for kw in node.keywords:
                if kw.arg == "real":
                    if real_val is not None:
                        raise NotImplementedError("complex() real specified twice")
                    real_val = self.visit(kw.value)
                    if real_val is None:
                        raise NotImplementedError("Unsupported complex real input")
                elif kw.arg == "imag":
                    if imag_val is not None:
                        raise NotImplementedError("complex() imag specified twice")
                    imag_val = self.visit(kw.value)
                    if imag_val is None:
                        raise NotImplementedError("Unsupported complex imag input")
                else:
                    raise NotImplementedError(
                        "complex only supports real/imag keywords"
                    )
            if real_val is None:
                real_val = MoltValue(self.next_var(), type_hint="float")
                self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=real_val))
            if imag_val is None:
                imag_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=imag_val))
                has_imag = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_imag))
            else:
                has_imag = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_imag))
            res = MoltValue(self.next_var(), type_hint="complex")
            self.emit(
                MoltOp(
                    kind="COMPLEX_FROM_OBJ",
                    args=[real_val, imag_val, has_imag],
                    result=res,
                )
            )
            return res
        if func_id == "int":
            if len(node.args) > 2:
                raise NotImplementedError("int expects 0-2 arguments")
            value: MoltValue | None = None
            base_val: MoltValue | None = None
            has_base_flag = False
            from_str_source = False
            str_source_node = (
                self._builtin_str_single_object_arg(node.args[0]) if node.args else None
            )
            if node.args:
                if str_source_node is not None:
                    value = self.visit(str_source_node)
                    from_str_source = True
                else:
                    value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported int input")
            if len(node.args) == 2:
                base_val = self.visit(node.args[1])
                if base_val is None:
                    raise NotImplementedError("Unsupported int base")
                has_base_flag = True
            for keyword in node.keywords:
                if keyword.arg is None:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if keyword.arg == "base":
                    if has_base_flag:
                        return self._emit_type_error_value(
                            "int() got multiple values for argument 'base'",
                            "int",
                        )
                    base_val = self.visit(keyword.value)
                    if base_val is None:
                        raise NotImplementedError("Unsupported int base")
                    has_base_flag = True
                else:
                    return self._emit_type_error_value(
                        f"int() got an unexpected keyword argument '{keyword.arg}'",
                        "int",
                    )
            if value is None:
                if has_base_flag:
                    return self._emit_type_error_value(
                        "int() missing string argument", "int"
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=res))
                return res
            if not has_base_flag:
                base_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=base_val))
            has_base = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[has_base_flag], result=has_base))
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind=("INT_FROM_STR_OF_OBJ" if from_str_source else "INT_FROM_OBJ"),
                    args=[value, base_val, has_base],
                    result=res,
                )
            )
            return res
        if func_id == "pow":
            if node.keywords:
                raise NotImplementedError("pow does not support keywords")
            if len(node.args) not in (2, 3):
                raise NotImplementedError("pow expects 2 or 3 arguments")
            base = self.visit(node.args[0])
            exp = self.visit(node.args[1])
            if base is None or exp is None:
                raise NotImplementedError("Unsupported pow inputs")
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
                raise NotImplementedError("Unsupported pow mod input")
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
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if len(node.args) not in (1, 2):
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            value = self.visit(node.args[0])
            if value is None:
                raise NotImplementedError("Unsupported round input")
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
        if func_id == "set":
            if node.keywords or len(node.args) > 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if not node.args:
                res = MoltValue(self.next_var(), type_hint="set")
                self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
                return res
            range_args = self._parse_range_call(node.args[0])
            if range_args is not None:
                start, stop, step, _lowerable = range_args
                range_obj = self._emit_range_obj_from_args(start, stop, step)
                return self._emit_set_from_iter(range_obj)
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError("Unsupported set input")
            return self._emit_set_from_iter(iterable)
        if func_id == "frozenset":
            if node.keywords or len(node.args) > 1:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            if not node.args:
                res = MoltValue(self.next_var(), type_hint="frozenset")
                self.emit(MoltOp(kind="FROZENSET_NEW", args=[], result=res))
                return res
            range_args = self._parse_range_call(node.args[0])
            if range_args is not None:
                start, stop, step, _lowerable = range_args
                range_obj = self._emit_range_obj_from_args(start, stop, step)
                return self._emit_frozenset_from_iter(range_obj)
            iterable = self.visit(node.args[0])
            if iterable is None:
                raise NotImplementedError("Unsupported frozenset input")
            return self._emit_frozenset_from_iter(iterable)
            return self._emit_tuple_from_iter(iterable)
        if func_id == "bytes":
            if any(kw.arg is None for kw in node.keywords):
                raise NotImplementedError("bytes does not support **kwargs")
            if len(node.args) > 3:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            source_expr = node.args[0] if node.args else None
            encoding_expr = node.args[1] if len(node.args) > 1 else None
            errors_expr = node.args[2] if len(node.args) > 2 else None
            has_encoding = encoding_expr is not None
            has_errors = errors_expr is not None
            for kw in node.keywords:
                if kw.arg == "source":
                    if source_expr is not None:
                        return self._emit_type_error_value(
                            "bytes() got multiple values for argument 'source'",
                            "bytes",
                        )
                    source_expr = kw.value
                elif kw.arg == "encoding":
                    if has_encoding:
                        return self._emit_type_error_value(
                            "bytes() got multiple values for argument 'encoding'",
                            "bytes",
                        )
                    encoding_expr = kw.value
                    has_encoding = True
                elif kw.arg == "errors":
                    if has_errors:
                        return self._emit_type_error_value(
                            "bytes() got multiple values for argument 'errors'",
                            "bytes",
                        )
                    errors_expr = kw.value
                    has_errors = True
                else:
                    msg = f"bytes() got an unexpected keyword argument '{kw.arg}'"
                    return self._emit_type_error_value(msg, "bytes")
            if source_expr is None and not has_encoding and not has_errors:
                res = MoltValue(self.next_var(), type_hint="bytes")
                self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=res))
                return res
            if source_expr is None:
                source_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=source_val))
            else:
                source_val = self.visit(source_expr)
                if source_val is None:
                    raise NotImplementedError("Unsupported bytes input")
            if has_encoding:
                if encoding_expr is None:
                    raise NotImplementedError("Unsupported bytes encoding")
                encoding_val = self.visit(encoding_expr)
                if encoding_val is None:
                    raise NotImplementedError("Unsupported bytes encoding")
            else:
                encoding_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=encoding_val))
            if has_errors:
                if errors_expr is None:
                    raise NotImplementedError("Unsupported bytes errors")
                errors_val = self.visit(errors_expr)
                if errors_val is None:
                    raise NotImplementedError("Unsupported bytes errors")
            else:
                errors_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=errors_val))
            res = MoltValue(self.next_var(), type_hint="bytes")
            if has_encoding or has_errors:
                self.emit(
                    MoltOp(
                        kind="BYTES_FROM_STR",
                        args=[source_val, encoding_val, errors_val],
                        result=res,
                    )
                )
            else:
                self.emit(MoltOp(kind="BYTES_FROM_OBJ", args=[source_val], result=res))
            return res
        if func_id == "bytearray":
            if any(kw.arg is None for kw in node.keywords):
                raise NotImplementedError("bytearray does not support **kwargs")
            if len(node.args) > 3:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                return self._emit_dynamic_call(node, callee, True)
            source_expr = node.args[0] if node.args else None
            encoding_expr = node.args[1] if len(node.args) > 1 else None
            errors_expr = node.args[2] if len(node.args) > 2 else None
            has_encoding = encoding_expr is not None
            has_errors = errors_expr is not None
            for kw in node.keywords:
                if kw.arg == "source":
                    if source_expr is not None:
                        return self._emit_type_error_value(
                            "bytearray() got multiple values for argument 'source'",
                            "bytearray",
                        )
                    source_expr = kw.value
                elif kw.arg == "encoding":
                    if has_encoding:
                        return self._emit_type_error_value(
                            "bytearray() got multiple values for argument 'encoding'",
                            "bytearray",
                        )
                    encoding_expr = kw.value
                    has_encoding = True
                elif kw.arg == "errors":
                    if has_errors:
                        return self._emit_type_error_value(
                            "bytearray() got multiple values for argument 'errors'",
                            "bytearray",
                        )
                    errors_expr = kw.value
                    has_errors = True
                else:
                    msg = f"bytearray() got an unexpected keyword argument '{kw.arg}'"
                    return self._emit_type_error_value(msg, "bytearray")
            if source_expr is None and not has_encoding and not has_errors:
                arg = MoltValue(self.next_var(), type_hint="bytes")
                self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=arg))
                res = MoltValue(self.next_var(), type_hint="bytearray")
                self.emit(MoltOp(kind="BYTEARRAY_FROM_OBJ", args=[arg], result=res))
                return res
            if source_expr is None:
                source_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=source_val))
                source_len_hint = None
            else:
                source_len_hint = self._const_int_from_expr(source_expr)
                source_val = self.visit(source_expr)
                if source_val is None:
                    raise NotImplementedError("Unsupported bytearray input")
            if has_encoding:
                if encoding_expr is None:
                    raise NotImplementedError("Unsupported bytearray encoding")
                encoding_val = self.visit(encoding_expr)
                if encoding_val is None:
                    raise NotImplementedError("Unsupported bytearray encoding")
            else:
                encoding_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=encoding_val))
            if has_errors:
                if errors_expr is None:
                    raise NotImplementedError("Unsupported bytearray errors")
                errors_val = self.visit(errors_expr)
                if errors_val is None:
                    raise NotImplementedError("Unsupported bytearray errors")
            else:
                errors_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=errors_val))
            res = MoltValue(self.next_var(), type_hint="bytearray")
            if has_encoding or has_errors:
                self.emit(
                    MoltOp(
                        kind="BYTEARRAY_FROM_STR",
                        args=[source_val, encoding_val, errors_val],
                        result=res,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="BYTEARRAY_FROM_OBJ",
                        args=[source_val],
                        result=res,
                    )
                )
                self._remember_bytearray_len_hint(
                    res,
                    source_len_hint
                    if source_len_hint is not None
                    else self.const_ints.get(source_val.name),
                )
            return res
        if func_id == "memoryview":
            if len(node.args) != 1:
                raise NotImplementedError("memoryview expects 1 argument")
            arg = self.visit(node.args[0])
            res = MoltValue(self.next_var(), type_hint="memoryview")
            self.emit(MoltOp(kind="MEMORYVIEW_NEW", args=[arg], result=res))
            return res
        if func_id in BUILTIN_FUNC_SPECS:
            if func_id == "open":
                needs_bind = True
            spec = BUILTIN_FUNC_SPECS[func_id]
            # CALL_FUNC bypasses argument binding; vararg/kwonly builtins must
            # route through CALL_BIND to preserve Python call semantics.
            needs_bind = needs_bind or (
                spec.vararg is not None or bool(spec.kwonly_params)
            )
            callee = self._emit_builtin_function(func_id)
            res = MoltValue(self.next_var(), type_hint="Any")
            if needs_bind:
                callargs = self._emit_call_args_builder(node)
                self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            else:
                args = self._emit_call_args(node.args)
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
            return res
        return CALL_NOT_HANDLED
