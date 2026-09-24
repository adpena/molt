"""CallNamedBuiltinDispatchMixin: named builtin call lowering orchestrator."""

from __future__ import annotations

import ast

from molt.compiler_analysis.python_builtin_shapes import BUILTIN_SHAPE_NAMES

from typing import (
    Any,
)

from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend._types import MoltOp, MoltValue

from molt.frontend.visitors.call_dispatch_builtin_constructors import (
    CallNamedBuiltinConstructorDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_builtin_fallback import (
    CallNamedBuiltinFallbackDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_builtin_iter import (
    CallNamedBuiltinIterDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_builtin_scalar import (
    CallNamedBuiltinScalarDispatchMixin,
)
from molt.frontend.visitors.call_dispatch_common import CALL_NOT_HANDLED
from molt.frontend._mixin_base import GeneratorMixinBase


class CallNamedBuiltinDispatchMixin(
    CallNamedBuiltinScalarDispatchMixin,
    CallNamedBuiltinIterDispatchMixin,
    CallNamedBuiltinConstructorDispatchMixin,
    CallNamedBuiltinFallbackDispatchMixin,
    GeneratorMixinBase,
):
    def _try_emit_shape_builtin_call(self, node: ast.Call) -> Any:
        """Lower the shape family from source-point identity and lifetime facts.

        Knowing a normal result's kind does not authorize skipping invocation
        hooks or the captured callable's lifetime across argument evaluation.
        The generic call owns both when elision is not proven safe.
        """
        index = self.python_binding_index
        fact = index.call_fact(node) if index is not None else None
        name = fact.exact_builtin_name() if fact is not None else None
        if name is None and not (
            isinstance(node.func, ast.Name) and node.func.id in BUILTIN_SHAPE_NAMES
        ):
            return CALL_NOT_HANDLED
        if (
            name is not None
            and fact is not None
            and fact.callee_elision_safe
            and not node.keywords
            and not any(isinstance(arg, ast.Starred) for arg in node.args)
        ):
            lowered = self._emit_proven_shape_builtin_call(node, name)
            if lowered is not CALL_NOT_HANDLED:
                return lowered
        callee = self.visit(node.func)
        if callee is None:
            raise FrontendRejection(Diagnostic.CALL_TARGET, "Unsupported call target")
        return self._emit_dynamic_call(node, callee)

    def _emit_proven_shape_builtin_call(self, node: ast.Call, name: str) -> Any:
        """Emit one identity-proven, callback-free positional shape operation."""
        if name == "str" and len(node.args) > 1:
            # Decoding is a runtime codec protocol, including error handlers.
            return CALL_NOT_HANDLED
        if name == "range":
            parsed = self._parse_range_call(node)
            if parsed is None:
                return CALL_NOT_HANDLED
            start, stop, step, _ = parsed
            return self._emit_range_obj_from_args(start, stop, step)
        if name in {"list", "tuple", "set", "frozenset"} and node.args:
            parsed = self._parse_range_call(node.args[0])
            if parsed is not None:
                start, stop, step, lowerable = parsed
                if name == "list" and lowerable:
                    return self._emit_range_list(start, stop, step)
                iterable = self._emit_range_obj_from_args(start, stop, step)
            else:
                iterable = self.visit(node.args[0])
            if iterable is None:
                raise FrontendRejection(
                    Diagnostic.OPERAND_VALUE, "Unsupported constructor input"
                )
            if name == "list":
                return self._emit_list_from_iter(iterable)
            if name == "set":
                return self._emit_set_from_iter(iterable)
            if name == "frozenset":
                return self._emit_frozenset_from_iter(iterable)
            exact = self._builtin_exact_type_from_expr(node.args[0])
            if exact == "tuple":
                return iterable
            if exact == "list":
                result = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp("TUPLE_FROM_LIST", [iterable], result))
                return result
            return self._emit_tuple_from_iter(iterable)
        if name == "len":
            raw_arg = node.args[0]
            if isinstance(raw_arg, ast.Constant) and isinstance(
                raw_arg.value, (str, bytes)
            ):
                return self._emit_const_value(len(raw_arg.value))
        # Fusion cannot move str(x) after evaluation of a later base operand:
        # conversion can raise even when its callback/lifetime facts are inert.
        str_source = (
            self._builtin_str_single_object_arg(node.args[0])
            if name == "int" and len(node.args) == 1
            else None
        )
        args = self._emit_call_args(
            [str_source] if str_source is not None else node.args
        )
        result = MoltValue(self.next_var(), type_hint=name if name != "len" else "int")
        if name in {"list", "tuple", "dict", "set", "frozenset"}:
            opcode = f"{name.upper()}_NEW" if not args else "DICT_FROM_OBJ"
        elif name == "len":
            opcode = "LEN"
        elif name == "bool":
            if not args:
                return self._emit_const_value(False)
            opcode = "BOOL"
        elif name == "str":
            return (
                self._emit_str_from_obj(args[0]) if args else self._emit_const_value("")
            )
        elif name == "float":
            if not args:
                return self._emit_const_value(0.0)
            opcode = "FLOAT_FROM_OBJ"
        elif name == "int":
            if not args:
                return self._emit_const_value(0)
            has_base = len(args) == 2
            if not has_base:
                args.append(self._emit_const_value(None))
            args.append(self._emit_const_value(has_base))
            opcode = "INT_FROM_STR_OF_OBJ" if str_source is not None else "INT_FROM_OBJ"
        elif name == "complex":
            if not args:
                args.append(self._emit_const_value(0.0))
            has_imag = len(args) == 2
            if not has_imag:
                args.append(self._emit_const_value(None))
            args.append(self._emit_const_value(has_imag))
            opcode = "COMPLEX_FROM_OBJ"
        elif name in {"bytes", "bytearray"}:
            if not args:
                if name == "bytes":
                    return self._emit_const_value(b"")
                args.append(self._emit_const_value(b""))
            if len(args) > 1:
                if len(args) == 2:
                    args.append(self._emit_const_value(None))
                opcode = f"{name.upper()}_FROM_STR"
            else:
                opcode = f"{name.upper()}_FROM_OBJ"
            if name == "bytearray":
                self._remember_bytearray_len_hint(
                    result, self.const_ints.get(args[0].name)
                )
        else:
            raise AssertionError(f"No builtin shape emitter for {name}")
        self.emit(MoltOp(opcode, args, result))
        return result

    def _try_emit_named_builtin_call(
        self, node: ast.Call, func_id: str, needs_bind: bool
    ) -> Any:
        if func_id in BUILTIN_SHAPE_NAMES:
            # Imported-name rewrites and legacy named dispatch are not a second
            # authority for this family. Preserve the actual source callable.
            lowered = self._try_emit_shape_builtin_call(node)
            if lowered is not CALL_NOT_HANDLED:
                return lowered
            callee = self.visit(node.func)
            if callee is None:
                raise FrontendRejection(
                    Diagnostic.CALL_TARGET, "Unsupported call target"
                )
            return self._emit_dynamic_call(node, callee)
        if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
            keyword.arg is None for keyword in node.keywords
        ):
            # Splat cardinality and duplicate/keyword errors belong to the
            # runtime binder. Individual builtin lowerers only see explicit
            # arguments; treating a starred operand as one argument is wrong.
            # Residual user names take the same generic path, without relying
            # on an incomplete builtin-name catalog to establish callability.
            callee = self.visit(node.func)
            if callee is None:
                raise FrontendRejection(
                    Diagnostic.CALL_TARGET, "Unsupported call target"
                )
            return self._emit_dynamic_call(node, callee)
        for lower in (
            self._try_emit_named_builtin_scalar_call,
            self._try_emit_named_builtin_iter_call,
            self._try_emit_named_builtin_constructor_call,
            self._try_emit_named_builtin_fallback_call,
        ):
            lowered = lower(node, func_id, needs_bind)
            if lowered is not CALL_NOT_HANDLED:
                return lowered
        return CALL_NOT_HANDLED
