"""SymbolNamingMixin: function symbols, code ids, and qualname stack helpers.

Move-only extraction from frontend/__init__.py. These helpers own the stable
symbol names and code-object ids shared by function, module, async, and
serialization lowering.
"""

from __future__ import annotations

import ast

from molt.python_private_names import python_definition_name
from molt.frontend._mixin_base import GeneratorMixinBase
from molt.frontend._types import CallableId
from molt.compiler_analysis.python_source_keys import python_node_source_key
from molt.compiler_analysis.python_lexical_scope import function_contains_yield
from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity
from molt.compiler_analysis.python_function_kind import (
    FunctionKind,
    STATEFUL_FUNCTION_KINDS,
)


class SymbolNamingMixin(GeneratorMixinBase):
    def _register_source_callable(
        self, node: ast.FunctionDef | ast.Lambda, symbol: str
    ) -> None:
        self.source_callable_symbols[
            CallableId(self.module_name, python_node_source_key(node))
        ] = symbol

    def _source_callable_candidate(self, node: ast.expr) -> str | None:
        """Resolve a code candidate from typed provenance, never a name hint.

        A candidate still requires the existing runtime callable guard unless
        the source-point binding fact proves its exact value and lifetime.
        """
        index = self.python_binding_index
        fact = index.expression_fact(node) if index is not None else None
        if fact is None or not fact.identities & int(PythonIdentity.USER_FUNCTION):
            return None
        if len(fact.result.source_origins) != 1:
            return None
        (source,) = fact.result.source_origins
        return self.source_callable_symbols.get(CallableId(self.module_name, source))

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
        self, name: str, *, kind: FunctionKind = FunctionKind.SYNC
    ) -> str:
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while self._function_symbol_in_use(symbol, kind=kind):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self._claim_function_symbol(symbol, name, kind=kind)
        return symbol

    def _source_definition_symbol(
        self,
        node: ast.FunctionDef | ast.AsyncFunctionDef,
        *,
        kind: FunctionKind = FunctionKind.SYNC,
    ) -> str:
        identity = CallableId(self.module_name, python_node_source_key(node))
        reserved = self.reserved_func_symbols.get(identity)
        if (
            reserved is not None
            and self.current_func_name == "molt_main"
            and reserved not in self.funcs_map
            and f"{reserved}_poll" not in self.funcs_map
        ):
            return reserved
        return self._function_symbol(node.name, kind=kind)

    def _reserve_source_callables(self, tree: ast.Module) -> None:
        """Reserve code slots by source definition before lowering any body."""
        pending = list(reversed(tree.body))
        while pending:
            node = pending.pop()
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if node.name in self.module_elided_deleted_funcs:
                    continue
                yields = function_contains_yield(node)
                kind = (
                    (FunctionKind.ASYNC_GENERATOR if yields else FunctionKind.ASYNC)
                    if isinstance(node, ast.AsyncFunctionDef)
                    else (FunctionKind.GENERATOR if yields else FunctionKind.SYNC)
                )
                identity = CallableId(self.module_name, python_node_source_key(node))
                symbol = self._function_symbol(node.name, kind=kind)
                self.reserved_func_symbols[identity] = symbol
                if kind is FunctionKind.SYNC and not getattr(node, "type_params", ()):
                    self._register_source_callable(node, symbol)
                    self._record_func_default_specs(symbol, node.args)
                continue
            if isinstance(node, (ast.ClassDef, ast.Lambda)):
                continue
            pending.extend(reversed(list(ast.iter_child_nodes(node))))

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
            code_id = len(self.func_code_ids)
            self.func_code_ids[symbol] = code_id
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
