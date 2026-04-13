"""Registry for DFlash-compatible draft adapters.

Adapters are intentionally model-specific and may come from external sources
over time. Molt core only keeps a lightweight registry boundary here.
"""

from __future__ import annotations


_DFLASH_ADAPTERS = {}


def register_dflash_adapter(name: str, adapter) -> None:
    if not name:
        raise ValueError("adapter name must be non-empty")
    _DFLASH_ADAPTERS[name] = adapter


def get_dflash_adapter(name: str):
    return _DFLASH_ADAPTERS.get(name)


def list_dflash_adapters():
    return sorted(_DFLASH_ADAPTERS)
