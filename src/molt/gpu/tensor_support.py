"""Pure dtype, RNG, and shape helpers for :mod:`molt.gpu.tensor`."""

from __future__ import annotations

import operator
from builtins import float as _float
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .tensor import Tensor

_TINYGRAD_FIRST_DEVICE_KEY0 = 347607321  # sha256((0).to_bytes(4,"big")) low 32 bits
_TINYGRAD_RNG_STATE = [0, 0]


def _product(seq):
    """Product of a sequence of integers."""
    result = 1
    for x in seq:
        result *= x
    return result


def _dtype_cast_kind(dtype) -> str | None:
    if dtype is _float:
        return "float"
    if dtype is int:
        return "int"
    if dtype is bool:
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


def _storage_for_dtype(
    dtype,
    *,
    default_float_format: str = "d",
    default_int_format: str = "q",
) -> tuple[type, str]:
    kind = _dtype_cast_kind(dtype)
    fmt = getattr(dtype, "fmt", None)
    if kind == "float":
        if fmt in {"e", "f", "d"}:
            return _float, fmt
        return _float, default_float_format
    if kind == "int":
        if fmt in {"?", "b", "h", "i", "q", "B", "H", "I", "Q"}:
            return int, fmt
        if dtype is bool:
            return int, "?"
        return int, default_int_format
    raise TypeError(f"unsupported dtype {dtype!r}")


def _tinygrad_dtype_for_storage(dtype, format_char: str):
    from tinygrad.dtypes import dtypes

    if format_char == "?":
        return dtypes.bool_
    if format_char == "b":
        return dtypes.int8
    if format_char == "h":
        return dtypes.int16
    if format_char == "i":
        return dtypes.int32
    if format_char == "q":
        return dtypes.int64
    if format_char == "B":
        return dtypes.uint8
    if format_char == "H":
        return dtypes.uint16
    if format_char == "I":
        return dtypes.uint32
    if format_char == "Q":
        return dtypes.uint64
    if format_char == "e":
        return dtypes.float16
    if format_char == "f":
        return dtypes.float32
    if format_char == "d":
        return dtypes.float64
    if dtype is int:
        return dtypes.int64
    return dtypes.float32


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
    return _float(mantissa) / _float(1 << 23)


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

def _broadcast_shape_pair(lhs_shape, rhs_shape) -> tuple[int, ...]:
    out_ndim = max(len(lhs_shape), len(rhs_shape))
    lhs = (1,) * (out_ndim - len(lhs_shape)) + tuple(lhs_shape)
    rhs = (1,) * (out_ndim - len(rhs_shape)) + tuple(rhs_shape)
    out = []
    for lhs_dim, rhs_dim in zip(lhs, rhs):
        if lhs_dim == rhs_dim:
            out.append(lhs_dim)
        elif lhs_dim == 1:
            out.append(rhs_dim)
        elif rhs_dim == 1:
            out.append(lhs_dim)
        else:
            raise ValueError(f"Cannot broadcast shapes {lhs_shape} and {rhs_shape}")
    return tuple(out)


def _broadcast_shape_many(*shapes) -> tuple[int, ...]:
    out = ()
    for shape in shapes:
        out = _broadcast_shape_pair(out, tuple(shape))
    return out


def _broadcast_source_index(out_index: int, out_shape, source_shape) -> int:
    if not source_shape:
        return 0
    padded = (1,) * (len(out_shape) - len(source_shape)) + tuple(source_shape)
    source_strides = _strides(padded)
    out_strides = _strides(tuple(out_shape))
    rem = out_index
    source_index = 0
    for axis, out_stride in enumerate(out_strides):
        coord = rem // out_stride
        rem %= out_stride
        if padded[axis] != 1:
            source_index += coord * source_strides[axis]
    return source_index


def _flat_index_to_coords(flat_index: int, shape) -> tuple[int, ...]:
    if not shape:
        return ()
    coords = []
    rem = flat_index
    for stride in _strides(shape):
        coord = rem // stride
        rem %= stride
        coords.append(coord)
    return tuple(coords)


def _coords_to_flat_index(coords, strides) -> int:
    index = 0
    for coord, stride in zip(coords, strides):
        index += coord * stride
    return index


def _movement_pair(value, op_name: str) -> tuple[int, int]:
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise ValueError(f"{op_name} expects pairs of two integers")
    left = operator.index(value[0])
    right = operator.index(value[1])
    return left, right


def _normalize_movement_pairs(
    spec, ndim: int, op_name: str
) -> tuple[tuple[int, int], ...]:
    if ndim == 0:
        if spec in ((), [], None):
            return ()
        raise ValueError(f"{op_name} on a scalar expects no bounds")
    if isinstance(spec, int):
        value = operator.index(spec)
        return tuple((value, value) for _ in range(ndim))
    if not isinstance(spec, (list, tuple)):
        raise TypeError(f"{op_name} expects an int, flat tuple, or pair tuple")
    if len(spec) == ndim and all(isinstance(item, (list, tuple)) for item in spec):
        return tuple(_movement_pair(item, op_name) for item in spec)
    if len(spec) != ndim * 2:
        raise ValueError(
            f"{op_name} expects {ndim} pairs or {ndim * 2} flat values, got {spec!r}"
        )
    flat_pairs = tuple(
        (operator.index(spec[idx]), operator.index(spec[idx + 1]))
        for idx in range(0, len(spec), 2)
    )
    return tuple(reversed(flat_pairs))


def _preferred_float_format(*tensors: "Tensor") -> str:
    formats = []
    for tensor in tensors:
        if tensor._dtype is _float and tensor._buf.element_type is _float:
            formats.append(tensor._buf.format_char)
    if formats and all(fmt == "f" for fmt in formats):
        return "f"
    return "d"


_INT_FORMAT_RANK = {
    "?": 0,
    "b": 1,
    "B": 1,
    "h": 2,
    "H": 2,
    "i": 3,
    "I": 3,
    "q": 4,
    "Q": 4,
}






def _div_result_dtype_and_format(
    lhs: "Tensor", rhs_tensor: "Tensor | None"
) -> tuple[type, str]:
    """Result dtype/format for true division (``__truediv__``).

    Upstream tinygrad ``Tensor.div`` runs with ``upcast=True`` by default, which
    casts both operands to ``least_upper_float`` of their dtypes before dividing,
    so integer inputs produce a *float* result (true division), e.g.
    ``Tensor([1, 4, 10]).div(Tensor([2, 3, 4]))`` -> ``[0.5, 1.333.., 2.5]``
    (docs.tinygrad.org/tensor/elementwise — ``div`` example). Division therefore
    never preserves an integer storage dtype, unlike add/sub/mul.

    The float precision follows :func:`_preferred_float_format`: float32 only when
    every float-typed operand is already float32, otherwise the float64 default.
    A Python scalar operand is a weak operand (like upstream's weak-typed const)
    and contributes no precision constraint, so the caller passes ``None`` for
    ``rhs_tensor`` and the format follows the tensor operand's float precision.
    """
    if rhs_tensor is None:
        return _float, _preferred_float_format(lhs)
    return _float, _preferred_float_format(lhs, rhs_tensor)


def _where_result_dtype_and_format(lhs: "Tensor", rhs: "Tensor") -> tuple[type, str]:
    if lhs._dtype is _float or rhs._dtype is _float:
        float_tensors = tuple(
            tensor
            for tensor in (lhs, rhs)
            if tensor._dtype is _float and tensor._buf.element_type is _float
        )
        return _float, _preferred_float_format(*float_tensors) if float_tensors else "d"

    lhs_format = lhs._buf.format_char
    rhs_format = rhs._buf.format_char
    lhs_rank = _INT_FORMAT_RANK.get(lhs_format)
    rhs_rank = _INT_FORMAT_RANK.get(rhs_format)
    if lhs_rank is None or rhs_rank is None:
        return lhs._dtype, lhs._buf.format_char
    if lhs_rank == 0 and rhs_rank != 0:
        return int, rhs_format
    if rhs_rank == 0 and lhs_rank != 0:
        return int, lhs_format
    return int, lhs_format if lhs_rank >= rhs_rank else rhs_format
