"""CallAttributeDispatchMixin: extracted visit_Call dispatch phase."""

from __future__ import annotations

import ast

from typing import (
    TYPE_CHECKING,
    Any,
)

from molt.frontend._types import (
    BUILTIN_TYPE_TAGS,
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


class CallAttributeDispatchMixin(_MixinBase):
    def _try_emit_attribute_receiver_call(
        self, node: ast.Call, needs_bind: bool
    ) -> Any:
        if isinstance(node.func, ast.Attribute):
            attr_node = node.func
            if (
                node.func.attr == "format"
                and isinstance(node.func.value, ast.Constant)
                and isinstance(node.func.value.value, str)
            ):
                lowered = self._lower_string_format_call(node, node.func.value.value)
                if lowered is not None:
                    return lowered
            # ...
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "nullcontext"
            ):
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "closing"
            ):
                if len(node.args) != 1:
                    raise NotImplementedError("closing expects 1 argument")
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "math"
                and node.func.attr == "trunc"
            ):
                if len(node.args) != 1:
                    raise NotImplementedError("math.trunc expects 1 argument")
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported math.trunc input")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TRUNC", args=[value], result=res))
                return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_json"
            ):
                if node.func.attr == "parse" and len(node.args) == 1:
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if self.parse_codec == "cbor":
                        kind = "CBOR_PARSE"
                    elif self.parse_codec == "json":
                        kind = "JSON_PARSE"
                    else:
                        kind = "MSGPACK_PARSE"
                    self.emit(MoltOp(kind=kind, args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_msgpack"
            ):
                if node.func.attr == "parse" and len(node.args) == 1:
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="MSGPACK_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_cbor"
            ):
                if node.func.attr == "parse" and len(node.args) == 1:
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CBOR_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_buffer"
            ):
                if node.func.attr == "new":
                    if len(node.args) not in (2, 3):
                        raise NotImplementedError(
                            "molt_buffer.new expects 2 or 3 arguments"
                        )
                    rows = self.visit(node.args[0])
                    cols = self.visit(node.args[1])
                    if len(node.args) == 3:
                        init = self.visit(node.args[2])
                    else:
                        init = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=init))
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_NEW", args=[rows, cols, init], result=res)
                    )
                    return res
                if node.func.attr == "get":
                    if len(node.args) != 3:
                        raise NotImplementedError("molt_buffer.get expects 3 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="BUFFER2D_GET", args=[buf, row, col], result=res)
                    )
                    return res
                if node.func.attr == "set":
                    if len(node.args) != 4:
                        raise NotImplementedError("molt_buffer.set expects 4 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    val = self.visit(node.args[3])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(
                            kind="BUFFER2D_SET", args=[buf, row, col, val], result=res
                        )
                    )
                    return res
                if node.func.attr == "matmul":
                    if len(node.args) != 2:
                        raise NotImplementedError(
                            "molt_buffer.matmul expects 2 arguments"
                        )
                    lhs = self.visit(node.args[0])
                    rhs = self.visit(node.args[1])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_MATMUL", args=[lhs, rhs], result=res)
                    )
                    return res
            receiver = self.visit(attr_node.value)
            if receiver is None:
                receiver = MoltValue("unknown_obj", type_hint="Unknown")
            obj_name = None
            exact_class = None
            if isinstance(attr_node.value, ast.Name):
                obj_name = attr_node.value.id
                exact_class = self.exact_locals.get(obj_name)

            def load_attr_callee() -> MoltValue:
                return self._emit_attribute_load(
                    attr_node, receiver, obj_name, exact_class
                )

            method = attr_node.attr
            if receiver.type_hint == "bytearray" and method in {
                "append",
                "clear",
                "extend",
                "insert",
                "pop",
                "remove",
                "resize",
            }:
                self._invalidate_bytearray_len_hint(obj_name, receiver)
            if method == "sort" and receiver.type_hint == "list":
                needs_bind = True
            if receiver.type_hint == "generator":
                if method == "send":
                    if len(node.args) != 1:
                        raise NotImplementedError("generator.send expects 1 argument")
                    arg = self.visit(node.args[0])
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="GEN_SEND", args=[receiver, arg], result=pair)
                    )
                    one = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=one))
                    zero = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                    value = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=value))
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                    self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
                    self._emit_stop_iteration_from_value(value)
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    return value
                if method == "throw":
                    if len(node.args) not in {1, 2, 3}:
                        raise NotImplementedError(
                            "generator.throw expects 1 to 3 arguments"
                        )
                    exc_type = self.visit(node.args[0])
                    if exc_type is None:
                        raise NotImplementedError("generator.throw expects exception")
                    if len(node.args) > 1:
                        value = self.visit(node.args[1])
                        if value is None:
                            raise NotImplementedError(
                                "generator.throw expects exception value"
                            )
                        callargs = MoltValue(self.next_var(), type_hint="callargs")
                        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                        self.emit(
                            MoltOp(
                                kind="CALLARGS_PUSH_POS",
                                args=[callargs, value],
                                result=MoltValue("none"),
                            )
                        )
                        arg = MoltValue(self.next_var(), type_hint="exception")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[exc_type, callargs],
                                result=arg,
                            )
                        )
                        if len(node.args) == 3:
                            tb_val = self.visit(node.args[2])
                            if tb_val is None:
                                raise NotImplementedError(
                                    "generator.throw expects traceback value"
                                )
                            self.emit(
                                MoltOp(
                                    kind="SETATTR_GENERIC_OBJ",
                                    args=[arg, "__traceback__", tb_val],
                                    result=MoltValue("none"),
                                )
                            )
                    else:
                        arg = exc_type
                    callee = load_attr_callee()
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_METHOD", args=[callee, arg], result=res)
                    )
                    return res
                if method == "close":
                    if node.args:
                        raise NotImplementedError("generator.close expects 0 arguments")
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="GEN_CLOSE", args=[receiver], result=res))
                    return res
            class_name = None
            class_info = self.classes.get(receiver.type_hint)
            receiver_is_class_obj = False
            if isinstance(node.func.value, ast.Name):
                candidate = node.func.value.id
                candidate_info = self.classes.get(candidate)
                if candidate in BUILTIN_TYPE_TAGS or candidate_info is not None:
                    receiver_is_class_obj = True
                    if candidate_info is not None:
                        class_name = candidate
                        class_info = candidate_info
            if receiver_is_class_obj:
                needs_bind = True
            lookup_class = class_name
            if lookup_class is None and receiver.type_hint in self.classes:
                lookup_class = receiver.type_hint
            method_info = None
            method_class = None
            if lookup_class:
                method_info, method_class = self._resolve_method_info(
                    lookup_class, method
                )
            if method_info and (
                needs_bind
                or method_info.get("descriptor") == "decorated"
                or method_info.get("has_vararg", False)
                or method_info.get("has_varkw", False)
                or method_info.get("has_closure", False)
            ):
                callee = load_attr_callee()
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res
            if method_info and not needs_bind:
                if class_name is None and receiver.type_hint not in self.classes:
                    callee = load_attr_callee()
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                func_val = method_info["func"]
                descriptor = method_info["descriptor"]
                args = self._emit_call_args(node.args)
                if descriptor == "function":
                    if class_name is None and receiver.type_hint in self.classes:
                        if not receiver_is_class_obj:
                            args = [receiver] + args
                elif descriptor == "classmethod":
                    if class_name is None and receiver.type_hint in self.classes:
                        class_name = receiver.type_hint
                    if class_name is None:
                        raise NotImplementedError("Unsupported classmethod call")
                    class_ref = (
                        receiver
                        if isinstance(node.func.value, ast.Name)
                        and class_name == node.func.value.id
                        else self._emit_module_attr_get(class_name)
                    )
                    args = [class_ref] + args
                elif descriptor != "staticmethod":
                    args = []
                if args or descriptor in {"function", "classmethod", "staticmethod"}:
                    param_count = method_info.get("param_count")
                    defaults = method_info.get("defaults", [])
                    has_vararg = method_info.get("has_vararg", False)
                    has_varkw = method_info.get("has_varkw", False)
                    kwonly_count = method_info.get("kwonly_count")
                    if param_count is not None:
                        fixed_param_count = param_count
                        if has_vararg:
                            fixed_param_count -= 1
                        if has_varkw:
                            fixed_param_count -= 1
                        func_obj = None
                        missing = fixed_param_count - len(args)
                        # Load the function object whenever a trailing default is
                        # filled: a const default needs the version stamp for the
                        # `__defaults__`-mutation deopt guard, a non-const default
                        # needs the live `__defaults__`/`__kwdefaults__` read.
                        if 0 < missing <= len(defaults):
                            class_ref = None
                            if lookup_class:
                                class_info = self.classes.get(lookup_class)
                                if class_info:
                                    class_ref = self._emit_module_attr_get_on(
                                        class_info["module"], lookup_class
                                    )
                            if class_ref is not None:
                                class_attr = self._emit_class_method_func(
                                    class_ref, method
                                )
                                if descriptor == "classmethod":
                                    func_obj = self._emit_bound_method_func(class_attr)
                                else:
                                    func_obj = class_attr
                            else:
                                callee = load_attr_callee()
                                if callee is not None:
                                    if descriptor == "classmethod":
                                        func_obj = self._emit_bound_method_func(callee)
                                    elif descriptor == "function":
                                        if isinstance(
                                            callee.type_hint, str
                                        ) and callee.type_hint.startswith(
                                            "BoundMethod:"
                                        ):
                                            func_obj = self._emit_bound_method_func(
                                                callee
                                            )
                                        else:
                                            func_obj = callee
                                    else:
                                        func_obj = callee
                        positional_limit = None
                        if isinstance(kwonly_count, int):
                            positional_limit = fixed_param_count - kwonly_count
                            if positional_limit < 0:
                                positional_limit = 0
                        args = self._apply_default_specs(
                            fixed_param_count,
                            defaults,
                            args,
                            node,
                            call_name=f"{lookup_class}.{method}",
                            func_obj=func_obj,
                            implicit_self=False,
                            positional_limit=positional_limit,
                        )
                        if args is None:
                            callee = load_attr_callee()
                            if callee is None:
                                raise NotImplementedError("Unsupported call target")
                            callargs = self._emit_call_args_builder(node)
                            res = MoltValue(self.next_var(), type_hint="Any")
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        if has_vararg:
                            if len(args) > fixed_param_count:
                                extra = args[fixed_param_count:]
                                tuple_val = MoltValue(
                                    self.next_var(), type_hint="tuple"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="TUPLE_NEW",
                                        args=extra,
                                        result=tuple_val,
                                    )
                                )
                                args = args[:fixed_param_count] + [tuple_val]
                            elif len(args) == fixed_param_count:
                                empty_tuple = MoltValue(
                                    self.next_var(), type_hint="tuple"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="TUPLE_NEW",
                                        args=[],
                                        result=empty_tuple,
                                    )
                                )
                                args = args + [empty_tuple]
                        if has_varkw:
                            empty_kwargs = MoltValue(self.next_var(), type_hint="dict")
                            self.emit(
                                MoltOp(
                                    kind="DICT_NEW",
                                    args=[],
                                    result=empty_kwargs,
                                )
                            )
                            args = args + [empty_kwargs]
                    res_hint = "Any"
                    return_hint = method_info["return_hint"]
                    # Builtin scalar/container return types must propagate as
                    # type hints — see _resolve_method_call_hints for the same
                    # fix; lane inference falls back to NaN-boxed accumulator
                    # if `int` returns are erased here.
                    if return_hint and (
                        return_hint in self.classes or return_hint in BUILTIN_TYPE_TAGS
                    ):
                        res_hint = return_hint
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    # Route known-method calls through CALL_BIND so descriptor binding and
                    # handle semantics stay aligned with dynamic attribute calls.
                    callee = load_attr_callee()
                    if callee is None:
                        target_name = func_val.type_hint.split(":", 1)[1]
                        self.emit(
                            MoltOp(kind="CALL", args=[target_name] + args, result=res)
                        )
                        return res
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
            if method == "add" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.add expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_ADD", args=[receiver, arg], result=res))
                return res
            if method == "discard" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.discard expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_DISCARD", args=[receiver, arg], result=res))
                return res
            if method == "remove" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.remove expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_REMOVE", args=[receiver, arg], result=res))
                return res
            if (
                method
                in {
                    "union",
                    "intersection",
                    "difference",
                    "symmetric_difference",
                }
                and receiver.type_hint in {"set", "frozenset"}
                and not any(isinstance(a, ast.Starred) for a in node.args)
            ):
                if method == "symmetric_difference":
                    if len(node.args) != 1:
                        raise NotImplementedError(
                            "set.symmetric_difference expects 1 argument"
                        )
                    other = self.visit(node.args[0])
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if other.type_hint not in {"set", "frozenset"}:
                        other = self._emit_set_from_iter(other)
                    op_kind = "BIT_XOR"
                    res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                    self.emit(MoltOp(kind=op_kind, args=[receiver, other], result=res))
                    return res
                if len(node.args) == 0:
                    if receiver.type_hint == "frozenset":
                        return self._emit_frozenset_from_iter(receiver)
                    return self._emit_set_from_iter(receiver)
                if method == "union":
                    res = self._emit_set_from_iter(receiver)
                    for arg in node.args:
                        other = self.visit(arg)
                        if other is None:
                            raise NotImplementedError("Unsupported set operation input")
                        if other.type_hint in {"set", "frozenset"}:
                            self.emit(
                                MoltOp(
                                    kind="SET_UPDATE",
                                    args=[res, other],
                                    result=MoltValue("none"),
                                )
                            )
                        else:
                            self._emit_set_update_from_iter(res, other)
                    if receiver.type_hint == "frozenset":
                        return self._emit_frozenset_from_iter(res)
                    return res
                res = receiver
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if other.type_hint not in {"set", "frozenset"}:
                        # intersection probes the receiver (bare unhashable
                        # context); difference inserts into a result set
                        # (set-element context on 3.14).
                        other = self._emit_set_from_iter(
                            other, probe=(method == "intersection")
                        )
                    op_kind = {
                        "intersection": "BIT_AND",
                        "difference": "SUB",
                    }[method]
                    next_res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                    self.emit(MoltOp(kind=op_kind, args=[res, other], result=next_res))
                    res = next_res
                return res
            if (
                method
                in {
                    "update",
                    "intersection_update",
                    "difference_update",
                    "symmetric_difference_update",
                }
                and receiver.type_hint == "set"
                and not any(isinstance(a, ast.Starred) for a in node.args)
            ):
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                if method == "symmetric_difference_update":
                    if len(node.args) != 1:
                        raise NotImplementedError(
                            "set.symmetric_difference_update expects 1 argument"
                        )
                if len(node.args) == 0:
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                op_kind = {
                    "update": "SET_UPDATE",
                    "intersection_update": "SET_INTERSECTION_UPDATE",
                    "difference_update": "SET_DIFFERENCE_UPDATE",
                    "symmetric_difference_update": "SET_SYMDIFF_UPDATE",
                }[method]
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if recv_slot is not None:
                        receiver = self._reload_async_value(
                            recv_slot, receiver.type_hint
                        )
                    if other.type_hint in {"set", "frozenset"} or method != "update":
                        if other.type_hint not in {"set", "frozenset"}:
                            # intersection_update probes the receiver (bare
                            # unhashable context); the other update-family ops
                            # insert (set-element context on 3.14).
                            other = self._emit_set_from_iter(
                                other, probe=(method == "intersection_update")
                            )
                        self.emit(
                            MoltOp(kind=op_kind, args=[receiver, other], result=res)
                        )
                    else:
                        self._emit_set_update_from_iter(receiver, other)
                return res
            if method == "append" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.append expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("list.append expects a value")
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(receiver, obj_name, arg.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_APPEND", args=[receiver, arg], result=res))
                return res
            if method == "extend" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.extend expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                other = self.visit(node.args[0])
                if other is None:
                    raise NotImplementedError("list.extend expects an iterable")
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(
                    receiver,
                    obj_name,
                    self._iterable_element_hint(other),
                )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_EXTEND", args=[receiver, other], result=res)
                )
                return res
            if method == "insert" and receiver.type_hint == "list":
                if len(node.args) != 2:
                    raise NotImplementedError("list.insert expects 2 arguments")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                idx = self.visit(node.args[0])
                val = self.visit(node.args[1])
                if idx is None or val is None:
                    raise NotImplementedError("list.insert expects index and value")
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(receiver, obj_name, val.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_INSERT", args=[receiver, idx, val], result=res)
                )
                return res
            if method == "remove" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.remove expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                val = self.visit(node.args[0])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REMOVE", args=[receiver, val], result=res))
                return res
            if method == "clear" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.clear expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.copy expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_COPY", args=[receiver], result=res))
                return res
            if method == "reverse" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.reverse expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REVERSE", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "list":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("list.index expects 1 to 3 arguments")
                val = self.visit(node.args[0])
                start = None
                end = None
                if len(node.args) >= 2:
                    start = self.visit(node.args[1])
                    if start is None:
                        raise NotImplementedError("Unsupported list.index start")
                if len(node.args) == 3:
                    end = self.visit(node.args[2])
                    if end is None:
                        raise NotImplementedError("Unsupported list.index end")
                for keyword in node.keywords:
                    if keyword.arg is None:
                        raise NotImplementedError(
                            "list.index does not support **kwargs"
                        )
                    if keyword.arg == "start":
                        if start is not None:
                            return self._emit_type_error_value(
                                "list.index() got multiple values for argument 'start'",
                                "int",
                            )
                        start = self.visit(keyword.value)
                        if start is None:
                            raise NotImplementedError("Unsupported list.index start")
                    elif keyword.arg == "end":
                        if end is not None:
                            return self._emit_type_error_value(
                                "list.index() got multiple values for argument 'end'",
                                "int",
                            )
                        end = self.visit(keyword.value)
                        if end is None:
                            raise NotImplementedError("Unsupported list.index end")
                    else:
                        return self._emit_type_error_value(
                            "list.index() got an unexpected keyword argument "
                            f"'{keyword.arg}'",
                            "int",
                        )
                if start is None and end is None:
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="LIST_INDEX", args=[receiver, val], result=res)
                    )
                    return res
                if start is None:
                    start = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=start))
                if end is None:
                    stop = MoltValue(self.next_var(), type_hint="missing")
                    self.emit(MoltOp(kind="MISSING", args=[], result=stop))
                else:
                    stop = end
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="LIST_INDEX_RANGE",
                        args=[receiver, val, start, stop],
                        result=res,
                    )
                )
                return res
            if method == "pop" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("dict.pop expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                    has_default = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=has_default))
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                    has_default = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=has_default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(
                        kind="DICT_POP",
                        args=[receiver, key, default, has_default],
                        result=res,
                    )
                )
                return res
            if method == "pop" and receiver.type_hint == "set":
                if node.args:
                    raise NotImplementedError("set.pop expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="SET_POP", args=[receiver], result=res))
                return res
            if method == "pop" and receiver.type_hint == "list":
                if len(node.args) > 1:
                    raise NotImplementedError("list.pop expects 0 or 1 argument")
                if node.args:
                    idx = self.visit(node.args[0])
                else:
                    idx = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=idx))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="LIST_POP", args=[receiver, idx], result=res))
                return res
            if method == "get" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("dict.get expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(kind="DICT_GET", args=[receiver, key, default], result=res)
                )
                return res
            if method == "setdefault" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.keywords or len(node.args) not in (1, 2):
                    raise NotImplementedError(
                        "dict.setdefault expects 1 or 2 arguments"
                    )
                key = self.visit(node.args[0])
                if (
                    len(node.args) == 2
                    and isinstance(node.args[1], ast.List)
                    and not node.args[1].elts
                ):
                    res_type = "Any"
                    if self.type_hint_policy == "trust":
                        hint = self._dict_value_hint(receiver)
                        if hint is not None:
                            res_type = hint
                    res = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(
                        MoltOp(
                            kind="DICT_SETDEFAULT_EMPTY_LIST",
                            args=[receiver, key],
                            result=res,
                        )
                    )
                    return res
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(
                        kind="DICT_SETDEFAULT",
                        args=[receiver, key, default],
                        result=res,
                    )
                )
                return res
            if method == "update" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if len(node.args) > 1:
                    msg = f"update expected at most 1 argument, got {len(node.args)}"
                    return self._emit_type_error_value(msg, "None")
                res = MoltValue(self.next_var(), type_hint="None")
                if node.args:
                    other = self.visit(node.args[0])
                    if other is None:
                        raise NotImplementedError("Unsupported dict.update input")
                    self.emit(
                        MoltOp(
                            kind="DICT_UPDATE",
                            args=[receiver, other],
                            result=res,
                        )
                    )
                for kw in node.keywords:
                    if kw.arg is None:
                        mapping = self.visit(kw.value)
                        if mapping is None:
                            raise NotImplementedError(
                                "Unsupported dict.update ** input"
                            )
                        self.emit(
                            MoltOp(
                                kind="DICT_UPDATE_KWSTAR",
                                args=[receiver, mapping],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        key = MoltValue(self.next_var(), type_hint="str")
                        self.emit(MoltOp(kind="CONST_STR", args=[kw.arg], result=key))
                        val = self.visit(kw.value)
                        if val is None:
                            raise NotImplementedError(
                                "Unsupported dict.update kw value"
                            )
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[receiver, key, val],
                                result=MoltValue("none"),
                            )
                        )
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if method == "clear" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.args or node.keywords:
                    raise NotImplementedError("dict.clear expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="DICT_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.args or node.keywords:
                    raise NotImplementedError("dict.copy expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_COPY", args=[receiver], result=res))
                return res
            if method == "popitem" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                if node.args or node.keywords:
                    raise NotImplementedError("dict.popitem expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="DICT_POPITEM", args=[receiver], result=res))
                return res
            if method == "keys" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                res = MoltValue(self.next_var(), type_hint="dict_keys_view")
                self.emit(MoltOp(kind="DICT_KEYS", args=[receiver], result=res))
                return res
            if method == "values" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                res = MoltValue(self.next_var(), type_hint="dict_values_view")
                self.emit(MoltOp(kind="DICT_VALUES", args=[receiver], result=res))
                return res
            if method == "items" and self._has_exact_builtin_receiver(
                attr_node.value, receiver, "dict"
            ):
                res = MoltValue(self.next_var(), type_hint="dict_items_view")
                self.emit(MoltOp(kind="DICT_ITEMS", args=[receiver], result=res))
                return res
            if method == "read" and receiver.type_hint.startswith("file"):
                if len(node.args) > 1:
                    raise NotImplementedError("file.read expects 0 or 1 argument")
                if node.args:
                    size_val = self.visit(node.args[0])
                else:
                    size_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=size_val))
                if receiver.type_hint == "file_bytes":
                    res_hint = "bytes"
                elif receiver.type_hint == "file_text":
                    res_hint = "str"
                else:
                    res_hint = "Any"
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="FILE_READ", args=[receiver, size_val], result=res)
                )
                return res
            if method == "write" and receiver.type_hint.startswith("file"):
                if len(node.args) != 1:
                    raise NotImplementedError("file.write expects 1 argument")
                data = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="FILE_WRITE", args=[receiver, data], result=res))
                return res
            if method == "close" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise NotImplementedError("file.close expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_CLOSE", args=[receiver], result=res))
                return res
            if method == "flush" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise NotImplementedError("file.flush expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_FLUSH", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise NotImplementedError("tuple.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TUPLE_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "tuple":
                if len(node.args) == 1 and not node.keywords:
                    val = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="TUPLE_INDEX", args=[receiver, val], result=res)
                    )
                    return res
            if method == "tobytes":
                if node.args:
                    raise NotImplementedError("tobytes expects 0 arguments")
                if receiver.type_hint == "memoryview":
                    res = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(
                        MoltOp(kind="MEMORYVIEW_TOBYTES", args=[receiver], result=res)
                    )
                    return res
            if method == "count":
                if receiver.type_hint in {"str", "bytes", "bytearray"}:
                    if len(node.args) not in (1, 2, 3):
                        pass
                    elif any(kw.arg is None for kw in node.keywords):
                        pass
                    else:
                        needle_node = node.args[0]
                        start_node: ast.expr | None = None
                        end_node: ast.expr | None = None
                        start_provided = False
                        end_provided = False
                        if len(node.args) >= 2:
                            start_node = node.args[1]
                            start_provided = True
                        if len(node.args) == 3:
                            end_node = node.args[2]
                            end_provided = True
                        for keyword in node.keywords:
                            if keyword.arg == "start":
                                if start_node is not None:
                                    return self._emit_type_error_value(
                                        "count() got multiple values for argument 'start'",
                                        "int",
                                    )
                                start_node = keyword.value
                                start_provided = True
                            elif keyword.arg == "end":
                                if end_node is not None:
                                    return self._emit_type_error_value(
                                        "count() got multiple values for argument 'end'",
                                        "int",
                                    )
                                end_node = keyword.value
                                end_provided = True
                            else:
                                return self._emit_type_error_value(
                                    "count() got an unexpected keyword argument "
                                    f"'{keyword.arg}'",
                                    "int",
                                )
                        needle = self.visit(needle_node)
                        use_slice = start_provided or end_provided
                        if receiver.type_hint == "str":
                            res = MoltValue(self.next_var(), type_hint="int")
                            if not use_slice:
                                self.emit(
                                    MoltOp(
                                        kind="STRING_COUNT",
                                        args=[receiver, needle],
                                        result=res,
                                    )
                                )
                                return res
                            if start_node is None:
                                start = MoltValue(self.next_var(), type_hint="int")
                                self.emit(MoltOp(kind="CONST", args=[0], result=start))
                            else:
                                start = self.visit(start_node)
                                if start is None:
                                    raise NotImplementedError(
                                        "Unsupported count start argument"
                                    )
                            if end_node is None:
                                end = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=end)
                                )
                            else:
                                end = self.visit(end_node)
                                if end is None:
                                    raise NotImplementedError(
                                        "Unsupported count end argument"
                                    )
                            has_end = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[end_provided],
                                    result=has_end,
                                )
                            )
                            has_start = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[start_provided],
                                    result=has_start,
                                )
                            )
                            self.emit(
                                MoltOp(
                                    kind="STRING_COUNT_SLICE",
                                    args=[
                                        receiver,
                                        needle,
                                        start,
                                        end,
                                        has_start,
                                        has_end,
                                    ],
                                    result=res,
                                )
                            )
                            return res
                        if receiver.type_hint in {"bytes", "bytearray"}:
                            res = MoltValue(self.next_var(), type_hint="int")
                            if not use_slice:
                                op_kind = (
                                    "BYTES_COUNT"
                                    if receiver.type_hint == "bytes"
                                    else "BYTEARRAY_COUNT"
                                )
                                self.emit(
                                    MoltOp(
                                        kind=op_kind,
                                        args=[receiver, needle],
                                        result=res,
                                    )
                                )
                                return res
                            if start_node is None:
                                start = MoltValue(self.next_var(), type_hint="int")
                                self.emit(MoltOp(kind="CONST", args=[0], result=start))
                            else:
                                start = self.visit(start_node)
                                if start is None:
                                    raise NotImplementedError(
                                        "Unsupported count start argument"
                                    )
                            if end_node is None:
                                end = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=end)
                                )
                            else:
                                end = self.visit(end_node)
                                if end is None:
                                    raise NotImplementedError(
                                        "Unsupported count end argument"
                                    )
                            has_end = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[end_provided],
                                    result=has_end,
                                )
                            )
                            has_start = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(
                                    kind="CONST_BOOL",
                                    args=[start_provided],
                                    result=has_start,
                                )
                            )
                            op_kind = (
                                "BYTES_COUNT_SLICE"
                                if receiver.type_hint == "bytes"
                                else "BYTEARRAY_COUNT_SLICE"
                            )
                            self.emit(
                                MoltOp(
                                    kind=op_kind,
                                    args=[
                                        receiver,
                                        needle,
                                        start,
                                        end,
                                        has_start,
                                        has_end,
                                    ],
                                    result=res,
                                )
                            )
                            return res
            if method == "startswith":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("startswith expects 1-3 arguments")
                needle = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "endswith":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("endswith expects 1-3 arguments")
                needle = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "join":
                if len(node.args) != 1:
                    callee = load_attr_callee()
                    return self._emit_dynamic_call(node, callee, True)
                items = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_JOIN", args=[receiver, items], result=res)
                    )
                    return res
            if method == "split":
                if len(node.args) > 2:
                    raise NotImplementedError("split expects 0-2 arguments")
                # Support keyword args: split(sep=',') and split(sep=',', maxsplit=2)
                kw_sep = next(
                    (kw.value for kw in node.keywords if kw.arg == "sep"), None
                )
                kw_maxsplit = next(
                    (kw.value for kw in node.keywords if kw.arg == "maxsplit"), None
                )
                if node.args:
                    needle = self.visit(node.args[0])
                elif kw_sep is not None:
                    needle = self.visit(kw_sep)
                else:
                    needle = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=needle))
                maxsplit = None
                if len(node.args) == 2:
                    maxsplit = self.visit(node.args[1])
                elif kw_maxsplit is not None:
                    maxsplit = self.visit(kw_maxsplit)
                res = MoltValue(self.next_var(), type_hint="list")
                if receiver.type_hint == "str":
                    if maxsplit is not None:
                        self.emit(
                            MoltOp(
                                kind="STRING_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="STRING_SPLIT", args=[receiver, needle], result=res
                            )
                        )
                    self._record_container_elem_hint(res, "str")
                    return res
                if receiver.type_hint == "bytes":
                    if maxsplit is not None:
                        self.emit(
                            MoltOp(
                                kind="BYTES_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="BYTES_SPLIT", args=[receiver, needle], result=res
                            )
                        )
                    self._record_container_elem_hint(res, "bytes")
                    return res
                if receiver.type_hint == "bytearray":
                    if maxsplit is not None:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_SPLIT",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                    self._record_container_elem_hint(res, "bytearray")
                    return res
            if method == "lower" and receiver.type_hint == "str":
                if node.args:
                    raise NotImplementedError("lower expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_LOWER", args=[receiver], result=res))
                return res
            if method == "upper" and receiver.type_hint == "str":
                if node.args:
                    raise NotImplementedError("upper expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_UPPER", args=[receiver], result=res))
                return res
            if method == "capitalize" and receiver.type_hint == "str":
                if node.args:
                    raise NotImplementedError("capitalize expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_CAPITALIZE", args=[receiver], result=res))
                return res
            if method == "strip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise NotImplementedError("strip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_STRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "lstrip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise NotImplementedError("lstrip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_LSTRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "rstrip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise NotImplementedError("rstrip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_RSTRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "replace":
                if receiver.type_hint in {"str", "bytes", "bytearray"}:
                    if any(isinstance(arg, ast.Starred) for arg in node.args):
                        pass
                    elif any(kw.arg is None for kw in node.keywords):
                        pass
                    else:
                        count_expr: ast.expr | None = None
                        extra_kw = False
                        for kw in node.keywords:
                            if kw.arg == "count":
                                count_expr = kw.value
                            else:
                                extra_kw = True
                                break
                        if not extra_kw and len(node.args) in (2, 3):
                            if len(node.args) == 3 and count_expr is not None:
                                pass
                            else:
                                old = self.visit(node.args[0])
                                new = self.visit(node.args[1])
                                if len(node.args) == 3:
                                    count = self.visit(node.args[2])
                                elif count_expr is not None:
                                    count = self.visit(count_expr)
                                else:
                                    count = MoltValue(self.next_var(), type_hint="int")
                                    self.emit(
                                        MoltOp(kind="CONST", args=[-1], result=count)
                                    )
                                res = MoltValue(
                                    self.next_var(), type_hint=receiver.type_hint
                                )
                                if receiver.type_hint == "str":
                                    self.emit(
                                        MoltOp(
                                            kind="STRING_REPLACE",
                                            args=[receiver, old, new, count],
                                            result=res,
                                        )
                                    )
                                    return res
                                if receiver.type_hint == "bytes":
                                    self.emit(
                                        MoltOp(
                                            kind="BYTES_REPLACE",
                                            args=[receiver, old, new, count],
                                            result=res,
                                        )
                                    )
                                    return res
                                if receiver.type_hint == "bytearray":
                                    self.emit(
                                        MoltOp(
                                            kind="BYTEARRAY_REPLACE",
                                            args=[receiver, old, new, count],
                                            result=res,
                                        )
                                    )
                                    return res
            if method == "find" and receiver.type_hint in {"str", "bytes", "bytearray"}:
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("find expects 1-3 arguments")
                needle = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_FIND", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_FIND",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_FIND", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            module_name = (
                self._imported_module_binding_target(obj_name) if obj_name else None
            )
            if module_name is None:
                callee = load_attr_callee()
                # Dynamic attribute calls must use binder semantics so bound methods
                # receive `self` even when local type inference is imprecise.
                return self._emit_dynamic_call(node, callee, True)
        return CALL_NOT_HANDLED
