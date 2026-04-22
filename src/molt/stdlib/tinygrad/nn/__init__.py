"""
tinygrad.nn — Neural network layers for inference.

All layers are composed from Tensor primitives. No new Rust code needed.
"""

from __future__ import annotations

import math
from tinygrad.tensor import Tensor


class Linear:
    """Fully connected layer: y = x @ W^T + b"""

    def __init__(self, in_features: int, out_features: int, bias: bool = True) -> None:
        self.in_features = in_features
        self.out_features = out_features
        # Xavier initialization
        bound = 1.0 / math.sqrt(in_features)
        self.weight = (Tensor.rand(out_features, in_features) * 2 * bound) - bound
        self.bias = (Tensor.rand(out_features) * 2 * bound) - bound if bias else None

    def __call__(self, x: Tensor) -> Tensor:
        out = x @ self.weight.T
        if self.bias is not None:
            out = out + self.bias
        return out


class LayerNorm:
    """Layer normalization."""

    def __init__(self, normalized_shape, eps: float = 1e-5) -> None:
        if isinstance(normalized_shape, int):
            normalized_shape = (normalized_shape,)
        self.normalized_shape = normalized_shape
        self.eps = eps
        n = 1
        for s in normalized_shape:
            n *= s
        self.weight = Tensor.ones(n)
        self.bias = Tensor.zeros(n)

    def __call__(self, x: Tensor) -> Tensor:
        return x.layernorm(self.normalized_shape, self.weight, self.bias, self.eps)


class Embedding:
    """Embedding lookup table."""

    def __init__(self, num_embeddings: int, embedding_dim: int) -> None:
        self.num_embeddings = num_embeddings
        self.embedding_dim = embedding_dim
        self.weight = Tensor.rand(num_embeddings, embedding_dim)

    def __call__(self, idx: Tensor) -> Tensor:
        return self.weight.gather(0, idx)


__all__ = ["Linear", "LayerNorm", "Embedding"]
