"""StringFormattingMixin: string conversion, format parsing, and template lowering.

Move-only extraction from frontend/__init__.py. This lowering authority owns
constant string extraction, object-to-string conversion helpers, f-string and
str.format token parsing/emission, format-spec rendering, and CPython 3.14
TemplateStr interpolation lowering shared by expression and call visitors.
"""

from __future__ import annotations

import ast
import string as _py_string
from typing import TYPE_CHECKING, Any

from molt.frontend._types import (
    FormatField,
    FormatLiteral,
    FormatParseState,
    FormatToken,
    MoltOp,
    MoltValue,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class StringFormattingMixin(_MixinBase):
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

    def _split_format_field_name(
        self, field_name: str
    ) -> tuple[int | str, list[tuple[bool, int | str]]] | None:
        if not field_name:
            return None
        idx = 0
        while idx < len(field_name) and field_name[idx] not in ".[":
            idx += 1
        first_text = field_name[:idx]
        if not first_text:
            return None
        if first_text.isdigit():
            first: int | str = int(first_text)
        else:
            first = first_text
        rest_items: list[tuple[bool, int | str]] = []
        while idx < len(field_name):
            ch = field_name[idx]
            if ch == ".":
                idx += 1
                start = idx
                while idx < len(field_name) and field_name[idx] not in ".[":
                    idx += 1
                if idx == start:
                    return None
                rest_items.append((True, field_name[start:idx]))
                continue
            if ch == "[":
                idx += 1
                start = idx
                while idx < len(field_name) and field_name[idx] != "]":
                    idx += 1
                if idx >= len(field_name):
                    return None
                key_text = field_name[start:idx]
                if not key_text:
                    return None
                if key_text.isdigit():
                    key: int | str = int(key_text)
                else:
                    key = key_text
                rest_items.append((False, key))
                idx += 1
                continue
            return None
        return first, rest_items

    def _parse_format_tokens(
        self,
        text: str,
        arg_count: int,
        kw_names: set[str],
        state: FormatParseState,
    ) -> list[FormatToken] | None:
        tokens: list[FormatToken] = []
        try:
            parsed = _py_string.Formatter().parse(text)
        except ValueError:
            return None
        for literal_text, field_name, format_spec, conversion in parsed:
            if literal_text:
                if tokens and isinstance(tokens[-1], FormatLiteral):
                    prior = tokens[-1]
                    tokens[-1] = FormatLiteral(prior.text + literal_text)
                else:
                    tokens.append(FormatLiteral(literal_text))
            if field_name is None:
                continue
            if conversion is not None and conversion not in {"r", "s", "a"}:
                return None
            if field_name == "":
                if state.used_manual:
                    return None
                state.used_auto = True
                key: int | str = state.next_auto
                state.next_auto += 1
                rest_items: list[tuple[bool, int | str]] = []
            else:
                if state.used_auto:
                    return None
                state.used_manual = True
                parsed_field = self._split_format_field_name(field_name)
                if parsed_field is None:
                    return None
                key, rest_items = parsed_field
            if isinstance(key, int):
                if key < 0 or key >= arg_count:
                    return None
            else:
                if key not in kw_names:
                    return None
            spec_tokens: list[FormatToken] | None = None
            if format_spec:
                spec_tokens = self._parse_format_tokens(
                    format_spec,
                    arg_count,
                    kw_names,
                    state,
                )
                if spec_tokens is None:
                    return None
            tokens.append(FormatField(key, rest_items, conversion, spec_tokens))
        return tokens

    def _emit_format_tokens(
        self,
        tokens: list[FormatToken],
        args: list[MoltValue],
        kwargs: dict[str, MoltValue],
    ) -> MoltValue:
        parts: list[MoltValue] = []
        for token in tokens:
            if isinstance(token, FormatLiteral):
                parts.append(self._emit_const_value(token.text))
                continue
            if isinstance(token.key, int):
                value = args[token.key]
            else:
                value = kwargs[token.key]
            for is_attr, name in token.rest:
                if is_attr:
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="GETATTR_GENERIC_OBJ",
                            args=[value, name],
                            result=res,
                        )
                    )
                    value = res
                else:
                    key_val = self._emit_const_value(name)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[value, key_val], result=res))
                    value = res
            if token.conversion is not None:
                if token.conversion == "r":
                    value = self._emit_repr_from_obj(value)
                elif token.conversion == "s":
                    value = self._emit_str_from_obj(value)
                elif token.conversion == "a":
                    value = self._emit_ascii_from_obj(value)
            if token.format_spec is None:
                spec_val = self._emit_const_value("")
            else:
                spec_val = self._emit_format_tokens(token.format_spec, args, kwargs)
            parts.append(self._emit_string_format_value(value, spec_val))
        return self._emit_string_join(parts)

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
                        raise NotImplementedError(
                            "Unsupported f-string format spec value"
                        )
                    if item.conversion != -1:
                        if item.conversion == ord("r"):
                            value = self._emit_repr_from_obj(value)
                        elif item.conversion == ord("s"):
                            value = self._emit_str_from_obj(value)
                        elif item.conversion == ord("a"):
                            value = self._emit_ascii_from_obj(value)
                        else:
                            raise NotImplementedError(
                                "Formatted value conversion not supported"
                            )
                    if item.format_spec is None:
                        parts.append(self._emit_string_format(value, ""))
                    else:
                        spec_val = self._emit_format_spec_value(item.format_spec)
                        parts.append(self._emit_string_format_value(value, spec_val))
                    continue
                raise NotImplementedError("Unsupported f-string format spec segment")
            return self._emit_string_join(parts)
        spec_val = self.visit(node)
        if spec_val is None:
            raise NotImplementedError("Unsupported f-string format spec")
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
            raise NotImplementedError("Unsupported t-string interpolation value")
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
            raise NotImplementedError("Unsupported t-string interpolation conversion")
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
