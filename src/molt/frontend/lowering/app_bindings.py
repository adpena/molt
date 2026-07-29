"""Frontend-owned entry-module callable binding authority.

The linker needs the final Python binding state, not guesses derived from
backend symbol spellings.  These hooks run on the frontend's real publication,
import-resolution, and deletion paths and serialize one resolved table onto the
entry module initializer in TIR.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any

from molt.frontend._types import MoltValue
from molt.frontend.sema import (
    FunctionKind,
    normalize_function_kind,
    parse_stateful_function_type_hint,
)

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class AppBindingAuthorityMixin(_MixinBase):
    def _is_entry_app_binding_scope(self) -> bool:
        return (
            self.current_func_name == "molt_main"
            and self.entry_module is not None
            and self.module_name == self.entry_module
        )

    def _app_callable_from_value(
        self, value: MoltValue
    ) -> tuple[FunctionKind, str] | None:
        hint = value.type_hint
        if isinstance(hint, str):
            for prefix in ("Func:", "ClosureFunc:"):
                if hint.startswith(prefix):
                    symbol = hint.removeprefix(prefix)
                    if symbol:
                        return FunctionKind.SYNC, symbol
        stateful = parse_stateful_function_type_hint(hint)
        if stateful is not None:
            return stateful.kind, stateful.poll_symbol
        return None

    @staticmethod
    def _app_binding_symbols(binding: Mapping[str, Any] | None) -> list[str]:
        if binding is None:
            return []
        symbols: list[str] = []
        symbol = binding.get("symbol")
        if isinstance(symbol, str) and symbol:
            symbols.append(symbol)
        superseded = binding.get("superseded_symbols")
        if isinstance(superseded, list):
            symbols.extend(
                item for item in superseded if isinstance(item, str) and item
            )
        return list(dict.fromkeys(symbols))

    def _replace_app_callable_binding(
        self,
        name: str,
        *,
        kind: FunctionKind | str,
        origin: str,
        disposition: str,
        reason: str | None,
        symbol: str | None,
        imported_from: str | None = None,
    ) -> None:
        if not self._is_entry_app_binding_scope():
            return
        normalized_kind = normalize_function_kind(kind)
        kind_value = normalized_kind.value if normalized_kind is not None else str(kind)
        previous = self.app_callable_bindings.get(name)
        superseded = self._app_binding_symbols(previous)
        if symbol is not None:
            superseded = [item for item in superseded if item != symbol]
        binding: dict[str, Any] = {
            "name": name,
            "qualified_name": f"{self.entry_module}.{name}",
            "kind": kind_value,
            "origin": origin,
            "disposition": disposition,
            "reason": reason,
            "symbol": symbol,
            "superseded_symbols": superseded,
        }
        if imported_from is not None:
            binding["imported_from"] = imported_from
        self.app_callable_bindings[name] = binding

    def _record_app_module_store(self, name: str, value: MoltValue) -> None:
        """Record a real entry-module store not yet classified by its producer."""

        if not self._is_entry_app_binding_scope() or name.startswith("__molt_"):
            return
        callable_binding = self._app_callable_from_value(value)
        previous = self.app_callable_bindings.get(name)
        if callable_binding is None:
            if previous is None:
                return
            self._replace_app_callable_binding(
                name,
                kind=str(previous.get("kind", "unknown")),
                origin="rebound",
                disposition="excluded",
                reason="dynamic-rebound-binding",
                symbol=None,
            )
            return
        kind, symbol = callable_binding
        self._replace_app_callable_binding(
            name,
            kind=kind,
            origin="dynamic_alias",
            disposition="excluded",
            reason=(
                "private-name"
                if name.startswith("_")
                else "dynamic-callable-alias-requires-module-dispatch"
            ),
            symbol=symbol,
        )

    def _record_source_app_callable(
        self,
        name: str,
        *,
        kind: FunctionKind,
        symbol: str,
        decorated: bool,
    ) -> None:
        if not self._is_entry_app_binding_scope():
            return
        disposition = "excluded"
        reason: str | None
        final_symbol: str | None = symbol
        if name.startswith("_"):
            reason = "private-name"
        elif decorated:
            reason = "decorated-binding-requires-module-dispatch"
            final_symbol = None
        elif kind != FunctionKind.SYNC:
            reason = "stateful-callable-requires-module-dispatch"
        elif self.control_flow_depth > 0:
            reason = "dynamic-module-binding-requires-module-dispatch"
        else:
            disposition = "export"
            reason = None
        self._replace_app_callable_binding(
            name,
            kind=kind,
            origin="source_function",
            disposition=disposition,
            reason=reason,
            symbol=final_symbol,
        )
        if decorated:
            binding = self.app_callable_bindings[name]
            superseded = self._app_binding_symbols(binding)
            if symbol not in superseded:
                superseded.append(symbol)
            binding["superseded_symbols"] = superseded

    def _record_imported_app_callable(
        self,
        name: str,
        *,
        module_name: str,
        attr_name: str,
        value: MoltValue,
        relative: bool,
    ) -> None:
        callable_binding = self._app_callable_from_value(value)
        if not self._is_entry_app_binding_scope() or callable_binding is None:
            return
        kind, symbol = callable_binding
        self._replace_app_callable_binding(
            name,
            kind=kind,
            origin="imported_function",
            disposition="excluded",
            reason=(
                "relative-import-binding-requires-module-dispatch"
                if relative
                else "imported-binding-requires-module-dispatch"
            ),
            symbol=symbol,
            imported_from=f"{module_name}.{attr_name}",
        )

    def _record_deleted_app_binding(self, name: str) -> None:
        if not self._is_entry_app_binding_scope():
            return
        previous = self.app_callable_bindings.get(name)
        if previous is None:
            declared_kind = normalize_function_kind(
                self.module_declared_funcs.get(name)
            )
            if declared_kind is None:
                return
            kind: FunctionKind | str = declared_kind
        else:
            kind = str(previous.get("kind", "unknown"))
        self._replace_app_callable_binding(
            name,
            kind=kind,
            origin="deleted",
            disposition="excluded",
            reason="deleted-binding",
            symbol=None,
        )
