"""tinygrad.nn compatibility surface backed by Molt."""

from molt.gpu.nn import Conv2d, Embedding, LayerNorm, Linear, Sequential
from molt.gpu.tensor import Tensor


class RMSNorm:
    """tinygrad-compatible RMSNorm backed by Molt Tensor ops."""

    def __init__(self, dim: int, eps: float = 1e-6):
        self.dim = dim
        self.eps = eps
        self.weight = Tensor([1.0] * dim, shape=(dim,))

    def __call__(self, x: Tensor) -> Tensor:
        return x.rms_norm(self.eps) * self.weight

    def __repr__(self) -> str:
        return f"RMSNorm(dim={self.dim}, eps={self.eps})"


__all__ = [
    "Conv2d",
    "Embedding",
    "LayerNorm",
    "Linear",
    "RMSNorm",
    "Sequential",
]
