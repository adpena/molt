"""DFlash adapter and contract surface for Molt GPU generation."""

from .adapters import get_dflash_adapter, list_dflash_adapters, register_dflash_adapter
from .contracts import (
    SpeculativeConditioning,
    SpeculativeDraftRequest,
    SpeculativeDraftResult,
    SpeculativeVerifyRequest,
    SpeculativeVerifyResult,
)

__all__ = [
    "SpeculativeConditioning",
    "SpeculativeDraftRequest",
    "SpeculativeDraftResult",
    "SpeculativeVerifyRequest",
    "SpeculativeVerifyResult",
    "register_dflash_adapter",
    "get_dflash_adapter",
    "list_dflash_adapters",
]
