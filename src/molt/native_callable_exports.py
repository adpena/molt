"""Canonical admission and transport model for native callable exports."""

from __future__ import annotations

from dataclasses import dataclass
import re
from typing import Any, Mapping

from molt.native_callable_abi import (
    native_callable_fixed_arity,
    native_callable_is_python_export,
    native_callable_python_export_abi_choices,
    native_callable_requires_direct_symbol_binding,
    native_callable_requires_explicit_export_arity,
    normalize_native_callable_abi,
)
from molt.python_module_names import canonical_python_module_name


class NativeCallableExportError(ValueError):
    """A native callable export failed canonical admission."""


_NATIVE_SYMBOL_RE = re.compile(r"[A-Za-z_.$][A-Za-z0-9_.$@]*")


@dataclass(frozen=True)
class NativeCallableExport:
    module: str
    name: str
    binding: str
    abi: str
    symbol: str | None = None
    provider_module: str | None = None
    arity: int | None = None
    effects: tuple[str, ...] = ()
    deterministic: bool = False

    @property
    def qualified_name(self) -> str:
        return f"{self.module}.{self.name}"

    @property
    def wrapper_payload_arity(self) -> int:
        if self.binding != "direct_symbol":
            raise NativeCallableExportError(
                f"native callable export {self.qualified_name!r} has no direct wrapper"
            )
        fixed = native_callable_fixed_arity(self.abi)
        if fixed is not None:
            return fixed
        if self.arity is None:
            raise NativeCallableExportError(
                f"native callable export {self.qualified_name!r} requires arity"
            )
        return self.arity

    def digest_payload(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "module": self.module,
            "name": self.name,
            "binding": self.binding,
            "abi": self.abi,
            "effects": list(self.effects),
            "deterministic": self.deterministic,
        }
        if self.symbol is not None:
            payload["symbol"] = self.symbol
        if self.provider_module is not None:
            payload["provider_module"] = self.provider_module
        if self.arity is not None:
            payload["arity"] = self.arity
        return payload


def normalize_native_callable_export(
    spec: Mapping[str, Any],
    *,
    qualified_name: str | None = None,
) -> NativeCallableExport:
    module = spec.get("module")
    name = spec.get("name")
    binding = spec.get("binding")
    symbol = spec.get("symbol")
    provider_module = spec.get("provider_module")
    arity = spec.get("arity")
    effects = spec.get("effects", ())
    deterministic = spec.get("deterministic", False)

    try:
        module = canonical_python_module_name(module, field="module")
        name = canonical_python_module_name(name, field="name")
    except ValueError as exc:
        raise NativeCallableExportError(str(exc)) from exc
    if "." in name:
        raise NativeCallableExportError("name must be a Python identifier")
    actual_qualified_name = f"{module}.{name}"
    if qualified_name is not None and qualified_name != actual_qualified_name:
        raise NativeCallableExportError(
            f"map key {qualified_name!r} does not match {actual_qualified_name!r}"
        )
    if binding not in {"module_attr", "direct_symbol"}:
        raise NativeCallableExportError(
            "binding must be 'module_attr' or 'direct_symbol'"
        )
    abi = normalize_native_callable_abi(spec.get("abi"))
    if abi is None:
        raise NativeCallableExportError(
            f"abi must be one of: {native_callable_python_export_abi_choices()}"
        )
    if not native_callable_is_python_export(abi):
        raise NativeCallableExportError(
            f"abi {abi!r} is internal and cannot be a Python callable export"
        )
    if binding == "module_attr" and native_callable_requires_direct_symbol_binding(abi):
        raise NativeCallableExportError(
            f"module_attr binding cannot use direct-symbol ABI {abi!r}"
        )

    normalized_symbol: str | None
    if symbol is None:
        normalized_symbol = None
    elif isinstance(symbol, str) and symbol.strip():
        normalized_symbol = symbol.strip()
    else:
        raise NativeCallableExportError("symbol must be a non-empty string")
    if (
        normalized_symbol is not None
        and _NATIVE_SYMBOL_RE.fullmatch(normalized_symbol) is None
    ):
        raise NativeCallableExportError(
            f"symbol has invalid native symbol {normalized_symbol!r}"
        )
    if binding == "direct_symbol" and normalized_symbol is None:
        raise NativeCallableExportError("direct_symbol binding requires symbol")
    if binding == "module_attr" and normalized_symbol is not None:
        raise NativeCallableExportError(
            "symbol is only valid for direct_symbol binding"
        )

    normalized_provider: str | None
    if provider_module is None:
        normalized_provider = None
    else:
        try:
            normalized_provider = canonical_python_module_name(
                provider_module, field="provider_module"
            )
        except ValueError as exc:
            raise NativeCallableExportError(str(exc)) from exc
    if binding != "module_attr" and normalized_provider is not None:
        raise NativeCallableExportError(
            "provider_module is only valid for module_attr binding"
        )

    if arity is not None and (
        not isinstance(arity, int) or isinstance(arity, bool) or arity < 0
    ):
        raise NativeCallableExportError("arity must be a non-negative integer")
    if binding == "direct_symbol" and native_callable_requires_explicit_export_arity(
        abi
    ):
        if arity is None:
            raise NativeCallableExportError(
                f"direct_symbol ABI {abi!r} requires explicit arity"
            )
    elif arity is not None:
        raise NativeCallableExportError(
            "arity is only valid for a variadic direct_symbol ABI"
        )

    if not isinstance(effects, (list, tuple)) or any(
        not isinstance(effect, str) or not effect.strip() for effect in effects
    ):
        raise NativeCallableExportError("effects must be a list of non-empty strings")
    if not isinstance(deterministic, bool):
        raise NativeCallableExportError("deterministic must be boolean")

    return NativeCallableExport(
        module=module,
        name=name,
        binding=binding,
        abi=abi,
        symbol=normalized_symbol,
        provider_module=normalized_provider,
        arity=arity,
        effects=tuple(sorted({effect.strip() for effect in effects})),
        deterministic=deterministic,
    )
