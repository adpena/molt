"""Literal tensor construction helpers for :mod:`molt.gpu.tensor`."""

from __future__ import annotations

from builtins import float as _float
from typing import TYPE_CHECKING

from . import Buffer, alloc
from .tensor_support import _storage_for_dtype

if TYPE_CHECKING:
    from .tensor import Tensor


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


def _write_flat_buffer(flat, dtype, *, default_float_format: str = "d") -> Buffer:
    element_type, format_char = _storage_for_dtype(
        dtype,
        default_float_format=default_float_format,
    )
    buf = alloc(len(flat), element_type, format_char=format_char)
    for idx, value in enumerate(flat):
        buf[idx] = value
    return buf


def _infer_literal_dtype(value):
    from tinygrad.dtypes import dtypes

    if isinstance(value, bool):
        return dtypes.bool
    if isinstance(value, int):
        return dtypes.int32
    if isinstance(value, _float):
        return dtypes.float32
    if isinstance(value, (list, tuple)):
        flat, _ = _flatten_nested(value)
        if not flat:
            return dtypes.float32
        if any(isinstance(item, _float) for item in flat):
            return dtypes.float32
        if all(isinstance(item, bool) for item in flat):
            return dtypes.bool
        if all(isinstance(item, (bool, int)) for item in flat):
            return dtypes.int32
    return dtypes.float32


def _tensor_type():
    from .tensor import Tensor

    return Tensor


def _tensor_operand(value) -> "Tensor":
    if isinstance(value, _tensor_type()):
        return value
    if isinstance(value, (bool, int, _float)):
        return _tensor_type()(value, dtype=_infer_literal_dtype(value))
    if isinstance(value, (list, tuple)):
        return _tensor_type()(value, dtype=_infer_literal_dtype(value))
    raise TypeError(
        f"where operand must be Tensor or scalar/list literal, got {type(value)!r}"
    )
