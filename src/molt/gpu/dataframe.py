"""
molt.gpu.dataframe — GPU-accelerated DataFrame with Polars/Pandas syntax.

Usage (Polars-style):
    import molt.gpu.dataframe as df

    # Create
    data = df.DataFrame({
        "price": [10.5, 20.3, 15.7, 30.2, 25.1],
        "quantity": [100, 200, 150, 300, 250],
        "category": ["A", "B", "A", "C", "B"],
    })

    # Select columns
    prices = data.select("price", "quantity")

    # Filter
    expensive = data.filter(data["price"] > 20.0)

    # Group by + aggregate
    by_category = data.group_by("category").agg(
        total_revenue=("price", "sum"),
        avg_quantity=("quantity", "mean"),
        count=("price", "count"),
    )

    # Sort
    sorted_data = data.sort("price", descending=True)

    # Join
    joined = left.join(right, on="id", how="inner")

    # Lazy evaluation (Polars-style)
    result = (data.lazy()
        .filter(col("price") > 10.0)
        .with_column(col("price") * col("quantity"), name="revenue")
        .group_by("category")
        .agg(sum("revenue"))
        .sort("revenue", descending=True)
        .collect())
"""

from . import Buffer, to_device, from_device, alloc
from . import ops
import struct
import json
import math


class Series:
    """A single column of data, backed by a GPU Buffer."""

    def __init__(self, name: str, data, dtype=None):
        self.name = name
        if isinstance(data, Buffer):
            self._buffer = data
            self._dtype = dtype or float
            self._size = data.size
            self._str_data = None
        elif isinstance(data, list):
            if data and isinstance(data[0], str):
                self._str_data = list(data)
                self._buffer = None
                self._dtype = str
                self._size = len(data)
            else:
                self._dtype = dtype or (float if any(isinstance(x, float) for x in data) else int)
                self._buffer = to_device([float(x) if self._dtype == float else x for x in data])
                self._size = len(data)
                self._str_data = None
        else:
            raise TypeError(f"Cannot create Series from {type(data)}")

    def __len__(self):
        return self._size

    def __getitem__(self, idx):
        if isinstance(idx, int):
            if self._str_data is not None:
                return self._str_data[idx]
            return self._buffer[idx]
        elif isinstance(idx, Series):
            # Boolean indexing
            return self._filter_by_mask(idx)
        elif isinstance(idx, slice):
            return self._slice(idx)
        raise TypeError(f"Cannot index Series with {type(idx)}")

    def _filter_by_mask(self, mask: 'Series') -> 'Series':
        if self._str_data is not None:
            filtered = [self._str_data[i] for i in range(self._size) if mask._get_bool(i)]
            return Series(self.name, filtered)
        else:
            filtered = [self._buffer[i] for i in range(self._size) if mask._get_bool(i)]
            return Series(self.name, filtered, dtype=self._dtype)

    def _slice(self, s: slice) -> 'Series':
        indices = range(*s.indices(self._size))
        if self._str_data is not None:
            return Series(self.name, [self._str_data[i] for i in indices])
        return Series(self.name, [self._buffer[i] for i in indices], dtype=self._dtype)

    def _get_bool(self, idx):
        if self._buffer is not None:
            val = self._buffer[idx]
            return bool(val)
        return bool(self._str_data[idx]) if self._str_data else False

    # Comparison operators -> return boolean Series
    def __gt__(self, other): return self._compare(other, lambda a, b: 1.0 if a > b else 0.0)
    def __lt__(self, other): return self._compare(other, lambda a, b: 1.0 if a < b else 0.0)
    def __ge__(self, other): return self._compare(other, lambda a, b: 1.0 if a >= b else 0.0)
    def __le__(self, other): return self._compare(other, lambda a, b: 1.0 if a <= b else 0.0)
    def __eq__(self, other): return self._compare(other, lambda a, b: 1.0 if a == b else 0.0)
    def __ne__(self, other): return self._compare(other, lambda a, b: 1.0 if a != b else 0.0)

    def _compare(self, other, op):
        if isinstance(other, (int, float)):
            result = [op(self._buffer[i], other) for i in range(self._size)]
        elif isinstance(other, str) and self._str_data is not None:
            result = [op(self._str_data[i], other) for i in range(self._size)]
        elif isinstance(other, Series):
            result = [op(self._buffer[i], other._buffer[i]) for i in range(self._size)]
        else:
            raise TypeError(f"Cannot compare with {type(other)}")
        return Series(f"{self.name}_cmp", result, dtype=float)

    # Arithmetic operators
    def __add__(self, other): return self._arith(other, lambda a, b: a + b, "add")
    def __sub__(self, other): return self._arith(other, lambda a, b: a - b, "sub")
    def __mul__(self, other): return self._arith(other, lambda a, b: a * b, "mul")
    def __truediv__(self, other): return self._arith(other, lambda a, b: a / b if b != 0 else float('inf'), "div")

    def _arith(self, other, op, name):
        if isinstance(other, (int, float)):
            result = [op(self._buffer[i], float(other)) for i in range(self._size)]
        elif isinstance(other, Series):
            result = [op(self._buffer[i], other._buffer[i]) for i in range(self._size)]
        else:
            raise TypeError(f"Cannot compute with {type(other)}")
        return Series(f"{self.name}_{name}", result, dtype=self._dtype)

    # Aggregations
    def sum(self):
        if self._buffer is None: raise TypeError("Cannot sum string Series")
        return ops.reduce(self._buffer, 'sum')

    def mean(self):
        if self._buffer is None: raise TypeError("Cannot mean string Series")
        return ops.reduce(self._buffer, 'mean')

    def min(self):
        if self._buffer is None: return min(self._str_data)
        return ops.reduce(self._buffer, 'min')

    def max(self):
        if self._buffer is None: return max(self._str_data)
        return ops.reduce(self._buffer, 'max')

    def count(self):
        return self._size

    def std(self):
        m = self.mean()
        variance = sum((self._buffer[i] - m) ** 2 for i in range(self._size)) / max(self._size - 1, 1)
        return math.sqrt(variance)

    def unique(self):
        if self._str_data is not None:
            return Series(self.name, list(dict.fromkeys(self._str_data)))
        seen = []
        for i in range(self._size):
            v = self._buffer[i]
            if v not in seen:
                seen.append(v)
        return Series(self.name, seen, dtype=self._dtype)

    def n_unique(self):
        return len(self.unique())

    def value_counts(self):
        counts = {}
        for i in range(self._size):
            val = self._str_data[i] if self._str_data is not None else self._buffer[i]
            counts[val] = counts.get(val, 0) + 1
        return counts

    def to_list(self):
        if self._str_data is not None:
            return list(self._str_data)
        return [self._buffer[i] for i in range(self._size)]

    def head(self, n=5):
        return self[:n]

    def tail(self, n=5):
        return self[-n:]

    def describe(self):
        if self._str_data is not None:
            return {"count": self._size, "unique": self.n_unique()}
        return {
            "count": self._size,
            "mean": self.mean(),
            "std": self.std(),
            "min": self.min(),
            "max": self.max(),
        }

    def __repr__(self):
        vals = self.to_list()[:5]
        suffix = f"... ({self._size} total)" if self._size > 5 else ""
        return f"Series('{self.name}', {vals}{suffix})"


class DataFrame:
    """GPU-accelerated DataFrame with Polars/Pandas syntax."""

    def __init__(self, data=None, columns=None):
        self._columns = {}  # name -> Series
        self._column_order = []

        if isinstance(data, dict):
            for name, values in data.items():
                self._columns[name] = Series(name, values) if not isinstance(values, Series) else values
                self._column_order.append(name)
        elif data is None:
            pass
        else:
            raise TypeError(f"Cannot create DataFrame from {type(data)}")

    @property
    def shape(self):
        if not self._columns:
            return (0, 0)
        first = next(iter(self._columns.values()))
        return (len(first), len(self._columns))

    @property
    def columns(self):
        return list(self._column_order)

    @property
    def dtypes(self):
        return {name: col._dtype for name, col in self._columns.items()}

    def __len__(self):
        if not self._columns:
            return 0
        return len(next(iter(self._columns.values())))

    def __getitem__(self, key):
        if isinstance(key, str):
            return self._columns[key]
        elif isinstance(key, (list, tuple)):
            return self.select(*key)
        elif isinstance(key, Series):
            # Boolean mask
            return self.filter(key)
        raise KeyError(f"Unknown key type: {type(key)}")

    def __setitem__(self, key, value):
        if isinstance(value, Series):
            self._columns[key] = Series(key, value.to_list(), dtype=value._dtype)
        elif isinstance(value, list):
            self._columns[key] = Series(key, value)
        else:
            raise TypeError(f"Cannot set column from {type(value)}")
        if key not in self._column_order:
            self._column_order.append(key)

    def select(self, *cols):
        result = DataFrame()
        for col in cols:
            if col in self._columns:
                result._columns[col] = self._columns[col]
                result._column_order.append(col)
        return result

    def drop(self, *cols):
        result = DataFrame()
        for name in self._column_order:
            if name not in cols:
                result._columns[name] = self._columns[name]
                result._column_order.append(name)
        return result

    def filter(self, mask):
        if isinstance(mask, Series):
            result = DataFrame()
            for name in self._column_order:
                result._columns[name] = self._columns[name]._filter_by_mask(mask)
                result._column_order.append(name)
            return result
        raise TypeError(f"filter expects a boolean Series, got {type(mask)}")

    def with_column(self, expr, name=None):
        result = DataFrame()
        for col_name in self._column_order:
            result._columns[col_name] = self._columns[col_name]
            result._column_order.append(col_name)
        if isinstance(expr, Series):
            col_name = name or expr.name
            result._columns[col_name] = Series(col_name, expr.to_list(), dtype=expr._dtype)
            if col_name not in result._column_order:
                result._column_order.append(col_name)
        return result

    def rename(self, mapping):
        result = DataFrame()
        for name in self._column_order:
            new_name = mapping.get(name, name)
            result._columns[new_name] = Series(new_name, self._columns[name].to_list(), dtype=self._columns[name]._dtype)
            result._column_order.append(new_name)
        return result

    def sort(self, by, descending=False):
        col = self._columns[by]
        indices = list(range(len(self)))
        vals = col.to_list()
        indices.sort(key=lambda i: vals[i], reverse=descending)

        result = DataFrame()
        for name in self._column_order:
            series = self._columns[name]
            sorted_vals = [series[i] for i in indices]
            result._columns[name] = Series(name, sorted_vals, dtype=series._dtype)
            result._column_order.append(name)
        return result

    def group_by(self, *by_cols):
        return GroupBy(self, list(by_cols))

    def join(self, other, on, how="inner"):
        left_col = self._columns[on].to_list()
        right_col = other._columns[on].to_list()

        # Build right index
        right_index = {}
        for i, val in enumerate(right_col):
            right_index.setdefault(val, []).append(i)

        result_data = {name: [] for name in self._column_order}
        for name in other._column_order:
            if name != on:
                result_data[name] = []

        for i, val in enumerate(left_col):
            if val in right_index:
                for j in right_index[val]:
                    for name in self._column_order:
                        result_data[name].append(self._columns[name][i])
                    for name in other._column_order:
                        if name != on:
                            result_data[name].append(other._columns[name][j])
            elif how == "left":
                for name in self._column_order:
                    result_data[name].append(self._columns[name][i])
                for name in other._column_order:
                    if name != on:
                        result_data[name].append(None)

        return DataFrame(result_data)

    def head(self, n=5):
        result = DataFrame()
        for name in self._column_order:
            result._columns[name] = self._columns[name].head(n)
            result._column_order.append(name)
        return result

    def tail(self, n=5):
        result = DataFrame()
        for name in self._column_order:
            result._columns[name] = self._columns[name].tail(n)
            result._column_order.append(name)
        return result

    def describe(self):
        return {name: col.describe() for name, col in self._columns.items()}

    def to_dict(self):
        return {name: col.to_list() for name, col in self._columns.items()}

    def to_csv(self, path=None):
        lines = [",".join(self._column_order)]
        for i in range(len(self)):
            row = [str(self._columns[name][i]) for name in self._column_order]
            lines.append(",".join(row))
        text = "\n".join(lines) + "\n"
        if path:
            with open(path, "w") as f:
                f.write(text)
        return text

    def lazy(self):
        return LazyFrame(self)

    def __repr__(self):
        rows = min(5, len(self))
        header = " | ".join(f"{name:>12}" for name in self._column_order)
        sep = "-" * len(header)
        lines = [header, sep]
        for i in range(rows):
            row = " | ".join(f"{str(self._columns[name][i]):>12}" for name in self._column_order)
            lines.append(row)
        if len(self) > 5:
            lines.append(f"... ({len(self)} rows total)")
        return "\n".join(lines)


class GroupBy:
    """Group-by aggregation (Polars-style)."""

    def __init__(self, df, by_cols):
        self._df = df
        self._by_cols = by_cols
        self._groups = self._compute_groups()

    def _compute_groups(self):
        groups = {}
        for i in range(len(self._df)):
            key = tuple(self._df._columns[col][i] for col in self._by_cols)
            groups.setdefault(key, []).append(i)
        return groups

    def agg(self, **aggregations):
        result_data = {col: [] for col in self._by_cols}
        for agg_name, (col_name, agg_func) in aggregations.items():
            result_data[agg_name] = []

        for key, indices in self._groups.items():
            for i, col in enumerate(self._by_cols):
                result_data[col].append(key[i])

            for agg_name, (col_name, agg_func) in aggregations.items():
                col = self._df._columns[col_name]
                values = [col[i] for i in indices]

                if agg_func == "sum":
                    result_data[agg_name].append(sum(values))
                elif agg_func == "mean":
                    result_data[agg_name].append(sum(values) / len(values))
                elif agg_func == "min":
                    result_data[agg_name].append(min(values))
                elif agg_func == "max":
                    result_data[agg_name].append(max(values))
                elif agg_func == "count":
                    result_data[agg_name].append(len(values))
                elif agg_func == "std":
                    m = sum(values) / len(values)
                    var = sum((v - m) ** 2 for v in values) / max(len(values) - 1, 1)
                    result_data[agg_name].append(math.sqrt(var))
                elif agg_func == "first":
                    result_data[agg_name].append(values[0])
                elif agg_func == "last":
                    result_data[agg_name].append(values[-1])
                else:
                    raise ValueError(f"Unknown aggregation: {agg_func}")

        return DataFrame(result_data)

    def count(self):
        result_data = {col: [] for col in self._by_cols}
        result_data["count"] = []
        for key, indices in self._groups.items():
            for i, col in enumerate(self._by_cols):
                result_data[col].append(key[i])
            result_data["count"].append(len(indices))
        return DataFrame(result_data)

    def sum(self):
        numeric_cols = [name for name, col in self._df._columns.items()
                       if name not in self._by_cols and col._dtype != str]
        aggs = {col: (col, "sum") for col in numeric_cols}
        return self.agg(**aggs)

    def mean(self):
        numeric_cols = [name for name, col in self._df._columns.items()
                       if name not in self._by_cols and col._dtype != str]
        aggs = {col: (col, "mean") for col in numeric_cols}
        return self.agg(**aggs)


class LazyFrame:
    """Lazy evaluation wrapper (Polars-style query optimization)."""

    def __init__(self, df):
        self._df = df
        self._ops = []

    def filter(self, expr):
        self._ops.append(("filter", expr))
        return self

    def select(self, *cols):
        self._ops.append(("select", cols))
        return self

    def with_column(self, expr, name=None):
        self._ops.append(("with_column", expr, name))
        return self

    def sort(self, by, descending=False):
        self._ops.append(("sort", by, descending))
        return self

    def group_by(self, *cols):
        self._ops.append(("group_by", cols))
        return self

    def agg(self, **aggregations):
        self._ops.append(("agg", aggregations))
        return self

    def collect(self):
        result = self._df
        i = 0
        while i < len(self._ops):
            op = self._ops[i]
            if op[0] == "filter":
                result = result.filter(op[1])
            elif op[0] == "select":
                result = result.select(*op[1])
            elif op[0] == "with_column":
                result = result.with_column(op[1], name=op[2])
            elif op[0] == "sort":
                result = result.sort(op[1], descending=op[2])
            elif op[0] == "group_by":
                # Next op should be agg
                if i + 1 < len(self._ops) and self._ops[i + 1][0] == "agg":
                    gb = result.group_by(*op[1])
                    result = gb.agg(**self._ops[i + 1][1])
                    i += 1
                else:
                    result = result.group_by(*op[1]).count()
            i += 1
        return result


# Convenience functions (Polars-style)
def col(name):
    """Column reference for lazy expressions."""
    return name


def read_csv(path, has_header=True):
    """Read a CSV file into a DataFrame."""
    with open(path) as f:
        lines = f.read().strip().split("\n")

    if has_header:
        headers = [h.strip() for h in lines[0].split(",")]
        data_lines = lines[1:]
    else:
        ncols = len(lines[0].split(","))
        headers = [f"col_{i}" for i in range(ncols)]
        data_lines = lines

    columns = {h: [] for h in headers}
    for line in data_lines:
        values = line.split(",")
        for h, v in zip(headers, values):
            v = v.strip()
            try:
                columns[h].append(float(v))
            except ValueError:
                columns[h].append(v)

    return DataFrame(columns)
