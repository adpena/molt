"""
tinygrad.tensor — Tinygrad-compatible Tensor class.

All operations record LazyOp DAGs. Computation is deferred until
realize() or numpy()/tolist() is called.

Compositions follow tinygrad conventions:
  exp(x) = EXP2(x * LOG2_E)
  log(x) = LOG2(x) * LN_2
  sigmoid(x) = 1 / (1 + exp(-x))
  relu(x) = MAX(x, 0)
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")


import math
import random as _random
import struct as _struct
from tinygrad.dtypes import DType, dtypes
from tinygrad.lazy import LazyBuffer, LazyOp
import tinygrad.realize

# GPU primitive intrinsics — these bridge to the Rust molt-gpu crate
# when compiled via `molt build --target wasm`. At runtime in CPython,
# these are None (Python-only fallback path is used instead).
_gpu_create = _require_intrinsic("molt_gpu_prim_create_tensor")
_gpu_create_raw = _require_intrinsic("molt_gpu_prim_create_tensor_raw")
_gpu_zeros = _require_intrinsic("molt_gpu_prim_zeros")
_gpu_zeros_dtype = _require_intrinsic("molt_gpu_prim_zeros_dtype")
_gpu_realize = _require_intrinsic("molt_gpu_prim_realize")
_gpu_dtype = _require_intrinsic("molt_gpu_prim_dtype")
_gpu_nbytes = _require_intrinsic("molt_gpu_prim_nbytes")
_gpu_read_raw = _require_intrinsic("molt_gpu_prim_read_data_raw")
_gpu_free = _require_intrinsic("molt_gpu_prim_free")
_gpu_unary = _require_intrinsic("molt_gpu_prim_unary")
_gpu_cast = _require_intrinsic("molt_gpu_prim_cast")
_gpu_binary = _require_intrinsic("molt_gpu_prim_binary")
_gpu_ternary = _require_intrinsic("molt_gpu_prim_ternary")
_gpu_reduce = _require_intrinsic("molt_gpu_prim_reduce")
_gpu_reduce_all = _require_intrinsic("molt_gpu_prim_reduce_all")
_gpu_reshape = _require_intrinsic("molt_gpu_prim_reshape")
_gpu_expand = _require_intrinsic("molt_gpu_prim_expand")
_gpu_permute = _require_intrinsic("molt_gpu_prim_permute")
_gpu_pad = _require_intrinsic("molt_gpu_prim_pad")
_gpu_shrink = _require_intrinsic("molt_gpu_prim_shrink")
_gpu_flip = _require_intrinsic("molt_gpu_prim_flip")
_gpu_contiguous = _require_intrinsic("molt_gpu_prim_contiguous")
_gpu_shape = _require_intrinsic("molt_gpu_prim_shape")
_gpu_numel = _require_intrinsic("molt_gpu_prim_numel")

_LOG2_E = math.log2(math.e)
_LN_2 = math.log(2)
_INVALID_HANDLE = (1 << 64) - 1
_DTYPE_CODES = (
    dtypes.bool_,
    dtypes.int8,
    dtypes.int16,
    dtypes.int32,
    dtypes.int64,
    dtypes.uint8,
    dtypes.uint16,
    dtypes.uint32,
    dtypes.uint64,
    dtypes.float16,
    dtypes.bfloat16,
    dtypes.float32,
    dtypes.float64,
)
_CODE_TO_DTYPE = {dtype.code: dtype for dtype in _DTYPE_CODES}
_INTEGER_DTYPES = (
    dtypes.int8,
    dtypes.int16,
    dtypes.int32,
    dtypes.int64,
    dtypes.uint8,
    dtypes.uint16,
    dtypes.uint32,
    dtypes.uint64,
)
_FLOAT_DTYPES = (dtypes.float16, dtypes.float32, dtypes.float64)
_RAW_STRUCT_DTYPES = _INTEGER_DTYPES + (
    dtypes.bool_,
    dtypes.float16,
    dtypes.float32,
    dtypes.float64,
)
_UNARY_OP_CODES = {
    "NEG": 5,
    "EXP2": 14,
    "LOG2": 15,
    "SIN": 16,
    "SQRT": 17,
    "RECIPROCAL": 18,
    "TRUNC": 19,
}
_BINARY_OP_CODES = {
    "ADD": 0,
    "SUB": 1,
    "MUL": 2,
    "IDIV": 3,
    "MOD": 4,
    "CMPLT": 6,
    "CMPEQ": 7,
    "CMPNE": 8,
    "AND": 9,
    "OR": 10,
    "XOR": 11,
    "SHL": 12,
    "SHR": 13,
    "MAX": 20,
}
_TERNARY_OP_CODES = {"WHERE": 21}
_CAST_OP_CODES = {"CAST": 22, "BITCAST": 23}
_REDUCE_OP_CODES = {"REDUCE_SUM": 24, "REDUCE_MAX": 25}
_DTYPE_PROMOTION_PRIORITY = {
    dtypes.bool_: 0,
    dtypes.int8: 1,
    dtypes.uint8: 2,
    dtypes.int16: 3,
    dtypes.uint16: 4,
    dtypes.int32: 5,
    dtypes.uint32: 6,
    dtypes.int64: 7,
    dtypes.uint64: 8,
    dtypes.float16: 11,
    dtypes.bfloat16: 12,
    dtypes.float32: 13,
    dtypes.float64: 14,
}
_DTYPE_PROMOTION_EDGES = {
    dtypes.bool_: (dtypes.int8, dtypes.uint8),
    dtypes.int8: (dtypes.int16,),
    dtypes.uint8: (dtypes.int16, dtypes.uint16),
    dtypes.int16: (dtypes.int32,),
    dtypes.uint16: (dtypes.int32, dtypes.uint32),
    dtypes.int32: (dtypes.int64,),
    dtypes.uint32: (dtypes.int64, dtypes.uint64),
    dtypes.int64: (dtypes.float16, dtypes.bfloat16),
    dtypes.uint64: (dtypes.float16, dtypes.bfloat16),
    dtypes.float16: (dtypes.float32,),
    dtypes.bfloat16: (dtypes.float32,),
    dtypes.float32: (dtypes.float64,),
    dtypes.float64: (),
}


class Tensor:
    """Tinygrad-compatible tensor with lazy evaluation."""

    def __init__(self, data=None, dtype: DType = None) -> None:
        if isinstance(data, LazyBuffer):
            self.lazydata = data
            return

        if data is None:
            self.lazydata = LazyBuffer(None, _resolve_dtype(dtype), ())
            return

        flat, shape, inferred_dtype = _flatten_data(data)
        resolved_dtype = _resolve_dtype(dtype or inferred_dtype or dtypes.float32)
        flat = _coerce_values_for_dtype(flat, resolved_dtype)
        self.lazydata = _lazy_load_from_values(flat, resolved_dtype, shape)

    @property
    def shape(self) -> tuple:
        return self.lazydata.shape

    @property
    def dtype(self) -> DType:
        return self.lazydata.dtype

    @property
    def ndim(self) -> int:
        return len(self.shape)

    def numel(self) -> int:
        return self.lazydata.numel

    # --- Realization ---

    def realize(self) -> "Tensor":
        """Force materialization of the lazy computation graph."""
        _realize_host_values(self.lazydata)
        return self

    def numpy(self) -> list:
        """Realize and return data as a nested Python list (numpy-like)."""
        flat = _realize_host_values(self.lazydata)
        return _unflatten_data(flat, self.shape)

    def tolist(self) -> list:
        """Realize and return data as a nested Python list."""
        return self.numpy()

    def item(self) -> float:
        """Return scalar value. Tensor must have exactly one element."""
        flat = _realize_host_values(self.lazydata)
        if len(flat) != 1:
            raise ValueError(f"item() requires tensor with 1 element, got {len(flat)}")
        return flat[0]

    # --- Creation Methods ---

    @staticmethod
    def zeros(*shape, dtype: DType = None) -> "Tensor":
        resolved_shape = _resolve_shape(shape)
        dt = _resolve_dtype(dtype)
        numel = 1
        for s in resolved_shape:
            numel *= s
        op = LazyOp("CONST", (), arg=0.0, dtype=dt, shape=resolved_shape)
        handle = _try_create_runtime_zeros(dt, resolved_shape)
        return tensor_from_lazy(LazyBuffer(op, dt, resolved_shape, handle=handle))

    @staticmethod
    def ones(*shape, dtype: DType = None) -> "Tensor":
        resolved_shape = _resolve_shape(shape)
        dt = _resolve_dtype(dtype)
        op = LazyOp("CONST", (), arg=1.0, dtype=dt, shape=resolved_shape)
        return tensor_from_lazy(LazyBuffer(op, dt, resolved_shape))

    @staticmethod
    def rand(*shape, dtype: DType = None) -> "Tensor":
        resolved_shape = _resolve_shape(shape)
        dt = _resolve_dtype(dtype)
        numel = 1
        for s in resolved_shape:
            numel *= s
        data = [_random.random() for _ in range(numel)]
        return tensor_from_lazy(
            _lazy_load_from_values(_coerce_values_for_dtype(data, dt), dt, resolved_shape)
        )

    @staticmethod
    def eye(n: int, dtype: DType = None) -> "Tensor":
        dt = _resolve_dtype(dtype)
        data = [1.0 if i == j else 0.0 for i in range(n) for j in range(n)]
        return tensor_from_lazy(
            _lazy_load_from_values(_coerce_values_for_dtype(data, dt), dt, (n, n))
        )

    @staticmethod
    def empty(*shape, dtype: DType = None) -> "Tensor":
        resolved_shape = _resolve_shape(shape)
        dt = _resolve_dtype(dtype)
        op = LazyOp("CONST", (), arg=0.0, dtype=dt, shape=resolved_shape)
        handle = _try_create_runtime_zeros(dt, resolved_shape)
        return tensor_from_lazy(LazyBuffer(op, dt, resolved_shape, handle=handle))

    @staticmethod
    def full(*shape, fill_value: float, dtype: DType = None) -> "Tensor":
        # If shape is passed as (shape_tuple, fill_value=...) handle it
        resolved_shape = _resolve_shape(shape)
        dt = _resolve_dtype(dtype)
        return Tensor._const(fill_value, resolved_shape, dt)

    # --- Unary Ops (compositions of 26 primitives) ---

    def exp(self) -> "Tensor":
        """exp(x) = EXP2(x * LOG2_E)"""
        return self._unary_compose("EXP2", pre_mul=_LOG2_E)

    def log(self) -> "Tensor":
        """log(x) = LOG2(x) * LN_2"""
        return self._unary_compose("LOG2", post_mul=_LN_2)

    def sqrt(self) -> "Tensor":
        return self._unary("SQRT")

    def sin(self) -> "Tensor":
        return self._unary("SIN")

    def cos(self) -> "Tensor":
        """cos(x) = sin(x + pi/2)"""
        offset = Tensor._const(math.pi / 2, self.shape, self.dtype)
        return (self + offset).sin()

    def neg(self) -> "Tensor":
        return self._unary("NEG")

    def reciprocal(self) -> "Tensor":
        return self._unary("RECIPROCAL")

    def relu(self) -> "Tensor":
        """relu(x) = max(x, 0)"""
        zero = Tensor._const(0.0, self.shape, self.dtype)
        return self.maximum(zero)

    def sigmoid(self) -> "Tensor":
        """sigmoid(x) = RECIPROCAL(1 + EXP2(-x * LOG2_E))"""
        neg_x = self.neg()
        exp_neg = neg_x._unary_compose("EXP2", pre_mul=_LOG2_E)
        one = Tensor._const(1.0, self.shape, self.dtype)
        return (one + exp_neg).reciprocal()

    def tanh(self) -> "Tensor":
        """tanh(x) = 2 * sigmoid(2x) - 1"""
        two = Tensor._const(2.0, self.shape, self.dtype)
        one = Tensor._const(1.0, self.shape, self.dtype)
        return two * (two * self).sigmoid() - one

    def gelu(self) -> "Tensor":
        """gelu(x) = x * sigmoid(1.702 * x)  (fast approximation)"""
        return self * (self * 1.702).sigmoid()

    def exp2(self) -> "Tensor":
        return self._unary("EXP2")

    def log2(self) -> "Tensor":
        return self._unary("LOG2")

    def trunc(self) -> "Tensor":
        return self._unary("TRUNC")

    def __neg__(self) -> "Tensor":
        return self.neg()

    def __abs__(self) -> "Tensor":
        return self.relu() + (-self).relu()

    # --- Binary Ops ---

    def __add__(self, other) -> "Tensor":
        return self._binary("ADD", other)

    def __radd__(self, other) -> "Tensor":
        return self._binary("ADD", other)

    def __sub__(self, other) -> "Tensor":
        return self._binary("SUB", other)

    def __rsub__(self, other) -> "Tensor":
        return Tensor._ensure_tensor(other, self.shape, self.dtype) - self

    def __mul__(self, other) -> "Tensor":
        return self._binary("MUL", other)

    def __rmul__(self, other) -> "Tensor":
        return self._binary("MUL", other)

    def __truediv__(self, other) -> "Tensor":
        """a / b = a * reciprocal(b)"""
        other_t = Tensor._ensure_tensor(other, self.shape, self.dtype)
        return self * other_t.reciprocal()

    def __rtruediv__(self, other) -> "Tensor":
        return Tensor._ensure_tensor(other, self.shape, self.dtype) / self

    def __floordiv__(self, other) -> "Tensor":
        return self._binary("IDIV", other)

    def __mod__(self, other) -> "Tensor":
        return self._binary("MOD", other)

    def __and__(self, other) -> "Tensor":
        return self._binary("AND", other)

    def __or__(self, other) -> "Tensor":
        return self._binary("OR", other)

    def __xor__(self, other) -> "Tensor":
        return self._binary("XOR", other)

    def __lshift__(self, other) -> "Tensor":
        return self._binary("SHL", other)

    def __rshift__(self, other) -> "Tensor":
        return self._binary("SHR", other)

    def maximum(self, other) -> "Tensor":
        return self._binary("MAX", other)

    def where(self, x, y) -> "Tensor":
        true_dtype = _where_operand_dtype(x)
        false_dtype = _where_operand_dtype(y)
        result_dtype = _least_upper_dtype(true_dtype, false_dtype)
        true_t = Tensor._ensure_tensor(x, self.shape, true_dtype)
        false_t = Tensor._ensure_tensor(y, self.shape, false_dtype)

        out_shape = _broadcast_shape(_broadcast_shape(self.shape, true_t.shape), false_t.shape)
        cond_t = _cast_tensor_for_where(self, dtypes.bool_)._broadcast_to(out_shape)
        true_t = _cast_tensor_for_where(true_t, result_dtype)._broadcast_to(out_shape)
        false_t = _cast_tensor_for_where(false_t, result_dtype)._broadcast_to(out_shape)
        op = LazyOp(
            "WHERE",
            (cond_t.lazydata, true_t.lazydata, false_t.lazydata),
            dtype=result_dtype,
            shape=out_shape,
        )
        handle = _try_runtime_ternary(
            "WHERE",
            cond_t.lazydata,
            true_t.lazydata,
            false_t.lazydata,
            out_shape,
        )
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, result_dtype, out_shape, handle=handle))
        _raise_if_any_handle_backed_runtime_failed(
            (cond_t.lazydata, true_t.lazydata, false_t.lazydata), "where"
        )

        cond_values = _realize_host_values(cond_t.lazydata)
        true_values = _realize_host_values(true_t.lazydata)
        false_values = _realize_host_values(false_t.lazydata)
        values = [
            true_values[i] if cond_values[i] != 0 else false_values[i]
            for i in range(_numel(out_shape))
        ]
        values = _coerce_values_for_dtype(values, result_dtype)
        return tensor_from_lazy(LazyBuffer(op, result_dtype, out_shape, data=values))

    def __lt__(self, other) -> "Tensor":
        return self._binary("CMPLT", other)

    def __eq__(self, other) -> "Tensor":
        return self._binary("CMPEQ", other)

    def __ne__(self, other) -> "Tensor":
        return self._binary("CMPNE", other)

    def __gt__(self, other) -> "Tensor":
        return Tensor._ensure_tensor(other, self.shape, self.dtype)._binary(
            "CMPLT", self
        )

    def __ge__(self, other) -> "Tensor":
        return 1.0 - (self < other)

    def __le__(self, other) -> "Tensor":
        return 1.0 - (self > other)

    # --- Reduce Ops ---

    def sum(self, axis=None) -> "Tensor":
        return self._reduce("REDUCE_SUM", axis)

    def max(self, axis=None) -> "Tensor":
        return self._reduce("REDUCE_MAX", axis)

    def mean(self, axis=None) -> "Tensor":
        """mean(x) = sum(x) / N"""
        s = self.sum(axis=axis)
        if axis is None:
            n = self.numel()
        else:
            ax = axis if axis >= 0 else self.ndim + axis
            n = self.shape[ax]
        return s * (1.0 / n)

    def argmax(self, axis: int = -1) -> "Tensor":
        """argmax via iterative max comparison."""
        flat = _realize_host_values(self.lazydata)
        if axis < 0:
            axis = self.ndim + axis

        out_shape = list(self.shape)
        reduce_size = out_shape[axis]
        out_shape[axis] = 1

        out_numel = 1
        for s in out_shape:
            out_numel *= s

        stride_after = 1
        for i in range(axis + 1, self.ndim):
            stride_after *= self.shape[i]
        stride_at = reduce_size * stride_after

        result = []
        for out_idx in range(out_numel):
            outer = out_idx // stride_after
            inner = out_idx % stride_after
            base = outer * stride_at + inner

            best_idx = 0
            best_val = flat[base]
            for r in range(1, reduce_size):
                idx = base + r * stride_after
                if flat[idx] > best_val:
                    best_val = flat[idx]
                    best_idx = r
            result.append(float(best_idx))

        result_shape = tuple(out_shape)
        op = LazyOp("LOAD", (), dtype=dtypes.int32, shape=result_shape)
        return tensor_from_lazy(LazyBuffer(op, dtypes.int32, result_shape, data=result))

    def softmax(self, axis: int = -1) -> "Tensor":
        """softmax(x) = exp(x - max(x)) / sum(exp(x - max(x)))"""
        m = self.max(axis=axis)
        # Broadcast max back to original shape for subtraction
        e = (self - m._broadcast_to(self.shape)).exp()
        s = e.sum(axis=axis)
        return e / s._broadcast_to(e.shape)

    def log_softmax(self, axis: int = -1) -> "Tensor":
        """log_softmax(x) = x - log(sum(exp(x - max(x)))) - max(x)"""
        return self.softmax(axis=axis).log()

    # --- Movement Ops (zero-cost via ShapeTracker) ---

    def reshape(self, *new_shape) -> "Tensor":
        resolved = _resolve_shape(new_shape)
        # Handle -1 dimension inference
        resolved = list(resolved)
        neg_idx = -1
        known_product = 1
        for i, s in enumerate(resolved):
            if s == -1:
                neg_idx = i
            else:
                known_product *= s
        if neg_idx >= 0:
            resolved[neg_idx] = self.numel() // known_product
        resolved = tuple(resolved)

        op = LazyOp("RESHAPE", (self.lazydata,), arg=resolved, dtype=self.dtype, shape=resolved)
        handle = _try_runtime_reshape(self.lazydata, resolved)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, self.dtype, resolved, handle=handle))
        _raise_if_handle_backed_runtime_failed(self.lazydata, "reshape")
        flat = _realize_host_values(self.lazydata)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, resolved, data=list(flat)))

    def permute(self, *order) -> "Tensor":
        if len(order) == 1 and isinstance(order[0], (list, tuple)):
            order = tuple(order[0])
        order = tuple(int(axis) for axis in order)
        if sorted(order) != list(range(self.ndim)):
            raise ValueError(f"permute order {order} is not a permutation of {self.ndim} axes")
        new_shape = tuple(self.shape[i] for i in order)
        op = LazyOp("PERMUTE", (self.lazydata,), arg=order, dtype=self.dtype, shape=new_shape)
        handle = _try_runtime_permute(self.lazydata, order)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, self.dtype, new_shape, handle=handle))
        _raise_if_handle_backed_runtime_failed(self.lazydata, "permute")
        flat = _realize_host_values(self.lazydata)
        result = _permute_data(flat, self.shape, order)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, new_shape, data=result))

    def expand(self, *new_shape) -> "Tensor":
        resolved = _resolve_shape(new_shape)
        if len(resolved) != self.ndim:
            raise ValueError(
                f"expand requires {self.ndim} dims for source shape {self.shape}, got {resolved}"
            )
        for old, new in zip(self.shape, resolved):
            if old != new and old != 1:
                raise ValueError(f"cannot expand shape {self.shape} to {resolved}")
        op = LazyOp("EXPAND", (self.lazydata,), arg=resolved, dtype=self.dtype, shape=resolved)
        handle = _try_runtime_expand(self.lazydata, resolved)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, self.dtype, resolved, handle=handle))
        _raise_if_handle_backed_runtime_failed(self.lazydata, "expand")
        flat = _realize_host_values(self.lazydata)
        result = _expand_data(flat, self.shape, resolved)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, resolved, data=result))

    def pad(self, padding, value: float = 0.0) -> "Tensor":
        """Pad tensor. padding is list of (before, after) pairs per dim."""
        padding = _normalize_pairs(padding, self.ndim, "pad")
        new_shape = tuple(s + before + after for s, (before, after) in zip(self.shape, padding))
        op = LazyOp(
            "PAD",
            (self.lazydata,),
            arg=(padding, value),
            dtype=self.dtype,
            shape=new_shape,
        )
        if value == 0.0:
            handle = _try_runtime_pad(self.lazydata, padding)
            if handle is not None:
                return tensor_from_lazy(LazyBuffer(op, self.dtype, new_shape, handle=handle))
            _raise_if_handle_backed_runtime_failed(self.lazydata, "pad")
        elif self.lazydata.handle is not None:
            raise RuntimeError("molt GPU runtime pad currently supports zero padding only")

        flat = _realize_host_values(self.lazydata)
        numel = 1
        for s in new_shape:
            numel *= s
        result = [value] * numel
        _copy_with_padding(flat, self.shape, result, new_shape, padding)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, new_shape, data=result))

    def shrink(self, bounds) -> "Tensor":
        """Extract sub-region. bounds is list of (start, end) per dim."""
        bounds = _normalize_pairs(bounds, self.ndim, "shrink")
        for dim, (start, end) in enumerate(bounds):
            if start > end or end > self.shape[dim]:
                raise ValueError(f"invalid shrink bounds {bounds} for shape {self.shape}")
        new_shape = tuple(e - s for s, e in bounds)
        op = LazyOp("SHRINK", (self.lazydata,), arg=bounds, dtype=self.dtype, shape=new_shape)
        handle = _try_runtime_shrink(self.lazydata, bounds)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, self.dtype, new_shape, handle=handle))
        _raise_if_handle_backed_runtime_failed(self.lazydata, "shrink")
        flat = _realize_host_values(self.lazydata)
        result = _shrink_data(flat, self.shape, bounds)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, new_shape, data=result))

    def flip(self, axis: int = 0) -> "Tensor":
        axis = int(axis)
        if axis < 0:
            axis += self.ndim
        if axis < 0 or axis >= self.ndim:
            raise ValueError(f"flip axis {axis} out of bounds for shape {self.shape}")
        op = LazyOp("FLIP", (self.lazydata,), arg=axis, dtype=self.dtype, shape=self.shape)
        handle = _try_runtime_flip(self.lazydata, axis)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, self.dtype, self.shape, handle=handle))
        _raise_if_handle_backed_runtime_failed(self.lazydata, "flip")
        flat = _realize_host_values(self.lazydata)
        result = _flip_data(flat, self.shape, axis)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, self.shape, data=result))

    def contiguous(self) -> "Tensor":
        """Force materialization."""
        op = LazyOp("CONTIGUOUS", (self.lazydata,), dtype=self.dtype, shape=self.shape)
        handle = _try_runtime_contiguous(self.lazydata)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, self.dtype, self.shape, handle=handle))
        _raise_if_handle_backed_runtime_failed(self.lazydata, "contiguous")
        flat = _realize_host_values(self.lazydata)
        return tensor_from_lazy(
            _lazy_load_from_values(_coerce_values_for_dtype(flat, self.dtype), self.dtype, self.shape)
        )

    @property
    def T(self) -> "Tensor":
        """Transpose (reverse all axes)."""
        order = list(range(self.ndim - 1, -1, -1))
        return self.permute(*order)

    def flatten(self, start_dim: int = 0, end_dim: int = -1) -> "Tensor":
        if end_dim < 0:
            end_dim = self.ndim + end_dim
        new_shape = list(self.shape[:start_dim])
        flat_size = 1
        for i in range(start_dim, end_dim + 1):
            flat_size *= self.shape[i]
        new_shape.append(flat_size)
        new_shape.extend(self.shape[end_dim + 1 :])
        return self.reshape(*new_shape)

    def unsqueeze(self, dim: int) -> "Tensor":
        if dim < 0:
            dim = self.ndim + 1 + dim
        new_shape = list(self.shape)
        new_shape.insert(dim, 1)
        return self.reshape(*new_shape)

    def squeeze(self, dim: int = None) -> "Tensor":
        if dim is not None:
            if self.shape[dim] != 1:
                return self
            new_shape = list(self.shape)
            new_shape.pop(dim)
            return self.reshape(*new_shape)
        new_shape = [s for s in self.shape if s != 1]
        if not new_shape:
            new_shape = [1]
        return self.reshape(*new_shape)

    # --- Falcon-OCR Required Methods ---

    def rms_norm(self, eps: float = 1e-6) -> "Tensor":
        """Unit-scale RMSNorm: x / sqrt(mean(x^2) + eps). No learned weight."""
        var = (self * self).mean(axis=-1)
        return self * (var + eps)._broadcast_to(self.shape).reciprocal().sqrt()

    def repeat_axis(self, axis: int, n_rep: int) -> "Tensor":
        """Repeat elements along an axis n_rep times.

        For a tensor of shape (..., H, ...), repeating axis with n_rep=2
        produces shape (..., H*n_rep, ...) by duplicating each slice along
        that axis.
        """
        if n_rep == 1:
            return self
        flat = _realize_host_values(self.lazydata)
        ndim = self.ndim
        shape = self.shape
        ax = axis if axis >= 0 else ndim + axis

        new_shape = list(shape)
        new_shape[ax] = shape[ax] * n_rep

        dst_numel = 1
        for s in new_shape:
            dst_numel *= s

        result = [0.0] * dst_numel

        for idx in range(dst_numel):
            remaining = idx
            indices = [0] * ndim
            for d in range(ndim - 1, -1, -1):
                indices[d] = remaining % new_shape[d]
                remaining //= new_shape[d]

            # Map repeated index back to source
            src_indices = list(indices)
            src_indices[ax] = indices[ax] // n_rep

            src_flat = 0
            stride = 1
            for d in range(ndim - 1, -1, -1):
                src_flat += src_indices[d] * stride
                stride *= shape[d]

            result[idx] = flat[src_flat]

        out_shape = tuple(new_shape)
        op = LazyOp("LOAD", (), dtype=self.dtype, shape=out_shape)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, out_shape, data=result))

    def split_last_dim(self, sizes: tuple) -> tuple:
        """Split the last dimension into multiple tensors of given sizes.

        Returns a tuple of Tensors, each with the last dimension sliced
        according to the sizes tuple.
        """
        flat = _realize_host_values(self.lazydata)
        total = sum(sizes)
        if total != self.shape[-1]:
            raise ValueError(
                f"split sizes {sizes} sum to {total}, but last dim is {self.shape[-1]}"
            )
        shape = self.shape
        leading_shape = shape[:-1]
        leading_numel = 1
        for s in leading_shape:
            leading_numel *= s
        last_dim = shape[-1]

        results = []
        offset = 0
        for sz in sizes:
            out_shape = leading_shape + (sz,)
            out_numel = leading_numel * sz
            out_data = [0.0] * out_numel
            for row in range(leading_numel):
                src_base = row * last_dim + offset
                dst_base = row * sz
                for i in range(sz):
                    out_data[dst_base + i] = flat[src_base + i]
            op = LazyOp("LOAD", (), dtype=self.dtype, shape=out_shape)
            results.append(
                tensor_from_lazy(LazyBuffer(op, self.dtype, out_shape, data=out_data))
            )
            offset += sz

        return tuple(results)

    def squared_relu_gate_interleaved(self) -> "Tensor":
        """Squared-ReLU gate from interleaved packing.

        Input has gate/up interleaved on the last axis:
        [g0, u0, g1, u1, ...]. Output is relu(gate)^2 * up with
        the last dimension halved.
        """
        flat = _realize_host_values(self.lazydata)
        shape = self.shape
        last_dim = shape[-1]
        if last_dim % 2 != 0:
            raise ValueError(
                f"squared_relu_gate_interleaved requires even last dim, got {last_dim}"
            )
        half_dim = last_dim // 2

        leading_shape = shape[:-1]
        leading_numel = 1
        for s in leading_shape:
            leading_numel *= s

        out_shape = leading_shape + (half_dim,)
        out_numel = leading_numel * half_dim
        out_data = [0.0] * out_numel

        for row in range(leading_numel):
            src_base = row * last_dim
            dst_base = row * half_dim
            for i in range(half_dim):
                gate = flat[src_base + 2 * i]
                up = flat[src_base + 2 * i + 1]
                gate = max(gate, 0.0)  # ReLU
                out_data[dst_base + i] = gate * gate * up  # squared ReLU * up

        op = LazyOp("LOAD", (), dtype=self.dtype, shape=out_shape)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, out_shape, data=out_data))

    # --- Matrix Ops ---

    def dot(self, other: "Tensor") -> "Tensor":
        """Matrix multiply via RESHAPE + EXPAND + MUL + REDUCE_SUM."""
        return self.matmul(other)

    def matmul(self, other: "Tensor") -> "Tensor":
        """Matrix multiplication. Supports 2D @ 2D."""
        other_t = other if isinstance(other, Tensor) else Tensor(other, dtype=self.dtype)
        if self.ndim == 2 and other_t.ndim == 2:
            m, k = self.shape
            k2, n = other_t.shape
            if k != k2:
                raise ValueError(f"matmul shape mismatch: ({m},{k}) @ ({k2},{n})")
            lhs = self.reshape(m, k, 1).expand(m, k, n)
            rhs = other_t.reshape(1, k, n).expand(m, k, n)
            return (lhs * rhs).sum(axis=1).reshape(m, n)
        if self.ndim == 1 and other_t.ndim == 1:
            if self.shape[0] != other_t.shape[0]:
                raise ValueError("dot product length mismatch")
            return (self * other_t).sum(axis=0).reshape(1)
        raise ValueError(
            f"matmul not supported for ndim {self.ndim} @ {other_t.ndim}"
        )

    def __matmul__(self, other: "Tensor") -> "Tensor":
        return self.matmul(other)

    @staticmethod
    def cat(*tensors, dim: int = 0) -> "Tensor":
        """Concatenate tensors along a dimension."""
        if len(tensors) == 1 and isinstance(tensors[0], (list, tuple)):
            tensors = tensors[0]
        if not tensors:
            raise ValueError("cat requires at least one tensor")

        all_data = [_realize_host_values(t.lazydata) for t in tensors]
        shapes = [t.shape for t in tensors]

        # Compute output shape
        out_shape = list(shapes[0])
        for i in range(1, len(shapes)):
            out_shape[dim] += shapes[i][dim]
        out_shape = tuple(out_shape)

        result = _cat_data(all_data, shapes, dim)
        dt = tensors[0].dtype
        op = LazyOp("LOAD", (), dtype=dt, shape=out_shape)
        return tensor_from_lazy(LazyBuffer(op, dt, out_shape, data=result))

    @staticmethod
    def stack(*tensors, dim: int = 0) -> "Tensor":
        """Stack tensors along a new dimension."""
        if len(tensors) == 1 and isinstance(tensors[0], (list, tuple)):
            tensors = tensors[0]
        unsqueezed = [t.unsqueeze(dim) for t in tensors]
        return Tensor.cat(*unsqueezed, dim=dim)

    # --- Indexing ---

    def gather(self, dim: int, index: "Tensor") -> "Tensor":
        """Gather elements along a dimension using index tensor."""
        if dim < 0:
            dim += self.ndim
        if dim == 0 and self.ndim >= 2 and len(index.shape) == 1:
            rows = self.tolist()
            idx_data = index.tolist()
            result = []
            for raw_idx in idx_data:
                row = int(raw_idx)
                if row < 0 or row >= len(rows):
                    raise IndexError(f"gather index {row} out of range for axis 0")
                result.append(rows[row])
            return self.__class__(result, dtype=self.dtype)

        src_data = _realize_host_values(self.lazydata)
        idx_data = _realize_host_values(index.lazydata)
        result = _gather_data(src_data, self.shape, idx_data, index.shape, dim)
        op = LazyOp("LOAD", (), dtype=self.dtype, shape=index.shape)
        return self.__class__(LazyBuffer(op, self.dtype, index.shape, data=result))

    def scatter(self, dim: int, index: "Tensor", src: "Tensor") -> "Tensor":
        """Scatter src elements into self along a dimension using index tensor."""
        if dim < 0:
            dim += self.ndim
        if dim == 0 and self.ndim >= 2 and len(index.shape) == 1:
            rows = self.tolist()
            src_rows = src.tolist()
            idx_data = index.tolist()
            if len(src_rows) != len(idx_data):
                raise ValueError(
                    f"scatter source has {len(src_rows)} rows for {len(idx_data)} axis-0 indices"
                )
            for pos, raw_idx in enumerate(idx_data):
                row = int(raw_idx)
                if row < 0 or row >= len(rows):
                    raise IndexError(f"scatter index {row} out of range for axis 0")
                rows[row] = src_rows[pos]
            return self.__class__(rows, dtype=self.dtype)

        self_data = list(_realize_host_values(self.lazydata))
        src_data = _realize_host_values(src.lazydata)
        idx_data = _realize_host_values(index.lazydata)
        _scatter_data(self_data, self.shape, src_data, idx_data, index.shape, dim)
        op = LazyOp("LOAD", (), dtype=self.dtype, shape=self.shape)
        return self.__class__(LazyBuffer(op, self.dtype, self.shape, data=self_data))

    def __getitem__(self, idx) -> "Tensor":
        """Basic integer/slice indexing."""
        flat = _realize_host_values(self.lazydata)
        if isinstance(idx, int):
            if self.ndim == 1:
                return tensor_from_data([flat[idx]], dtype=self.dtype)
            # Select along first dimension
            inner = 1
            for s in self.shape[1:]:
                inner *= s
            start = idx * inner
            new_data = flat[start : start + inner]
            new_shape = self.shape[1:]
            op = LazyOp("LOAD", (), dtype=self.dtype, shape=new_shape)
            return tensor_from_lazy(
                LazyBuffer(op, self.dtype, new_shape, data=new_data)
            )
        raise TypeError(f"Unsupported index type: {type(idx)}")

    # --- Specialized Compositions ---

    def scaled_dot_product_attention(
        self,
        k: "Tensor",
        v: "Tensor",
        attn_mask: "Tensor" = None,
        scale: float = None,
        is_causal: bool = False,
    ) -> "Tensor":
        """Scaled dot-product attention: softmax(Q @ K^T * scale + mask) @ V.

        Can be called as:
          q.scaled_dot_product_attention(k, v, mask, scale)
          Tensor.scaled_dot_product_attention(q, k, v, mask, scale)  (first arg is self=q)
        """
        q = self
        d_k = q.shape[-1]
        actual_scale = scale if scale is not None else (1.0 / math.sqrt(d_k))
        scores = (q @ k.T) * actual_scale
        if is_causal:
            n = scores.shape[-1]
            mask_data = []
            for i in range(n):
                for j in range(n):
                    mask_data.append(0.0 if j <= i else float("-inf"))
            cmask = tensor_from_lazy(
                LazyBuffer(
                    LazyOp("LOAD", (), dtype=scores.dtype, shape=(n, n)),
                    scores.dtype,
                    (n, n),
                    data=mask_data,
                )
            )
            scores = scores + cmask
        if attn_mask is not None:
            scores = scores + attn_mask
        weights = scores.softmax(axis=-1)
        return weights @ v

    def layernorm(
        self,
        normalized_shape=None,
        weight: "Tensor" = None,
        bias: "Tensor" = None,
        eps: float = 1e-5,
        axis=-1,
    ) -> "Tensor":
        """Layer normalization over one or more axes.

        Matches upstream tinygrad's call shape: `axis` accepts an int or a
        tuple of ints (negative indexing supported). Reduction is computed
        as the mean / variance over the union of the specified axes; the
        reduced dimensions are kept as size 1 so broadcasting back to the
        original shape works regardless of which axes were reduced.
        """
        if isinstance(axis, (list, tuple)):
            axes = tuple(int(a) for a in axis)
        else:
            axes = (int(axis),)
        # Normalize negative axes and sort descending so sequential reduction
        # keeps the remaining indices valid.
        axes = tuple(a if a >= 0 else self.ndim + a for a in axes)
        if any(a < 0 or a >= self.ndim for a in axes):
            raise ValueError(
                f"layernorm axis {axis!r} out of range for tensor of ndim {self.ndim}"
            )
        axes_desc = tuple(sorted(set(axes), reverse=True))

        def _reduce_mean(t):
            out = t
            for a in axes_desc:
                out = out.mean(axis=a)
            return out

        mean = _reduce_mean(self)
        diff = self - mean._broadcast_to(self.shape)
        var = _reduce_mean(diff * diff)
        inv_std = (var + eps).reciprocal().sqrt()
        normed = diff * inv_std._broadcast_to(self.shape)
        if weight is not None:
            normed = normed * weight
        if bias is not None:
            normed = normed + bias
        return normed

    def conv2d(
        self,
        weight: "Tensor",
        bias: "Tensor" = None,
        groups=1,
        stride=1,
        dilation=1,
        padding=0,
        dtype=None,
    ) -> "Tensor":
        """Convolution matching tinygrad Tensor.conv2d call semantics."""
        x = self
        if x.ndim < 3:
            raise ValueError(f"conv2d input must have at least 3 dims, got {x.shape}")
        if x.ndim != weight.ndim:
            raise ValueError(
                f"Input Tensor shape {x.shape} does not match weights {weight.shape}"
            )
        spatial_ndim = x.ndim - 2
        stride_t = _make_tuple(stride, spatial_ndim, "stride")
        dilation_t = _make_tuple(dilation, spatial_ndim, "dilation")
        pads = _resolve_pool_pads(padding, spatial_ndim)
        groups = int(groups)
        if groups <= 0:
            raise ValueError("conv2d groups must be positive")

        batch, in_channels = x.shape[:2]
        out_channels, weight_in_channels = weight.shape[:2]
        spatial = x.shape[2:]
        kernels = weight.shape[2:]
        if in_channels % groups != 0 or out_channels % groups != 0:
            raise ValueError("conv2d channels must be divisible by groups")
        if groups * weight_in_channels != in_channels:
            raise ValueError(
                f"Input Tensor shape {x.shape} does not match weights {weight.shape}"
            )
        if bias is not None and bias.shape != (out_channels,):
            raise ValueError(
                f"conv2d bias shape mismatch: {bias.shape} vs ({out_channels},)"
            )

        out_spatial = tuple(
            (
                spatial[i]
                + pads[i][0]
                + pads[i][1]
                - dilation_t[i] * (kernels[i] - 1)
                - 1
            )
            // stride_t[i]
            + 1
            for i in range(spatial_ndim)
        )
        if any(size < 0 for size in out_spatial):
            raise ValueError(f"conv2d output shape is invalid: {out_spatial}")

        x_data = _realize_host_values(x.lazydata)
        w_data = _realize_host_values(weight.lazydata)
        b_data = None
        if bias is not None:
            b_data = _realize_host_values(bias.lazydata)

        out_shape = (batch, out_channels, *out_spatial)
        out_data = [0.0] * _numel(out_shape)
        out_channels_per_group = out_channels // groups
        x_strides = _strides_for_shape(x.shape)
        w_strides = _strides_for_shape(weight.shape)
        out_strides = _strides_for_shape(out_shape)

        for n in range(batch):
            for oc in range(out_channels):
                group = oc // out_channels_per_group
                input_base_channel = group * weight_in_channels
                for out_coords in _iter_indices(out_spatial):
                    acc = 0.0 if b_data is None else b_data[oc]
                    for local_ic in range(weight_in_channels):
                        ic = input_base_channel + local_ic
                        for kernel_coords in _iter_indices(kernels):
                            src_coords = tuple(
                                out_coords[i] * stride_t[i]
                                - pads[i][0]
                                + kernel_coords[i] * dilation_t[i]
                                for i in range(spatial_ndim)
                            )
                            if not all(
                                0 <= src_coords[i] < spatial[i]
                                for i in range(spatial_ndim)
                            ):
                                continue
                            x_index = _flat_index((n, ic, *src_coords), x_strides)
                            w_index = _flat_index(
                                (oc, local_ic, *kernel_coords), w_strides
                            )
                            acc += x_data[x_index] * w_data[w_index]
                    out_index = _flat_index((n, oc, *out_coords), out_strides)
                    out_data[out_index] = acc

        out_dtype = dtype or x.dtype
        op = LazyOp("LOAD", (), dtype=out_dtype, shape=out_shape)
        return tensor_from_lazy(LazyBuffer(op, out_dtype, out_shape, data=out_data))

    def conv_transpose2d(
        self,
        weight: "Tensor",
        bias: "Tensor" = None,
        groups=1,
        stride=1,
        dilation=1,
        padding=0,
        output_padding=0,
    ) -> "Tensor":
        """Transposed convolution matching tinygrad Tensor.conv_transpose2d."""
        x = self
        if x.ndim < 3:
            raise ValueError(
                f"conv_transpose2d input must have at least 3 dims, got {x.shape}"
            )
        if x.ndim != weight.ndim:
            raise ValueError(
                f"Input Tensor shape {x.shape} does not match weights {weight.shape}"
            )
        spatial_ndim = x.ndim - 2
        stride_t = _make_tuple(stride, spatial_ndim, "stride")
        dilation_t = _make_tuple(dilation, spatial_ndim, "dilation")
        output_padding_t = _make_tuple(output_padding, spatial_ndim, "output_padding")
        pads = _resolve_pool_pads(padding, spatial_ndim)
        groups = int(groups)
        if groups <= 0:
            raise ValueError("conv_transpose2d groups must be positive")

        batch, in_channels = x.shape[:2]
        weight_in_channels, out_channels_per_group = weight.shape[:2]
        spatial = x.shape[2:]
        kernels = weight.shape[2:]
        if weight_in_channels != in_channels:
            raise ValueError(
                f"Input Tensor shape {x.shape} does not match weights {weight.shape}"
            )
        if in_channels % groups != 0:
            raise ValueError("conv_transpose2d input channels must divide groups")
        out_channels = out_channels_per_group * groups
        if bias is not None and bias.shape != (out_channels,):
            raise ValueError(
                f"conv_transpose2d bias shape mismatch: {bias.shape} vs ({out_channels},)"
            )

        out_spatial = tuple(
            (spatial[i] - 1) * stride_t[i]
            - pads[i][0]
            - pads[i][1]
            + dilation_t[i] * (kernels[i] - 1)
            + output_padding_t[i]
            + 1
            for i in range(spatial_ndim)
        )
        if any(size < 0 for size in out_spatial):
            raise ValueError(f"conv_transpose2d output shape is invalid: {out_spatial}")

        out_shape = (batch, out_channels, *out_spatial)
        out_data = [0.0] * _numel(out_shape)
        x_data = _realize_host_values(x.lazydata)
        w_data = _realize_host_values(weight.lazydata)
        x_strides = _strides_for_shape(x.shape)
        w_strides = _strides_for_shape(weight.shape)
        out_strides = _strides_for_shape(out_shape)
        input_channels_per_group = in_channels // groups

        for n in range(batch):
            for ic in range(in_channels):
                group = ic // input_channels_per_group
                for in_coords in _iter_indices(spatial):
                    x_index = _flat_index((n, ic, *in_coords), x_strides)
                    x_val = x_data[x_index]
                    for local_oc in range(out_channels_per_group):
                        oc = group * out_channels_per_group + local_oc
                        for kernel_coords in _iter_indices(kernels):
                            out_coords = tuple(
                                in_coords[i] * stride_t[i]
                                - pads[i][0]
                                + kernel_coords[i] * dilation_t[i]
                                for i in range(spatial_ndim)
                            )
                            if not all(
                                0 <= out_coords[i] < out_spatial[i]
                                for i in range(spatial_ndim)
                            ):
                                continue
                            w_index = _flat_index(
                                (ic, local_oc, *kernel_coords), w_strides
                            )
                            out_index = _flat_index((n, oc, *out_coords), out_strides)
                            out_data[out_index] += x_val * w_data[w_index]

        if bias is not None:
            b_data = _realize_host_values(bias.lazydata)
            for n in range(batch):
                for oc in range(out_channels):
                    for out_coords in _iter_indices(out_spatial):
                        out_index = _flat_index((n, oc, *out_coords), out_strides)
                        out_data[out_index] += b_data[oc]

        op = LazyOp("LOAD", (), dtype=x.dtype, shape=out_shape)
        return tensor_from_lazy(LazyBuffer(op, x.dtype, out_shape, data=out_data))

    # --- Internal Helpers ---

    def _unary(self, op_name: str) -> "Tensor":
        op = LazyOp(op_name, (self.lazydata,), dtype=self.dtype, shape=self.shape)
        handle = _try_runtime_unary(op_name, self.lazydata)
        return tensor_from_lazy(LazyBuffer(op, self.dtype, self.shape, handle=handle))

    def _unary_compose(
        self, op_name: str, pre_mul: float = None, post_mul: float = None
    ) -> "Tensor":
        src = self
        if pre_mul is not None:
            src = src * pre_mul
        result = src._unary(op_name)
        if post_mul is not None:
            result = result * post_mul
        return result

    def _binary(self, op_name: str, other) -> "Tensor":
        other_t = Tensor._ensure_tensor(other, self.shape, self.dtype)
        # Determine output shape via broadcasting
        out_shape = _broadcast_shape(self.shape, other_t.shape)
        is_cmp = op_name in ("CMPLT", "CMPEQ", "CMPNE")
        out_dtype = dtypes.bool_ if is_cmp else self.dtype
        op = LazyOp(
            op_name,
            (self.lazydata, other_t.lazydata),
            dtype=out_dtype,
            shape=out_shape,
        )
        handle = _try_runtime_binary(op_name, self.lazydata, other_t.lazydata, out_shape)
        return tensor_from_lazy(LazyBuffer(op, out_dtype, out_shape, handle=handle))

    def _reduce(self, op_name: str, axis) -> "Tensor":
        if axis is not None:
            ax = axis if axis >= 0 else self.ndim + axis
            out_shape = list(self.shape)
            out_shape[ax] = 1
            out_shape = tuple(out_shape)
        else:
            out_shape = (1,)
        op = LazyOp(
            op_name,
            (self.lazydata,),
            arg=axis,
            dtype=self.dtype,
            shape=out_shape,
        )
        handle = (
            _try_runtime_reduce(op_name, self.lazydata, ax)
            if axis is not None
            else _try_runtime_reduce_all(op_name, self.lazydata, out_shape)
        )
        return tensor_from_lazy(LazyBuffer(op, self.dtype, out_shape, handle=handle))

    def _broadcast_to(self, target_shape: tuple) -> "Tensor":
        """Broadcast this tensor to target_shape by repeating data."""
        target_shape = tuple(target_shape)
        if self.shape == target_shape:
            return self
        if len(self.shape) > len(target_shape):
            raise ValueError(f"cannot broadcast shape {self.shape} to {target_shape}")
        source = self
        if len(self.shape) < len(target_shape):
            padded = (1,) * (len(target_shape) - len(self.shape)) + self.shape
            source = self.reshape(padded)
        return source.expand(target_shape)

    @staticmethod
    def _const(val: float, shape: tuple, dtype: DType) -> "Tensor":
        coerced = _coerce_values_for_dtype([val] * _numel(shape), dtype)
        op = LazyOp("CONST", (), arg=val, dtype=dtype, shape=shape)
        handle = _try_create_runtime_tensor(coerced, dtype, shape)
        return tensor_from_lazy(LazyBuffer(op, dtype, shape, data=coerced, handle=handle))

    @staticmethod
    def _ensure_tensor(x, shape: tuple, dtype: DType) -> "Tensor":
        if isinstance(x, Tensor):
            return x
        if isinstance(x, (int, float)):
            return Tensor._const(float(x), (1,), dtype)
        return tensor_from_data(x, dtype=dtype)

    def cast(self, dtype) -> "Tensor":
        dt = _resolve_dtype(dtype)
        op = LazyOp("CAST", (self.lazydata,), dtype=dt, shape=self.shape)
        handle = _try_runtime_cast("CAST", self.lazydata, dt)
        if handle is not None:
            return tensor_from_lazy(LazyBuffer(op, dt, self.shape, handle=handle))
        flat = _realize_host_values(self.lazydata)
        return tensor_from_lazy(
            _lazy_load_from_values(_coerce_values_for_dtype(flat, dt), dt, self.shape)
        )

    def float(self) -> "Tensor":
        return self.cast(dtypes.float32)

    def __repr__(self) -> str:
        return f"<Tensor shape={self.shape} dtype={self.dtype}>"


def tensor_from_lazy(lazydata):
    return Tensor(lazydata)


def tensor_from_data(data, dtype=None):
    return Tensor(data, dtype=dtype)


# --- Utility Functions ---


def _resolve_dtype(dtype) -> DType:
    if dtype is None:
        return dtypes.float32
    if isinstance(dtype, DType):
        return dtype
    if dtype is float:
        return dtypes.float32
    if dtype is int:
        return dtypes.int32
    if dtype is bool:
        return dtypes.bool_
    raise TypeError(f"unsupported dtype {dtype!r}")


def _where_operand_dtype(value) -> DType:
    if isinstance(value, Tensor):
        return value.dtype
    if isinstance(value, (bytes, bytearray, memoryview)):
        return dtypes.uint8
    if isinstance(value, bool):
        return dtypes.bool_
    if isinstance(value, int):
        return dtypes.int32
    if isinstance(value, float):
        return dtypes.float32
    flat, _shape, inferred_dtype = _flatten_data(value)
    if inferred_dtype is not None:
        return inferred_dtype
    if not flat:
        return dtypes.float32
    if all(isinstance(item, bool) for item in flat):
        return dtypes.bool_
    if all(isinstance(item, (bool, int)) and not isinstance(item, float) for item in flat):
        return dtypes.int32
    if all(isinstance(item, (bool, int, float)) for item in flat):
        return dtypes.float32
    raise TypeError(f"cannot infer tensor dtype for {value!r}")


def _dtype_promotion_parents(dtype: DType) -> set:
    if dtype not in _DTYPE_PROMOTION_PRIORITY:
        raise TypeError(f"unsupported dtype promotion for {dtype!r}")
    parents = {dtype}
    for parent in _DTYPE_PROMOTION_EDGES[dtype]:
        parents.update(_dtype_promotion_parents(parent))
    return parents


def _least_upper_dtype(*operand_dtypes: DType) -> DType:
    if not operand_dtypes:
        return dtypes.float32
    parent_sets = [_dtype_promotion_parents(dtype) for dtype in operand_dtypes]
    common = set.intersection(*parent_sets)
    if not common:
        raise TypeError(f"no common dtype for {operand_dtypes!r}")
    return min(common, key=lambda dtype: _DTYPE_PROMOTION_PRIORITY[dtype])


def _cast_tensor_for_where(tensor: Tensor, dtype: DType) -> Tensor:
    dt = _resolve_dtype(dtype)
    if tensor.dtype == dt:
        return tensor
    op = LazyOp("CAST", (tensor.lazydata,), dtype=dt, shape=tensor.shape)
    handle = _try_runtime_cast("CAST", tensor.lazydata, dt)
    if handle is not None:
        return tensor_from_lazy(LazyBuffer(op, dt, tensor.shape, handle=handle))
    _raise_if_handle_backed_runtime_failed(tensor.lazydata, "where")
    flat = _realize_host_values(tensor.lazydata)
    return tensor_from_lazy(
        _lazy_load_from_values(_coerce_values_for_dtype(flat, dt), dt, tensor.shape)
    )


def _coerce_values_for_dtype(values: list, dtype: DType) -> list:
    if dtype is dtypes.bool_:
        return [bool(value) for value in values]
    if dtype in _INTEGER_DTYPES:
        return [int(value) for value in values]
    if dtype in _FLOAT_DTYPES or dtype is dtypes.bfloat16:
        return [float(value) for value in values]
    raise TypeError(f"unsupported tensor dtype {dtype!r}")


def _pack_typed_values(values: list, dtype: DType) -> bytes:
    if dtype is dtypes.bfloat16:
        raise RuntimeError("bfloat16 raw tensor packing requires runtime-owned bf16 packing")
    if dtype not in _RAW_STRUCT_DTYPES:
        raise RuntimeError(f"unsupported raw tensor dtype {dtype!r}")
    if not values:
        return b""
    return _struct.pack("<" + str(len(values)) + dtype.fmt, *values)


def _unpack_typed_values(raw: bytes | bytearray, dtype: DType) -> list:
    if dtype is dtypes.bfloat16:
        raise RuntimeError("bfloat16 raw tensor readback requires runtime-owned bf16 unpacking")
    if dtype not in _RAW_STRUCT_DTYPES:
        raise RuntimeError(f"unsupported raw tensor dtype {dtype!r}")
    if len(raw) % dtype.itemsize != 0:
        raise RuntimeError("raw tensor byte length is not divisible by dtype itemsize")
    count = len(raw) // dtype.itemsize
    if count == 0:
        return []
    return list(_struct.unpack("<" + str(count) + dtype.fmt, bytes(raw)))


def _expected_storage_nbytes(shape: tuple, dtype: DType) -> int:
    return _numel(shape) * dtype.itemsize


def _runtime_handle_or_none(raw_handle) -> int | None:
    if isinstance(raw_handle, int) and raw_handle != _INVALID_HANDLE:
        return raw_handle
    return None


def _try_create_runtime_tensor(values: list, dtype: DType, shape: tuple) -> int | None:
    if dtype.code is None:
        return None
    try:
        raw = _pack_typed_values(values, dtype)
    except RuntimeError:
        return None
    try:
        return _runtime_handle_or_none(_gpu_create_raw(raw, len(raw), dtype.code, shape))
    except Exception:
        return None


def _try_create_runtime_zeros(dtype: DType, shape: tuple) -> int | None:
    if dtype.code is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_zeros_dtype(dtype.code, shape))
    except Exception:
        return None


def _ensure_runtime_handle(lazydata: LazyBuffer) -> int | None:
    if lazydata.handle is not None:
        return lazydata.handle
    if lazydata._data is None:
        return None
    handle = _try_create_runtime_tensor(lazydata._data, lazydata.dtype, lazydata.shape)
    if handle is not None:
        lazydata._handle = handle
    return handle


def _try_runtime_unary(op_name: str, src: LazyBuffer) -> int | None:
    op_code = _UNARY_OP_CODES.get(op_name)
    src_handle = _ensure_runtime_handle(src)
    if op_code is None or src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_unary(op_code, src_handle))
    except Exception:
        return None


def _try_runtime_binary(
    op_name: str, lhs: LazyBuffer, rhs: LazyBuffer, out_shape: tuple
) -> int | None:
    op_code = _BINARY_OP_CODES.get(op_name)
    lhs_handle = _ensure_runtime_handle(lhs)
    rhs_handle = _ensure_runtime_handle(rhs)
    if (
        op_code is None
        or lhs_handle is None
        or rhs_handle is None
        or lhs.shape != out_shape
    ):
        return None
    try:
        return _runtime_handle_or_none(_gpu_binary(op_code, lhs_handle, rhs_handle))
    except Exception:
        return None


def _try_runtime_ternary(
    op_name: str,
    cond: LazyBuffer,
    a: LazyBuffer,
    b: LazyBuffer,
    out_shape: tuple,
) -> int | None:
    op_code = _TERNARY_OP_CODES.get(op_name)
    cond_handle = _ensure_runtime_handle(cond)
    a_handle = _ensure_runtime_handle(a)
    b_handle = _ensure_runtime_handle(b)
    if (
        op_code is None
        or cond_handle is None
        or a_handle is None
        or b_handle is None
        or cond.dtype != dtypes.bool_
        or cond.shape != out_shape
        or a.shape != out_shape
        or b.shape != out_shape
        or a.dtype != b.dtype
    ):
        return None
    try:
        return _runtime_handle_or_none(_gpu_ternary(op_code, cond_handle, a_handle, b_handle))
    except Exception:
        return None


def _try_runtime_cast(op_name: str, src: LazyBuffer, dst_dtype: DType) -> int | None:
    op_code = _CAST_OP_CODES.get(op_name)
    src_handle = _ensure_runtime_handle(src)
    if op_code is None or src_handle is None or dst_dtype.code is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_cast(op_code, src_handle, dst_dtype.code))
    except Exception:
        return None


def _try_runtime_reduce(op_name: str, src: LazyBuffer, axis: int) -> int | None:
    op_code = _REDUCE_OP_CODES.get(op_name)
    src_handle = _ensure_runtime_handle(src)
    if op_code is None or src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_reduce(op_code, src_handle, axis))
    except Exception:
        return None


def _try_runtime_reduce_all(op_name: str, src: LazyBuffer, out_shape: tuple) -> int | None:
    op_code = _REDUCE_OP_CODES.get(op_name)
    src_handle = _ensure_runtime_handle(src)
    if op_code is None or src_handle is None:
        return None
    try:
        handle = _runtime_handle_or_none(_gpu_reduce_all(op_code, src_handle))
        if handle is None:
            return None
        if out_shape == (1,):
            return handle
        return _runtime_handle_or_none(_gpu_reshape(handle, out_shape))
    except Exception:
        return None


def _try_runtime_reshape(src: LazyBuffer, shape: tuple) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_reshape(src_handle, shape))
    except Exception:
        return None


def _try_runtime_expand(src: LazyBuffer, shape: tuple) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_expand(src_handle, shape))
    except Exception:
        return None


def _try_runtime_permute(src: LazyBuffer, order: tuple) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_permute(src_handle, order))
    except Exception:
        return None


def _try_runtime_pad(src: LazyBuffer, padding: tuple) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_pad(src_handle, _flatten_pairs(padding)))
    except Exception:
        return None


def _try_runtime_shrink(src: LazyBuffer, bounds: tuple) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_shrink(src_handle, _flatten_pairs(bounds)))
    except Exception:
        return None


def _try_runtime_flip(src: LazyBuffer, axis: int) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_flip(src_handle, axis))
    except Exception:
        return None


def _try_runtime_contiguous(src: LazyBuffer) -> int | None:
    src_handle = _ensure_runtime_handle(src)
    if src_handle is None:
        return None
    try:
        return _runtime_handle_or_none(_gpu_contiguous(src_handle))
    except Exception:
        return None


def _raise_if_handle_backed_runtime_failed(src: LazyBuffer, op_name: str) -> None:
    if src.handle is not None:
        raise RuntimeError(f"molt GPU runtime {op_name} failed")


def _raise_if_any_handle_backed_runtime_failed(srcs: tuple, op_name: str) -> None:
    for src in srcs:
        _raise_if_handle_backed_runtime_failed(src, op_name)


def _normalize_pairs(pairs, ndim: int, op_name: str) -> tuple:
    normalized = tuple((int(start), int(end)) for start, end in pairs)
    if len(normalized) != ndim:
        raise ValueError(f"{op_name} requires {ndim} pairs, got {normalized}")
    for start, end in normalized:
        if start < 0 or end < 0:
            raise ValueError(f"{op_name} pairs must be non-negative, got {normalized}")
    return normalized


def _flatten_pairs(pairs: tuple) -> tuple:
    return tuple(value for pair in pairs for value in pair)


def _read_runtime_values(handle: int, expected_dtype: DType, shape: tuple) -> list:
    realized = _gpu_realize(handle)
    if realized not in (0, None):
        raise RuntimeError("molt GPU tensor realization failed")

    dtype_code = _gpu_dtype(handle)
    dtype = _CODE_TO_DTYPE.get(dtype_code, expected_dtype)
    nbytes = _gpu_nbytes(handle)
    if not isinstance(nbytes, int) or nbytes == _INVALID_HANDLE:
        nbytes = _expected_storage_nbytes(shape, dtype)

    raw = bytearray(nbytes)
    if dtype.code is None:
        raise RuntimeError("runtime tensor readback requires a canonical dtype code")
    read = _gpu_read_raw(handle, dtype.code, raw, nbytes)
    if read != nbytes:
        raise RuntimeError("molt GPU tensor raw readback failed")
    return _unpack_typed_values(raw, dtype)


def _realize_host_values(lazydata: LazyBuffer) -> list:
    if lazydata._data is not None:
        return list(lazydata._data)
    if lazydata.handle is not None:
        values = _read_runtime_values(lazydata.handle, lazydata.dtype, lazydata.shape)
        lazydata._data = values
        return list(values)
    return tinygrad.realize.realize(lazydata)


def _lazy_load_from_values(values: list, dtype: DType, shape: tuple) -> LazyBuffer:
    op = LazyOp("LOAD", (), arg=None, dtype=dtype, shape=shape)
    handle = _try_create_runtime_tensor(values, dtype, shape)
    return LazyBuffer(op, dtype, shape, data=list(values), handle=handle)


def _resolve_shape(args) -> tuple:
    """Resolve shape from *args: (3, 4) or ((3, 4),)."""
    if len(args) == 1 and isinstance(args[0], (list, tuple)):
        return tuple(args[0])
    return tuple(args)


def _flatten_data(data) -> tuple:
    """Flatten nested lists/tuples into (flat_list, shape)."""
    if isinstance(data, (bytes, bytearray, memoryview)):
        raw = bytes(data)
        return [int(b) for b in raw], (len(raw),), dtypes.uint8
    if isinstance(data, bool):
        return [data], (1,), dtypes.bool_
    if isinstance(data, (int, float)):
        return [data], (1,), None
    if not isinstance(data, (list, tuple)):
        return [data], (1,), None

    shape = []
    current = data
    while isinstance(current, (list, tuple)):
        shape.append(len(current))
        if len(current) == 0:
            break
        current = current[0]

    flat = []
    _flatten_recursive(data, flat)
    return flat, tuple(shape), None


def _flatten_recursive(data, out: list) -> None:
    if isinstance(data, (bool, int, float)):
        out.append(data)
    elif isinstance(data, (list, tuple)):
        for item in data:
            _flatten_recursive(item, out)
    else:
        out.append(data)


def _unflatten_data(flat: list, shape: tuple) -> list:
    """Reshape flat list back to nested list matching shape."""
    if len(shape) == 0:
        return flat[0] if flat else 0.0
    if len(shape) == 1:
        return list(flat)

    size = shape[0]
    inner_size = 1
    for s in shape[1:]:
        inner_size *= s

    result = []
    for i in range(size):
        start = i * inner_size
        chunk = flat[start : start + inner_size]
        result.append(_unflatten_data(chunk, shape[1:]))
    return result


def _broadcast_shape(a: tuple, b: tuple) -> tuple:
    """Compute broadcast output shape."""
    if a == b:
        return a
    ndim = max(len(a), len(b))
    a_padded = (1,) * (ndim - len(a)) + a
    b_padded = (1,) * (ndim - len(b)) + b
    result = []
    for sa, sb in zip(a_padded, b_padded):
        if sa == sb:
            result.append(sa)
        elif sa == 1:
            result.append(sb)
        elif sb == 1:
            result.append(sa)
        else:
            raise ValueError(f"Cannot broadcast shapes {a} and {b}")
    return tuple(result)


def _numel(shape: tuple) -> int:
    total = 1
    for size in shape:
        total *= size
    return total


def _make_tuple(value, size: int, name: str) -> tuple:
    if isinstance(value, int):
        return (value,) * size
    if isinstance(value, (list, tuple)):
        out = tuple(int(v) for v in value)
        if len(out) != size:
            raise ValueError(f"{name} must have {size} values, got {out}")
        return out
    raise TypeError(f"{name} must be an int or tuple")


def _resolve_pool_pads(padding, spatial_ndim: int) -> tuple:
    if isinstance(padding, int):
        return tuple((padding, padding) for _ in range(spatial_ndim))
    if not isinstance(padding, (list, tuple)):
        raise TypeError("padding must be an int or tuple")
    padding = tuple(int(p) for p in padding)
    if len(padding) == spatial_ndim:
        return tuple((p, p) for p in padding)
    if len(padding) == spatial_ndim * 2:
        pairs = tuple((padding[i], padding[i + 1]) for i in range(0, len(padding), 2))
        return tuple(reversed(pairs))
    raise ValueError(
        f"padding must have length {spatial_ndim} or {spatial_ndim * 2}, got {padding}"
    )


def _strides_for_shape(shape: tuple) -> tuple:
    strides = []
    stride = 1
    for size in reversed(shape):
        strides.append(stride)
        stride *= size
    return tuple(reversed(strides))


def _flat_index(indices: tuple, strides: tuple) -> int:
    out = 0
    for idx, stride in zip(indices, strides):
        out += idx * stride
    return out


def _iter_indices(shape: tuple):
    if not shape:
        yield ()
        return
    for idx in range(shape[0]):
        for rest in _iter_indices(shape[1:]):
            yield (idx, *rest)


def _permute_data(flat: list, shape: tuple, order: tuple) -> list:
    """Permute data according to axis order."""
    ndim = len(shape)
    numel = len(flat)
    new_shape = tuple(shape[i] for i in order)
    result = [0.0] * numel

    for idx in range(numel):
        # Convert flat index to multi-dim index in original shape
        indices = []
        remaining = idx
        for d in range(ndim - 1, -1, -1):
            indices.append(remaining % shape[d])
            remaining //= shape[d]
        indices.reverse()

        # Permute indices
        new_indices = [indices[order[d]] for d in range(ndim)]

        # Convert back to flat index in new shape
        new_idx = 0
        stride = 1
        for d in range(ndim - 1, -1, -1):
            new_idx += new_indices[d] * stride
            stride *= new_shape[d]

        result[new_idx] = flat[idx]

    return result


def _expand_data(flat: list, shape: tuple, new_shape: tuple) -> list:
    """Expand/broadcast data to new shape."""
    if shape == new_shape:
        return list(flat)

    ndim = len(new_shape)
    old_ndim = len(shape)
    # Pad shape with leading 1s
    padded = (1,) * (ndim - old_ndim) + shape

    numel = 1
    for s in new_shape:
        numel *= s
    result = [0.0] * numel

    for idx in range(numel):
        remaining = idx
        src_idx = 0
        src_stride = 1
        for d in range(ndim - 1, -1, -1):
            dim_idx = remaining % new_shape[d]
            remaining //= new_shape[d]
            if padded[d] == 1:
                pass  # broadcast: always index 0
            else:
                src_idx += dim_idx * src_stride
            if padded[d] != 1:
                src_stride *= padded[d]

        if src_idx < len(flat):
            result[idx] = flat[src_idx]

    return result


def _cat_data(all_data: list, shapes: list, dim: int) -> list:
    """Concatenate data along a dimension."""
    ndim = len(shapes[0])
    out_shape = list(shapes[0])
    for i in range(1, len(shapes)):
        out_shape[dim] += shapes[i][dim]

    out_numel = 1
    for s in out_shape:
        out_numel *= s
    result = [0.0] * out_numel

    # For each element in output, find which input tensor it comes from
    for idx in range(out_numel):
        # Decompose index
        indices = []
        remaining = idx
        for d in range(ndim - 1, -1, -1):
            indices.append(remaining % out_shape[d])
            remaining //= out_shape[d]
        indices.reverse()

        # Find which tensor this index belongs to along dim
        dim_idx = indices[dim]
        tensor_idx = 0
        offset = 0
        for t in range(len(shapes)):
            if dim_idx < offset + shapes[t][dim]:
                tensor_idx = t
                break
            offset += shapes[t][dim]

        # Compute source index
        src_indices = list(indices)
        src_indices[dim] = dim_idx - offset
        src_shape = shapes[tensor_idx]
        src_flat_idx = 0
        stride = 1
        for d in range(ndim - 1, -1, -1):
            src_flat_idx += src_indices[d] * stride
            stride *= src_shape[d]

        result[idx] = all_data[tensor_idx][src_flat_idx]

    return result


def _shrink_data(flat: list, shape: tuple, bounds: list) -> list:
    """Extract sub-region from flat data."""
    ndim = len(shape)
    new_shape = tuple(e - s for s, e in bounds)
    numel = 1
    for s in new_shape:
        numel *= s
    result = [0.0] * numel

    for idx in range(numel):
        # Decompose output index
        remaining = idx
        src_idx = 0
        src_stride = 1
        for d in range(ndim - 1, -1, -1):
            dim_idx = remaining % new_shape[d]
            remaining //= new_shape[d]
            src_dim_idx = dim_idx + bounds[d][0]
            src_idx += src_dim_idx * src_stride
            src_stride *= shape[d]

        result[idx] = flat[src_idx]

    return result


def _flip_data(flat: list, shape: tuple, axis: int) -> list:
    """Flip data along an axis."""
    ndim = len(shape)
    numel = len(flat)
    result = [0.0] * numel

    for idx in range(numel):
        remaining = idx
        src_idx = 0
        src_stride = 1
        for d in range(ndim - 1, -1, -1):
            dim_idx = remaining % shape[d]
            remaining //= shape[d]
            if d == axis:
                dim_idx = shape[d] - 1 - dim_idx
            src_idx += dim_idx * src_stride
            src_stride *= shape[d]

        result[idx] = flat[src_idx]

    return result


def _copy_with_padding(
    src: list, src_shape: tuple, dst: list, dst_shape: tuple, padding: list
) -> None:
    """Copy src data into dst with padding offsets."""
    ndim = len(src_shape)
    src_numel = len(src)

    for idx in range(src_numel):
        # Decompose source index
        remaining = idx
        dst_idx = 0
        dst_stride = 1
        for d in range(ndim - 1, -1, -1):
            dim_idx = remaining % src_shape[d]
            remaining //= src_shape[d]
            dst_dim_idx = dim_idx + padding[d][0]
            dst_idx += dst_dim_idx * dst_stride
            dst_stride *= dst_shape[d]

        dst[dst_idx] = src[idx]


def _gather_data(
    src: list, src_shape: tuple, idx_data: list, idx_shape: tuple, dim: int
) -> list:
    """Gather elements from src along dim using index."""
    ndim = len(src_shape)
    numel = len(idx_data)
    result = [0.0] * numel

    for i in range(numel):
        # Decompose index
        remaining = i
        indices = [0] * ndim
        for d in range(ndim - 1, -1, -1):
            indices[d] = remaining % idx_shape[d]
            remaining //= idx_shape[d]

        # Replace dim index with gathered index
        indices[dim] = int(idx_data[i])

        # Compute flat source index
        src_idx = 0
        stride = 1
        for d in range(ndim - 1, -1, -1):
            src_idx += indices[d] * stride
            stride *= src_shape[d]

        result[i] = src[src_idx]

    return result


def _scatter_data(
    dst: list, dst_shape: tuple, src: list, idx_data: list, idx_shape: tuple, dim: int
) -> None:
    """Scatter src elements into dst along dim using index."""
    ndim = len(dst_shape)
    numel = len(idx_data)

    for i in range(numel):
        remaining = i
        indices = [0] * ndim
        for d in range(ndim - 1, -1, -1):
            indices[d] = remaining % idx_shape[d]
            remaining //= idx_shape[d]

        indices[dim] = int(idx_data[i])

        dst_idx = 0
        stride = 1
        for d in range(ndim - 1, -1, -1):
            dst_idx += indices[d] * stride
            stride *= dst_shape[d]

        dst[dst_idx] = src[i]
