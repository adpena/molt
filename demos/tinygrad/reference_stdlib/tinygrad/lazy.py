"""
tinygrad.lazy — LazyOp DAG construction.

Each Tensor operation creates a LazyOp node instead of executing immediately.
The DAG is materialized on realize() via schedule -> fuse -> render -> execute.
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from tinygrad.dtypes import DType, dtypes


class LazyOp:
    """A single node in the lazy computation DAG."""

    __slots__ = ("op", "srcs", "arg", "dtype", "shape", "_realized")

    def __init__(
        self,
        op: str,
        srcs: tuple,
        arg: object = None,
        dtype: DType = None,
        shape: tuple = (),
    ) -> None:
        self.op = op
        self.srcs = srcs
        self.arg = arg
        self.dtype = dtype or dtypes.float32
        self.shape = shape
        self._realized: object = None

    @property
    def realized(self) -> bool:
        return self._realized is not None

    def __repr__(self) -> str:
        src_strs = ", ".join(repr(s) for s in self.srcs)
        return f"LazyOp({self.op}, [{src_strs}], {self.arg})"


class LazyBuffer:
    """A lazy buffer backed by a LazyOp or realized data.

    This is the internal representation that Tensor wraps.
    """

    __slots__ = ("op", "dtype", "shape", "_data", "_handle")

    def __init__(
        self,
        op: LazyOp | None,
        dtype: DType,
        shape: tuple,
        data: list | None = None,
        handle: int | None = None,
    ) -> None:
        self.op = op
        self.dtype = dtype
        self.shape = shape
        self._data = data
        self._handle = handle

    @property
    def realized(self) -> bool:
        return self._data is not None or self._handle is not None

    @property
    def handle(self) -> int | None:
        return self._handle

    @property
    def numel(self) -> int:
        result = 1
        for s in self.shape:
            result *= s
        return result

    def realize(self) -> list:
        """Materialize this buffer by executing the LazyOp DAG."""
        if self._data is not None:
            return self._data
        if self._handle is not None:
            raise RuntimeError("handle-only LazyBuffer requires typed tensor readback")
        if self.op is None:
            raise RuntimeError("LazyBuffer has no op and no data")
        self._data = _execute_lazy_op(self.op, self.shape)
        return self._data

    def __repr__(self) -> str:
        status = "realized" if self.realized else "lazy"
        return f"LazyBuffer(shape={self.shape}, dtype={self.dtype}, {status})"


def _execute_lazy_op(op: LazyOp, shape: tuple) -> list:
    """Execute a LazyOp DAG recursively on CPU.

    This is the reference CPU executor. GPU execution goes through
    the molt-gpu Rust FFI path instead.
    """
    # Realize all source buffers first
    src_data = []
    src_shapes = []
    for src in op.srcs:
        if isinstance(src, LazyBuffer):
            src_data.append(src.realize())
            src_shapes.append(src.shape)
        elif isinstance(src, (int, float)):
            src_data.append(src)
            src_shapes.append(())
        else:
            src_data.append(src)
            src_shapes.append(())

    numel = 1
    for s in shape:
        numel *= s

    return _dispatch_op(op.op, src_data, src_shapes, op.arg, numel, shape)


# Element-wise binary kernels for the CPU reference executor. Operands are
# already broadcast to the output extent before these are applied.
_BINARY_OPS = {
    "ADD": lambda a, b: a + b,
    "SUB": lambda a, b: a - b,
    "MUL": lambda a, b: a * b,
    "IDIV": lambda a, b: int(a) // int(b) if int(b) != 0 else 0,
    "MOD": lambda a, b: int(a) % int(b) if int(b) != 0 else 0,
    "MAX": max,
    "CMPLT": lambda a, b: 1.0 if a < b else 0.0,
    "CMPEQ": lambda a, b: 1.0 if a == b else 0.0,
    "CMPNE": lambda a, b: 1.0 if a != b else 0.0,
    "AND": lambda a, b: float(int(a) & int(b)),
    "OR": lambda a, b: float(int(a) | int(b)),
    "XOR": lambda a, b: float(int(a) ^ int(b)),
    "SHL": lambda a, b: float(int(a) << int(b)),
    "SHR": lambda a, b: float(int(a) >> int(b)),
}


def _dispatch_op(
    op_name: str,
    srcs: list,
    src_shapes: list,
    arg: object,
    numel: int,
    shape: tuple,
) -> list:
    """Dispatch a single op on CPU data."""
    import math

    if op_name == "CONST":
        return [arg] * numel

    if op_name == "LOAD":
        return srcs[0] if isinstance(srcs[0], list) else list(srcs[0])

    # Unary ops
    if op_name == "NEG":
        return [-x for x in srcs[0]]
    if op_name == "EXP2":
        return [2.0**x for x in srcs[0]]
    if op_name == "LOG2":
        return [math.log2(x) if x > 0 else float("-inf") for x in srcs[0]]
    if op_name == "SIN":
        return [math.sin(x) for x in srcs[0]]
    if op_name == "SQRT":
        return [math.sqrt(x) if x >= 0 else float("nan") for x in srcs[0]]
    if op_name == "RECIPROCAL":
        return [1.0 / x if x != 0 else float("inf") for x in srcs[0]]
    if op_name == "TRUNC":
        return [math.trunc(x) for x in srcs[0]]
    if op_name == "CAST":
        return list(srcs[0])  # CPU reference: no actual type conversion

    # Binary ops. Operands are broadcast to the output ``shape`` using
    # NumPy-style right-aligned broadcasting (size-1 / missing leading dims are
    # stretched). ``src_shapes`` carries each operand's shape so that
    # multi-axis broadcasts such as ``(1, C, 1, 1)`` against ``(N, C, H, W)``
    # (e.g. GroupNorm/BatchNorm affine) resolve correctly in this CPU reference.
    binary_fn = _BINARY_OPS.get(op_name)
    if binary_fn is not None:
        a, b = _broadcast_pair(
            srcs[0], srcs[1], numel, src_shapes[0], src_shapes[1], shape
        )
        return [binary_fn(a[i], b[i]) for i in range(numel)]

    # Ternary
    if op_name == "WHERE":
        cond = srcs[0]
        a, b = srcs[1], srcs[2]
        return [a[i] if cond[i] != 0 else b[i] for i in range(numel)]

    # Reduce ops
    if op_name == "REDUCE_SUM":
        return _reduce_op(srcs[0], arg, src_shapes[0], lambda acc, x: acc + x, 0.0)
    if op_name == "REDUCE_MAX":
        return _reduce_op(
            srcs[0],
            arg,
            src_shapes[0],
            lambda acc, x: max(acc, x),
            float("-inf"),
        )

    raise ValueError(f"Unknown op: {op_name}")


def _strides_for(shape: tuple) -> list:
    """Row-major (C-contiguous) strides for ``shape``."""
    strides = [1] * len(shape)
    acc = 1
    for i in range(len(shape) - 1, -1, -1):
        strides[i] = acc
        acc *= shape[i]
    return strides


def _broadcast_to_shape(data, src_shape: tuple, out_shape: tuple, out_numel: int):
    """Expand ``data`` (flat, row-major over ``src_shape``) to ``out_shape``.

    Implements NumPy-style right-aligned broadcasting: source dims of size 1 (or
    absent leading dims) are stretched by using a stride of 0 along that axis.
    Returns a flat list of length ``out_numel``.
    """
    if not out_shape:
        # Scalar output.
        return [float(data[0]) if not isinstance(data, (int, float)) else float(data)]

    ndim = len(out_shape)
    src_shape = tuple(src_shape)
    # Right-align the source shape against the output shape.
    padded = (1,) * (ndim - len(src_shape)) + src_shape
    base_strides = _strides_for(padded)
    # Zero out strides on broadcast (size-1) source axes.
    eff_strides = [
        0 if padded[axis] == 1 and out_shape[axis] != 1 else base_strides[axis]
        for axis in range(ndim)
    ]

    out = [0.0] * out_numel
    coord = [0] * ndim
    src_index = 0
    for flat in range(out_numel):
        out[flat] = data[src_index]
        # Increment the mixed-radix coordinate (last axis fastest) and update
        # the source index by the effective (broadcast-aware) strides.
        axis = ndim - 1
        while axis >= 0:
            coord[axis] += 1
            src_index += eff_strides[axis]
            if coord[axis] < out_shape[axis]:
                break
            coord[axis] = 0
            src_index -= eff_strides[axis] * out_shape[axis]
            axis -= 1
    return out


def _broadcast_pair(
    a,
    b,
    numel: int,
    a_shape: tuple = None,
    b_shape: tuple = None,
    out_shape: tuple = None,
) -> tuple:
    """Broadcast two operands to the output extent (``numel`` / ``out_shape``).

    Scalars and exact-length / length-1 operands take the fast path. When the
    operand and output shapes are known and differ by more than a trailing
    size-1 axis, full NumPy-style broadcasting is applied via
    :func:`_broadcast_to_shape`.
    """

    def _expand(value, value_shape):
        if isinstance(value, (int, float)):
            return [float(value)] * numel
        if len(value) == numel:
            return value
        if len(value) == 1:
            return value * numel
        if out_shape is not None and value_shape is not None:
            return _broadcast_to_shape(value, value_shape, out_shape, numel)
        # No shape information: fall back to repetition when evenly divisible.
        if numel % len(value) == 0:
            return value * (numel // len(value))
        raise ValueError(f"cannot broadcast operand of length {len(value)} to {numel}")

    return _expand(a, a_shape), _expand(b, b_shape)


def _reduce_op(
    data: list,
    axis: int | None,
    shape: tuple,
    fn,
    init: float,
) -> list:
    """Perform a reduction along an axis."""
    if axis is None:
        result = init
        for x in data:
            result = fn(result, x)
        return [result]

    ndim = len(shape)
    if axis < 0:
        axis = ndim + axis

    # Compute output shape
    out_shape = list(shape)
    reduce_size = out_shape[axis]
    out_shape[axis] = 1

    out_numel = 1
    for s in out_shape:
        out_numel *= s

    result = []
    # For each output element, reduce along the axis
    stride_after = 1
    for i in range(axis + 1, ndim):
        stride_after *= shape[i]
    stride_at = reduce_size * stride_after

    for out_idx in range(out_numel):
        # Map output index to input base index
        outer = out_idx // stride_after
        inner = out_idx % stride_after
        base = outer * stride_at + inner

        acc = init
        for r in range(reduce_size):
            idx = base + r * stride_after
            acc = fn(acc, data[idx])
        result.append(acc)

    return result
