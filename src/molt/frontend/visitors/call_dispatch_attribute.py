"""CallAttributeDispatchMixin: extracted visit_Call dispatch phase."""

from __future__ import annotations

import ast
from molt.compiler_analysis.python_builtin_shapes import BUILTIN_SHAPE_NAMES
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


class CallAttributeDispatchMixin(_MixinBase):
    def _dotted_attribute_parts(self, expr: ast.AST) -> tuple[str, ...] | None:
        if isinstance(expr, ast.Name):
            return (expr.id,)
        if isinstance(expr, ast.Attribute):
            base = self._dotted_attribute_parts(expr.value)
            if base is None:
                return None
            return (*base, expr.attr)
        return None

    def _dotted_imported_module_target(
        self, receiver_parts: tuple[str, ...]
    ) -> str | None:
        if len(receiver_parts) < 2:
            return None
        root = receiver_parts[0]
        imported_target = self._imported_module_binding_target(root)
        if imported_target is None:
            return None
        receiver_name = ".".join(receiver_parts)
        if receiver_name == imported_target:
            return receiver_name
        imported_root = imported_target.split(".", 1)[0]
        if root != imported_root:
            return ".".join((imported_target, *receiver_parts[1:]))
        return None

    def _try_emit_attribute_receiver_call(self, node: ast.Call) -> Any:
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "nullcontext expects 0 or 1 argument",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "closing expects 1 argument"
                    )
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "math"
                and node.func.attr == "trunc"
            ):
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "math.trunc expects 1 argument",
                    )
                value = self.visit(node.args[0])
                if value is None:
                    raise FrontendRejection(
                        Diagnostic.OPERAND_VALUE, "Unsupported math.trunc input"
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TRUNC", args=[value], result=res))
                return res
            receiver = self.visit(attr_node.value)
            if receiver is None:
                receiver = MoltValue("unknown_obj", type_hint="Unknown")
            obj_name = None
            if isinstance(attr_node.value, ast.Name):
                obj_name = attr_node.value.id
            exact_class = self._exact_class_for_value(receiver, obj_name)

            def load_attr_callee() -> MoltValue:
                return self._emit_attribute_load(
                    attr_node, receiver, obj_name, exact_class
                )

            receiver_kind = self._builtin_exact_type_from_expr(attr_node.value)
            if receiver_kind is not None:
                # A source-point result is exact. Do not require agreement from
                # an older transport/annotation hint, and do not mutate its SSA
                # producer merely to project the fact at this use.
                receiver = MoltValue(receiver.name, type_hint=receiver_kind)
            elif receiver.type_hint in BUILTIN_SHAPE_NAMES:
                # Every builtin method family shares this admission boundary.
                # An annotation or stale frontend cache is not exact-class proof.
                return self._emit_dynamic_call(node, load_attr_callee())

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
            if receiver.type_hint == "generator":
                if method == "send":
                    if len(node.args) != 1:
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "generator.send expects 1 argument",
                        )
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
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "generator.throw expects 1 to 3 arguments",
                        )
                    exc_type = self.visit(node.args[0])
                    if exc_type is None:
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "generator.throw expects exception",
                        )
                    if len(node.args) > 1:
                        value = self.visit(node.args[1])
                        if value is None:
                            raise FrontendRejection(
                                Diagnostic.CALL_SIGNATURE,
                                "generator.throw expects exception value",
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
                                raise FrontendRejection(
                                    Diagnostic.CALL_SIGNATURE,
                                    "generator.throw expects traceback value",
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
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "generator.close expects 0 arguments",
                        )
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="GEN_CLOSE", args=[receiver], result=res))
                    return res
            class_name = None
            if isinstance(node.func.value, ast.Name):
                candidate = node.func.value.id
                candidate_info = self.classes.get(candidate)
                if candidate_info is not None:
                    class_name = candidate
            lookup_class = class_name
            if lookup_class is None and receiver.type_hint in self.classes:
                lookup_class = receiver.type_hint
            method_info = None
            if lookup_class:
                method_info, _ = self._resolve_method_info(lookup_class, method)
            if method_info:
                # Attribute lookup owns descriptor binding; the actual callable
                # owns defaults, implicit receiver arguments, and variadic shape.
                callee = load_attr_callee()
                if callee is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_TARGET, "Unsupported call target"
                    )
                return self._emit_dynamic_call(node, callee)
            if method == "add" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "set.add expects 1 argument"
                    )
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_ADD", args=[receiver, arg], result=res))
                return res
            if method == "discard" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "set.discard expects 1 argument",
                    )
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_DISCARD", args=[receiver, arg], result=res))
                return res
            if method == "remove" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "set.remove expects 1 argument",
                    )
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
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "set.symmetric_difference expects 1 argument",
                        )
                    other = self.visit(node.args[0])
                    if other is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported set operation input",
                        )
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
                            raise FrontendRejection(
                                Diagnostic.OPERAND_VALUE,
                                "Unsupported set operation input",
                            )
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
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported set operation input",
                        )
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
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "set.symmetric_difference_update expects 1 argument",
                        )
                if len(node.args) == 0:
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                op_kind = {
                    "update": "SET_UPDATE",
                    "intersection_update": "SET_INTERSECTION_UPDATE",
                    "difference_update": "SET_DIFFERENCE_UPDATE",
                    "symmetric_difference_update": "SET_SYMDIFF_UPDATE",
                }[method]
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported set operation input",
                        )
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
                            MoltOp(
                                kind=op_kind,
                                args=[receiver, other],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        self._emit_set_update_from_iter(receiver, other)
                # The mutation helpers above have no SSA result. Materialize
                # the method call's Python-level None exactly once after every
                # iterable has been consumed, so multi-argument update calls
                # cannot define one result name multiple times and iterable
                # expansion cannot leave the returned value undefined.
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if method == "append" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.append expects 1 argument",
                    )
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                arg = self.visit(node.args[0])
                if arg is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "list.append expects a value"
                    )
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                self._record_list_element_write(receiver, obj_name, arg.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_APPEND", args=[receiver, arg], result=res))
                return res
            if method == "extend" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.extend expects 1 argument",
                    )
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                other = self.visit(node.args[0])
                if other is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.extend expects an iterable",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.insert expects 2 arguments",
                    )
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                idx = self.visit(node.args[0])
                val = self.visit(node.args[1])
                if idx is None or val is None:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.insert expects index and value",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.remove expects 1 argument",
                    )
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                val = self.visit(node.args[0])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REMOVE", args=[receiver, val], result=res))
                return res
            if method == "clear" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.clear expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.copy expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_COPY", args=[receiver], result=res))
                return res
            if method == "reverse" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.reverse expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REVERSE", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.count expects 1 argument",
                    )
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "list":
                if len(node.args) not in (1, 2, 3):
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.index expects 1 to 3 arguments",
                    )
                val = self.visit(node.args[0])
                start = None
                end = None
                if len(node.args) >= 2:
                    start = self.visit(node.args[1])
                    if start is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported list.index start",
                        )
                if len(node.args) == 3:
                    end = self.visit(node.args[2])
                    if end is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE,
                            "Unsupported list.index end",
                        )
                for keyword in node.keywords:
                    if keyword.arg is None:
                        raise FrontendRejection(
                            Diagnostic.CALL_SIGNATURE,
                            "list.index does not support **kwargs",
                        )
                    if keyword.arg == "start":
                        if start is not None:
                            return self._emit_type_error_value(
                                "list.index() got multiple values for argument 'start'",
                                "int",
                            )
                        start = self.visit(keyword.value)
                        if start is None:
                            raise FrontendRejection(
                                Diagnostic.OPERAND_VALUE,
                                "Unsupported list.index start",
                            )
                    elif keyword.arg == "end":
                        if end is not None:
                            return self._emit_type_error_value(
                                "list.index() got multiple values for argument 'end'",
                                "int",
                            )
                        end = self.visit(keyword.value)
                        if end is None:
                            raise FrontendRejection(
                                Diagnostic.OPERAND_VALUE,
                                "Unsupported list.index end",
                            )
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
            if method == "pop" and receiver.type_hint == "dict":
                if len(node.args) not in (1, 2):
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "dict.pop expects 1 or 2 arguments",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "set.pop expects 0 arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="SET_POP", args=[receiver], result=res))
                return res
            if method == "pop" and receiver.type_hint == "list":
                if len(node.args) > 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "list.pop expects 0 or 1 argument",
                    )
                if node.args:
                    idx = self.visit(node.args[0])
                else:
                    idx = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=idx))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="LIST_POP", args=[receiver, idx], result=res))
                return res
            if method == "get" and receiver.type_hint == "dict":
                if len(node.args) not in (1, 2):
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "dict.get expects 1 or 2 arguments",
                    )
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
            if method == "setdefault" and receiver.type_hint == "dict":
                if node.keywords or len(node.args) not in (1, 2):
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "dict.setdefault expects 1 or 2 arguments",
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
            if method == "update" and receiver.type_hint == "dict":
                if (
                    node.keywords
                    or len(node.args) > 1
                    or any(isinstance(argument, ast.Starred) for argument in node.args)
                ):
                    callee = load_attr_callee()
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                if node.args:
                    other = self.visit(node.args[0])
                    if other is None:
                        raise FrontendRejection(
                            Diagnostic.OPERAND_VALUE, "Unsupported dict.update input"
                        )
                    self.emit(
                        MoltOp(kind="DICT_UPDATE", args=[receiver, other], result=res)
                    )
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if method == "clear" and receiver.type_hint == "dict":
                if node.args or node.keywords:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "dict.clear expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="DICT_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and receiver.type_hint == "dict":
                if node.args or node.keywords:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "dict.copy expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_COPY", args=[receiver], result=res))
                return res
            if method == "popitem" and receiver.type_hint == "dict":
                if node.args or node.keywords:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "dict.popitem expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="DICT_POPITEM", args=[receiver], result=res))
                return res
            if method == "keys" and receiver.type_hint == "dict":
                res = MoltValue(self.next_var(), type_hint="dict_keys_view")
                self.emit(MoltOp(kind="DICT_KEYS", args=[receiver], result=res))
                return res
            if method == "values" and receiver.type_hint == "dict":
                res = MoltValue(self.next_var(), type_hint="dict_values_view")
                self.emit(MoltOp(kind="DICT_VALUES", args=[receiver], result=res))
                return res
            if method == "items" and receiver.type_hint == "dict":
                res = MoltValue(self.next_var(), type_hint="dict_items_view")
                self.emit(MoltOp(kind="DICT_ITEMS", args=[receiver], result=res))
                return res
            if method == "read" and receiver.type_hint.startswith("file"):
                if len(node.args) > 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "file.read expects 0 or 1 argument",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "file.write expects 1 argument",
                    )
                data = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="FILE_WRITE", args=[receiver, data], result=res))
                return res
            if method == "close" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "file.close expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_CLOSE", args=[receiver], result=res))
                return res
            if method == "flush" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "file.flush expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_FLUSH", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "tuple.count expects 1 argument",
                    )
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
            if method == "tobytes" and receiver.type_hint == "memoryview":
                if node.args:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "tobytes expects 0 arguments"
                    )
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
                                    raise FrontendRejection(
                                        Diagnostic.OPERAND_VALUE,
                                        "Unsupported count start argument",
                                    )
                            if end_node is None:
                                end = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=end)
                                )
                            else:
                                end = self.visit(end_node)
                                if end is None:
                                    raise FrontendRejection(
                                        Diagnostic.OPERAND_VALUE,
                                        "Unsupported count end argument",
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
                                    raise FrontendRejection(
                                        Diagnostic.OPERAND_VALUE,
                                        "Unsupported count start argument",
                                    )
                            if end_node is None:
                                end = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=end)
                                )
                            else:
                                end = self.visit(end_node)
                                if end is None:
                                    raise FrontendRejection(
                                        Diagnostic.OPERAND_VALUE,
                                        "Unsupported count end argument",
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "startswith expects 1-3 arguments",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "endswith expects 1-3 arguments",
                    )
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
                    return self._emit_dynamic_call(node, callee)
                items = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_JOIN", args=[receiver, items], result=res)
                    )
                    return res
            if method == "split":
                if len(node.args) > 2:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "split expects 0-2 arguments"
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "lower expects 0 arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_LOWER", args=[receiver], result=res))
                return res
            if method == "upper" and receiver.type_hint == "str":
                if node.args:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "upper expects 0 arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_UPPER", args=[receiver], result=res))
                return res
            if method == "capitalize" and receiver.type_hint == "str":
                if node.args:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "capitalize expects 0 arguments",
                    )
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_CAPITALIZE", args=[receiver], result=res))
                return res
            if method == "strip" and receiver.type_hint in {
                "str",
                "bytes",
                "bytearray",
            }:
                if len(node.args) > 1:
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "strip expects 0 or 1 arguments",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "lstrip expects 0 or 1 arguments",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE,
                        "rstrip expects 0 or 1 arguments",
                    )
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
                    raise FrontendRejection(
                        Diagnostic.CALL_SIGNATURE, "find expects 1-3 arguments"
                    )
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
                # Object dispatch binds the actual descriptor result; only the
                # argument syntax determines whether a builder is necessary.
                return self._emit_dynamic_call(node, callee)
        return CALL_NOT_HANDLED
