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
import struct
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


_MOLT_GPU_LINEAR_CONTIGUOUS = _load_optional_intrinsic("molt_gpu_linear_contiguous")
_MOLT_GPU_LINEAR_SPLIT_LAST_DIM_CONTIGUOUS = _load_optional_intrinsic(
    "molt_gpu_linear_split_last_dim_contiguous"
)
_MOLT_GPU_BROADCAST_BINARY_CONTIGUOUS = _load_optional_intrinsic(
    "molt_gpu_broadcast_binary_contiguous"
)
_MOLT_GPU_MATMUL_CONTIGUOUS = _load_optional_intrinsic("molt_gpu_matmul_contiguous")
_MOLT_GPU_PERMUTE_CONTIGUOUS = _load_optional_intrinsic("molt_gpu_permute_contiguous")
_MOLT_GPU_RMS_NORM_LAST_AXIS_CONTIGUOUS = _load_optional_intrinsic(
    "molt_gpu_rms_norm_last_axis_contiguous"
)
_MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS = _load_optional_intrinsic(
    "molt_gpu_softmax_last_axis_contiguous"
)
_MOLT_GPU_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS = _load_optional_intrinsic(
    "molt_gpu_squared_relu_gate_interleaved_contiguous"
)

_OP_ADD = 0
_OP_SUB = 1
_OP_MUL = 2
_OP_DIV = 3


def _product(seq):
    """Product of a sequence of integers."""
    result = 1
    for x in seq:
        result *= x
    return result


def _strides(shape):
    strides = []
    stride = 1
    for size in reversed(shape):
        strides.append(stride)
        stride *= size
    strides.reverse()
    return tuple(strides)


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
        return from_device(self._buf)[:self.size]

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

    def reshape(self, *shape) -> 'Tensor':
        """Return a tensor with the same data but a new shape.

        One dimension can be -1, which is inferred from the others.
        """
        if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
            shape = tuple(shape[0])

        # Handle -1 dimension
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
            inferred = self.size // known
            shape = shape[:neg_idx] + (inferred,) + shape[neg_idx + 1:]

        if _product(shape) != self.size:
            raise ValueError(
                f"Cannot reshape tensor of size {self.size} into shape {shape}"
            )
        return Tensor(self._buf, shape=shape, dtype=self._dtype)

    def transpose(self) -> 'Tensor':
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

    def permute(self, *dims) -> 'Tensor':
        """Reorder tensor dimensions."""
        if len(dims) == 1 and isinstance(dims[0], (list, tuple)):
            dims = tuple(dims[0])
        else:
            dims = tuple(dims)

        if len(dims) != self.ndim:
            raise ValueError(
                f"permute expected {self.ndim} dims for shape {self._shape}, got {dims}"
            )

        normalized = []
        for dim in dims:
            if dim < 0:
                dim += self.ndim
            if dim < 0 or dim >= self.ndim:
                raise ValueError(f"permute dim {dim} out of range for ndim={self.ndim}")
            normalized.append(dim)
        if sorted(normalized) != list(range(self.ndim)):
            raise ValueError(f"permute dims must be a permutation of 0..{self.ndim - 1}")

        if self.ndim <= 1:
            return Tensor(self._buf, shape=self._shape, dtype=self._dtype)

        old_shape = self._shape
        new_shape = tuple(old_shape[dim] for dim in normalized)
        if _MOLT_GPU_PERMUTE_CONTIGUOUS is not None:
            out_bits = _MOLT_GPU_PERMUTE_CONTIGUOUS(
                self._buf._data,
                self._buf.format_char,
                old_shape,
                normalized,
                self._buf.format_char,
            )
            out_buf = Buffer(
                out_bits,
                self._buf.element_type,
                self.size,
                format_char=self._buf.format_char,
            )
            return Tensor(out_buf, shape=new_shape, dtype=self._dtype)
        data = self._data_list()
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

        return self._from_flat(result, new_shape)

    @property
    def T(self) -> 'Tensor':
        """Transpose (property alias)."""
        return self.transpose()

    def flatten(self) -> 'Tensor':
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

    def _broadcast_op(
        self,
        other,
        op_code: int,
        _fast=_MOLT_GPU_BROADCAST_BINARY_CONTIGUOUS,
    ):
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

        if _fast is not None:
            out_bits = _fast(
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

    def __add__(self, other) -> 'Tensor':
        return self._broadcast_op(other, _OP_ADD)

    def __radd__(self, other) -> 'Tensor':
        return self._broadcast_op(other, _OP_ADD)

    def __sub__(self, other) -> 'Tensor':
        return self._broadcast_op(other, _OP_SUB)

    def __rsub__(self, other) -> 'Tensor':
        if isinstance(other, (int, float)):
            data = self._data_list()
            return self._from_flat(
                [float(other) - x for x in data], self._shape
            )
        return NotImplemented

    def __mul__(self, other) -> 'Tensor':
        return self._broadcast_op(other, _OP_MUL)

    def __rmul__(self, other) -> 'Tensor':
        return self._broadcast_op(other, _OP_MUL)

    def __truediv__(self, other) -> 'Tensor':
        return self._broadcast_op(other, _OP_DIV)

    def __rtruediv__(self, other) -> 'Tensor':
        if isinstance(other, (int, float)):
            data = self._data_list()
            def _safe_rdiv(x):
                try:
                    return float(other) / x
                except ZeroDivisionError:
                    if other > 0:
                        return float('inf')
                    elif other < 0:
                        return float('-inf')
                    else:
                        return float('nan')
            return self._from_flat([_safe_rdiv(x) for x in data], self._shape)
        return NotImplemented

    def __neg__(self) -> 'Tensor':
        data = self._data_list()
        return self._from_flat([-x for x in data], self._shape)

    # ── Matrix multiplication ─────────────────────────────────────────

    def __matmul__(self, other) -> 'Tensor':
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
                raise ValueError(
                    f"Dot product size mismatch: {len(a)} vs {len(b)}"
                )
            return Tensor(sum(a[i] * b[i] for i in range(len(a))))

        # Ensure 2D
        a = self if self.ndim >= 2 else self.reshape(1, self.size)
        b = other if other.ndim >= 2 else other.reshape(other.size, 1)

        a_rows, a_cols = a._shape[-2], a._shape[-1]
        b_rows, b_cols = b._shape[-2], b._shape[-1]

        if a_cols != b_rows:
            raise ValueError(
                f"Matmul shape mismatch: {a._shape} @ {b._shape}"
            )

        a_batch_shape = a._shape[:-2]
        b_batch_shape = b._shape[:-2]
        out_batch_ndim = max(len(a_batch_shape), len(b_batch_shape))
        padded_a_batch_shape = (1,) * (out_batch_ndim - len(a_batch_shape)) + a_batch_shape
        padded_b_batch_shape = (1,) * (out_batch_ndim - len(b_batch_shape)) + b_batch_shape
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

        if _MOLT_GPU_MATMUL_CONTIGUOUS is not None:
            out_bits = _MOLT_GPU_MATMUL_CONTIGUOUS(
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

        if _runtime_intrinsics_active():
            raise RuntimeError("intrinsic unavailable: molt_gpu_matmul_contiguous")

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
                        s += a_data[a_off + i * a_cols + k] * b_data[b_off + k * b_cols + j]
                    result.append(s)

        out_buf = alloc(len(result), a._dtype, format_char=result_format)
        for idx, value in enumerate(result):
            out_buf[idx] = value
        return Tensor(out_buf, shape=out_shape, dtype=a._dtype)

    def linear(self, weight) -> 'Tensor':
        """Apply a linear projection with weight shaped (out_features, in_features).

        This computes ``self @ weight.T`` without materializing a transposed copy
        of ``weight``. Leading dimensions on ``self`` are treated as batch dims.
        """
        if not isinstance(weight, Tensor):
            return NotImplemented

        if weight.ndim != 2:
            raise ValueError(f"linear weight must be 2D, got {weight.shape}")

        if self.ndim == 0:
            raise ValueError("linear input must be at least 1D")

        in_features = self._shape[-1]
        out_features, weight_in = weight._shape
        if in_features != weight_in:
            raise ValueError(
                f"Linear shape mismatch: {self._shape} with weight {weight._shape}"
            )

        outer = _product(self._shape[:-1]) if self.ndim > 1 else 1
        result_format = None
        if self._dtype is float and weight._dtype is float:
            result_format = _preferred_float_format(self, weight)
        else:
            result_format = self._buf.format_char

        if _MOLT_GPU_LINEAR_CONTIGUOUS is not None:
            out_bits = _MOLT_GPU_LINEAR_CONTIGUOUS(
                self._buf._data,
                self._buf.format_char,
                weight._buf._data,
                weight._buf.format_char,
                outer,
                in_features,
                out_features,
                result_format,
            )
            out_buf = Buffer(
                out_bits,
                self._dtype,
                outer * out_features,
                format_char=result_format,
            )
            out_shape = self._shape[:-1] + (out_features,)
            if not out_shape:
                out_shape = (out_features,)
            return Tensor(out_buf, shape=out_shape, dtype=self._dtype)

        if _runtime_intrinsics_active():
            raise RuntimeError("intrinsic unavailable: molt_gpu_linear_contiguous")

        x_data = self._data_list()
        out_buf = alloc(
            outer * out_features,
            self._dtype,
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

        out_shape = self._shape[:-1] + (out_features,)
        if not out_shape:
            out_shape = (out_features,)
        return Tensor(out_buf, shape=out_shape, dtype=self._dtype)

    def linear_split_last_dim(self, weight, sizes) -> tuple['Tensor', ...]:
        """Apply a linear projection, then split the output last dimension."""
        if not isinstance(weight, Tensor):
            return NotImplemented

        if weight.ndim != 2:
            raise ValueError(f"linear weight must be 2D, got {weight.shape}")
        if self.ndim == 0:
            raise ValueError("linear input must be at least 1D")

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

        in_features = self._shape[-1]
        out_features, weight_in = weight._shape
        if in_features != weight_in:
            raise ValueError(
                f"Linear shape mismatch: {self._shape} with weight {weight._shape}"
            )
        if sum(sizes) != out_features:
            raise ValueError(
                f"split sizes {sizes} do not match projected dimension {out_features}"
            )

        outer = _product(self._shape[:-1]) if self.ndim > 1 else 1
        if self._dtype is float and weight._dtype is float:
            result_dtype = self._dtype
            result_format = _preferred_float_format(self, weight)
        else:
            result_dtype = self._dtype
            result_format = self._buf.format_char
        prefix_shape = self._shape[:-1]

        if _MOLT_GPU_LINEAR_SPLIT_LAST_DIM_CONTIGUOUS is not None:
            out_parts = _MOLT_GPU_LINEAR_SPLIT_LAST_DIM_CONTIGUOUS(
                self._buf._data,
                self._buf.format_char,
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
                Tensor(
                    Buffer(part_bits, result_dtype, outer * size, format_char=result_format),
                    shape=prefix_shape + (size,),
                    dtype=result_dtype,
                )
                for size, part_bits in zip(sizes, out_parts)
            )

        if _runtime_intrinsics_active():
            raise RuntimeError("intrinsic unavailable: molt_gpu_linear_split_last_dim_contiguous")

        return self.linear(weight).split_last_dim(sizes)

    def split_last_dim(self, sizes) -> tuple['Tensor', ...]:
        """Split the last dimension into contiguous views copied into new buffers."""
        if self.ndim == 0:
            raise ValueError("split_last_dim requires a tensor with at least 1 dimension")
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
                out_buf._data[dst_base:dst_base + span] = src[
                    row_base + offset:row_base + offset + span
                ]
                offset += span

        return tuple(
            Tensor(out_buf, shape=result_shape_prefix + (size,), dtype=self._dtype)
            for size, out_buf in outputs
        )

    def repeat_axis(self, axis: int, repeats: int) -> 'Tensor':
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
            out_shape = self._shape[:axis] + (0,) + self._shape[axis + 1:]
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

        outer = _product(self._shape[:axis]) if axis > 0 else 1
        axis_len = self._shape[axis]
        inner = _product(self._shape[axis + 1:]) if axis + 1 < self.ndim else 1
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
                chunk = src[src_base:src_base + chunk_bytes]
                dst_base = dst_outer + axis_idx * repeats * chunk_bytes
                for rep in range(repeats):
                    rep_base = dst_base + rep * chunk_bytes
                    out[rep_base:rep_base + chunk_bytes] = chunk

        out_shape = self._shape[:axis] + (out_axis_len,) + self._shape[axis + 1:]
        out_buf = Buffer(
            out,
            self._buf.element_type,
            outer * out_axis_len * inner,
            format_char=self._buf.format_char,
        )
        return Tensor(out_buf, shape=out_shape, dtype=self._dtype)

    def take_rows(self, indices, *, allow_negative: bool = True) -> 'Tensor':
        """Gather slices along axis 0 without materializing the full tensor."""
        if self.ndim == 0:
            raise ValueError("take_rows requires a tensor with at least 1 dimension")

        if not isinstance(indices, Tensor):
            indices = Tensor(indices)

        rows = indices._data_list()
        row_shape = self._shape[1:]
        row_size = _product(row_shape) if row_shape else 1
        width = row_size * self._buf.itemsize
        out = bytearray(len(rows) * width)

        for out_row, raw_idx in enumerate(rows):
            idx = int(raw_idx)
            if idx != raw_idx:
                raise TypeError(f"take_rows indices must be integers, got {raw_idx!r}")
            if idx < 0 and allow_negative:
                idx += self._shape[0]
            if idx < 0 or idx >= self._shape[0]:
                raise IndexError(
                    f"Index {raw_idx} out of range for axis 0 with size {self._shape[0]}"
                )
            src_start = idx * width
            dst_start = out_row * width
            out[dst_start:dst_start + width] = self._buf._data[src_start:src_start + width]

        out_buf = Buffer(
            out,
            self._buf.element_type,
            len(rows) * row_size,
            format_char=self._buf.format_char,
        )
        return Tensor(out_buf, shape=indices.shape + row_shape, dtype=self._dtype)

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
            out_shape = self._shape[:axis] + (1,) + self._shape[axis + 1:]
        else:
            out_shape = self._shape[:axis] + self._shape[axis + 1:]
        if not out_shape:
            out_shape = ()

        out_size = _product(out_shape) if out_shape else 1
        result = [None] * out_size

        # Stride-based reduction
        outer = _product(self._shape[:axis]) if axis > 0 else 1
        axis_len = self._shape[axis]
        inner = _product(self._shape[axis + 1:]) if axis + 1 < self.ndim else 1

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

    def sum(self, axis=None, keepdim: bool = False) -> 'Tensor':
        """Sum elements, optionally along an axis."""
        return self._reduce(lambda a, b: a + b, axis=axis, initial=0.0, keepdim=keepdim)

    def mean(self, axis=None, keepdim: bool = False) -> 'Tensor':
        """Mean of elements, optionally along an axis."""
        s = self.sum(axis=axis, keepdim=keepdim)
        if axis is None:
            n = self.size
        else:
            if axis < 0:
                axis = self.ndim + axis
            n = self._shape[axis]
        return s / float(n)

    def max(self, axis=None, keepdim: bool = False) -> 'Tensor':
        """Max element, optionally along an axis."""
        return self._reduce(lambda a, b: a if a >= b else b, axis=axis, keepdim=keepdim)

    def min(self, axis=None, keepdim: bool = False) -> 'Tensor':
        """Min element, optionally along an axis."""
        return self._reduce(lambda a, b: a if a <= b else b, axis=axis, keepdim=keepdim)

    # ── Activation functions ──────────────────────────────────────────

    def relu(self) -> 'Tensor':
        """Rectified linear unit: max(0, x)."""
        data = self._data_list()
        return self._from_flat([x if x > 0 else 0.0 for x in data], self._shape)

    def sigmoid(self) -> 'Tensor':
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

    def tanh(self) -> 'Tensor':
        """Hyperbolic tangent."""
        data = self._data_list()
        return self._from_flat([math.tanh(x) for x in data], self._shape)

    def softmax(self, axis=-1) -> 'Tensor':
        """Softmax along an axis (default: last axis).

        softmax(x)_i = exp(x_i - max(x)) / sum(exp(x_j - max(x)))
        """
        if axis < 0:
            axis = self.ndim + axis
        if self.ndim == 0:
            return Tensor(1.0)

        if axis == self.ndim - 1 and _MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS is not None:
            out_bits = _MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS(
                self._buf._data,
                self._buf.format_char,
                self._shape,
                self._buf.format_char,
            )
            out_buf = Buffer(
                out_bits,
                self._dtype,
                self.size,
                format_char=self._buf.format_char,
            )
            return Tensor(out_buf, shape=self._shape, dtype=self._dtype)

        if axis == self.ndim - 1 and _runtime_intrinsics_active():
            raise RuntimeError("intrinsic unavailable: molt_gpu_softmax_last_axis_contiguous")

        data = self._data_list()

        # Compute outer/inner strides
        outer = _product(self._shape[:axis]) if axis > 0 else 1
        axis_len = self._shape[axis]
        inner = _product(self._shape[axis + 1:]) if axis + 1 < self.ndim else 1

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

    def rms_norm(self, eps: float) -> 'Tensor':
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

        if _MOLT_GPU_RMS_NORM_LAST_AXIS_CONTIGUOUS is not None:
            out_bits = _MOLT_GPU_RMS_NORM_LAST_AXIS_CONTIGUOUS(
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

        if _runtime_intrinsics_active():
            raise RuntimeError("intrinsic unavailable: molt_gpu_rms_norm_last_axis_contiguous")

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

    def squared_relu_gate_interleaved(self) -> 'Tensor':
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

        if _MOLT_GPU_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS is not None:
            out_bits = _MOLT_GPU_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS(
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

        if _runtime_intrinsics_active():
            raise RuntimeError(
                "intrinsic unavailable: molt_gpu_squared_relu_gate_interleaved_contiguous"
            )

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

    def exp(self) -> 'Tensor':
        """Element-wise exponential.

        Inputs are clamped to [-709, 709] to prevent float64 overflow.
        """
        data = self._data_list()
        return self._from_flat(
            [math.exp(max(-709.0, min(709.0, x))) for x in data],
            self._shape,
        )

    def log(self) -> 'Tensor':
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

    def sqrt(self) -> 'Tensor':
        """Element-wise square root.

        Negative values produce NaN (matches IEEE 754 / NumPy behavior).
        """
        data = self._data_list()
        return self._from_flat(
            [math.sqrt(x) if x >= 0 else float('nan') for x in data],
            self._shape,
        )

    def abs(self) -> 'Tensor':
        """Element-wise absolute value."""
        data = self._data_list()
        return self._from_flat([abs(x) for x in data], self._shape)

    def clamp(self, min_val=None, max_val=None) -> 'Tensor':
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
                return flat[:shape[0]]
            chunk = _product(shape[1:])
            return [_nest(flat[i * chunk:(i + 1) * chunk], shape[1:])
                    for i in range(shape[0])]

        return _nest(data, self._shape)

    def item(self):
        """Extract a scalar value from a 0-d or 1-element tensor."""
        if self.size != 1:
            raise ValueError(
                f"item() requires a single-element tensor, got size {self.size}"
            )
        return self._data_list()[0]

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
        """Basic indexing: t[i] returns a sub-tensor along the first axis."""
        if not self._shape:
            raise IndexError("Cannot index a 0-d tensor")
        if isinstance(idx, int):
            if idx < 0:
                idx += self._shape[0]
            if idx < 0 or idx >= self._shape[0]:
                raise IndexError(f"Index {idx} out of range for axis 0 with size {self._shape[0]}")
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
        raise TypeError(f"Tensor indexing with {type(idx)} not supported")


# ── Module-level constructors ─────────────────────────────────────────

def zeros(*shape, dtype=float) -> Tensor:
    """Create a zero-filled tensor."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)
    return Tensor([0.0] * size, shape=shape, dtype=dtype)


def ones(*shape, dtype=float) -> Tensor:
    """Create a tensor filled with ones."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)
    return Tensor([1.0] * size, shape=shape, dtype=dtype)


def randn(*shape, seed=None) -> Tensor:
    """Create a tensor with random normal values (Box-Muller transform).

    Uses a simple LCG PRNG for reproducibility without importing random.
    """
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)

    # Simple LCG PRNG (no external deps)
    if seed is None:
        import time as _time

        seed = int(_time.time() * 1000) % (2**31)
    state = seed
    TWO_PI = 2.0 * math.pi

    result = []
    for i in range(0, size, 2):
        # LCG step
        state = (state * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        u1 = (state >> 11) / (1 << 53)
        if u1 == 0.0:
            u1 = 1e-10
        state = (state * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        u2 = (state >> 11) / (1 << 53)

        z0 = math.sqrt(-2.0 * math.log(u1)) * math.cos(TWO_PI * u2)
        z1 = math.sqrt(-2.0 * math.log(u1)) * math.sin(TWO_PI * u2)
        result.append(z0)
        if i + 1 < size:
            result.append(z1)

    return Tensor(result[:size], shape=shape)


def rand(*shape, seed=None) -> Tensor:
    """Create a tensor with uniform random values in [0, 1)."""
    if len(shape) == 1 and isinstance(shape[0], (list, tuple)):
        shape = tuple(shape[0])
    size = _product(shape)

    if seed is None:
        import time as _time

        seed = int(_time.time() * 1000) % (2**31)
    state = seed
    result = []
    for _ in range(size):
        state = (state * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        result.append((state >> 11) / (1 << 53))

    return Tensor(result, shape=shape)
