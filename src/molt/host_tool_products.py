"""Closed Cargo products for compiler, host-only WASM inspection and JIT custody.

The executable and package share a name. Features, CPU policy and profile are
admitted by the existing Cargo plan; this type does not own a second build path.
"""

from collections.abc import Sequence
from enum import Enum
from typing import assert_never


class HostToolProduct(Enum):
    BACKEND = "molt-backend"
    WASM_FACTS = "molt-wasm-facts"
    # The development JIT runner (HF-371): the native codegen plus
    # cranelift_jit, run from a source checkout only.
    JIT = "molt-jit"

    def executable_name(
        self, *, os_name: str, features: Sequence[str] | None = None
    ) -> str:
        stem = self.value
        if features is not None:
            stem += "." + ("_".join(sorted(features)).replace("-", "_") or "default")
        return stem + (".exe" if os_name == "nt" else "")

    @property
    def required_features(self) -> tuple[str, ...]:
        match self:
            case HostToolProduct.BACKEND:
                return ()
            case HostToolProduct.WASM_FACTS:
                return ("host-tool",)
            case HostToolProduct.JIT:
                return ()
        assert_never(self)

    @property
    def label(self) -> str:
        match self:
            case HostToolProduct.BACKEND:
                return "Backend"
            case HostToolProduct.WASM_FACTS:
                return "WASM facts tool"
            case HostToolProduct.JIT:
                return "JIT runner"
        assert_never(self)
