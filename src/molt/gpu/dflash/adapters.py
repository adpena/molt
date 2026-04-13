"""Typed registry and resolver for DFlash-compatible draft adapters.

Adapters are intentionally model-specific and may come from external sources
over time. Molt core only keeps a lightweight registry boundary here.
"""

from __future__ import annotations

from .contracts import DFlashRuntime, DFlashSelectionContext


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


def _adapter_supports(spec: DFlashAdapterSpec, context) -> bool:
    supported = spec.supports(context)
    if not isinstance(supported, bool):
        raise TypeError("dflash adapter supports() must return bool")
    return supported


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
    return adapter if _adapter_supports(adapter, context) else None


def resolve_default_dflash_adapter(context):
    if not has_dflash_backend(context.backend):
        return None
    candidates = []
    for name in list_dflash_adapters():
        adapter = get_dflash_adapter(name)
        if adapter is not None and _adapter_supports(adapter, context):
            candidates.append(adapter)
    if not candidates:
        return None
    candidates.sort(key=lambda adapter: (-adapter.priority, adapter.name))
    top = candidates[0]
    tied = [adapter for adapter in candidates if adapter.priority == top.priority]
    if len(tied) > 1:
        names = ", ".join(adapter.name for adapter in tied)
        raise ValueError(f"multiple dflash adapters match with the same priority: {names}")
    return top


def resolve_dflash_runtime(context, preferred_name: str | None = None):
    if preferred_name is None:
        adapter = resolve_default_dflash_adapter(context)
    else:
        adapter = resolve_dflash_adapter(context, preferred_name=preferred_name)
    if adapter is None:
        return None
    runtime = adapter.create_runtime(context)
    if not isinstance(runtime, DFlashRuntime):
        raise TypeError("dflash adapter create_runtime() must return DFlashRuntime")
    return runtime


def build_dflash_runtime(
    model,
    prompt_tokens,
    *,
    backend: str | None,
    dflash_adapter: str | None = None,
    eos_token_id=None,
    max_new_tokens: int = 100,
    block_size: int = 16,
):
    context = DFlashSelectionContext(
        model=model,
        backend=backend,
        prompt_tokens=prompt_tokens,
        eos_token_id=eos_token_id,
        max_new_tokens=max_new_tokens,
        block_size=block_size,
    )
    runtime = resolve_dflash_runtime(context, preferred_name=dflash_adapter)
    if runtime is not None:
        return runtime
    if dflash_adapter is not None:
        raise LookupError(f"dflash adapter '{dflash_adapter}' is unavailable for this context")
    return None
