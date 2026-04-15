"""
tinygrad.lazy — LazyOp DAG construction.

Each Tensor operation creates a LazyOp node instead of executing immediately.
The DAG is materialized on realize() via schedule -> fuse -> render -> execute.
"""

from __future__ import annotations

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

    __slots__ = ("op", "dtype", "shape", "_data")

    def __init__(
        self,
        op: LazyOp | None,
        dtype: DType,
        shape: tuple,
        data: list | None = None,
    ) -> None:
        self.op = op
        self.dtype = dtype
        self.shape = shape
        self._data = data

    @property
    def realized(self) -> bool:
        return self._data is not None

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
    for src in op.srcs:
        if isinstance(src, LazyBuffer):
            src_data.append(src.realize())
        elif isinstance(src, (int, float)):
            src_data.append(src)
        else:
            src_data.append(src)

    numel = 1
    for s in shape:
        numel *= s

    return _dispatch_op(op.op, src_data, op.arg, numel, shape)


def _dispatch_op(
    op_name: str,
    srcs: list,
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
        return [2.0 ** x for x in srcs[0]]
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

    # Binary ops
    if op_name == "ADD":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [a[i] + b[i] for i in range(numel)]
    if op_name == "SUB":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [a[i] - b[i] for i in range(numel)]
    if op_name == "MUL":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [a[i] * b[i] for i in range(numel)]
    if op_name == "IDIV":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [int(a[i]) // int(b[i]) if int(b[i]) != 0 else 0 for i in range(numel)]
    if op_name == "MOD":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [int(a[i]) % int(b[i]) if int(b[i]) != 0 else 0 for i in range(numel)]
    if op_name == "MAX":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [max(a[i], b[i]) for i in range(numel)]
    if op_name == "CMPLT":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [1.0 if a[i] < b[i] else 0.0 for i in range(numel)]
    if op_name == "CMPEQ":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [1.0 if a[i] == b[i] else 0.0 for i in range(numel)]
    if op_name == "CMPNE":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [1.0 if a[i] != b[i] else 0.0 for i in range(numel)]
    if op_name == "AND":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [float(int(a[i]) & int(b[i])) for i in range(numel)]
    if op_name == "OR":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [float(int(a[i]) | int(b[i])) for i in range(numel)]
    if op_name == "XOR":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [float(int(a[i]) ^ int(b[i])) for i in range(numel)]
    if op_name == "SHL":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [float(int(a[i]) << int(b[i])) for i in range(numel)]
    if op_name == "SHR":
        a, b = _broadcast_pair(srcs[0], srcs[1], numel)
        return [float(int(a[i]) >> int(b[i])) for i in range(numel)]

    # Ternary
    if op_name == "WHERE":
        cond = srcs[0]
        a, b = srcs[1], srcs[2]
        return [a[i] if cond[i] != 0 else b[i] for i in range(numel)]

    # Reduce ops
    if op_name == "REDUCE_SUM":
        return _reduce_op(srcs[0], arg, shape, lambda acc, x: acc + x, 0.0)
    if op_name == "REDUCE_MAX":
        return _reduce_op(srcs[0], arg, shape, lambda acc, x: max(acc, x), float("-inf"))

    raise ValueError(f"Unknown op: {op_name}")


def _broadcast_pair(a: list | float, b: list | float, numel: int) -> tuple:
    """Broadcast a scalar or list to match numel."""
    if isinstance(a, (int, float)):
        a = [float(a)] * numel
    if isinstance(b, (int, float)):
        b = [float(b)] * numel
    if len(a) == 1 and numel > 1:
        a = a * numel
    if len(b) == 1 and numel > 1:
        b = b * numel
    return a, b


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
