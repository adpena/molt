"""Public API surface shim for ``asyncio.base_futures``."""

from __future__ import annotations

import reprlib


from . import format_helpers


def isfuture(obj) -> bool:
    return (
        hasattr(obj.__class__, "_asyncio_future_blocking")
        and obj._asyncio_future_blocking is not None
    )


__all__ = ["format_helpers", "isfuture", "reprlib"]
