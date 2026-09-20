"""Two-dimensional integer buffers with a native Molt storage authority."""

import operator as _operator
import sys as _sys

import _intrinsics as _molt_intrinsics


_MOLT_RUNTIME_ACTIVE = _molt_intrinsics.runtime_active()

if _MOLT_RUNTIME_ACTIVE:
    _MOLT_BUFFER2D_NEW = _molt_intrinsics.require_intrinsic(
        "molt_buffer2d_new", globals()
    )
    _MOLT_BUFFER2D_GET = _molt_intrinsics.require_intrinsic(
        "molt_buffer2d_get", globals()
    )
    _MOLT_BUFFER2D_ROWS = _molt_intrinsics.require_intrinsic(
        "molt_buffer2d_rows", globals()
    )
    _MOLT_BUFFER2D_COLS = _molt_intrinsics.require_intrinsic(
        "molt_buffer2d_cols", globals()
    )
    _MOLT_BUFFER2D_SET = _molt_intrinsics.require_intrinsic(
        "molt_buffer2d_set", globals()
    )


def _integer(value, name: str) -> int:
    try:
        return _operator.index(value)
    except TypeError:
        raise TypeError(f"{name} must be an integer") from None


def _dimension(value, name: str) -> int:
    result = _integer(value, name)
    if result < 0:
        raise ValueError(f"{name} must be non-negative")
    if result > _sys.maxsize:
        raise OverflowError(f"{name} is too large")
    return result


def _index(value, length: int) -> int:
    result = _operator.index(value)
    if result < 0:
        result += length
    if result < 0 or result >= length:
        raise IndexError("buffer2d index out of range")
    return result


class Buffer2D:
    _native: object
    _data: list[int]
    _fallback_rows: int
    _fallback_cols: int

    def __init__(self, rows: int, cols: int, init: int = 0) -> None:
        checked_rows = _dimension(rows, "rows")
        checked_cols = _dimension(cols, "cols")
        checked_init = _integer(init, "init")
        if _MOLT_RUNTIME_ACTIVE:
            self._native = _MOLT_BUFFER2D_NEW(checked_rows, checked_cols, checked_init)
            return

        cell_count = checked_rows * checked_cols
        if cell_count > _sys.maxsize:
            raise MemoryError("buffer2d allocation failed")
        try:
            data = [checked_init] * cell_count
        except MemoryError:
            raise MemoryError("buffer2d allocation failed") from None
        self._data = data
        self._fallback_rows = checked_rows
        self._fallback_cols = checked_cols

    @property
    def rows(self) -> int:
        if _MOLT_RUNTIME_ACTIVE:
            return _MOLT_BUFFER2D_ROWS(self._native)
        return self._fallback_rows

    @property
    def cols(self) -> int:
        if _MOLT_RUNTIME_ACTIVE:
            return _MOLT_BUFFER2D_COLS(self._native)
        return self._fallback_cols

    def get(self, row: int, col: int) -> int:
        if _MOLT_RUNTIME_ACTIVE:
            return _MOLT_BUFFER2D_GET(self._native, row, col)
        data = self._data
        rows, cols = self._fallback_rows, self._fallback_cols
        selected_row = _index(row, rows)
        selected_col = _index(col, cols)
        return data[selected_row * cols + selected_col]

    def set(self, row: int, col: int, value: int) -> None:
        checked_value = _integer(value, "value")
        if _MOLT_RUNTIME_ACTIVE:
            _MOLT_BUFFER2D_SET(self._native, row, col, checked_value)
            return None
        data = self._data
        rows, cols = self._fallback_rows, self._fallback_cols
        selected_row = _index(row, rows)
        selected_col = _index(col, cols)
        data[selected_row * cols + selected_col] = checked_value
        return None


def _checked_buffer(value, name: str) -> Buffer2D:
    if not isinstance(value, Buffer2D):
        raise TypeError(f"{name} must be a Buffer2D")
    return value


def new(rows: int, cols: int, init: int = 0) -> Buffer2D:
    return Buffer2D(rows, cols, init)


def get(buf: Buffer2D, row: int, col: int) -> int:
    return _checked_buffer(buf, "buf").get(row, col)


def set(buf: Buffer2D, row: int, col: int, value: int) -> Buffer2D:
    checked = _checked_buffer(buf, "buf")
    checked.set(row, col, value)
    return checked


def matmul(a: Buffer2D, b: Buffer2D) -> Buffer2D:
    left = _checked_buffer(a, "a")
    right = _checked_buffer(b, "b")
    if left.cols != right.rows:
        raise ValueError("matmul dimension mismatch")
    # Public accessors are observable Python calls, including on exact instances.
    # A raw-storage kernel cannot substitute for this protocol without proof.
    out = Buffer2D(left.rows, right.cols, 0)
    if out.rows == 0 or out.cols == 0:
        return out
    for i in range(left.rows):
        for j in range(right.cols):
            acc = 0
            for k in range(left.cols):
                acc = acc + left.get(i, k) * right.get(k, j)
            out.set(i, j, acc)
    return out
