"""Registry and resolver for DFlash-compatible draft adapters.

Adapters are intentionally model-specific and may come from external sources
over time. Molt core only keeps a lightweight registry boundary here.
"""

from __future__ import annotations

from .contracts import DFlashRuntime


_DFLASH_ADAPTERS = {}


def register_dflash_adapter(name: str, adapter) -> None:
    if not name:
        raise ValueError("adapter name must be non-empty")
    _DFLASH_ADAPTERS[name] = adapter


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
    resolved_backend = backend.strip().lower()

    def _adapter_matches(adapter) -> bool:
        supports = getattr(adapter, "supports", None)
        if callable(supports):
            return bool(supports(context))
        matcher = getattr(adapter, "matches", None)
        if callable(matcher):
            return bool(matcher(context.model, resolved_backend))
        return True

    if preferred_name is not None:
        adapter = get_dflash_adapter(preferred_name)
        if adapter is None:
            return None
        return adapter if _adapter_matches(adapter) else None
    return None


def resolve_dflash_runtime(context, preferred_name: str | None = None):
    adapter = resolve_dflash_adapter(context, preferred_name=preferred_name)
    if adapter is None:
        return None
    if not hasattr(adapter, "create_runtime"):
        raise TypeError("dflash adapter must define create_runtime()")
    runtime = adapter.create_runtime(
        context.model,
        list(context.prompt_tokens),
        eos_token_id=context.eos_token_id,
        max_new_tokens=context.max_new_tokens,
        block_size=context.block_size,
        backend=context.backend,
    )
    if not isinstance(runtime, DFlashRuntime):
        raise TypeError("dflash adapter create_runtime() must return DFlashRuntime")
    return runtime
