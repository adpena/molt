"""
molt.gpu.ops — High-level GPU data processing operations.

RAPIDS-style API for GPU-accelerated data processing:
- map: apply a function to each element
- reduce: reduce a buffer to a scalar (sum, product, min, max)
- filter: select elements matching a predicate
- scan: prefix sum (inclusive/exclusive)
- sort: GPU radix sort
- gather/scatter: indexed read/write
- where: conditional select

These operations work on gpu.Buffer objects and dispatch to
Metal/WebGPU/CUDA GPU kernels when compiled by Molt.
In interpreted mode, they run on CPU as a fallback.
"""

from . import Buffer, to_device, alloc
import struct
import math


def map(func, buf: Buffer, dtype: type | None = None) -> Buffer:
    """Apply func to each element of buf, returning a new Buffer.

    Like numpy.vectorize but runs on GPU when compiled.

    Example:
        result = gpu.ops.map(lambda x: x * 2.0, data)
    """
    dtype = dtype or buf.element_type
    n = buf.size
    result_format = (
        buf.format_char if dtype is float and buf.element_type is float else None
    )
    result = alloc(n, dtype, format_char=result_format)

    # Interpreted fallback: apply element-wise on CPU
    fmt = buf.format_char
    out_fmt = result.format_char
    for i in range(n):
        val = struct.unpack_from(fmt, buf._data, i * buf.itemsize)[0]
        out_val = func(val)
        if isinstance(result._data, bytes):
            result._data = bytearray(result._data)
        struct.pack_into(out_fmt, result._data, i * result.itemsize, out_val)

    return result


def reduce(buf: Buffer, op: str = "sum", initial=None):
    """Reduce a buffer to a scalar value.

    Operations: 'sum', 'product', 'min', 'max', 'mean'

    Example:
        total = gpu.ops.reduce(data, 'sum')
        maximum = gpu.ops.reduce(data, 'max')
    """
    n = buf.size
    if n == 0:
        return initial if initial is not None else 0

    fmt = buf.format_char
    values = [struct.unpack_from(fmt, buf._data, i * buf.itemsize)[0] for i in range(n)]

    if op == "sum":
        return sum(values) if initial is None else sum(values, initial)
    elif op == "product":
        result = initial if initial is not None else 1
        for v in values:
            result *= v
        return result
    elif op == "min":
        return min(values) if initial is None else min(min(values), initial)
    elif op == "max":
        return max(values) if initial is None else max(max(values), initial)
    elif op == "mean":
        return sum(values) / n
    else:
        raise ValueError(f"Unknown reduce operation: {op}")


def filter(pred, buf: Buffer) -> Buffer:
    """Select elements where pred(element) is true.

    Example:
        positives = gpu.ops.filter(lambda x: x > 0, data)
    """
    fmt = buf.format_char
    selected = []
    for i in range(buf.size):
        val = struct.unpack_from(fmt, buf._data, i * buf.itemsize)[0]
        if pred(val):
            selected.append(val)

    if not selected:
        return alloc(0, buf.element_type, format_char=buf.format_char)

    result = alloc(len(selected), buf.element_type, format_char=buf.format_char)
    result._data = bytearray(result._data)
    for i, val in enumerate(selected):
        struct.pack_into(fmt, result._data, i * result.itemsize, val)
    result._size = len(selected)
    return result


def scan(buf: Buffer, op: str = "sum", exclusive: bool = False) -> Buffer:
    """Prefix scan (cumulative operation).

    Inclusive: scan([1,2,3,4], 'sum') -> [1,3,6,10]
    Exclusive: scan([1,2,3,4], 'sum', exclusive=True) -> [0,1,3,6]

    Operations: 'sum', 'product', 'min', 'max'
    """
    n = buf.size
    if n == 0:
        return alloc(0, buf.element_type, format_char=buf.format_char)

    fmt = buf.format_char
    values = [struct.unpack_from(fmt, buf._data, i * buf.itemsize)[0] for i in range(n)]

    # Use type-appropriate identity values so they can be packed into the buffer's format.
    # For int buffers, float('inf') cannot be packed as 'q'; use 2**62 instead.
    if buf.element_type is float:
        identity = {
            "sum": 0.0,
            "product": 1.0,
            "min": float("inf"),
            "max": float("-inf"),
        }
    else:
        identity = {"sum": 0, "product": 1, "min": 2**62, "max": -(2**62)}
    combine = {
        "sum": lambda a, b: a + b,
        "product": lambda a, b: a * b,
        "min": lambda a, b: min(a, b),
        "max": lambda a, b: max(a, b),
    }

    acc = identity.get(op, 0)
    fn = combine[op]
    result_vals = []

    for v in values:
        if exclusive:
            result_vals.append(acc)
            acc = fn(acc, v)
        else:
            acc = fn(acc, v)
            result_vals.append(acc)

    result = alloc(n, buf.element_type, format_char=buf.format_char)
    result._data = bytearray(result._data)
    for i, val in enumerate(result_vals):
        struct.pack_into(
            fmt,
            result._data,
            i * result.itemsize,
            val if isinstance(val, (int, float)) else float(val),
        )

    return result


def gather(buf: Buffer, indices: Buffer) -> Buffer:
    """Indexed read: result[i] = buf[indices[i]].

    Example:
        selected = gpu.ops.gather(data, index_buf)
    """
    n = indices.size
    result = alloc(n, buf.element_type, format_char=buf.format_char)
    result._data = bytearray(result._data)
    val_fmt = buf.format_char
    idx_fmt = indices.format_char

    for i in range(n):
        idx = struct.unpack_from(idx_fmt, indices._data, i * indices.itemsize)[0]
        if idx < 0 or idx >= buf.size:
            raise IndexError(f"gather index {idx} out of range [0, {buf.size})")
        val = struct.unpack_from(val_fmt, buf._data, idx * buf.itemsize)[0]
        struct.pack_into(val_fmt, result._data, i * result.itemsize, val)

    return result


def scatter(buf: Buffer, indices: Buffer, values: Buffer) -> Buffer:
    """Indexed write: result = copy(buf); result[indices[i]] = values[i].

    Example:
        updated = gpu.ops.scatter(data, index_buf, new_values)
    """
    result = alloc(buf.size, buf.element_type, format_char=buf.format_char)
    result._data = bytearray(buf._data)  # copy source
    val_fmt = (
        values.format_char
        if values.element_type == buf.element_type
        else buf.format_char
    )
    idx_fmt = indices.format_char

    n = min(indices.size, values.size)
    for i in range(n):
        idx = struct.unpack_from(idx_fmt, indices._data, i * indices.itemsize)[0]
        if idx < 0 or idx >= buf.size:
            raise IndexError(f"scatter index {idx} out of range [0, {buf.size})")
        val = struct.unpack_from(val_fmt, values._data, i * values.itemsize)[0]
        struct.pack_into(result.format_char, result._data, idx * result.itemsize, val)

    return result


def where(cond: Buffer, a: Buffer, b: Buffer) -> Buffer:
    """Element-wise conditional select: result[i] = a[i] if cond[i] else b[i].

    Note: condition buffer must contain int values (0=false, nonzero=true).
    Float conditions use the raw bit pattern.

    Example:
        result = gpu.ops.where(mask, data_a, data_b)
    """
    n = min(cond.size, a.size, b.size)
    result = alloc(n, a.element_type, format_char=a.format_char)
    result._data = bytearray(result._data)
    cond_fmt = cond.format_char

    for i in range(n):
        c = struct.unpack_from(cond_fmt, cond._data, i * cond.itemsize)[0]
        src = a if c else b
        val = struct.unpack_from(src.format_char, src._data, i * src.itemsize)[0]
        struct.pack_into(result.format_char, result._data, i * result.itemsize, val)

    return result


def arange(start, stop=None, step=1, dtype=float) -> Buffer:
    """Create a Buffer with evenly spaced values (like numpy.arange).

    Example:
        indices = gpu.ops.arange(0, 100)
        data = gpu.ops.arange(0.0, 1.0, 0.01)
    """
    if stop is None:
        stop = start
        start = 0

    values = []
    v = start
    while (step > 0 and v < stop) or (step < 0 and v > stop):
        values.append(v)
        v += step

    return to_device(values)


def zeros(n: int, dtype=float) -> Buffer:
    """Create a zero-filled Buffer."""
    return alloc(n, dtype)


def ones(n: int, dtype=float) -> Buffer:
    """Create a one-filled Buffer."""
    if dtype is float:
        return to_device([1.0] * n)
    else:
        return to_device([1] * n)


def linspace(start: float, stop: float, n: int) -> Buffer:
    """Create a Buffer with n evenly spaced values from start to stop."""
    if n <= 1:
        return to_device([start])
    step = (stop - start) / (n - 1)
    return to_device([start + i * step for i in range(n)])


def dot(a: Buffer, b: Buffer) -> float:
    """Dot product of two buffers.

    Example:
        result = gpu.ops.dot(vec_a, vec_b)
    """
    n = min(a.size, b.size)
    fmt = a.format_char
    total = 0.0
    for i in range(n):
        va = struct.unpack_from(fmt, a._data, i * a.itemsize)[0]
        vb = struct.unpack_from(b.format_char, b._data, i * b.itemsize)[0]
        total += va * vb
    return total


def norm(buf: Buffer, ord: int = 2) -> float:
    """Vector norm.

    ord=1: L1 norm (sum of absolute values)
    ord=2: L2 norm (Euclidean distance)
    ord=inf: Linf norm (max absolute value)
    """
    fmt = buf.format_char
    values = [
        struct.unpack_from(fmt, buf._data, i * buf.itemsize)[0] for i in range(buf.size)
    ]

    if ord == 1:
        return sum(abs(v) for v in values)
    elif ord == 2:
        return math.sqrt(sum(v * v for v in values))
    elif ord == float("inf"):
        return max(abs(v) for v in values) if values else 0.0
    else:
        return sum(abs(v) ** ord for v in values) ** (1.0 / ord)
