"""SymbolNamingMixin: function symbols, code ids, and qualname stack helpers.

Move-only extraction from frontend/__init__.py. These helpers own the stable
symbol names and code-object ids shared by function, module, async, and
serialization lowering.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from molt.frontend._types import MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class SymbolNamingMixin(_MixinBase):
    def _function_symbol(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None and self.current_func_name == "molt_main":
            self.func_symbol_names[reserved] = name
            self._register_code_symbol(reserved)
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while symbol in self.funcs_map or f"{symbol}_poll" in self.funcs_map:
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        while (
            symbol in self.func_symbol_names
            or symbol in self.reserved_func_symbols.values()
            or symbol in self.reserved_external_func_symbols
            or f"{symbol}_poll" in self.funcs_map
        ):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.func_symbol_names[symbol] = name
        self._register_code_symbol(symbol)
        return symbol

    def _reserve_function_symbol(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while (
            symbol in self.funcs_map
            or f"{symbol}_poll" in self.funcs_map
            or symbol in self.func_symbol_names
            or symbol in self.reserved_func_symbols.values()
            or symbol in self.reserved_external_func_symbols
        ):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.reserved_func_symbols[name] = symbol
        self.func_symbol_names[symbol] = name
        self._register_code_symbol(symbol)
        return symbol

    def _lambda_symbol(self) -> str:
        self.lambda_counter += 1
        symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        while symbol in self.funcs_map:
            self.lambda_counter += 1
            symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        self.func_symbol_names[symbol] = "<lambda>"
        self._register_code_symbol(symbol)
        return symbol

    def _genexpr_symbol(self) -> str:
        self.genexpr_counter += 1
        symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        while symbol in self.funcs_map:
            self.genexpr_counter += 1
            symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        self.func_symbol_names[symbol] = "<genexpr>"
        self._register_code_symbol(symbol)
        return symbol

    def _register_code_symbol(self, symbol: str) -> int:
        code_id = self.func_code_ids.get(symbol)
        if code_id is None:
            code_id = self.code_id_counter
            self.func_code_ids[symbol] = code_id
            self.code_id_counter += 1
        return code_id

    def _code_symbol_for_value(self, func_val: MoltValue) -> str | None:
        hint = func_val.type_hint
        if isinstance(hint, str):
            if hint.startswith("Func:") or hint.startswith("ClosureFunc:"):
                return hint.split(":", 1)[1]
        return None

    def _qualname_prefix(self) -> str:
        if not self.qualname_stack:
            return ""
        parts: list[str] = []
        for name, is_function in self.qualname_stack:
            parts.append(name)
            if is_function:
                parts.append("<locals>")
        return ".".join(parts)

    def _qualname_for_def(self, name: str) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return name
        return f"{prefix}.{name}"

    def _push_qualname(self, name: str, is_function: bool) -> None:
        self.qualname_stack.append((name, is_function))

    def _pop_qualname(self) -> None:
        if self.qualname_stack:
            self.qualname_stack.pop()
