"""Typed registry and resolver for DFlash-compatible draft adapters.

Adapters are intentionally model-specific and may come from external sources
over time. Molt core only keeps a lightweight registry boundary here.
"""

from __future__ import annotations

from .contracts import DFlashRuntime


_DFLASH_ADAPTERS = {}


class DFlashAdapterSpec:
    """Typed adapter record for model/target-specific DFlash integrations."""

    def __init__(
        self,
        *,
        name: str,
        supports,
        create_runtime,
        priority: int = 0,
    ) -> None:
        if not name:
            raise ValueError("adapter name must be non-empty")
        if not callable(supports):
            raise TypeError("dflash adapter supports must be callable")
        if not callable(create_runtime):
            raise TypeError("dflash adapter create_runtime must be callable")
        self.name = name
        self.supports = supports
        self.create_runtime = create_runtime
        self.priority = priority


def register_dflash_adapter(spec: DFlashAdapterSpec) -> None:
    if not isinstance(spec, DFlashAdapterSpec):
        raise TypeError("register_dflash_adapter expects DFlashAdapterSpec")
    if spec.name in _DFLASH_ADAPTERS:
        raise ValueError(f"dflash adapter '{spec.name}' is already registered")
    _DFLASH_ADAPTERS[spec.name] = spec


def get_dflash_adapter(name: str):
    return _DFLASH_ADAPTERS.get(name)


def list_dflash_adapters():
    return sorted(_DFLASH_ADAPTERS)


def has_dflash_backend(backend: str | None) -> bool:
    if backend is None:
        return False
    return backend.strip() != ""


def resolve_dflash_adapter(context, preferred_name: str | None = None):
    backend = context.backend
    if not has_dflash_backend(backend):
        return None
    if preferred_name is None:
        return None

    adapter = get_dflash_adapter(preferred_name)
    if adapter is None:
        return None
    return adapter if bool(adapter.supports(context)) else None


def resolve_dflash_runtime(context, preferred_name: str | None = None):
    adapter = resolve_dflash_adapter(context, preferred_name=preferred_name)
    if adapter is None:
        return None
    runtime = adapter.create_runtime(context)
    if not isinstance(runtime, DFlashRuntime):
        raise TypeError("dflash adapter create_runtime() must return DFlashRuntime")
    return runtime
