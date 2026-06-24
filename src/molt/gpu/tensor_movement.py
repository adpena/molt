"""Movement helpers for :mod:`molt.gpu.tensor`."""

from __future__ import annotations

import operator
from typing import TYPE_CHECKING

from . import Buffer, alloc
from .tensor_support import (
    _coords_to_flat_index,
    _flat_index_to_coords,
    _normalize_movement_pairs,
    _product,
    _strides,
)

if TYPE_CHECKING:
    from .tensor import Tensor


def _tensor_module():
    from . import tensor as tensor_module

    return tensor_module


def _tensor_type():
    return _tensor_module().Tensor


def _tensor_from_storage_values(source: "Tensor", values, shape) -> "Tensor":
    out_buf = alloc(
        len(values),
        source._buf.element_type,
        format_char=source._buf.format_char,
    )
    for idx, value in enumerate(values):
        out_buf[idx] = value
    return _tensor_module()._tensor_from_buffer(out_buf, tuple(shape), source._dtype)


def tensor_pad(x: "Tensor", padding, value=0.0) -> "Tensor":
    if not isinstance(x, _tensor_type()):
        return NotImplemented
    padding = _normalize_movement_pairs(padding, x.ndim, "pad")
    for before, after in padding:
        if before < 0 or after < 0:
            raise ValueError(f"pad expects non-negative padding, got {padding}")
    new_shape = tuple(
        size + before + after for size, (before, after) in zip(x._shape, padding)
    )
    out_size = _product(new_shape) if new_shape else 1
    result = [value] * out_size
    if x.size:
        old_data = x._data_list()
        new_strides = _strides(new_shape)
        for old_index, old_value in enumerate(old_data):
            old_coords = _flat_index_to_coords(old_index, x._shape)
            new_coords = tuple(
                coord + before for coord, (before, _after) in zip(old_coords, padding)
            )
            result[_coords_to_flat_index(new_coords, new_strides)] = old_value
    return _tensor_from_storage_values(x, result, new_shape)


def tensor_shrink(x: "Tensor", bounds) -> "Tensor":
    if not isinstance(x, _tensor_type()):
        return NotImplemented
    bounds = _normalize_movement_pairs(bounds, x.ndim, "shrink")
    for dim, (start, end) in enumerate(bounds):
        if start < 0 or end < start or end > x._shape[dim]:
            raise ValueError(f"invalid shrink bounds {bounds} for shape {x._shape}")
    new_shape = tuple(end - start for start, end in bounds)
    out_size = _product(new_shape) if new_shape else 1
    old_data = x._data_list()
    old_strides = _strides(x._shape)
    result = []
    for out_index in range(out_size):
        out_coords = _flat_index_to_coords(out_index, new_shape)
        src_coords = tuple(
            coord + start for coord, (start, _end) in zip(out_coords, bounds)
        )
        result.append(old_data[_coords_to_flat_index(src_coords, old_strides)])
    return _tensor_from_storage_values(x, result, new_shape)


def _normalize_flip_axes(axis, ndim: int) -> tuple[int, ...]:
    if isinstance(axis, (list, tuple)):
        raw_axes = tuple(axis)
    else:
        raw_axes = (axis,)
    axes = []
    for raw_axis in raw_axes:
        normalized = operator.index(raw_axis)
        if normalized < 0:
            normalized += ndim
        if normalized < 0 or normalized >= ndim:
            raise ValueError(f"flip axis {raw_axis} out of bounds for ndim={ndim}")
        if normalized not in axes:
            axes.append(normalized)
    return tuple(axes)


def tensor_flip(x: "Tensor", axis=0) -> "Tensor":
    if not isinstance(x, _tensor_type()):
        return NotImplemented
    axes = _normalize_flip_axes(axis, x.ndim)
    axis_set = set(axes)
    old_data = x._data_list()
    old_strides = _strides(x._shape)
    result = []
    for out_index in range(x.size):
        coords = _flat_index_to_coords(out_index, x._shape)
        src_coords = tuple(
            x._shape[dim] - 1 - coord if dim in axis_set else coord
            for dim, coord in enumerate(coords)
        )
        result.append(old_data[_coords_to_flat_index(src_coords, old_strides)])
    return _tensor_from_storage_values(x, result, x._shape)


def tensor_contiguous(x: "Tensor") -> "Tensor":
    if not isinstance(x, _tensor_type()):
        return NotImplemented
    size_bytes = x.size * x._buf.itemsize
    out_buf = Buffer(
        bytearray(x._buf._data[:size_bytes]),
        x._buf.element_type,
        x.size,
        format_char=x._buf.format_char,
    )
    return _tensor_module()._tensor_from_buffer(out_buf, x._shape, x._dtype)
