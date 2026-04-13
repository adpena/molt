"""Registry for DFlash-compatible draft adapters.

Adapters are intentionally model-specific and may come from external sources
over time. Molt core only keeps a lightweight registry boundary here.
"""

from __future__ import annotations


_DFLASH_ADAPTERS = {}
_SUPPORTED_GPU_BACKENDS = {"webgpu", "metal", "cuda", "hip", "amd"}


def register_dflash_adapter(name: str, adapter) -> None:
    if not name:
        raise ValueError("adapter name must be non-empty")
    _DFLASH_ADAPTERS[name] = adapter


def get_dflash_adapter(name: str):
    return _DFLASH_ADAPTERS.get(name)


def list_dflash_adapters():
    return sorted(_DFLASH_ADAPTERS)


def is_supported_dflash_backend(backend: str | None) -> bool:
    if backend is None:
        return False
    return backend.strip().lower() in _SUPPORTED_GPU_BACKENDS


def resolve_dflash_adapter(model, backend: str | None, preferred_name: str | None = None):
    if not is_supported_dflash_backend(backend):
        return None
    resolved_backend = backend.strip().lower()

    def _adapter_matches(adapter) -> bool:
        supported = getattr(adapter, "supported_backends", None)
        if supported is not None and resolved_backend not in supported:
            return False
        matcher = getattr(adapter, "matches", None)
        if callable(matcher):
            return bool(matcher(model, resolved_backend))
        return True

    if preferred_name is not None:
        adapter = get_dflash_adapter(preferred_name)
        if adapter is None:
            return None
        return adapter if _adapter_matches(adapter) else None

    for name in list_dflash_adapters():
        adapter = get_dflash_adapter(name)
        if adapter is not None and _adapter_matches(adapter):
            return adapter
    return None
