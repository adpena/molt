"""StringFormattingMixin: string conversion, f-string, and template lowering.

This lowering authority owns constant string extraction, object-to-string
conversion helpers, f-string format-spec rendering, and CPython 3.14 TemplateStr
interpolation lowering shared by expression and call visitors. str.format uses
ordinary call lowering and the canonical runtime field parser.
"""

from __future__ import annotations

import ast
from typing import Any

from molt.frontend._types import (
    MoltOp,
    MoltValue,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection
from molt.frontend._mixin_base import GeneratorMixinBase


class StringFormattingMixin(GeneratorMixinBase):
    @staticmethod
    def _try_extract_const_str(node: ast.expr) -> str | None:
        """Recursively extract a constant string from an AST node.

        Handles plain string constants and chained Add operations
        over string constants (e.g. ``"a" + "b" + "c"``).
        """
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = StringFormattingMixin._try_extract_const_str(node.left)
            if left is None:
                return None
            right = StringFormattingMixin._try_extract_const_str(node.right)
            if right is None:
                return None
            return left + right
        return None

    def _emit_str_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_repr_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="REPR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_ascii_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="ASCII_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_string_join(self, parts: list[MoltValue]) -> MoltValue:
        if not parts:
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
            return res
        if len(parts) == 1:
            return parts[0]
        sep = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=sep))
        items = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=res))
        return res

    def _emit_string_format_value(self, value: MoltValue, spec: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_FORMAT", args=[value, spec], result=res))
        return res

    def _emit_string_format(self, value: MoltValue, spec: str) -> MoltValue:
        spec_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[spec], result=spec_val))
        return self._emit_string_format_value(value, spec_val)

    def _emit_format_spec_value(self, node: ast.expr) -> MoltValue:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            spec_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.value], result=spec_val))
            return spec_val
        if isinstance(node, ast.JoinedStr):
            parts: list[MoltValue] = []
            for item in node.values:
                if isinstance(item, ast.Constant) and isinstance(item.value, str):
                    lit = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[item.value], result=lit))
                    parts.append(lit)
                    continue
                if isinstance(item, ast.FormattedValue):
                    value = self.visit(item.value)
                    if value is None:
                        raise FrontendRejection(
                            Diagnostic.SYNTAX_FORM,
                            "Unsupported f-string format spec value",
                        )
                    if item.conversion != -1:
                        if item.conversion == ord("r"):
                            value = self._emit_repr_from_obj(value)
                        elif item.conversion == ord("s"):
                            value = self._emit_str_from_obj(value)
                        elif item.conversion == ord("a"):
                            value = self._emit_ascii_from_obj(value)
                        else:
                            raise FrontendRejection(
                                Diagnostic.OPERAND_VALUE,
                                "Formatted value conversion not supported",
                            )
                    if item.format_spec is None:
                        parts.append(self._emit_string_format(value, ""))
                    else:
                        spec_val = self._emit_format_spec_value(item.format_spec)
                        parts.append(self._emit_string_format_value(value, spec_val))
                    continue
                raise FrontendRejection(
                    Diagnostic.SYNTAX_FORM,
                    "Unsupported f-string format spec segment",
                )
            return self._emit_string_join(parts)
        spec_val = self.visit(node)
        if spec_val is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM, "Unsupported f-string format spec"
            )
        return self._emit_str_from_obj(spec_val)

    def _emit_template_interpolation(self, node: Any) -> MoltValue:
        """Lower a single ``ast.Interpolation`` inside a ``t"..."`` literal.

        Constructs a ``string.templatelib.Interpolation`` instance with
        ``(value, expression, conversion, format_spec)`` matching CPython 3.14
        semantics. ``conversion`` is the single-letter str ('s'/'r'/'a') or
        ``None``; ``format_spec`` is the rendered format-spec text or ``""``.
        """
        value = self.visit(node.value)
        if value is None:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported t-string interpolation value",
            )
        # expression — the literal source text of the interpolated expression.
        expression_text = node.str if node.str is not None else ""
        expression_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(kind="CONST_STR", args=[expression_text], result=expression_val)
        )
        # conversion — None for -1, otherwise single-char str.
        conversion = node.conversion
        if conversion == -1:
            conversion_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=conversion_val))
        elif conversion in (ord("s"), ord("r"), ord("a")):
            conversion_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(
                    kind="CONST_STR",
                    args=[chr(conversion)],
                    result=conversion_val,
                )
            )
        else:
            raise FrontendRejection(
                Diagnostic.SYNTAX_FORM,
                "Unsupported t-string interpolation conversion",
            )
        # format_spec — rendered to str via shared f-string format-spec helper.
        if node.format_spec is None:
            format_spec_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=format_spec_val))
        else:
            format_spec_val = self._emit_format_spec_value(node.format_spec)
        # Construct ``Interpolation(value, expression, conversion, format_spec)``.
        interp_class = self._emit_module_attr_get_on(
            "string.templatelib", "Interpolation"
        )
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        for arg in (value, expression_val, conversion_val, format_spec_val):
            push_res = MoltValue(self.next_var(), type_hint="None")
            self.emit(
                MoltOp(
                    kind="CALLARGS_PUSH_POS",
                    args=[callargs, arg],
                    result=push_res,
                )
            )
        interp_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="CALL_BIND",
                args=[interp_class, callargs],
                result=interp_val,
            )
        )
        return interp_val
