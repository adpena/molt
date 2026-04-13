"""tinygrad.nn compatibility surface backed by Molt."""

import math

from molt.gpu.nn import LayerNorm, Sequential
from molt.gpu.tensor import Tensor


def _pair(value):
    if isinstance(value, tuple):
        return value
    return (value, value)


class Conv2d:
    """tinygrad-compatible Conv2d layer."""

    def __init__(
        self,
        in_channels: int,
        out_channels: int,
        kernel_size,
        stride=1,
        padding=0,
        dilation=1,
        groups=1,
        bias=True,
    ):
        if groups != 1:
            raise NotImplementedError("tinygrad Conv2d groups != 1 not implemented yet")
        if dilation != 1:
            raise NotImplementedError("tinygrad Conv2d dilation != 1 not implemented yet")
        if isinstance(padding, str):
            if padding.lower() != "same":
                raise ValueError(f"Invalid padding string {padding!r}, only 'same' is supported")
            stride_pair = _pair(stride)
            if stride_pair != (1, 1):
                raise ValueError("padding='same' is not supported for strided convolutions")
            kh, kw = _pair(kernel_size)
            padding = (kh // 2, kw // 2)

        self.in_channels = in_channels
        self.out_channels = out_channels
        self.kernel_size = _pair(kernel_size)
        self.stride = _pair(stride)
        self.padding = _pair(padding)
        self.dilation = dilation
        self.groups = groups

        kh, kw = self.kernel_size
        scale = 1.0 / math.sqrt(in_channels * kh * kw)
        self.weight = Tensor.uniform(
            out_channels,
            in_channels // groups,
            kh,
            kw,
            low=-scale,
            high=scale,
        )
        self.bias = Tensor.uniform(out_channels, low=-scale, high=scale) if bias else None

    def __call__(self, x: Tensor) -> Tensor:
        return x.conv2d(
            self.weight,
            self.bias,
            self.groups,
            self.stride,
            self.dilation,
            self.padding,
        )

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
            f"Conv2d({self.in_channels}, {self.out_channels}, kernel_size={self.kernel_size}, "
            f"stride={self.stride}, padding={self.padding})"
        )


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
        out = x.linear(self.weight)
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
