"""Contracts for TurboQuant vector and KV-cache compression.

This package implements the practical structured-rotation TurboQuant path:
Hadamard-based random rotations for the MSE stage and QJL residual sketches for
unbiased inner-product estimation. The current runtime targets power-of-two
vector dimensions explicitly and raises otherwise.
"""

from __future__ import annotations


class TurboQuantConfig:
    """Configuration for a TurboQuant codec instance."""

    def __init__(
        self,
        *,
        dim: int,
        bits: int,
        seed: int = 0,
        qjl_seed: int | None = None,
        rotation: str = "hadamard",
    ) -> None:
        if dim <= 0:
            raise ValueError("TurboQuant dimension must be positive")
        if bits < 2:
            raise ValueError("TurboQuant requires at least 2 total bits")
        if rotation != "hadamard":
            raise ValueError("TurboQuant currently supports only hadamard rotation")
        self.dim = int(dim)
        self.bits = int(bits)
        self.stage_bits = int(bits - 1)
        self.seed = int(seed)
        self.qjl_seed = int(seed if qjl_seed is None else qjl_seed)
        self.rotation = rotation


class TurboQuantMSEVector:
    """Encoded vector for the MSE stage of TurboQuant."""

    def __init__(self, indices, *, norm: float, mse_weights=None) -> None:
        self.indices = list(indices)
        self.norm = float(norm)
        self.mse_weights = None if mse_weights is None else list(mse_weights)


class TurboQuantProdVector:
    """Encoded vector for full TurboQuant with residual QJL correction."""

    def __init__(
        self,
        indices,
        *,
        norm: float,
        residual_signs,
        residual_norm: float,
        mse_weights=None,
        residual_scale: float | None = None,
    ) -> None:
        self.indices = list(indices)
        self.norm = float(norm)
        self.residual_signs = list(residual_signs)
        self.residual_norm = float(residual_norm)
        self.mse_weights = None if mse_weights is None else list(mse_weights)
        self.residual_scale = None if residual_scale is None else float(residual_scale)
