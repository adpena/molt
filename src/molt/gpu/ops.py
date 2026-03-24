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

from . import Buffer, to_device, from_device, alloc, kernel, thread_id
import struct
import math


def map(func, buf: Buffer, dtype: type = None) -> Buffer:
    """Apply func to each element of buf, returning a new Buffer.

    Like numpy.vectorize but runs on GPU when compiled.

    Example:
        result = gpu.ops.map(lambda x: x * 2.0, data)
    """
    dtype = dtype or buf.element_type
    n = buf.size
    result = alloc(n, dtype)

    # Interpreted fallback: apply element-wise on CPU
    fmt = 'd' if buf.element_type == float else 'q'
    out_fmt = 'd' if dtype == float else 'q'
    for i in range(n):
        val = struct.unpack_from(fmt, buf._data, i * 8)[0]
        out_val = func(val)
        if isinstance(result._data, bytes):
            result._data = bytearray(result._data)
        struct.pack_into(out_fmt, result._data, i * 8, out_val)

    return result


def reduce(buf: Buffer, op: str = 'sum', initial=None):
    """Reduce a buffer to a scalar value.

    Operations: 'sum', 'product', 'min', 'max', 'mean'

    Example:
        total = gpu.ops.reduce(data, 'sum')
        maximum = gpu.ops.reduce(data, 'max')
    """
    n = buf.size
    if n == 0:
        return initial or 0

    fmt = 'd' if buf.element_type == float else 'q'
    values = [struct.unpack_from(fmt, buf._data, i * 8)[0] for i in range(n)]

    if op == 'sum':
        return sum(values) if initial is None else sum(values, initial)
    elif op == 'product':
        result = initial if initial is not None else 1
        for v in values:
            result *= v
        return result
    elif op == 'min':
        return min(values) if initial is None else min(min(values), initial)
    elif op == 'max':
        return max(values) if initial is None else max(max(values), initial)
    elif op == 'mean':
        return sum(values) / n
    else:
        raise ValueError(f"Unknown reduce operation: {op}")


def filter(pred, buf: Buffer) -> Buffer:
    """Select elements where pred(element) is true.

    Example:
        positives = gpu.ops.filter(lambda x: x > 0, data)
    """
    fmt = 'd' if buf.element_type == float else 'q'
    selected = []
    for i in range(buf.size):
        val = struct.unpack_from(fmt, buf._data, i * 8)[0]
        if pred(val):
            selected.append(val)

    if not selected:
        return alloc(0, buf.element_type)

    result = alloc(len(selected), buf.element_type)
    result._data = bytearray(result._data)
    for i, val in enumerate(selected):
        struct.pack_into(fmt, result._data, i * 8, val)
    result._size = len(selected)
    return result


def scan(buf: Buffer, op: str = 'sum', exclusive: bool = False) -> Buffer:
    """Prefix scan (cumulative operation).

    Inclusive: scan([1,2,3,4], 'sum') -> [1,3,6,10]
    Exclusive: scan([1,2,3,4], 'sum', exclusive=True) -> [0,1,3,6]

    Operations: 'sum', 'product', 'min', 'max'
    """
    n = buf.size
    if n == 0:
        return alloc(0, buf.element_type)

    fmt = 'd' if buf.element_type == float else 'q'
    values = [struct.unpack_from(fmt, buf._data, i * 8)[0] for i in range(n)]

    identity = {'sum': 0, 'product': 1, 'min': float('inf'), 'max': float('-inf')}
    combine = {
        'sum': lambda a, b: a + b,
        'product': lambda a, b: a * b,
        'min': lambda a, b: min(a, b),
        'max': lambda a, b: max(a, b),
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

    result = alloc(n, buf.element_type)
    result._data = bytearray(result._data)
    for i, val in enumerate(result_vals):
        struct.pack_into(fmt, result._data, i * 8, val if isinstance(val, (int, float)) else float(val))

    return result


def gather(buf: Buffer, indices: Buffer) -> Buffer:
    """Indexed read: result[i] = buf[indices[i]].

    Example:
        selected = gpu.ops.gather(data, index_buf)
    """
    n = indices.size
    result = alloc(n, buf.element_type)
    result._data = bytearray(result._data)
    val_fmt = 'd' if buf.element_type == float else 'q'
    idx_fmt = 'q'  # indices are always int

    for i in range(n):
        idx = struct.unpack_from(idx_fmt, indices._data, i * 8)[0]
        val = struct.unpack_from(val_fmt, buf._data, idx * 8)[0]
        struct.pack_into(val_fmt, result._data, i * 8, val)

    return result


def scatter(buf: Buffer, indices: Buffer, values: Buffer) -> Buffer:
    """Indexed write: result = copy(buf); result[indices[i]] = values[i].

    Example:
        updated = gpu.ops.scatter(data, index_buf, new_values)
    """
    result = alloc(buf.size, buf.element_type)
    result._data = bytearray(buf._data)  # copy source
    val_fmt = 'd' if buf.element_type == float else 'q'
    idx_fmt = 'q'

    n = min(indices.size, values.size)
    for i in range(n):
        idx = struct.unpack_from(idx_fmt, indices._data, i * 8)[0]
        val = struct.unpack_from(val_fmt, values._data, i * 8)[0]
        struct.pack_into(val_fmt, result._data, idx * 8, val)

    return result


def where(cond: Buffer, a: Buffer, b: Buffer) -> Buffer:
    """Element-wise conditional select: result[i] = a[i] if cond[i] else b[i].

    Example:
        result = gpu.ops.where(mask, data_a, data_b)
    """
    n = min(cond.size, a.size, b.size)
    result = alloc(n, a.element_type)
    result._data = bytearray(result._data)
    cond_fmt = 'q'  # condition as int (0 = false, nonzero = true)
    val_fmt = 'd' if a.element_type == float else 'q'

    for i in range(n):
        c = struct.unpack_from(cond_fmt, cond._data, i * 8)[0]
        val = struct.unpack_from(val_fmt, (a if c else b)._data, i * 8)[0]
        struct.pack_into(val_fmt, result._data, i * 8, val)

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
    if dtype == float:
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
    fmt = 'd' if a.element_type == float else 'q'
    total = 0.0
    for i in range(n):
        va = struct.unpack_from(fmt, a._data, i * 8)[0]
        vb = struct.unpack_from(fmt, b._data, i * 8)[0]
        total += va * vb
    return total


def norm(buf: Buffer, ord: int = 2) -> float:
    """Vector norm.

    ord=1: L1 norm (sum of absolute values)
    ord=2: L2 norm (Euclidean distance)
    ord=inf: Linf norm (max absolute value)
    """
    fmt = 'd' if buf.element_type == float else 'q'
    values = [struct.unpack_from(fmt, buf._data, i * 8)[0] for i in range(buf.size)]

    if ord == 1:
        return sum(abs(v) for v in values)
    elif ord == 2:
        return math.sqrt(sum(v * v for v in values))
    elif ord == float('inf'):
        return max(abs(v) for v in values) if values else 0.0
    else:
        return sum(abs(v) ** ord for v in values) ** (1.0 / ord)
