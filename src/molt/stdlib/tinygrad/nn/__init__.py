"""
tinygrad.nn — Neural network layers for inference.

All layers are composed from Tensor primitives. No new Rust code needed.
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")


import math
from tinygrad.tensor import Tensor


def _prod(values) -> int:
    out = 1
    for value in values:
        out *= value
    return out


def _make_tuple(value, size: int) -> tuple:
    if isinstance(value, int):
        return (value,) * size
    if isinstance(value, (list, tuple)):
        out = tuple(value)
        if len(out) != size:
            raise ValueError(f"expected {size} values, got {out}")
        return out
    raise TypeError("expected int or tuple")


def _flatten(values) -> tuple:
    out = []
    for value in values:
        if isinstance(value, (list, tuple)):
            out.extend(_flatten(value))
        else:
            out.append(value)
    return tuple(out)


def _uniform(*shape, low: float, high: float) -> Tensor:
    return Tensor.rand(*shape) * (high - low) + low


class Conv2d:
    """Applies a 2D convolution over an input signal."""

    def __init__(
        self,
        in_channels: int,
        out_channels: int,
        kernel_size: int | tuple[int, ...],
        stride=1,
        padding: int | tuple[int, ...] | str = 0,
        dilation=1,
        groups=1,
        bias=True,
    ) -> None:
        self.kernel_size = _make_tuple(kernel_size, 2)
        if isinstance(padding, str):
            if padding.lower() != "same":
                raise ValueError(
                    f"Invalid padding string {padding!r}, only 'same' is supported"
                )
            if stride != 1:
                raise ValueError("padding='same' is not supported for strided convolutions")
            dilation_t = _make_tuple(dilation, len(self.kernel_size))
            pad = [
                (
                    d * (k - 1) // 2,
                    d * (k - 1) - d * (k - 1) // 2,
                )
                for d, k in zip(dilation_t, self.kernel_size[::-1])
            ]
            padding = _flatten(pad)
        self.stride, self.dilation, self.groups, self.padding = (
            stride,
            dilation,
            groups,
            padding,
        )
        scale = 1.0 / math.sqrt(in_channels * _prod(self.kernel_size))
        self.weight = _uniform(
            out_channels,
            in_channels // groups,
            *self.kernel_size,
            low=-scale,
            high=scale,
        )
        self.bias = _uniform(out_channels, low=-scale, high=scale) if bias else None

    def __call__(self, x: Tensor) -> Tensor:
        return x.conv2d(
            self.weight,
            self.bias,
            self.groups,
            self.stride,
            self.dilation,
            self.padding,
        )


class ConvTranspose2d(Conv2d):
    """Applies a 2D transposed convolution over an input signal."""

    def __init__(
        self,
        in_channels: int,
        out_channels: int,
        kernel_size: int | tuple[int, ...],
        stride=1,
        padding=0,
        output_padding=0,
        dilation=1,
        groups=1,
        bias=True,
    ) -> None:
        super().__init__(
            in_channels,
            out_channels,
            kernel_size,
            stride,
            padding,
            dilation,
            groups,
            bias,
        )
        scale = 1.0 / math.sqrt(in_channels * _prod(self.kernel_size))
        self.weight = _uniform(
            in_channels,
            out_channels // groups,
            *self.kernel_size,
            low=-scale,
            high=scale,
        )
        self.output_padding = output_padding

    def __call__(self, x: Tensor) -> Tensor:
        return x.conv_transpose2d(
            self.weight,
            self.bias,
            self.groups,
            self.stride,
            self.dilation,
            self.padding,
            self.output_padding,
        )


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


class GroupNorm:
    """Applies Group Normalization over a mini-batch of inputs."""

    def __init__(
        self, num_groups: int, num_channels: int, eps=1e-5, affine=True
    ) -> None:
        self.num_groups, self.num_channels, self.eps = (
            num_groups,
            num_channels,
            eps,
        )
        self.weight = Tensor.ones(num_channels) if affine else None
        self.bias = Tensor.zeros(num_channels) if affine else None

    def __call__(self, x: Tensor) -> Tensor:
        x = x.reshape(x.shape[0], self.num_groups, -1).layernorm(
            eps=self.eps
        ).reshape(x.shape)
        if self.weight is None or self.bias is None:
            return x
        affine_shape = (1, -1, *[1] * (x.ndim - 2))
        return x * self.weight.reshape(*affine_shape) + self.bias.reshape(
            *affine_shape
        )


class Embedding:
    """Embedding lookup table."""

    def __init__(self, num_embeddings: int, embedding_dim: int) -> None:
        self.num_embeddings = num_embeddings
        self.embedding_dim = embedding_dim
        self.weight = Tensor.rand(num_embeddings, embedding_dim)

    def __call__(self, idx: Tensor) -> Tensor:
        return self.weight.gather(0, idx)


__all__ = [
    "Conv2d",
    "ConvTranspose2d",
    "Linear",
    "LayerNorm",
    "GroupNorm",
    "Embedding",
]
