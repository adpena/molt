"""
molt.gpu.tensor — Lightweight tensor for ML inference.

Supports: reshape, transpose, matmul, elementwise ops, broadcasting.
Designed for inference (forward pass), not training (no autograd).

Usage:
    from molt.gpu.tensor import Tensor

    a = Tensor([[1, 2], [3, 4]])
    b = Tensor([[5, 6], [7, 8]])
    c = a @ b
    print(c.to_list())  # [[19, 22], [43, 50]]
"""

from __future__ import annotations

import math
import operator
import os
import _intrinsics as _molt_intrinsics
from . import Buffer, alloc, to_device, from_device


def _load_optional_intrinsic(name: str):
    loader = getattr(_molt_intrinsics, "load_intrinsic", None)
    if callable(loader):
        return loader(name)
    require = getattr(_molt_intrinsics, "require_intrinsic", None)
    if callable(require):
        try:
            return require(name)
        except RuntimeError:
            return None
    return None


def _runtime_intrinsics_active() -> bool:
    runtime_active = getattr(_molt_intrinsics, "runtime_active", None)
    if callable(runtime_active):
        return bool(runtime_active())
    return False


def _requested_gpu_backend() -> str | None:
    backend = os.environ.get("MOLT_GPU_BACKEND")
    if backend is None:
        return None
    backend = backend.strip()
    return backend or None


_UNRESOLVED = object()
_MOLT_GPU_BUFFER_TO_LIST = _UNRESOLVED
_MOLT_GPU_TENSOR_FROM_BUFFER = _UNRESOLVED
_MOLT_GPU_TENSOR_FROM_PARTS = _UNRESOLVED
_MOLT_GPU_TENSOR_ZEROS = _UNRESOLVED
_MOLT_GPU_REPEAT_AXIS_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_LINEAR_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_LINEAR_SPLIT_LAST_DIM_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_TENSOR_LINEAR_SPLIT_LAST_DIM = _UNRESOLVED
_MOLT_GPU_TENSOR_SCALED_DOT_PRODUCT_ATTENTION = _UNRESOLVED
_MOLT_GPU_TENSOR_TAKE_ROWS = _UNRESOLVED
_MOLT_GPU_TENSOR_CONCAT_FIRST_DIM = _UNRESOLVED
_MOLT_GPU_TENSOR_SCATTER_ROWS = _UNRESOLVED
_MOLT_GPU_LINEAR_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_BROADCAST_BINARY_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_MATMUL_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_PERMUTE_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_RMS_NORM_LAST_AXIS_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS = _UNRESOLVED
_MOLT_GPU_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS = _UNRESOLVED


def _resolve_optional_intrinsic(cache_name: str, intrinsic_name: str):
    intrinsic = globals().get(cache_name, _UNRESOLVED)
    if intrinsic is not _UNRESOLVED:
        return intrinsic

    loader = getattr(_molt_intrinsics, "load_intrinsic", None)
    if callable(loader):
        try:
            intrinsic = loader(intrinsic_name)
        except RuntimeError as exc:
            if _requested_gpu_backend() is not None:
                raise RuntimeError(f"intrinsic unavailable: {intrinsic_name}") from exc
        else:
            if intrinsic is not None:
                globals()[cache_name] = intrinsic
                return intrinsic

    require = getattr(_molt_intrinsics, "require_intrinsic", None)
    if callable(require):
        try:
            intrinsic = require(intrinsic_name)
        except RuntimeError as exc:
            if _requested_gpu_backend() is not None:
                raise RuntimeError(f"intrinsic unavailable: {intrinsic_name}") from exc
        else:
            if intrinsic is not None:
                globals()[cache_name] = intrinsic
                return intrinsic

    if _requested_gpu_backend() is not None:
        raise RuntimeError(f"intrinsic unavailable: {intrinsic_name}")
    return None


_OP_ADD = 0
_OP_SUB = 1
_OP_MUL = 2
_OP_DIV = 3

_TINYGRAD_FIRST_DEVICE_KEY0 = 347607321  # sha256((0).to_bytes(4,"big")) low 32 bits
_TINYGRAD_RNG_STATE = [0, 0]


def _product(seq):
    """Product of a sequence of integers."""
    result = 1
    for x in seq:
        result *= x
    return result


def _dtype_cast_kind(dtype) -> str | None:
    if dtype is float:
        return "float"
    if dtype is int:
        return "int"
    name = getattr(dtype, "name", None)
    fmt = getattr(dtype, "fmt", None)
    if name in {"float", "float32", "float64", "float16", "bfloat16", "half", "double"}:
        return "float"
    if fmt in {"f", "d", "e"}:
        return "float"
    if name in {
        "bool",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "short",
        "long",
        "uchar",
        "ushort",
        "uint",
        "ulong",
    }:
        return "int"
    if fmt in {"?", "b", "h", "i", "q", "B", "H", "I", "Q"}:
        return "int"
    return None


def _u32(value):
    return (value or 0) & 0xFFFFFFFF


def _rotl32(value, bits):
    value = _u32(value)
    return _u32((value << bits) | (value >> (32 - bits)))


def _uint32_to_unit_float(value):
    # tinygrad constructs a float in [1, 2) by fixing exponent=127 and using
    # the top 23 random bits as the mantissa, then subtracts 1.0. That is
    # exactly mantissa / 2**23, which avoids a boxed struct.unpack path and
    # lowers cleanly in compiled Molt.
    mantissa = (value >> 9) & 0x7FFFFF
    return float(mantissa) / float(1 << 23)


def _tinygrad_rand_values_seeded(count, seed_value, counter_base):
    if count <= 0:
        return []
    mask = 0xFFFFFFFF
    key0 = _TINYGRAD_FIRST_DEVICE_KEY0
    seed_bits = (seed_value or 0) & mask
    key1 = (key0 ^ seed_bits ^ 0x1BD11BDA) & mask
    base = counter_base

    num_pairs = (count + 1) // 2
    low = []
    high = []
    for idx in range(num_pairs):
        x0 = (base + idx + key0) & mask
        x1 = (base + num_pairs + idx + seed_bits) & mask
        for round_idx in range(5):
            if round_idx % 2 == 0:
                rots = (13, 15, 26, 6)
            else:
                rots = (17, 29, 16, 24)
            for rot in rots:
                x0 = (x0 + x1) & mask
                rotated = ((x1 << rot) | (x1 >> (32 - rot))) & mask
                x1 = (x0 ^ rotated) & mask
            if round_idx % 3 == 0:
                k_add0 = seed_bits
                k_add1 = key1
            elif round_idx % 3 == 1:
                k_add0 = key1
                k_add1 = key0
            else:
                k_add0 = key0
                k_add1 = seed_bits
            x0 = (x0 + k_add0) & mask
            x1 = (x1 + k_add1 + round_idx + 1) & mask
        lo_bits, hi_bits = x0, x1
        low.append(_uint32_to_unit_float(lo_bits))
        high.append(_uint32_to_unit_float(hi_bits))
    return (low + high)[:count]


def _tinygrad_current_seed():
    return _u32(_TINYGRAD_RNG_STATE[0])


def _tinygrad_consume_rand_values(count):
    counter_root = _TINYGRAD_RNG_STATE[1]
    _TINYGRAD_RNG_STATE[1] = counter_root + count
    return _tinygrad_rand_values_seeded(count, _tinygrad_current_seed(), counter_root)


def _tinygrad_seeded_rand_values(count, seed, counter_base=0):
    return _tinygrad_rand_values_seeded(count, _u32(seed), counter_base)


def _strides(shape):
    strides = []
    stride = 1
    for size in reversed(shape):
        strides.append(stride)
        stride *= size
    strides.reverse()
    return tuple(strides)


def _normalize_tensor_index(shape, idx):
    if not isinstance(idx, tuple):
        idx = (idx,)
    if idx.count(Ellipsis) > 1:
        raise IndexError("an index can only have a single ellipsis")
    result = []
    expanded = False
    for item in idx:
        if item is Ellipsis:
            remaining = len(shape) - (len(idx) - 1)
            if remaining < 0:
                raise IndexError("too many indices for tensor")
            result.extend(slice(None) for _ in range(remaining))
            expanded = True
        else:
            result.append(item)
    if not expanded and len(result) < len(shape):
        result.extend(slice(None) for _ in range(len(shape) - len(result)))
    if len(result) > len(shape):
        raise IndexError("too many indices for tensor")
    return tuple(result)


def _preferred_float_format(*tensors: "Tensor") -> str:
    formats = []
    for tensor in tensors:
        if tensor._dtype is float and tensor._buf.element_type is float:
            formats.append(tensor._buf.format_char)
    if formats and all(fmt == "f" for fmt in formats):
        return "f"
    return "d"


def _binary_result_dtype_and_format(lhs: "Tensor", rhs) -> tuple[type, str]:
    if isinstance(rhs, Tensor):
        if lhs._dtype is float or rhs._dtype is float:
            if lhs.size != 1 and rhs.size == 1 and lhs._dtype is float:
                result_format = _preferred_float_format(lhs)
            elif lhs.size == 1 and rhs.size != 1 and rhs._dtype is float:
                result_format = _preferred_float_format(rhs)
            else:
                float_tensors = tuple(
                    tensor
                    for tensor in (lhs, rhs)
                    if tensor._dtype is float and tensor._buf.element_type is float
                )
                result_format = (
                    _preferred_float_format(*float_tensors) if float_tensors else "d"
                )
            return float, result_format
        return lhs._dtype, lhs._buf.format_char
    if isinstance(rhs, float):
        if lhs._dtype is float:
            return float, _preferred_float_format(lhs)
        return float, "d"
    if isinstance(rhs, int):
        if lhs._dtype is float:
            return float, _preferred_float_format(lhs)
        return lhs._dtype, lhs._buf.format_char
    raise TypeError(f"Unsupported binary operand type: {type(rhs)!r}")


def _tensor_from_parts(
    data,
    element_type: type,
    size: int,
    format_char: str,
    shape,
    dtype: type,
) -> "Tensor":
    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_TENSOR_FROM_PARTS", "molt_gpu_tensor_from_parts"
    )
    if intrinsic is not None:
        return intrinsic(
            Tensor,
            Buffer,
            data,
            element_type,
            size,
            format_char,
            shape,
            dtype,
        )
    return Tensor(
        Buffer(data, element_type, size, format_char=format_char),
        shape=shape,
        dtype=dtype,
    )


def _tensor_from_buffer(buf: Buffer, shape, dtype: type) -> "Tensor":
    return Tensor(buf, shape=shape, dtype=dtype)


def _buffer_to_list(buf: Buffer, size: int) -> list:
    return from_device(buf)[:size]


def tensor_linear(x: "Tensor", weight: "Tensor") -> "Tensor":
    if not isinstance(weight, Tensor):
        return NotImplemented

    x_shape = x._shape
    weight_shape = weight._shape
    if len(weight_shape) != 2:
        raise ValueError(f"linear weight must be 2D, got {weight_shape}")

    if len(x_shape) == 0:
        raise ValueError("linear input must be at least 1D")

    in_features = x_shape[-1]
    out_features, weight_in = weight_shape
    if in_features != weight_in:
        raise ValueError(f"Linear shape mismatch: {x_shape} with weight {weight_shape}")

    outer = _product(x_shape[:-1]) if len(x_shape) > 1 else 1
    if x._dtype is float and weight._dtype is float:
        result_dtype = x._dtype
        if (
            x._buf.element_type is float
            and weight._buf.element_type is float
            and x._buf.format_char == "f"
            and weight._buf.format_char == "f"
        ):
            result_format = "f"
        else:
            result_format = "d"
    else:
        result_dtype = x._dtype
        result_format = x._buf.format_char
    out_shape = x_shape[:-1] + (out_features,)
    if not out_shape:
        out_shape = (out_features,)

    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_LINEAR_CONTIGUOUS", "molt_gpu_linear_contiguous"
    )
    if intrinsic is not None:
        out_bits = intrinsic(
            x._buf._data,
            x._buf.format_char,
            weight._buf._data,
            weight._buf.format_char,
            outer,
            in_features,
            out_features,
            result_format,
        )
        return _tensor_from_parts(
            out_bits,
            result_dtype,
            outer * out_features,
            result_format,
            out_shape,
            result_dtype,
        )

    x_data = tensor_data_list(x)
    out_buf = alloc(
        outer * out_features,
        result_dtype,
        format_char=result_format,
    )

    for batch in range(outer):
        x_off = batch * in_features
        out_off = batch * out_features
        for out_idx in range(out_features):
            w_off = out_idx * in_features
            acc = 0.0
            for k in range(in_features):
                acc += x_data[x_off + k] * weight._buf[w_off + k]
            out_buf[out_off + out_idx] = acc

    return _tensor_from_buffer(out_buf, out_shape, result_dtype)


def tensor_linear_split_last_dim(
    x: "Tensor", weight: "Tensor", sizes
) -> tuple["Tensor", ...]:
    if not isinstance(weight, Tensor):
        return NotImplemented

    x_shape = x._shape
    weight_shape = weight._shape
    if len(weight_shape) != 2:
        raise ValueError(f"linear weight must be 2D, got {weight_shape}")
    if len(x_shape) == 0:
        raise ValueError("linear input must be at least 1D")

    normalized_sizes = []
    for size in sizes:
        if isinstance(size, bool):
            raise TypeError("split sizes must be integers")
        if isinstance(size, int):
            normalized_sizes.append(size)
            continue
        try:
            normalized_sizes.append(operator.index(size))
        except TypeError as exc:
            raise TypeError("split sizes must be integers") from exc
    sizes = tuple(normalized_sizes)
    if any(size < 0 for size in sizes):
        raise ValueError("split sizes must be non-negative")

    in_features = x_shape[-1]
    out_features, weight_in = weight_shape
    if in_features != weight_in:
        raise ValueError(f"Linear shape mismatch: {x_shape} with weight {weight_shape}")
    if sum(sizes) != out_features:
        raise ValueError(
            f"split sizes {sizes} do not match projected dimension {out_features}"
        )

    outer = _product(x_shape[:-1]) if len(x_shape) > 1 else 1
    if x._dtype is float and weight._dtype is float:
        result_dtype = x._dtype
        if (
            x._buf.element_type is float
            and weight._buf.element_type is float
            and x._buf.format_char == "f"
            and weight._buf.format_char == "f"
        ):
            result_format = "f"
        else:
            result_format = "d"
    else:
        result_dtype = x._dtype
        result_format = x._buf.format_char
    prefix_shape = x_shape[:-1]

    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_LINEAR_SPLIT_LAST_DIM_CONTIGUOUS",
        "molt_gpu_linear_split_last_dim_contiguous",
    )
    if intrinsic is not None:
        out_parts = intrinsic(
            x._buf._data,
            x._buf.format_char,
            weight._buf._data,
            weight._buf.format_char,
            outer,
            in_features,
            sizes,
            result_format,
        )
        if len(out_parts) != len(sizes):
            raise RuntimeError("intrinsic returned wrong split count")
        return tuple(
            _tensor_from_parts(
                part_bits,
                result_dtype,
                outer * size,
                result_format,
                prefix_shape + (size,),
                result_dtype,
            )
            for size, part_bits in zip(sizes, out_parts)
        )

    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_TENSOR_LINEAR_SPLIT_LAST_DIM",
        "molt_gpu_tensor__tensor_linear_split_last_dim",
    )
    if intrinsic is not None:
        return intrinsic(x, weight, sizes)

    return tensor_linear(x, weight).split_last_dim(sizes)


def tensor_linear_squared_relu_gate_interleaved(
    x: "Tensor", weight: "Tensor"
) -> "Tensor":
    if not isinstance(weight, Tensor):
        return NotImplemented
    x_shape = x._shape
    weight_shape = weight._shape
    if len(weight_shape) != 2:
        raise ValueError(f"linear weight must be 2D, got {weight_shape}")
    if len(x_shape) == 0:
        raise ValueError("linear input must be at least 1D")

    in_features = x_shape[-1]
    out_features, weight_in = weight_shape
    if in_features != weight_in:
        raise ValueError(f"Linear shape mismatch: {x_shape} with weight {weight_shape}")
    if out_features % 2 != 0:
        raise ValueError(
            f"interleaved gate weight output dimension must be even, got {out_features}"
        )

    outer = _product(x_shape[:-1]) if len(x_shape) > 1 else 1
    hidden = out_features // 2
    prefix_shape = x_shape[:-1]
    out_shape = prefix_shape + (hidden,)
    if x._dtype is float and weight._dtype is float:
        result_dtype = x._dtype
        if (
            x._buf.element_type is float
            and weight._buf.element_type is float
            and x._buf.format_char == "f"
            and weight._buf.format_char == "f"
        ):
            result_format = "f"
        else:
            result_format = "d"
    else:
        result_dtype = x._dtype
        result_format = x._buf.format_char

    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_LINEAR_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS",
        "molt_gpu_linear_squared_relu_gate_interleaved_contiguous",
    )
    if intrinsic is not None:
        out_bits = intrinsic(
            x._buf._data,
            x._buf.format_char,
            weight._buf._data,
            weight._buf.format_char,
            outer,
            in_features,
            result_format,
        )
        return _tensor_from_parts(
            out_bits,
            result_dtype,
            outer * hidden,
            result_format,
            out_shape,
            result_dtype,
        )

    projected = tensor_linear(x, weight)
    data = tensor_data_list(projected)
    axis_len = projected._shape[-1]
    hidden = axis_len // 2
    out_buf = alloc(_product(out_shape), result_dtype, format_char=result_format)

    for row in range(outer):
        in_base = row * axis_len
        out_base = row * hidden
        for i in range(hidden):
            gate = float(data[in_base + 2 * i])
            up = float(data[in_base + 2 * i + 1])
            relu = gate if gate > 0.0 else 0.0
            out_buf[out_base + i] = relu * relu * up

    return _tensor_from_buffer(out_buf, out_shape, result_dtype)


def tensor_conv2d(
    x: "Tensor",
    weight: "Tensor",
    bias: "Tensor | None" = None,
    groups: int = 1,
    stride=1,
    dilation=1,
    padding=0,
) -> "Tensor":
    if not isinstance(x, Tensor) or not isinstance(weight, Tensor):
        return NotImplemented
    if x.ndim == 3:
        x = x.reshape(1, *x.shape)
    if x.ndim != 4:
        raise ValueError(f"conv2d input must be 3D or 4D, got {x.shape}")
    if weight.ndim != 4:
        raise ValueError(f"conv2d weight must be 4D, got {weight.shape}")
    if bias is not None and not isinstance(bias, Tensor):
        return NotImplemented

    sh, sw = stride if isinstance(stride, tuple) else (stride, stride)
    dh, dw = dilation if isinstance(dilation, tuple) else (dilation, dilation)
    ph, pw = padding if isinstance(padding, tuple) else (padding, padding)

    batch, in_c, in_h, in_w = x.shape
    out_c, weight_in_c, kh, kw = weight.shape
    if groups <= 0:
        raise ValueError("conv2d groups must be positive")
    if in_c % groups != 0 or out_c % groups != 0:
        raise ValueError("conv2d channels must be divisible by groups")
    if weight_in_c != in_c // groups:
        raise ValueError(
            f"conv2d weight input channels mismatch: {weight.shape} vs input {x.shape} groups={groups}"
        )
    if bias is not None and bias.shape != (out_c,):
        raise ValueError(f"conv2d bias shape mismatch: {bias.shape} vs ({out_c},)")

    out_h = (in_h + 2 * ph - dh * (kh - 1) - 1) // sh + 1
    out_w = (in_w + 2 * pw - dw * (kw - 1) - 1) // sw + 1

    x_data = x._data_list()
    w_data = weight._data_list()
    b_data = bias._data_list() if bias is not None else None
    out = [0.0] * (batch * out_c * out_h * out_w)

    out_channels_per_group = out_c // groups
    in_channels_per_group = in_c // groups

    for b in range(batch):
        for oc in range(out_c):
            group = oc // out_channels_per_group
            ic_base = group * in_channels_per_group
            for oh in range(out_h):
                for ow in range(out_w):
                    acc = 0.0
                    for local_ic in range(weight_in_c):
                        ic = ic_base + local_ic
                        for fh in range(kh):
                            ih = oh * sh - ph + fh * dh
                            if ih < 0 or ih >= in_h:
                                continue
                            for fw in range(kw):
                                iw = ow * sw - pw + fw * dw
                                if iw < 0 or iw >= in_w:
                                    continue
                                x_idx = ((b * in_c + ic) * in_h + ih) * in_w + iw
                                w_idx = (
                                    (oc * weight_in_c + local_ic) * kh + fh
                                ) * kw + fw
                                acc += x_data[x_idx] * w_data[w_idx]
                    if b_data is not None:
                        acc += b_data[oc]
                    out_idx = ((b * out_c + oc) * out_h + oh) * out_w + ow
                    out[out_idx] = acc

    return Tensor(out, shape=(batch, out_c, out_h, out_w), dtype=x._dtype)


def tensor_permute_dims(x: "Tensor", dims) -> "Tensor":
    if not isinstance(x, Tensor):
        return NotImplemented
    x_ndim = len(x._shape)
    if len(dims) == 1 and isinstance(dims[0], (list, tuple)):
        dims = tuple(dims[0])
    else:
        dims = tuple(dims)

    if len(dims) != x_ndim:
        raise ValueError(
            f"permute expected {x_ndim} dims for shape {x._shape}, got {dims}"
        )

    normalized = []
    for dim in dims:
        if dim < 0:
            dim += x_ndim
        if dim < 0 or dim >= x_ndim:
            raise ValueError(f"permute dim {dim} out of range for ndim={x_ndim}")
        normalized.append(dim)
    if sorted(normalized) != list(range(x_ndim)):
        raise ValueError(f"permute dims must be a permutation of 0..{x_ndim - 1}")

    if x_ndim <= 1:
        return _tensor_from_buffer(x._buf, x._shape, x._dtype)

    old_shape = x._shape
    new_shape = tuple(old_shape[dim] for dim in normalized)
    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_PERMUTE_CONTIGUOUS", "molt_gpu_permute_contiguous"
    )
    if intrinsic is not None:
        out_bits = intrinsic(
            x._buf._data,
            x._buf.format_char,
            old_shape,
            normalized,
            x._buf.format_char,
        )
        return _tensor_from_parts(
            out_bits,
            x._buf.element_type,
            x.size,
            x._buf.format_char,
            new_shape,
            x._dtype,
        )

    data = tensor_data_list(x)
    result = [0.0] * len(data)

    old_strides = []
    stride = 1
    for size in reversed(old_shape):
        old_strides.append(stride)
        stride *= size
    old_strides.reverse()

    new_strides = []
    stride = 1
    for size in reversed(new_shape):
        new_strides.append(stride)
        stride *= size
    new_strides.reverse()

    for old_index, value in enumerate(data):
        rem = old_index
        coords = []
        for axis_stride, axis_size in zip(old_strides, old_shape):
            coord = rem // axis_stride
            rem %= axis_stride
            coords.append(coord)

        new_index = 0
        for axis, coord in enumerate(normalized):
            new_index += coords[coord] * new_strides[axis]
        result[new_index] = value

    out_buf = alloc(len(result), x._dtype, format_char=x._buf.format_char)
    for idx, value in enumerate(result):
        out_buf[idx] = value
    return _tensor_from_buffer(out_buf, new_shape, x._dtype)


def tensor_softmax_last_axis(x: "Tensor") -> "Tensor":
    if not isinstance(x, Tensor):
        return NotImplemented
    if x.ndim == 0:
        return Tensor(1.0)

    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS",
        "molt_gpu_softmax_last_axis_contiguous",
    )
    if intrinsic is not None:
        out_bits = intrinsic(
            x._buf._data,
            x._buf.format_char,
            x._shape,
            x._buf.format_char,
        )
        return _tensor_from_parts(
            out_bits,
            x._dtype,
            x.size,
            x._buf.format_char,
            x._shape,
            x._dtype,
        )

    data = tensor_data_list(x)
    outer = _product(x._shape[:-1]) if x.ndim > 1 else 1
    axis_len = x._shape[-1]
    result = [0.0] * len(data)

    for row in range(outer):
        base = row * axis_len
        vals = data[base : base + axis_len]
        max_val = max(vals)
        exps = [math.exp(v - max_val) for v in vals]
        total = sum(exps)
        for idx, exp_v in enumerate(exps):
            result[base + idx] = exp_v / total

    out_buf = alloc(len(result), x._dtype, format_char=x._buf.format_char)
    for idx, value in enumerate(result):
        out_buf[idx] = value
    return _tensor_from_buffer(out_buf, x._shape, x._dtype)


def tensor_reshape_view(x: "Tensor", shape) -> "Tensor":
    if not isinstance(x, Tensor):
        return NotImplemented
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])

    neg_idx = None
    known = 1
    for i, s in enumerate(shape):
        if s == -1:
            if neg_idx is not None:
                raise ValueError("Only one dimension can be -1")
            neg_idx = i
        else:
            known *= s

    if neg_idx is not None:
        inferred = x.size // known
        shape = shape[:neg_idx] + (inferred,) + shape[neg_idx + 1 :]

    if _product(shape) != x.size:
        raise ValueError(f"Cannot reshape tensor of size {x.size} into shape {shape}")
    return _tensor_from_buffer(x._buf, shape, x._dtype)


def tensor_data_list(x: "Tensor") -> list:
    if not isinstance(x, Tensor):
        raise TypeError(f"Expected Tensor, got {type(x)!r}")
    return _buffer_to_list(x._buf, x.size)


def _normalize_axis0_row_index(
    raw_idx,
    axis0_size: int,
    *,
    allow_negative: bool,
    op_name: str,
) -> int:
    idx = int(raw_idx)
    if idx != raw_idx:
        raise TypeError(f"{op_name} indices must be integers, got {raw_idx!r}")
    display_idx = idx
    if idx < 0 and allow_negative:
        idx += axis0_size
    if idx < 0 or idx >= axis0_size:
        raise IndexError(
            f"Index {display_idx} out of range for axis 0 with size {axis0_size}"
        )
    return idx


def tensor_take_rows(x: "Tensor", indices, *, allow_negative: bool = True) -> "Tensor":
    if not isinstance(x, Tensor):
        return NotImplemented
    if x.ndim == 0:
        raise ValueError("take_rows requires a tensor with at least 1 dimension")

    if not isinstance(indices, Tensor):
        indices = Tensor(indices)

    rows = tensor_data_list(indices)
    row_shape = x._shape[1:]
    row_size = _product(row_shape) if row_shape else 1
    width = row_size * x._buf.itemsize
    out = bytearray(len(rows) * width)

    for out_row, raw_idx in enumerate(rows):
        idx = _normalize_axis0_row_index(
            raw_idx,
            x._shape[0],
            allow_negative=allow_negative,
            op_name="take_rows",
        )
        src_start = idx * width
        dst_start = out_row * width
        out[dst_start : dst_start + width] = x._buf._data[src_start : src_start + width]

    out_buf = Buffer(
        out,
        x._buf.element_type,
        len(rows) * row_size,
        format_char=x._buf.format_char,
    )
    return _tensor_from_buffer(out_buf, indices.shape + row_shape, x._dtype)


def tensor_concat_first_dim(tensors) -> "Tensor":
    if not tensors:
        raise ValueError("concat_first_dim requires at least one tensor")
    if len(tensors) == 1:
        return tensors[0]
    first = tensors[0]
    if not isinstance(first, Tensor):
        raise TypeError(f"Expected Tensor, got {type(first)!r}")
    if first.ndim == 0:
        raise ValueError("concat_first_dim requires tensors with at least 1 dimension")
    tail_shape = first._shape[1:]
    if len(tensors) == 2:
        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_TENSOR_CONCAT_FIRST_DIM",
            "molt_gpu_tensor__tensor_concat_first_dim",
        )
        if intrinsic is not None:
            return intrinsic(tensors[0], tensors[1])
    total_rows = 0
    total_bytes = 0
    for tensor in tensors:
        if not isinstance(tensor, Tensor):
            raise TypeError(f"Expected Tensor, got {type(tensor)!r}")
        if tensor.ndim != first.ndim:
            raise ValueError(
                f"concat_first_dim rank mismatch: {tensor.ndim} vs {first.ndim}"
            )
        if tensor._shape[1:] != tail_shape:
            raise ValueError(
                f"concat_first_dim shape mismatch: {tensor._shape} vs {(None,) + tail_shape}"
            )
        if tensor._buf.format_char != first._buf.format_char:
            raise ValueError("concat_first_dim requires matching buffer formats")
        total_rows += tensor._shape[0]
        total_bytes += tensor.size * tensor._buf.itemsize
    out = bytearray(total_bytes)
    cursor = 0
    for tensor in tensors:
        width = tensor.size * tensor._buf.itemsize
        out[cursor : cursor + width] = tensor._buf._data[:width]
        cursor += width
    row_size = _product(tail_shape) if tail_shape else 1
    out_buf = Buffer(
        out,
        first._buf.element_type,
        total_rows * row_size,
        format_char=first._buf.format_char,
    )
    return _tensor_from_buffer(out_buf, (total_rows,) + tail_shape, first._dtype)


def tensor_scatter_rows(
    base: "Tensor", indices, updates: "Tensor", *, allow_negative: bool = True
) -> "Tensor":
    if not isinstance(base, Tensor) or not isinstance(updates, Tensor):
        return NotImplemented
    if base.ndim == 0 or updates.ndim == 0:
        raise ValueError("scatter_rows requires tensors with at least 1 dimension")
    if base._shape[1:] != updates._shape[1:]:
        raise ValueError(
            f"scatter_rows trailing shape mismatch: {base._shape} vs {updates._shape}"
        )
    if base._buf.format_char != updates._buf.format_char:
        raise ValueError("scatter_rows requires matching buffer formats")
    if not isinstance(indices, Tensor):
        indices = Tensor(indices)
    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_TENSOR_SCATTER_ROWS", "molt_gpu_tensor__tensor_scatter_rows"
    )
    if intrinsic is not None:
        return intrinsic(base, indices, updates, allow_negative)
    rows = tensor_data_list(indices)
    if len(rows) != updates._shape[0]:
        raise ValueError(
            f"scatter_rows update row count mismatch: {len(rows)} vs {updates._shape[0]}"
        )
    row_shape = base._shape[1:]
    row_size = _product(row_shape) if row_shape else 1
    width = row_size * base._buf.itemsize
    out = bytearray(base._buf._data[: base.size * base._buf.itemsize])
    for src_row, raw_idx in enumerate(rows):
        idx = _normalize_axis0_row_index(
            raw_idx,
            base._shape[0],
            allow_negative=allow_negative,
            op_name="scatter_rows",
        )
        dst_start = idx * width
        src_start = src_row * width
        out[dst_start : dst_start + width] = updates._buf._data[
            src_start : src_start + width
        ]
    out_buf = Buffer(
        out,
        base._buf.element_type,
        base.size,
        format_char=base._buf.format_char,
    )
    return _tensor_from_buffer(out_buf, base._shape, base._dtype)


def _flatten_nested(data):
    """Flatten a nested list and infer its shape."""
    if not isinstance(data, (list, tuple)):
        return [data], ()

    shape = []
    current = data
    while isinstance(current, (list, tuple)):
        shape.append(len(current))
        if len(current) == 0:
            break
        current = current[0]

    flat = []

    def _walk(obj, depth):
        if depth == len(shape):
            flat.append(obj)
        else:
            for item in obj:
                _walk(item, depth + 1)

    _walk(data, 0)
    return flat, tuple(shape)


class Tensor:
    """N-dimensional array backed by a GPU buffer.

    Stores data as a flat gpu.Buffer with an associated shape tuple.
    All operations work elementwise on the flat buffer in interpreted mode
    and map to GPU kernels when compiled by Molt.
    """

    def __init__(self, data, shape=None, dtype=float):
        """Create a tensor from nested list, flat list + shape, or Buffer.

        Args:
            data: nested list, flat list, Buffer, or scalar
            shape: optional shape tuple (required if data is flat list)
            dtype: element type (float or int)
        """
        if isinstance(data, Buffer):
            self._buf = data
            if shape is None:
                self._shape = (data.size,)
            else:
                if isinstance(shape, int):
                    shape = (shape,)
                self._shape = tuple(shape)
            self._dtype = dtype
        elif isinstance(data, (int, float)):
            # Scalar
            self._buf = to_device([float(data)])
            self._shape = ()
            self._dtype = dtype
        elif isinstance(data, (bytes, bytearray, memoryview)):
            raw = bytes(data)
            self._buf = Buffer(raw, int, len(raw), format_char="B")
            self._shape = (len(raw),)
            self._dtype = int
        elif isinstance(data, (list, tuple)):
            if shape is not None:
                # Flat list + explicit shape
                if isinstance(shape, int):
                    shape = (shape,)
                self._shape = tuple(shape)
                flat = [float(x) for x in data]
            else:
                # Nested list — infer shape
                flat, inferred = _flatten_nested(data)
                flat = [float(x) for x in flat]
                self._shape = inferred
            self._buf = to_device(flat)
            self._dtype = dtype
        else:
            raise TypeError(f"Cannot create Tensor from {type(data)}")

    @property
    def shape(self) -> tuple:
        """Shape of the tensor as a tuple of ints."""
        return self._shape

    @property
    def ndim(self) -> int:
        """Number of dimensions."""
        return len(self._shape)

    @property
    def size(self) -> int:
        """Total number of elements."""
        return _product(self._shape) if self._shape else 1

    def _data_list(self) -> list:
        """Return flat data as a Python list of floats."""
        return from_device(self._buf)[: self.size]

    def _from_flat(self, flat, shape):
        """Create a new Tensor from a flat list and shape."""
        return Tensor(flat, shape=shape, dtype=self._dtype)

    def _copy_contiguous_buffer(self, start_elem: int, elem_count: int) -> Buffer:
        """Copy a contiguous element range into a new buffer preserving format."""
        width = self._buf.itemsize
        start = start_elem * width
        end = (start_elem + elem_count) * width
        return Buffer(
            self._buf._data[start:end],
            self._buf.element_type,
            elem_count,
            format_char=self._buf.format_char,
        )

    # ── Shape operations ──────────────────────────────────────────────

    def reshape(self, *shape) -> "Tensor":
        """Return a tensor with the same data but a new shape.

        One dimension can be -1, which is inferred from the others.
        """
        return tensor_reshape_view(self, shape)

    def transpose(self, dim0=None, dim1=None) -> "Tensor":
        """Transpose a 2D tensor or swap two explicit axes."""
        if dim0 is not None or dim1 is not None:
            if dim0 is None or dim1 is None:
                raise TypeError("transpose expects either zero args or two dims")
            ndim = self.ndim
            a = dim0 + ndim if dim0 < 0 else dim0
            b = dim1 + ndim if dim1 < 0 else dim1
            if a < 0 or a >= ndim or b < 0 or b >= ndim:
                raise ValueError(
                    f"transpose dims {(dim0, dim1)} out of range for ndim={ndim}"
                )
            dims = list(range(ndim))
            dims[a], dims[b] = dims[b], dims[a]
            return tensor_permute_dims(self, tuple(dims))

        """Transpose a 2D tensor (swap rows and columns)."""
        if self.ndim < 2:
            return Tensor(self._buf, shape=self._shape, dtype=self._dtype)

        rows, cols = self._shape[-2], self._shape[-1]
        batch_shape = self._shape[:-2]
        batch_size = _product(batch_shape) if batch_shape else 1
        data = self._data_list()
        result = [0.0] * len(data)
        stride = rows * cols

        for b in range(batch_size):
            base = b * stride
            for r in range(rows):
                for c in range(cols):
                    result[base + c * rows + r] = data[base + r * cols + c]

        new_shape = batch_shape + (cols, rows)
        return self._from_flat(result, new_shape)

    def permute(self, *dims) -> "Tensor":
        """Reorder tensor dimensions."""
        return tensor_permute_dims(self, dims)

    def unsqueeze(self, dim: int) -> "Tensor":
        ndim = self.ndim + 1
        if dim < 0:
            dim += ndim
        if dim < 0 or dim >= ndim:
            raise ValueError(f"unsqueeze dim {dim} out of range for ndim={self.ndim}")
        shape = list(self._shape)
        shape.insert(dim, 1)
        return Tensor(self._buf, shape=tuple(shape), dtype=self._dtype)

    def squeeze(self, dim=None) -> "Tensor":
        if dim is None:
            shape = tuple(size for size in self._shape if size != 1)
            return Tensor(self._buf, shape=shape, dtype=self._dtype)
        if self.ndim == 0:
            return Tensor(self._buf, shape=self._shape, dtype=self._dtype)
        if dim < 0:
            dim += self.ndim
        if dim < 0 or dim >= self.ndim:
            raise ValueError(f"squeeze dim {dim} out of range for ndim={self.ndim}")
        if self._shape[dim] != 1:
            return Tensor(self._buf, shape=self._shape, dtype=self._dtype)
        shape = list(self._shape)
        del shape[dim]
        return Tensor(self._buf, shape=tuple(shape), dtype=self._dtype)

    def expand(self, *shape) -> "Tensor":
        if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
            shape = tuple(shape[0])
        else:
            shape = tuple(shape)
        if len(shape) < self.ndim:
            raise ValueError(f"cannot expand {self._shape} to {shape}")
        padded = (1,) * (len(shape) - self.ndim) + self._shape
        for src, dst in zip(padded, shape):
            if src != 1 and src != dst:
                raise ValueError(f"cannot expand {self._shape} to {shape}")
        if not shape:
            return Tensor(self._buf, shape=(), dtype=self._dtype)
        src_data = self._data_list()
        src_strides = _strides(padded) if padded else ()
        out = []
        total = _product(shape)
        for flat_idx in range(total):
            rem = flat_idx
            coords = []
            for stride, axis in zip(_strides(shape), shape):
                coord = rem // stride
                rem %= stride
                coords.append(coord)
            src_index = 0
            for coord, axis_size, stride in zip(coords, padded, src_strides):
                src_index += (0 if axis_size == 1 else coord) * stride
            out.append(src_data[src_index])
        return self._from_flat(out, shape)

    def cast(self, dtype) -> "Tensor":
        kind = _dtype_cast_kind(dtype)
        if kind == "float":
            format_char = "f" if self._buf.format_char == "f" else "d"
            out_buf = alloc(self.size, float, format_char=format_char)
            for idx, value in enumerate(self._data_list()):
                out_buf[idx] = float(value)
            return Tensor(out_buf, shape=self._shape, dtype=float)
        if kind == "int":
            out_buf = alloc(self.size, int, format_char="q")
            for idx, value in enumerate(self._data_list()):
                out_buf[idx] = int(value)
            return Tensor(out_buf, shape=self._shape, dtype=int)
        raise TypeError(f"unsupported cast dtype {dtype!r}")

    def float(self) -> "Tensor":
        return self.cast(float)

    def cat(self, other, dim=0) -> "Tensor":
        if not isinstance(other, Tensor):
            raise TypeError(f"Expected Tensor, got {type(other)!r}")
        ndim = self.ndim
        if ndim != other.ndim:
            raise ValueError(f"cat rank mismatch: {self._shape} vs {other._shape}")
        if dim < 0:
            dim += ndim
        if dim < 0 or dim >= ndim:
            raise ValueError(f"cat dim {dim} out of range for ndim={ndim}")
        if dim == 0:
            return tensor_concat_first_dim((self, other))

        if any(
            a != b
            for axis, (a, b) in enumerate(zip(self._shape, other._shape))
            if axis != dim
        ):
            raise ValueError(f"cat shape mismatch: {self._shape} vs {other._shape}")
        if self._dtype is not other._dtype:
            raise ValueError("cat requires matching dtypes")
        if self._buf.format_char != other._buf.format_char:
            raise ValueError("cat requires matching buffer formats")

        perm = [dim]
        for axis in range(ndim):
            if axis != dim:
                perm.append(axis)
        left = tensor_permute_dims(self, perm)
        right = tensor_permute_dims(other, perm)
        concatenated = tensor_concat_first_dim((left, right))

        inverse = [0] * ndim
        for axis, src in enumerate(perm):
            inverse[src] = axis
        return tensor_permute_dims(concatenated, inverse)

    @staticmethod
    def stack(*tensors, dim=0) -> "Tensor":
        if not tensors:
            raise ValueError("stack requires at least one tensor")
        if len(tensors) == 1 and isinstance(tensors[0], (list, tuple)):
            tensors = tuple(tensors[0])
        first = tensors[0]
        if not isinstance(first, Tensor):
            raise TypeError(f"Expected Tensor, got {type(first)!r}")
        ndim = first.ndim + 1
        if dim < 0:
            dim += ndim
        if dim < 0 or dim >= ndim:
            raise ValueError(f"stack dim {dim} out of range for ndim={first.ndim}")
        for tensor in tensors:
            if not isinstance(tensor, Tensor):
                raise TypeError(f"Expected Tensor, got {type(tensor)!r}")
            if tensor.shape != first.shape:
                raise ValueError(
                    f"stack shape mismatch: {tensor.shape} vs {first.shape}"
                )
            if tensor._dtype is not first._dtype:
                raise ValueError("stack requires matching dtypes")
            if tensor._buf.format_char != first._buf.format_char:
                raise ValueError("stack requires matching buffer formats")

        expanded = [tensor.unsqueeze(dim) for tensor in tensors]
        if dim == 0:
            return tensor_concat_first_dim(tuple(expanded))

        perm = [dim]
        for axis in range(ndim):
            if axis != dim:
                perm.append(axis)
        permuted = [tensor_permute_dims(tensor, perm) for tensor in expanded]
        concatenated = tensor_concat_first_dim(tuple(permuted))

        inverse = [0] * ndim
        for axis, src in enumerate(perm):
            inverse[src] = axis
        return tensor_permute_dims(concatenated, inverse)

    @staticmethod
    def zeros(*shape) -> "Tensor":
        return zeros(*shape)

    @staticmethod
    def manual_seed(seed=0) -> None:
        _TINYGRAD_RNG_STATE[0] = _u32(seed)
        _TINYGRAD_RNG_STATE[1] = 0

    @staticmethod
    def rand(*shape) -> "Tensor":
        return rand(*shape)

    @staticmethod
    def randn(*shape) -> "Tensor":
        return randn(*shape)

    @staticmethod
    def uniform(*shape, low=0.0, high=1.0, dtype=None, requires_grad=None) -> "Tensor":
        return uniform(
            *shape,
            low=low,
            high=high,
            dtype=dtype,
            requires_grad=requires_grad,
        )

    @staticmethod
    def scaled_uniform(*shape, dtype=None, requires_grad=None) -> "Tensor":
        return scaled_uniform(
            *shape,
            dtype=dtype,
            requires_grad=requires_grad,
        )

    @staticmethod
    def glorot_uniform(*shape, dtype=None, requires_grad=None) -> "Tensor":
        return glorot_uniform(
            *shape,
            dtype=dtype,
            requires_grad=requires_grad,
        )

    @staticmethod
    def arange(start, stop=None, step=1) -> "Tensor":
        if stop is None:
            start, stop = 0, start
        values = []
        cur = start
        if step == 0:
            raise ValueError("step must not be zero")
        if step > 0:
            while cur < stop:
                values.append(cur)
                cur += step
        else:
            while cur > stop:
                values.append(cur)
                cur += step
        return Tensor(values, shape=(len(values),), dtype=float)

    @property
    def T(self) -> "Tensor":
        """Transpose (property alias)."""
        return self.transpose()

    def flatten(self) -> "Tensor":
        """Return a 1D view of the tensor."""
        return self.reshape(self.size)

    # ── Elementwise arithmetic ────────────────────────────────────────

    def _apply_binary_op(self, a: float, b: float, op_code: int) -> float:
        if op_code == _OP_ADD:
            return a + b
        if op_code == _OP_SUB:
            return a - b
        if op_code == _OP_MUL:
            return a * b
        if op_code == _OP_DIV:
            try:
                return a / b
            except ZeroDivisionError:
                if a > 0:
                    return float("inf")
                if a < 0:
                    return float("-inf")
                return float("nan")
        raise ValueError(f"Unsupported binary op code {op_code!r}")

    def _broadcast_op(self, other, op_code: int):
        """Apply a binary op with scalar or tensor broadcasting."""
        if isinstance(other, (int, float)):
            scalar = float(other)
            result_dtype, result_format = _binary_result_dtype_and_format(self, other)
            result_buf = alloc(self.size, result_dtype, format_char=result_format)
            src_buf = self._buf
            apply = self._apply_binary_op
            for i in range(self.size):
                result_buf[i] = apply(src_buf[i], scalar, op_code)
            return Tensor(result_buf, shape=self._shape, dtype=result_dtype)
        if not isinstance(other, Tensor):
            return NotImplemented

        a_buf = self._buf
        b_buf = other._buf
        apply = self._apply_binary_op

        result_dtype, result_format = _binary_result_dtype_and_format(self, other)

        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_BROADCAST_BINARY_CONTIGUOUS",
            "molt_gpu_broadcast_binary_contiguous",
        )
        if intrinsic is not None:
            out_bits = intrinsic(
                a_buf._data,
                a_buf.format_char,
                self._shape,
                b_buf._data,
                b_buf.format_char,
                other._shape,
                op_code,
                result_format,
            )
            out_ndim = max(self.ndim, other.ndim)
            a_shape = (1,) * (out_ndim - self.ndim) + self._shape
            b_shape = (1,) * (out_ndim - other.ndim) + other._shape
            out_shape = []
            for a_dim, b_dim in zip(a_shape, b_shape):
                if a_dim == b_dim:
                    out_shape.append(a_dim)
                elif a_dim == 1:
                    out_shape.append(b_dim)
                elif b_dim == 1:
                    out_shape.append(a_dim)
                else:
                    raise ValueError(
                        f"Cannot broadcast shapes {self._shape} and {other._shape}"
                    )
            out_buf = Buffer(
                out_bits,
                result_dtype,
                _product(out_shape),
                format_char=result_format,
            )
            return Tensor(out_buf, shape=tuple(out_shape), dtype=result_dtype)

        # Same shape — elementwise
        if self._shape == other._shape:
            result_buf = alloc(self.size, result_dtype, format_char=result_format)
            for i in range(self.size):
                result_buf[i] = apply(a_buf[i], b_buf[i], op_code)
            return Tensor(result_buf, shape=self._shape, dtype=result_dtype)

        # Broadcast: one of them is a scalar
        if other.size == 1:
            scalar = b_buf[0]
            result_buf = alloc(self.size, result_dtype, format_char=result_format)
            for i in range(self.size):
                result_buf[i] = apply(a_buf[i], scalar, op_code)
            return Tensor(result_buf, shape=self._shape, dtype=result_dtype)
        if self.size == 1:
            scalar = a_buf[0]
            result_buf = alloc(other.size, result_dtype, format_char=result_format)
            for i in range(other.size):
                result_buf[i] = apply(scalar, b_buf[i], op_code)
            return Tensor(result_buf, shape=other._shape, dtype=result_dtype)

        out_ndim = max(self.ndim, other.ndim)
        a_shape = (1,) * (out_ndim - self.ndim) + self._shape
        b_shape = (1,) * (out_ndim - other.ndim) + other._shape
        out_shape = []
        for a_dim, b_dim in zip(a_shape, b_shape):
            if a_dim == b_dim:
                out_shape.append(a_dim)
            elif a_dim == 1:
                out_shape.append(b_dim)
            elif b_dim == 1:
                out_shape.append(a_dim)
            else:
                raise ValueError(
                    f"Cannot broadcast shapes {self._shape} and {other._shape}"
                )

        a_strides = _strides(a_shape)
        b_strides = _strides(b_shape)
        out_strides = _strides(tuple(out_shape))
        result_shape = tuple(out_shape)
        result_buf = alloc(
            _product(out_shape),
            result_dtype,
            format_char=result_format,
        )

        for out_index in range(result_buf.size):
            rem = out_index
            a_index = 0
            b_index = 0
            for axis, out_stride in enumerate(out_strides):
                coord = rem // out_stride
                rem %= out_stride
                if a_shape[axis] != 1:
                    a_index += coord * a_strides[axis]
                if b_shape[axis] != 1:
                    b_index += coord * b_strides[axis]
            result_buf[out_index] = apply(a_buf[a_index], b_buf[b_index], op_code)

        return Tensor(result_buf, shape=result_shape, dtype=result_dtype)

    def __add__(self, other) -> "Tensor":
        return self._broadcast_op(other, _OP_ADD)

    def __radd__(self, other) -> "Tensor":
        return self._broadcast_op(other, _OP_ADD)

    def __sub__(self, other) -> "Tensor":
        return self._broadcast_op(other, _OP_SUB)

    def __rsub__(self, other) -> "Tensor":
        if isinstance(other, (int, float)):
            data = self._data_list()
            return self._from_flat([float(other) - x for x in data], self._shape)
        return NotImplemented

    def __mul__(self, other) -> "Tensor":
        return self._broadcast_op(other, _OP_MUL)

    def __rmul__(self, other) -> "Tensor":
        return self._broadcast_op(other, _OP_MUL)

    def __truediv__(self, other) -> "Tensor":
        return self._broadcast_op(other, _OP_DIV)

    def __rtruediv__(self, other) -> "Tensor":
        if isinstance(other, (int, float)):
            data = self._data_list()

            def _safe_rdiv(x):
                try:
                    return float(other) / x
                except ZeroDivisionError:
                    if other > 0:
                        return float("inf")
                    elif other < 0:
                        return float("-inf")
                    else:
                        return float("nan")

            return self._from_flat([_safe_rdiv(x) for x in data], self._shape)
        return NotImplemented

    def __pow__(self, other) -> "Tensor":
        if isinstance(other, (int, float)):
            data = self._data_list()
            exp = float(other)
            return self._from_flat([x**exp for x in data], self._shape)
        if isinstance(other, Tensor):
            if self.shape != other.shape:
                raise ValueError(f"pow shape mismatch: {self.shape} vs {other.shape}")
            a = self._data_list()
            b = other._data_list()
            return self._from_flat([x**y for x, y in zip(a, b)], self._shape)
        return NotImplemented

    def __rpow__(self, other) -> "Tensor":
        if isinstance(other, (int, float)):
            data = self._data_list()
            base = float(other)
            return self._from_flat([base**x for x in data], self._shape)
        return NotImplemented

    def __neg__(self) -> "Tensor":
        data = self._data_list()
        return self._from_flat([-x for x in data], self._shape)

    # ── Matrix multiplication ─────────────────────────────────────────

    def __matmul__(self, other) -> "Tensor":
        """Matrix multiply (2D @ 2D, or batched).

        Supports:
            (M, K) @ (K, N) -> (M, N)
            (B, M, K) @ (B, K, N) -> (B, M, N)
            (1, K) @ (K, N) -> (1, N)
        """
        if not isinstance(other, Tensor):
            return NotImplemented

        # 1D @ 1D -> dot product
        if self.ndim == 1 and other.ndim == 1:
            a = self._data_list()
            b = other._data_list()
            if len(a) != len(b):
                raise ValueError(f"Dot product size mismatch: {len(a)} vs {len(b)}")
            return Tensor(sum(a[i] * b[i] for i in range(len(a))))

        # Ensure 2D
        a = self if self.ndim >= 2 else self.reshape(1, self.size)
        b = other if other.ndim >= 2 else other.reshape(other.size, 1)

        a_rows, a_cols = a._shape[-2], a._shape[-1]
        b_rows, b_cols = b._shape[-2], b._shape[-1]

        if a_cols != b_rows:
            raise ValueError(f"Matmul shape mismatch: {a._shape} @ {b._shape}")

        a_batch_shape = a._shape[:-2]
        b_batch_shape = b._shape[:-2]
        out_batch_ndim = max(len(a_batch_shape), len(b_batch_shape))
        padded_a_batch_shape = (1,) * (
            out_batch_ndim - len(a_batch_shape)
        ) + a_batch_shape
        padded_b_batch_shape = (1,) * (
            out_batch_ndim - len(b_batch_shape)
        ) + b_batch_shape
        out_batch_shape = []
        for a_dim, b_dim in zip(padded_a_batch_shape, padded_b_batch_shape):
            if a_dim == b_dim:
                out_batch_shape.append(a_dim)
            elif a_dim == 1:
                out_batch_shape.append(b_dim)
            elif b_dim == 1:
                out_batch_shape.append(a_dim)
            else:
                raise ValueError(
                    f"Matmul batch shape mismatch: {self._shape} @ {other._shape}"
                )
        out_batch_shape = tuple(out_batch_shape)
        batch_count = _product(out_batch_shape) if out_batch_shape else 1
        out_shape = out_batch_shape + (a_rows, b_cols)
        if not out_shape:
            out_shape = (a_rows, b_cols)
        if a._dtype is float and b._dtype is float:
            result_format = _preferred_float_format(a, b)
        else:
            result_format = a._buf.format_char

        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_MATMUL_CONTIGUOUS", "molt_gpu_matmul_contiguous"
        )
        if intrinsic is not None:
            out_bits = intrinsic(
                a._buf._data,
                a._buf.format_char,
                a._shape,
                b._buf._data,
                b._buf.format_char,
                b._shape,
                result_format,
            )
            out_buf = Buffer(
                out_bits,
                a._dtype,
                _product(out_shape),
                format_char=result_format,
            )
            return Tensor(out_buf, shape=out_shape, dtype=a._dtype)

        a_data = a._data_list()
        b_data = b._data_list()

        result = []
        a_stride = a_rows * a_cols
        b_stride = b_rows * b_cols

        a_batch_strides = _strides(padded_a_batch_shape) if padded_a_batch_shape else ()
        b_batch_strides = _strides(padded_b_batch_shape) if padded_b_batch_shape else ()
        out_batch_strides = _strides(out_batch_shape) if out_batch_shape else ()

        for batch in range(batch_count):
            rem = batch
            a_batch_index = 0
            b_batch_index = 0
            for axis, out_stride in enumerate(out_batch_strides):
                coord = rem // out_stride
                rem %= out_stride
                if padded_a_batch_shape[axis] != 1:
                    a_batch_index += coord * a_batch_strides[axis]
                if padded_b_batch_shape[axis] != 1:
                    b_batch_index += coord * b_batch_strides[axis]
            a_off = a_batch_index * a_stride
            b_off = b_batch_index * b_stride
            for i in range(a_rows):
                for j in range(b_cols):
                    s = 0.0
                    for k in range(a_cols):
                        s += (
                            a_data[a_off + i * a_cols + k]
                            * b_data[b_off + k * b_cols + j]
                        )
                    result.append(s)

        out_buf = alloc(len(result), a._dtype, format_char=result_format)
        for idx, value in enumerate(result):
            out_buf[idx] = value
        return Tensor(out_buf, shape=out_shape, dtype=a._dtype)

    def linear(self, weight) -> "Tensor":
        """Apply a linear projection with weight shaped (out_features, in_features).

        This computes ``self @ weight.T`` without materializing a transposed copy
        of ``weight``. Leading dimensions on ``self`` are treated as batch dims.
        """
        return tensor_linear(self, weight)

    def linear_split_last_dim(self, weight, sizes) -> tuple["Tensor", ...]:
        """Apply a linear projection, then split the output last dimension."""
        return tensor_linear_split_last_dim(self, weight, sizes)

    def linear_squared_relu_gate_interleaved(self, weight) -> "Tensor":
        """Apply linear(weight) then interleaved squared-ReLU gating."""
        return tensor_linear_squared_relu_gate_interleaved(self, weight)

    def conv2d(
        self,
        weight,
        bias=None,
        groups: int = 1,
        stride=1,
        dilation=1,
        padding=0,
    ) -> "Tensor":
        return tensor_conv2d(
            self,
            weight,
            bias=bias,
            groups=groups,
            stride=stride,
            dilation=dilation,
            padding=padding,
        )

    def split_last_dim(self, sizes) -> tuple["Tensor", ...]:
        """Split the last dimension into contiguous views copied into new buffers."""
        if self.ndim == 0:
            raise ValueError(
                "split_last_dim requires a tensor with at least 1 dimension"
            )
        normalized_sizes = []
        for size in sizes:
            if isinstance(size, bool):
                raise TypeError("split sizes must be integers")
            try:
                normalized_sizes.append(operator.index(size))
            except TypeError as exc:
                raise TypeError("split sizes must be integers") from exc
        sizes = tuple(normalized_sizes)
        if any(size < 0 for size in sizes):
            raise ValueError("split sizes must be non-negative")
        if sum(sizes) != self._shape[-1]:
            raise ValueError(
                f"split sizes {sizes} do not match last dimension {self._shape[-1]}"
            )

        outer = _product(self._shape[:-1]) if self.ndim > 1 else 1
        itemsize = self._buf.itemsize
        row_width = self._shape[-1] * itemsize
        result_shape_prefix = self._shape[:-1]
        outputs = []
        for size in sizes:
            out_buf = Buffer(
                bytearray(outer * size * itemsize),
                self._buf.element_type,
                outer * size,
                format_char=self._buf.format_char,
            )
            outputs.append((size, out_buf))

        src = self._buf._data
        for row in range(outer):
            row_base = row * row_width
            offset = 0
            for size, out_buf in outputs:
                span = size * itemsize
                dst_base = row * span
                out_buf._data[dst_base : dst_base + span] = src[
                    row_base + offset : row_base + offset + span
                ]
                offset += span

        return tuple(
            Tensor(out_buf, shape=result_shape_prefix + (size,), dtype=self._dtype)
            for size, out_buf in outputs
        )

    def repeat_axis(self, axis: int, repeats: int) -> "Tensor":
        """Repeat contiguous slices along an axis."""
        if repeats < 0:
            raise ValueError("repeats must be non-negative")
        if self.ndim == 0:
            raise ValueError("repeat_axis requires a tensor with at least 1 dimension")
        if axis < 0:
            axis += self.ndim
        if axis < 0 or axis >= self.ndim:
            raise ValueError(f"Invalid axis {axis} for tensor with {self.ndim} dims")
        if repeats == 1:
            return Tensor(self._buf, shape=self._shape, dtype=self._dtype)
        if repeats == 0:
            out_shape = self._shape[:axis] + (0,) + self._shape[axis + 1 :]
            return Tensor(
                Buffer(
                    bytearray(0),
                    self._buf.element_type,
                    0,
                    format_char=self._buf.format_char,
                ),
                shape=out_shape,
                dtype=self._dtype,
            )

        out_shape = (
            self._shape[:axis]
            + (self._shape[axis] * repeats,)
            + self._shape[axis + 1 :]
        )
        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_REPEAT_AXIS_CONTIGUOUS",
            "molt_gpu_repeat_axis_contiguous",
        )
        if intrinsic is not None:
            out_bits = intrinsic(
                self._buf._data,
                self._buf.format_char,
                self._shape,
                axis,
                repeats,
                self._buf.format_char,
            )
            return _tensor_from_parts(
                out_bits,
                self._buf.element_type,
                _product(out_shape),
                self._buf.format_char,
                out_shape,
                self._dtype,
            )

        outer = _product(self._shape[:axis]) if axis > 0 else 1
        axis_len = self._shape[axis]
        inner = _product(self._shape[axis + 1 :]) if axis + 1 < self.ndim else 1
        chunk_bytes = inner * self._buf.itemsize
        src_axis_bytes = axis_len * chunk_bytes
        out_axis_len = axis_len * repeats
        out = bytearray(outer * out_axis_len * chunk_bytes)
        src = self._buf._data

        for outer_idx in range(outer):
            src_outer = outer_idx * src_axis_bytes
            dst_outer = outer_idx * out_axis_len * chunk_bytes
            for axis_idx in range(axis_len):
                src_base = src_outer + axis_idx * chunk_bytes
                chunk = src[src_base : src_base + chunk_bytes]
                dst_base = dst_outer + axis_idx * repeats * chunk_bytes
                out[dst_base : dst_base + repeats * chunk_bytes] = chunk * repeats

        out_shape = self._shape[:axis] + (out_axis_len,) + self._shape[axis + 1 :]
        out_buf = Buffer(
            out,
            self._buf.element_type,
            outer * out_axis_len * inner,
            format_char=self._buf.format_char,
        )
        return Tensor(out_buf, shape=out_shape, dtype=self._dtype)

    def take_rows(self, indices, *, allow_negative: bool = True) -> "Tensor":
        """Gather slices along axis 0 without materializing the full tensor."""
        return tensor_take_rows(self, indices, allow_negative=allow_negative)

    def gather(self, dim: int, index: "Tensor") -> "Tensor":
        """tinygrad-compatible gather for Falcon-style row selection."""
        if dim < 0:
            dim += self.ndim
        if dim != 0:
            raise NotImplementedError("Tensor.gather currently supports dim=0")
        return tensor_take_rows(self, index, allow_negative=False)

    def scatter(self, dim: int, index: "Tensor", src: "Tensor") -> "Tensor":
        """tinygrad-compatible scatter for Falcon-style row updates."""
        if dim < 0:
            dim += self.ndim
        if dim != 0:
            raise NotImplementedError("Tensor.scatter currently supports dim=0")
        return tensor_scatter_rows(self, index, src, allow_negative=False)

    # ── Reductions ────────────────────────────────────────────────────

    def _reduce(self, op, axis=None, initial=None, keepdim: bool = False):
        """Generic reduction along an axis."""
        data = self._data_list()

        if axis is None:
            # Reduce all elements to a scalar
            result = data[0] if initial is None else initial
            start = 0 if initial is not None else 1
            for i in range(start, len(data)):
                result = op(result, data[i])
            return Tensor(result)

        # Normalize negative axis
        if axis < 0:
            axis = self.ndim + axis
        if axis < 0 or axis >= self.ndim:
            raise ValueError(f"Invalid axis {axis} for tensor with {self.ndim} dims")

        # Compute output shape (remove or preserve the reduction axis)
        if keepdim:
            out_shape = self._shape[:axis] + (1,) + self._shape[axis + 1 :]
        else:
            out_shape = self._shape[:axis] + self._shape[axis + 1 :]
        if not out_shape:
            out_shape = ()

        out_size = _product(out_shape) if out_shape else 1
        result = [None] * out_size

        # Stride-based reduction
        outer = _product(self._shape[:axis]) if axis > 0 else 1
        axis_len = self._shape[axis]
        inner = _product(self._shape[axis + 1 :]) if axis + 1 < self.ndim else 1

        for o in range(outer):
            for inn in range(inner):
                out_idx = o * inner + inn
                vals = []
                for a in range(axis_len):
                    idx = o * axis_len * inner + a * inner + inn
                    vals.append(data[idx])
                acc = vals[0] if initial is None else initial
                start = 0 if initial is not None else 1
                for i in range(start, len(vals)):
                    acc = op(acc, vals[i])
                result[out_idx] = acc

        if not out_shape:
            return Tensor(result[0])
        return self._from_flat(result, out_shape)

    def sum(self, axis=None, keepdim: bool = False) -> "Tensor":
        """Sum elements, optionally along an axis."""
        return self._reduce(lambda a, b: a + b, axis=axis, initial=0.0, keepdim=keepdim)

    def mean(self, axis=None, keepdim: bool = False) -> "Tensor":
        """Mean of elements, optionally along an axis."""
        s = self.sum(axis=axis, keepdim=keepdim)
        if axis is None:
            n = self.size
        else:
            if axis < 0:
                axis = self.ndim + axis
            n = self._shape[axis]
        return s / float(n)

    def max(self, axis=None, keepdim: bool = False) -> "Tensor":
        """Max element, optionally along an axis."""
        return self._reduce(lambda a, b: a if a >= b else b, axis=axis, keepdim=keepdim)

    def min(self, axis=None, keepdim: bool = False) -> "Tensor":
        """Min element, optionally along an axis."""
        return self._reduce(lambda a, b: a if a <= b else b, axis=axis, keepdim=keepdim)

    # ── Activation functions ──────────────────────────────────────────

    def relu(self) -> "Tensor":
        """Rectified linear unit: max(0, x)."""
        data = self._data_list()
        return self._from_flat([x if x > 0 else 0.0 for x in data], self._shape)

    def sigmoid(self) -> "Tensor":
        """Logistic sigmoid: 1 / (1 + exp(-x))."""
        data = self._data_list()
        result = []
        for x in data:
            # Clamp to avoid overflow
            if x > 500:
                result.append(1.0)
            elif x < -500:
                result.append(0.0)
            else:
                result.append(1.0 / (1.0 + math.exp(-x)))
        return self._from_flat(result, self._shape)

    def tanh(self) -> "Tensor":
        """Hyperbolic tangent."""
        data = self._data_list()
        return self._from_flat([math.tanh(x) for x in data], self._shape)

    def sin(self) -> "Tensor":
        data = self._data_list()
        return self._from_flat([math.sin(x) for x in data], self._shape)

    def cos(self) -> "Tensor":
        data = self._data_list()
        return self._from_flat([math.cos(x) for x in data], self._shape)

    def softmax(self, axis=-1) -> "Tensor":
        """Softmax along an axis (default: last axis).

        softmax(x)_i = exp(x_i - max(x)) / sum(exp(x_j - max(x)))
        """
        if axis < 0:
            axis = self.ndim + axis
        if self.ndim == 0:
            return Tensor(1.0)

        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS",
            "molt_gpu_softmax_last_axis_contiguous",
        )
        if axis == self.ndim - 1 and intrinsic is not None:
            return tensor_softmax_last_axis(self)

        data = self._data_list()

        # Compute outer/inner strides
        outer = _product(self._shape[:axis]) if axis > 0 else 1
        axis_len = self._shape[axis]
        inner = _product(self._shape[axis + 1 :]) if axis + 1 < self.ndim else 1

        result = [0.0] * len(data)

        for o in range(outer):
            for inn in range(inner):
                # Gather values along axis
                indices = []
                vals = []
                for a in range(axis_len):
                    idx = o * axis_len * inner + a * inner + inn
                    indices.append(idx)
                    vals.append(data[idx])

                # Numerically stable softmax
                max_val = max(vals)
                exps = [math.exp(v - max_val) for v in vals]
                total = sum(exps)
                for i, idx in enumerate(indices):
                    result[idx] = exps[i] / total

        out_buf = alloc(len(result), self._dtype, format_char=self._buf.format_char)
        for idx, value in enumerate(result):
            out_buf[idx] = value
        return Tensor(out_buf, shape=self._shape, dtype=self._dtype)

    def layernorm(self, axis=-1, eps: float = 1e-5) -> "Tensor":
        """Layer normalization over one or more axes."""
        if self.ndim == 0:
            raise ValueError("layernorm requires a tensor with at least 1 dimension")

        if isinstance(axis, int):
            axes = (axis,)
        else:
            axes = tuple(axis)
        if not axes:
            raise ValueError("layernorm axis must be non-empty")

        normalized = []
        for dim in axes:
            if dim < 0:
                dim += self.ndim
            if dim < 0 or dim >= self.ndim:
                raise ValueError(
                    f"Invalid axis {axis} for tensor with {self.ndim} dims"
                )
            normalized.append(dim)
        if len(set(normalized)) != len(normalized):
            raise ValueError("layernorm axes must be unique")
        mean = self
        for dim in normalized:
            mean = mean.mean(dim, keepdim=True)
        centered = self - mean
        var = centered * centered
        for dim in normalized:
            var = var.mean(dim, keepdim=True)
        return centered * (var + eps).rsqrt()

    def scaled_dot_product_attention(
        self,
        k: "Tensor",
        v: "Tensor",
        attn_mask: "Tensor | None" = None,
        scale: float | None = None,
        is_causal: bool = False,
    ) -> "Tensor":
        """tinygrad-compatible SDPA instance method."""
        actual_scale = (
            scale if scale is not None else (1.0 / math.sqrt(self._shape[-1]))
        )
        mask = attn_mask
        if is_causal:
            seq_q = self._shape[-2]
            seq_k = k._shape[-2]
            values = []
            offset = seq_k - seq_q
            for q_idx in range(seq_q):
                allowed_until = q_idx + offset
                for k_idx in range(seq_k):
                    values.append(0.0 if k_idx <= allowed_until else float("-inf"))
            causal_mask = Tensor(values, shape=(1, 1, seq_q, seq_k), dtype=float)
            mask = causal_mask if mask is None else mask + causal_mask
        return tensor_scaled_dot_product_attention(self, k, v, mask, actual_scale)

    def rms_norm(self, eps: float) -> "Tensor":
        """RMSNorm over the last axis."""
        if self.ndim == 0:
            raise ValueError("rms_norm requires a tensor with at least 1 dimension")
        if self._shape[-1] == 0:
            raise ValueError("rms_norm last axis must be non-empty")

        if self._dtype is float and self._buf.element_type is float:
            result_dtype = self._dtype
            result_format = self._buf.format_char
        else:
            result_dtype = float
            result_format = "d"

        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_RMS_NORM_LAST_AXIS_CONTIGUOUS",
            "molt_gpu_rms_norm_last_axis_contiguous",
        )
        if intrinsic is not None:
            out_bits = intrinsic(
                self._buf._data,
                self._buf.format_char,
                self._shape,
                float(eps),
                result_format,
            )
            out_buf = Buffer(
                out_bits,
                result_dtype,
                self.size,
                format_char=result_format,
            )
            return Tensor(out_buf, shape=self._shape, dtype=result_dtype)

        data = self._data_list()
        axis_len = self._shape[-1]
        outer = self.size // axis_len
        out_buf = alloc(self.size, result_dtype, format_char=result_format)
        axis_len_f = float(axis_len)

        for row in range(outer):
            base = row * axis_len
            sumsq = 0.0
            for i in range(axis_len):
                value = float(data[base + i])
                sumsq += value * value
            scale = 1.0 / math.sqrt((sumsq / axis_len_f) + float(eps))
            for i in range(axis_len):
                out_buf[base + i] = float(data[base + i]) * scale

        return Tensor(out_buf, shape=self._shape, dtype=result_dtype)

    def squared_relu_gate_interleaved(self) -> "Tensor":
        """Apply relu(gate)^2 * up over an interleaved last axis."""
        if self.ndim == 0:
            raise ValueError(
                "squared_relu_gate_interleaved requires a tensor with at least 1 dimension"
            )
        if self._shape[-1] % 2 != 0:
            raise ValueError("squared_relu_gate_interleaved last axis must be even")

        out_shape = self._shape[:-1] + (self._shape[-1] // 2,)
        if self._dtype is float and self._buf.element_type is float:
            result_dtype = self._dtype
            result_format = self._buf.format_char
        else:
            result_dtype = float
            result_format = "d"

        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS",
            "molt_gpu_squared_relu_gate_interleaved_contiguous",
        )
        if intrinsic is not None:
            out_bits = intrinsic(
                self._buf._data,
                self._buf.format_char,
                self._shape,
                result_format,
            )
            out_buf = Buffer(
                out_bits,
                result_dtype,
                _product(out_shape),
                format_char=result_format,
            )
            return Tensor(out_buf, shape=out_shape, dtype=result_dtype)

        data = self._data_list()
        axis_len = self._shape[-1]
        hidden = axis_len // 2
        outer = self.size // axis_len
        out_buf = alloc(_product(out_shape), result_dtype, format_char=result_format)

        for row in range(outer):
            in_base = row * axis_len
            out_base = row * hidden
            for i in range(hidden):
                gate = float(data[in_base + 2 * i])
                up = float(data[in_base + 2 * i + 1])
                relu = gate if gate > 0.0 else 0.0
                out_buf[out_base + i] = relu * relu * up

        return Tensor(out_buf, shape=out_shape, dtype=result_dtype)

    def exp(self) -> "Tensor":
        """Element-wise exponential.

        Inputs are clamped to [-709, 709] to prevent float64 overflow.
        """
        data = self._data_list()
        return self._from_flat(
            [math.exp(max(-709.0, min(709.0, x))) for x in data],
            self._shape,
        )

    def log(self) -> "Tensor":
        """Element-wise natural logarithm.

        Values <= 0 are clamped to a tiny positive epsilon to avoid
        math domain errors.  This matches the safe-log convention used
        in most ML frameworks.
        """
        data = self._data_list()
        _EPS = 1e-45
        return self._from_flat(
            [math.log(x) if x > 0 else math.log(_EPS) for x in data],
            self._shape,
        )

    def sqrt(self) -> "Tensor":
        """Element-wise square root.

        Negative values produce NaN (matches IEEE 754 / NumPy behavior).
        """
        data = self._data_list()
        return self._from_flat(
            [math.sqrt(x) if x >= 0 else float("nan") for x in data],
            self._shape,
        )

    def rsqrt(self) -> "Tensor":
        """Element-wise reciprocal square root."""
        data = self._data_list()
        return self._from_flat(
            [
                1.0 / math.sqrt(x)
                if x > 0
                else float("inf")
                if x == 0
                else float("nan")
                for x in data
            ],
            self._shape,
        )

    def abs(self) -> "Tensor":
        """Element-wise absolute value."""
        data = self._data_list()
        return self._from_flat([abs(x) for x in data], self._shape)

    def clamp(self, min_val=None, max_val=None) -> "Tensor":
        """Clamp values to [min_val, max_val]."""
        data = self._data_list()
        result = []
        for x in data:
            if min_val is not None and x < min_val:
                x = float(min_val)
            if max_val is not None and x > max_val:
                x = float(max_val)
            result.append(x)
        return self._from_flat(result, self._shape)

    # ── Conversion / display ──────────────────────────────────────────

    def to_list(self) -> list:
        """Convert tensor to a (possibly nested) Python list."""
        data = self._data_list()
        if not self._shape:
            return data[0] if data else 0.0

        def _nest(flat, shape):
            if len(shape) == 1:
                return flat[: shape[0]]
            chunk = _product(shape[1:])
            return [
                _nest(flat[i * chunk : (i + 1) * chunk], shape[1:])
                for i in range(shape[0])
            ]

        return _nest(data, self._shape)

    def tolist(self) -> list:
        """tinygrad-compatible alias for to_list()."""
        return self.to_list()

    def item(self):
        """Extract a scalar value from a 0-d or 1-element tensor."""
        if self.size != 1:
            raise ValueError(
                f"item() requires a single-element tensor, got size {self.size}"
            )
        return self._data_list()[0]

    def argmax(self, axis=None, keepdim: bool = False) -> "Tensor":
        data = self._data_list()
        if not data:
            raise ValueError("argmax() requires a non-empty tensor")
        if axis is None:
            best = 0
            for idx in range(1, len(data)):
                if data[idx] > data[best]:
                    best = idx
            return Tensor(float(best))

        if axis < 0:
            axis = self.ndim + axis
        if axis < 0 or axis >= self.ndim:
            raise ValueError(f"Invalid axis {axis} for tensor with {self.ndim} dims")

        axis_len = self._shape[axis]
        outer = _product(self._shape[:axis]) if axis > 0 else 1
        inner = _product(self._shape[axis + 1 :]) if axis + 1 < self.ndim else 1
        result = [0.0] * (outer * inner)

        for o in range(outer):
            for inn in range(inner):
                best_idx = 0
                best_val = data[o * axis_len * inner + inn]
                for a in range(1, axis_len):
                    idx = o * axis_len * inner + a * inner + inn
                    if data[idx] > best_val:
                        best_val = data[idx]
                        best_idx = a
                result[o * inner + inn] = float(best_idx)

        if keepdim:
            out_shape = self._shape[:axis] + (1,) + self._shape[axis + 1 :]
        else:
            out_shape = self._shape[:axis] + self._shape[axis + 1 :]
        if not out_shape:
            return Tensor(result[0])
        return Tensor(result, shape=out_shape)

    def maximum(self, other) -> "Tensor":
        if isinstance(other, (int, float)):
            data = self._data_list()
            val = float(other)
            return self._from_flat([x if x >= val else val for x in data], self._shape)
        if isinstance(other, Tensor):
            if self.shape != other.shape:
                raise ValueError(
                    f"maximum shape mismatch: {self.shape} vs {other.shape}"
                )
            a = self._data_list()
            b = other._data_list()
            return self._from_flat(
                [x if x >= y else y for x, y in zip(a, b)], self._shape
            )
        return NotImplemented

    def __repr__(self) -> str:
        if self.size <= 20:
            return f"Tensor({self.to_list()}, shape={self._shape})"
        data = self._data_list()
        preview = data[:4]
        return f"Tensor([{', '.join(f'{v:.4f}' for v in preview)}, ...], shape={self._shape})"

    def __len__(self) -> int:
        if not self._shape:
            raise TypeError("len() of a 0-d tensor")
        return self._shape[0]

    def __getitem__(self, idx):
        """tinygrad-style indexing subset: ints, slices, tuples, ellipsis, axis-0 tensor gather."""
        if not self._shape:
            raise IndexError("Cannot index a 0-d tensor")
        if isinstance(idx, Tensor):
            return tensor_take_rows(self, idx)
        if isinstance(idx, int):
            if idx < 0:
                idx += self._shape[0]
            if idx < 0 or idx >= self._shape[0]:
                raise IndexError(
                    f"Index {idx} out of range for axis 0 with size {self._shape[0]}"
                )
            sub_shape = self._shape[1:]
            sub_size = _product(sub_shape) if sub_shape else 1
            start = idx * sub_size
            if not sub_shape:
                return Tensor(self._buf[start], dtype=self._dtype)
            return Tensor(
                self._copy_contiguous_buffer(start, sub_size),
                shape=sub_shape,
                dtype=self._dtype,
            )
        if isinstance(idx, slice):
            start, stop, step = idx.indices(self._shape[0])
            indices = list(range(start, stop, step))
            if not indices:
                return Tensor([], shape=(0,) + self._shape[1:], dtype=self._dtype)
            if step == 1:
                sub_shape = (len(indices),) + self._shape[1:]
                sub_size = _product(self._shape[1:]) if len(self._shape) > 1 else 1
                start_elem = start * sub_size
                elem_count = len(indices) * sub_size
                return Tensor(
                    self._copy_contiguous_buffer(start_elem, elem_count),
                    shape=sub_shape,
                    dtype=self._dtype,
                )
            return tensor_take_rows(self, indices, allow_negative=False)
        if isinstance(idx, tuple):
            normalized = _normalize_tensor_index(self._shape, idx)
            data = self._data_list()
            strides = _strides(self._shape)
            axis_indices = []
            out_shape = []
            scalar = True
            for axis, selector in enumerate(normalized):
                size = self._shape[axis]
                if isinstance(selector, int):
                    sel = selector + size if selector < 0 else selector
                    if sel < 0 or sel >= size:
                        raise IndexError(
                            f"Index {selector} out of range for axis {axis} with size {size}"
                        )
                    axis_indices.append([sel])
                elif isinstance(selector, slice):
                    scalar = False
                    vals = list(range(*selector.indices(size)))
                    axis_indices.append(vals)
                    out_shape.append(len(vals))
                else:
                    raise TypeError(
                        f"Tensor indexing with {type(selector)} not supported"
                    )

            if any(len(vals) == 0 for vals in axis_indices):
                return Tensor([], shape=tuple(out_shape), dtype=self._dtype)

            out = []

            def _walk(axis, offset):
                if axis == len(axis_indices):
                    out.append(data[offset])
                    return
                stride = strides[axis]
                for coord in axis_indices[axis]:
                    _walk(axis + 1, offset + coord * stride)

            _walk(0, 0)
            if scalar:
                return Tensor(out[0], dtype=self._dtype)
            return self._from_flat(out, tuple(out_shape))
        raise TypeError(f"Tensor indexing with {type(idx)} not supported")

    def dot(self, other) -> "Tensor":
        """tinygrad-compatible matrix product alias."""
        return self @ other


def tensor_scaled_dot_product_attention(
    q: "Tensor",
    k: "Tensor",
    v: "Tensor",
    mask: "Tensor | None" = None,
    scale: float = 1.0,
) -> "Tensor":
    cache = getattr(k, "_kv_cache", None)
    if (
        cache is not None
        and cache is getattr(v, "_kv_cache", None)
        and getattr(k, "_kv_role", None) == "key"
        and getattr(v, "_kv_role", None) == "value"
    ):
        return cache.attention(q, scale=scale, mask=mask)

    if _requested_gpu_backend() in {"webgpu", "metal"}:
        intrinsic = _resolve_optional_intrinsic(
            "_MOLT_GPU_TENSOR_SCALED_DOT_PRODUCT_ATTENTION",
            "molt_gpu_tensor__tensor_scaled_dot_product_attention",
        )
        if intrinsic is not None:
            return intrinsic(q, k, v, mask, scale)

    scores = (q @ tensor_permute_dims(k, (0, 1, 3, 2))) * scale
    if mask is not None:
        scores = scores + mask
    attn = tensor_softmax_last_axis(scores)
    return attn @ v


def zeros(*shape, dtype=float) -> Tensor:
    """Create a zero-filled tensor."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    intrinsic = _resolve_optional_intrinsic(
        "_MOLT_GPU_TENSOR_ZEROS", "molt_gpu_tensor__zeros"
    )
    if intrinsic is not None:
        return intrinsic(shape, dtype)
    size = _product(shape)
    return Tensor([0.0] * size, shape=shape, dtype=dtype)


def ones(*shape, dtype=float) -> Tensor:
    """Create a tensor filled with ones."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)
    return Tensor([1.0] * size, shape=shape, dtype=dtype)


def randn(*shape, seed=None) -> Tensor:
    """Create a tensor with tinygrad-style Box-Muller normal samples."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)
    if seed is None:
        src = rand((2,) + shape)._data_list()
    else:
        src = _tinygrad_seeded_rand_values(size * 2, seed, 0)
    u0 = src[:size]
    u1 = src[size : size * 2]
    result = []
    for a, b in zip(u0, u1):
        result.append(math.cos(2.0 * math.pi * a) * math.sqrt(-2.0 * math.log(1.0 - b)))
    return Tensor(result, shape=shape)


def rand(*shape, seed=None) -> Tensor:
    """Create a tensor with tinygrad-style uniform random values in [0, 1)."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)
    values = (
        _tinygrad_consume_rand_values(size)
        if seed is None
        else _tinygrad_seeded_rand_values(size, seed)
    )
    return Tensor(values, shape=shape)


def uniform(
    *shape, low=0.0, high=1.0, dtype=None, requires_grad=None, seed=None
) -> Tensor:
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)
    base = (
        _tinygrad_consume_rand_values(size)
        if seed is None
        else _tinygrad_seeded_rand_values(size, seed)
    )
    values = [((high - low) * value) + low for value in base]
    out = Tensor(values, shape=shape, dtype=dtype or float)
    if dtype is not None:
        out = out.cast(dtype)
    return out


def scaled_uniform(*shape, dtype=None, requires_grad=None, seed=None) -> Tensor:
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    scale = _product(shape) ** -0.5
    return (
        uniform(
            *shape,
            low=-1.0,
            high=1.0,
            dtype=dtype,
            requires_grad=requires_grad,
            seed=seed,
        )
        * scale
    )


def glorot_uniform(*shape, dtype=None, requires_grad=None, seed=None) -> Tensor:
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    if not shape:
        raise ValueError("glorot_uniform requires at least one dimension")
    scale = (6.0 / (shape[0] + _product(shape[1:]))) ** 0.5
    return (
        uniform(
            *shape,
            low=-1.0,
            high=1.0,
            dtype=dtype,
            requires_grad=requires_grad,
            seed=seed,
        )
        * scale
    )
