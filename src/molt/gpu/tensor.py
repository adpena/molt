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

import math
import struct
from . import Buffer, alloc, to_device, from_device


def _product(seq):
    """Product of a sequence of integers."""
    result = 1
    for x in seq:
        result *= x
    return result


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

    @property
    def T(self) -> 'Tensor':
        """Transpose (property alias)."""
        return self.transpose()

    def flatten(self) -> 'Tensor':
        """Return a 1D view of the tensor."""
        return self.reshape(self.size)

    # ── Elementwise arithmetic ────────────────────────────────────────

    def _broadcast_op(self, other, op):
        """Apply a binary op with scalar or tensor broadcasting."""
        if isinstance(other, (int, float)):
            data = self._data_list()
            return self._from_flat([op(x, float(other)) for x in data], self._shape)
        if not isinstance(other, Tensor):
            return NotImplemented

        a_data = self._data_list()
        b_data = other._data_list()

        # Same shape — elementwise
        if self._shape == other._shape:
            return self._from_flat(
                [op(a_data[i], b_data[i]) for i in range(len(a_data))],
                self._shape,
            )

        # Broadcast: one of them is a scalar
        if other.size == 1:
            s = b_data[0]
            return self._from_flat([op(x, s) for x in a_data], self._shape)
        if self.size == 1:
            s = a_data[0]
            return other._from_flat([op(s, x) for x in b_data], other._shape)

        # Broadcast: trailing dimensions match (simple case)
        if self.ndim >= other.ndim:
            big, small = self, other
            big_data, small_data = a_data, b_data
        else:
            big, small = other, self
            big_data, small_data = b_data, a_data

        small_size = small.size
        if big.size % small_size == 0:
            result = [0.0] * big.size
            for i in range(big.size):
                result[i] = op(big_data[i], small_data[i % small_size])
            return self._from_flat(result, big._shape)

        raise ValueError(
            f"Cannot broadcast shapes {self._shape} and {other._shape}"
        )

    def __add__(self, other) -> 'Tensor':
        return self._broadcast_op(other, lambda a, b: a + b)

    def __radd__(self, other) -> 'Tensor':
        return self._broadcast_op(other, lambda a, b: a + b)

    def __sub__(self, other) -> 'Tensor':
        return self._broadcast_op(other, lambda a, b: a - b)

    def __rsub__(self, other) -> 'Tensor':
        if isinstance(other, (int, float)):
            data = self._data_list()
            return self._from_flat(
                [float(other) - x for x in data], self._shape
            )
        return NotImplemented

    def __mul__(self, other) -> 'Tensor':
        return self._broadcast_op(other, lambda a, b: a * b)

    def __rmul__(self, other) -> 'Tensor':
        return self._broadcast_op(other, lambda a, b: a * b)

    def __truediv__(self, other) -> 'Tensor':
        return self._broadcast_op(other, lambda a, b: a / b)

    def __rtruediv__(self, other) -> 'Tensor':
        if isinstance(other, (int, float)):
            data = self._data_list()
            return self._from_flat(
                [float(other) / x for x in data], self._shape
            )
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

        a_data = a._data_list()
        b_data = b._data_list()

        batch_a = _product(a._shape[:-2]) if a.ndim > 2 else 1
        batch_b = _product(b._shape[:-2]) if b.ndim > 2 else 1
        batches = max(batch_a, batch_b)

        result = []
        a_stride = a_rows * a_cols
        b_stride = b_rows * b_cols

        for batch in range(batches):
            a_off = (batch % batch_a) * a_stride
            b_off = (batch % batch_b) * b_stride
            for i in range(a_rows):
                for j in range(b_cols):
                    s = 0.0
                    for k in range(a_cols):
                        s += a_data[a_off + i * a_cols + k] * b_data[b_off + k * b_cols + j]
                    result.append(s)

        if batches > 1:
            out_shape = (batches, a_rows, b_cols)
        else:
            out_shape = (a_rows, b_cols)

        return self._from_flat(result, out_shape)

    # ── Reductions ────────────────────────────────────────────────────

    def _reduce(self, op, axis=None, initial=None):
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

        # Compute output shape (remove the reduction axis)
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

    def sum(self, axis=None) -> 'Tensor':
        """Sum elements, optionally along an axis."""
        return self._reduce(lambda a, b: a + b, axis=axis, initial=0.0)

    def mean(self, axis=None) -> 'Tensor':
        """Mean of elements, optionally along an axis."""
        s = self.sum(axis=axis)
        if axis is None:
            n = self.size
        else:
            if axis < 0:
                axis = self.ndim + axis
            n = self._shape[axis]
        return s / float(n)

    def max(self, axis=None) -> 'Tensor':
        """Max element, optionally along an axis."""
        return self._reduce(lambda a, b: a if a >= b else b, axis=axis)

    def min(self, axis=None) -> 'Tensor':
        """Min element, optionally along an axis."""
        return self._reduce(lambda a, b: a if a <= b else b, axis=axis)

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

        return self._from_flat(result, self._shape)

    def exp(self) -> 'Tensor':
        """Element-wise exponential."""
        data = self._data_list()
        return self._from_flat([math.exp(x) for x in data], self._shape)

    def log(self) -> 'Tensor':
        """Element-wise natural logarithm."""
        data = self._data_list()
        return self._from_flat([math.log(x) for x in data], self._shape)

    def sqrt(self) -> 'Tensor':
        """Element-wise square root."""
        data = self._data_list()
        return self._from_flat([math.sqrt(x) for x in data], self._shape)

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
            data = self._data_list()
            start = idx * sub_size
            sub_data = data[start:start + sub_size]
            if not sub_shape:
                return Tensor(sub_data[0])
            return self._from_flat(sub_data, sub_shape)
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
        seed = 42
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
        seed = 42
    state = seed
    result = []
    for _ in range(size):
        state = (state * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        result.append((state >> 11) / (1 << 53))

    return Tensor(result, shape=shape)
