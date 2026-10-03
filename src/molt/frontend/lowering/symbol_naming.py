"""SymbolNamingMixin: function symbols, code ids, and qualname stack helpers.

Move-only extraction from frontend/__init__.py. These helpers own the stable
symbol names and code-object ids shared by function, module, async, and
serialization lowering.
"""

from __future__ import annotations

import ast

from molt.python_private_names import python_definition_name
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend.sema import FunctionKind, STATEFUL_FUNCTION_KINDS


class SymbolNamingMixin(GeneratorMixinBase):
    @staticmethod
    def _sanitize_module_name(name: str) -> str:
        out: list[str] = []
        for ch in name:
            if ch.isalnum() or ch == "_":
                out.append(ch)
            else:
                out.append("_")
        if not out:
            return "module"
        return "".join(out)

    @classmethod
    def module_init_symbol(cls, name: str) -> str:
        return f"molt_init_{cls._sanitize_module_name(name)}"

    def _function_symbol_in_use(
        self, symbol: str, *, kind: FunctionKind = FunctionKind.SYNC
    ) -> bool:
        # Source reservations, allocated helpers, and materialized functions
        # share one namespace. Stateful callables own their poll target from
        # reservation onward, before either body has been emitted.
        symbols = (
            (symbol, f"{symbol}_poll") if kind in STATEFUL_FUNCTION_KINDS else (symbol,)
        )
        return (
            any(
                candidate in self.funcs_map
                or candidate in self.func_symbol_names
                or candidate in self.reserved_func_symbols.values()
                for candidate in symbols
            )
            or f"{symbol}_poll" in self.funcs_map
        )

    def _claim_function_symbol(
        self, symbol: str, name: str, *, kind: FunctionKind = FunctionKind.SYNC
    ) -> None:
        self.func_symbol_names[symbol] = name
        if kind in STATEFUL_FUNCTION_KINDS:
            self.func_symbol_names[f"{symbol}_poll"] = name
        self._register_code_symbol(symbol)

    def _function_symbol(
        self,
        name: str,
        *,
        kind: FunctionKind = FunctionKind.SYNC,
        reuse_reserved: bool = False,
    ) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if (
            reuse_reserved
            and reserved is not None
            and self.current_func_name == "molt_main"
            and reserved not in self.funcs_map
            and not (
                f"{reserved}_poll" in self.funcs_map
                and self.func_symbol_names.get(f"{reserved}_poll") == name
            )
            and (
                kind not in STATEFUL_FUNCTION_KINDS
                or self.func_symbol_names.get(f"{reserved}_poll") == name
                or not self._function_symbol_in_use(f"{reserved}_poll")
            )
        ):
            self._claim_function_symbol(reserved, name, kind=kind)
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while self._function_symbol_in_use(symbol, kind=kind):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self._claim_function_symbol(symbol, name, kind=kind)
        return symbol

    def _reserve_function_symbol(
        self, name: str, *, kind: FunctionKind = FunctionKind.SYNC
    ) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while self._function_symbol_in_use(symbol, kind=kind):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.reserved_func_symbols[name] = symbol
        self._claim_function_symbol(symbol, name, kind=kind)
        return symbol

    def _lambda_symbol(self, *, kind: FunctionKind = FunctionKind.SYNC) -> str:
        self.lambda_counter += 1
        symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        while self._function_symbol_in_use(symbol, kind=kind):
            self.lambda_counter += 1
            symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        self._claim_function_symbol(symbol, "<lambda>", kind=kind)
        return symbol

    def _genexpr_symbol(self) -> str:
        self.genexpr_counter += 1
        symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        while self._function_symbol_in_use(symbol, kind=FunctionKind.GENERATOR):
            self.genexpr_counter += 1
            symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        self._claim_function_symbol(symbol, "<genexpr>", kind=FunctionKind.GENERATOR)
        return symbol

    def _register_code_symbol(self, symbol: str) -> int:
        code_id = self.func_code_ids.get(symbol)
        if code_id is None:
            code_id = self.code_id_counter
            self.func_code_ids[symbol] = code_id
            self.code_id_counter += 1
        return code_id

    def _qualname_prefix(self) -> str:
        if not self.qualname_stack:
            return ""
        name, is_function = self.qualname_stack[-1]
        return f"{name}.<locals>" if is_function else name

    def _definition_qualname(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef
    ) -> str:
        name = python_definition_name(node)
        if self._binding_targets_module_namespace(node.name):
            return name
        return self._qualname_for_def(name)

    def _qualname_for_def(self, name: str) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return name
        return f"{prefix}.{name}"

    def _push_qualname(
        self, name: str, is_function: bool, *, qualname: str | None = None
    ) -> None:
        # Store complete lexical names. A global declaration can reset the
        # enclosing prefix, and its children must inherit that reset as well.
        if qualname is None:
            qualname = self._qualname_for_def(name)
        self.qualname_stack.append((qualname, is_function))

    def _pop_qualname(self) -> None:
        if self.qualname_stack:
            self.qualname_stack.pop()
