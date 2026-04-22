"""
molt.gpu.fusion — Kernel fusion for compound GPU operations.

Instead of:
  map(x*x, data) -> temp buffer -> reduce(sum, temp) -> result
  = 2 kernel launches + 1 intermediate buffer

Fused:
  fused_map_reduce(x*x, sum, data) -> result
  = 1 kernel launch + 0 intermediate buffers

This is the key optimization that can beat MLX: MLX dispatches each op
separately, but Molt can fuse chains at compile time.
"""

import struct
from . import alloc


# ── Reduce helpers ────────────────────────────────────────────────────

_REDUCE_INIT = {
    "sum": 0.0,
    "product": 1.0,
    "min": float("inf"),
    "max": float("-inf"),
    "count": 0,
}


def _reduce_combine(op, acc, val):
    """Combine accumulator with a new value for a given reduce op."""
    if op == "sum":
        return acc + val
    elif op == "product":
        return acc * val
    elif op == "min":
        return val if val < acc else acc
    elif op == "max":
        return val if val > acc else acc
    elif op == "count":
        return acc + 1
    elif op == "mean":
        # For mean, we accumulate (sum, count) — handled separately
        return acc + val
    else:
        raise ValueError(f"Unknown reduce operation: {op}")


def _read_buf(buf, i):
    """Read element i from a Buffer."""
    return struct.unpack_from(buf.format_char, buf._data, i * buf.itemsize)[0]


# ── Fused two-stage operations ────────────────────────────────────────


def fused_map_reduce(map_fn, reduce_op, buf):
    """Fused map+reduce: single pass, no intermediate buffer.

    Applies map_fn to each element and immediately folds into the
    reduce accumulator. Zero intermediate memory allocation.

    Args:
        map_fn: function to apply to each element (e.g. lambda x: x * x)
        reduce_op: 'sum', 'product', 'min', 'max', or 'mean'
        buf: input gpu.Buffer

    Returns:
        scalar result of the fused map+reduce

    Example:
        sum_of_squares = fused_map_reduce(lambda x: x * x, 'sum', data)
    """
    n = buf.size
    if n == 0:
        return _REDUCE_INIT.get(reduce_op, 0.0)

    is_mean = reduce_op == "mean"
    op = "sum" if is_mean else reduce_op
    acc = _REDUCE_INIT.get(op, 0.0)

    for i in range(n):
        val = map_fn(_read_buf(buf, i))
        acc = _reduce_combine(op, acc, val)

    if is_mean:
        return acc / n
    return acc


def fused_filter_reduce(pred, reduce_op, buf):
    """Fused filter+reduce: single pass.

    Filters elements by pred and immediately reduces matching elements.
    No intermediate buffer for filtered results.

    Args:
        pred: predicate function (e.g. lambda x: x > 0)
        reduce_op: 'sum', 'product', 'min', 'max', 'count', or 'mean'
        buf: input gpu.Buffer

    Returns:
        scalar result of the fused filter+reduce

    Example:
        positive_sum = fused_filter_reduce(lambda x: x > 0, 'sum', data)
    """
    n = buf.size
    if n == 0:
        return _REDUCE_INIT.get(reduce_op, 0.0)

    is_mean = reduce_op == "mean"
    op = "sum" if is_mean else reduce_op
    acc = _REDUCE_INIT.get(op, 0.0)
    count = 0

    for i in range(n):
        val = _read_buf(buf, i)
        if pred(val):
            acc = _reduce_combine(op, acc, val)
            count += 1

    if is_mean:
        return acc / count if count > 0 else 0.0
    if reduce_op == "count":
        return count
    return acc


def fused_map_filter(map_fn, pred, buf):
    """Fused map+filter: single pass, one output buffer.

    Applies map_fn then filters — but in a single pass over the input.
    Only allocates the output buffer (no intermediate mapped buffer).

    Args:
        map_fn: function to apply to each element
        pred: predicate to filter mapped values
        buf: input gpu.Buffer

    Returns:
        gpu.Buffer containing only mapped values that pass the predicate

    Example:
        # Square values, keep only those > 100
        result = fused_map_filter(lambda x: x * x, lambda x: x > 100, data)
    """
    n = buf.size
    out_fmt = buf.format_char

    selected = []
    for i in range(n):
        val = _read_buf(buf, i)
        mapped = map_fn(val)
        if pred(mapped):
            selected.append(mapped)

    if not selected:
        return alloc(0, buf.element_type, format_char=buf.format_char)

    result = alloc(len(selected), buf.element_type, format_char=buf.format_char)
    result._data = bytearray(result._data)
    for i, val in enumerate(selected):
        struct.pack_into(out_fmt, result._data, i * result.itemsize, val)
    result._size = len(selected)
    return result


def fused_map_filter_reduce(map_fn, pred, reduce_op, buf):
    """Fused map+filter+reduce: single pass, zero intermediate buffers.

    The full pipeline in one pass: map each element, filter, then reduce.

    Args:
        map_fn: function to apply to each element
        pred: predicate to filter mapped values
        reduce_op: 'sum', 'product', 'min', 'max', 'count', or 'mean'
        buf: input gpu.Buffer

    Returns:
        scalar result

    Example:
        # Sum of squares of positive values
        result = fused_map_filter_reduce(
            lambda x: x * x, lambda x: x > 0, 'sum', data
        )
    """
    n = buf.size
    if n == 0:
        return _REDUCE_INIT.get(reduce_op, 0.0)

    is_mean = reduce_op == "mean"
    op = "sum" if is_mean else reduce_op
    acc = _REDUCE_INIT.get(op, 0.0)
    count = 0

    for i in range(n):
        val = _read_buf(buf, i)
        mapped = map_fn(val)
        if pred(mapped):
            acc = _reduce_combine(op, acc, mapped)
            count += 1

    if is_mean:
        return acc / count if count > 0 else 0.0
    if reduce_op == "count":
        return count
    return acc


# ── DataFrame integration ────────────────────────────────────────────


def fused_group_agg(df, by_col, agg_col, agg_op):
    """Fused group-by + aggregation in a single pass.

    Instead of: group_by -> materialize groups -> aggregate each,
    this does everything in one scan of the data.

    Args:
        df: a gpu.dataframe.DataFrame
        by_col: column name to group by
        agg_col: column name to aggregate
        agg_op: 'sum', 'mean', 'min', 'max', or 'count'

    Returns:
        dict mapping group key -> aggregated value

    Example:
        totals = fused_group_agg(df, 'category', 'price', 'sum')
        # {'A': 26.2, 'B': 45.4, 'C': 30.2}
    """

    by_series = df[by_col]
    agg_series = df[agg_col]
    n = len(by_series)

    # Single pass: accumulate per-group
    groups = {}  # key -> (accumulator, count)

    init = _REDUCE_INIT.get(agg_op if agg_op != "mean" else "sum", 0.0)

    for i in range(n):
        key = by_series[i]
        val = agg_series[i]

        if key not in groups:
            if agg_op in ("min", "max"):
                groups[key] = (val, 1)
            else:
                groups[key] = (init, 0)

        acc, cnt = groups[key]

        if agg_op == "sum" or agg_op == "mean":
            groups[key] = (acc + val, cnt + 1)
        elif agg_op == "min":
            groups[key] = (val if val < acc else acc, cnt + 1)
        elif agg_op == "max":
            groups[key] = (val if val > acc else acc, cnt + 1)
        elif agg_op == "count":
            groups[key] = (acc, cnt + 1)
        elif agg_op == "product":
            new_acc = acc * val if cnt > 0 else val
            groups[key] = (new_acc, cnt + 1)

    # Finalize
    result = {}
    for key, (acc, cnt) in groups.items():
        if agg_op == "mean":
            result[key] = acc / cnt if cnt > 0 else 0.0
        elif agg_op == "count":
            result[key] = cnt
        else:
            result[key] = acc

    return result


# ── Pipeline builder ─────────────────────────────────────────────────


class FusedPipeline:
    """Build a pipeline of operations that get fused into a single kernel.

    Instead of materializing intermediate buffers between map/filter/reduce,
    the pipeline applies all operations in a single pass over the data.

    Usage:
        result = (FusedPipeline(data)
            .map(lambda x: x * x)
            .filter(lambda x: x > 100)
            .reduce('sum'))
    """

    def __init__(self, input_buf):
        self._input = input_buf
        self._ops = []

    def map(self, func):
        """Add a map operation to the pipeline."""
        self._ops.append(("map", func))
        return self

    def filter(self, pred):
        """Add a filter operation."""
        self._ops.append(("filter", pred))
        return self

    def reduce(self, op="sum"):
        """Add a terminal reduce operation and execute the pipeline."""
        self._ops.append(("reduce", op))
        return self.execute()

    def execute(self):
        """Execute the fused pipeline in a single pass over the data.

        Instead of materializing intermediate buffers, all map/filter ops
        are applied to each element inline, and reduce accumulates on the fly.
        """
        n = self._input.size
        if n == 0:
            # Check if there's a reduce terminal
            for op_type, *_ in self._ops:
                if op_type == "reduce":
                    return _REDUCE_INIT.get(_[0] if _ else "sum", 0.0)
            return alloc(
                0, self._input.element_type, format_char=self._input.format_char
            )

        # Separate ops into stages
        transforms = []  # (type, func) pairs for map/filter
        terminal = None  # ('reduce', op) or None

        for entry in self._ops:
            if entry[0] == "reduce":
                terminal = entry
            else:
                transforms.append(entry)

        if terminal is not None:
            # Fused transform + reduce: single pass, zero intermediate buffers
            reduce_op = terminal[1]
            is_mean = reduce_op == "mean"
            op = "sum" if is_mean else reduce_op
            acc = _REDUCE_INIT.get(op, 0.0)
            count = 0

            for i in range(n):
                val = _read_buf(self._input, i)

                # Apply transforms in order
                skip = False
                for t_type, t_func in transforms:
                    if t_type == "map":
                        val = t_func(val)
                    elif t_type == "filter":
                        if not t_func(val):
                            skip = True
                            break

                if not skip:
                    acc = _reduce_combine(op, acc, val)
                    count += 1

            if is_mean:
                return acc / count if count > 0 else 0.0
            if reduce_op == "count":
                return count
            return acc

        else:
            # No reduce terminal — produce a buffer
            # Single pass: apply all map/filter in order
            selected = []

            for i in range(n):
                val = _read_buf(self._input, i)

                skip = False
                for t_type, t_func in transforms:
                    if t_type == "map":
                        val = t_func(val)
                    elif t_type == "filter":
                        if not t_func(val):
                            skip = True
                            break

                if not skip:
                    selected.append(val)

            if not selected:
                return alloc(
                    0, self._input.element_type, format_char=self._input.format_char
                )

            result = alloc(
                len(selected),
                self._input.element_type,
                format_char=self._input.format_char,
            )
            result._data = bytearray(result._data)
            for i, val in enumerate(selected):
                struct.pack_into(
                    result.format_char, result._data, i * result.itemsize, val
                )
            result._size = len(selected)
            return result
