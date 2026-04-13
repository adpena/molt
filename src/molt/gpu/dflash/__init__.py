"""DFlash adapter and contract surface for Molt GPU generation."""

from .adapters import (
    get_dflash_adapter,
    is_supported_dflash_backend,
    list_dflash_adapters,
    register_dflash_adapter,
    resolve_dflash_adapter,
)
from .contracts import (
    DFlashRuntime,
    SpeculativeConditioning,
    SpeculativeDraftRequest,
    SpeculativeDraftResult,
    SpeculativeVerifyRequest,
    SpeculativeVerifyResult,
)

__all__ = [
    "SpeculativeConditioning",
    "DFlashRuntime",
    "SpeculativeDraftRequest",
    "SpeculativeDraftResult",
    "SpeculativeVerifyRequest",
    "SpeculativeVerifyResult",
    "register_dflash_adapter",
    "get_dflash_adapter",
    "resolve_dflash_adapter",
    "is_supported_dflash_backend",
    "list_dflash_adapters",
]
