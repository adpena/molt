"""tinygrad.nn compatibility surface backed by Molt."""

import math

from molt.gpu.nn import Conv2d, LayerNorm, Sequential
from molt.gpu.tensor import Tensor


class Linear:
    """tinygrad-compatible Linear layer."""

    def __init__(self, in_features: int, out_features: int, bias: bool = True):
        self.in_features = in_features
        self.out_features = out_features
        self.has_bias = bias
        bound = 1.0 / math.sqrt(in_features)
        self.weight = Tensor.uniform(out_features, in_features, low=-bound, high=bound)
        self.bias = (
            Tensor.uniform(out_features, low=-bound, high=bound) if bias else None
        )

    def __call__(self, x: Tensor) -> Tensor:
        squeezed = False
        if x.ndim == 1:
            x = x.reshape(1, x.size)
            squeezed = True
        out = x @ self.weight.T
        if self.bias is not None:
            out = out + self.bias
        if squeezed:
            out = out.reshape(self.out_features)
        return out

    def load_weights(self, weight, bias=None):
        if not isinstance(weight, Tensor):
            weight = Tensor(weight)
        self.weight = weight
        if bias is not None:
            if not isinstance(bias, Tensor):
                bias = Tensor(bias)
            self.bias = bias

    def parameters(self) -> list:
        params = [self.weight]
        if self.bias is not None:
            params.append(self.bias)
        return params

    def __repr__(self) -> str:
        return (
            f"Linear(in_features={self.in_features}, "
            f"out_features={self.out_features}, "
            f"bias={self.has_bias})"
        )


class Embedding:
    """tinygrad-compatible Embedding layer."""

    def __init__(self, vocab_size: int, embed_size: int):
        self.vocab_sz = vocab_size
        self.embed_sz = embed_size
        self.weight = Tensor.glorot_uniform(vocab_size, embed_size)

    def __call__(self, idx: Tensor) -> Tensor:
        return self.weight.take_rows(idx, allow_negative=False)

    def load_weights(self, weight):
        if not isinstance(weight, Tensor):
            weight = Tensor(weight)
        self.weight = weight

    def parameters(self) -> list:
        return [self.weight]

    def __repr__(self) -> str:
        return f"Embedding({self.vocab_sz}, {self.embed_sz})"


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
