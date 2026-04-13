"""DFlash adapter and contract surface for Molt GPU generation."""

from .adapters import (
    DFlashAdapterSpec,
    get_dflash_adapter,
    has_dflash_backend,
    list_dflash_adapters,
    register_dflash_adapter,
    resolve_dflash_adapter,
    resolve_default_dflash_adapter,
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
from .runtime import (
    SpeculativeDecodeResult,
    speculative_decode_greedy,
    speculative_decode_greedy_conditioned,
)

__all__ = [
    "SpeculativeConditioning",
    "DFlashRuntime",
    "DFlashSelectionContext",
    "DFlashAdapterSpec",
    "SpeculativeDecodeResult",
    "SpeculativeDraftRequest",
    "SpeculativeDraftResult",
    "SpeculativeVerifyRequest",
    "SpeculativeVerifyResult",
    "speculative_decode_greedy",
    "speculative_decode_greedy_conditioned",
    "register_dflash_adapter",
    "get_dflash_adapter",
    "resolve_dflash_adapter",
    "resolve_default_dflash_adapter",
    "resolve_dflash_runtime",
    "has_dflash_backend",
    "list_dflash_adapters",
]
