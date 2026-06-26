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
        return CALL_NOT_HANDLED
