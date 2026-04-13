"""DFlash adapter and contract surface for Molt GPU generation."""

from .adapters import (
    get_dflash_adapter,
    has_dflash_backend,
    list_dflash_adapters,
    register_dflash_adapter,
    resolve_dflash_adapter,
    resolve_dflash_runtime,
)
from .contracts import (
    DFlashRuntime,
    DFlashSelectionContext,
    SpeculativeConditioning,
    SpeculativeDraftRequest,
    SpeculativeDraftResult,
    SpeculativeVerifyRequest,
    SpeculativeVerifyResult,
)

__all__ = [
    "SpeculativeConditioning",
    "DFlashRuntime",
    "DFlashSelectionContext",
    "SpeculativeDraftRequest",
    "SpeculativeDraftResult",
    "SpeculativeVerifyRequest",
    "SpeculativeVerifyResult",
    "register_dflash_adapter",
    "get_dflash_adapter",
    "resolve_dflash_adapter",
    "resolve_dflash_runtime",
    "has_dflash_backend",
    "list_dflash_adapters",
]
