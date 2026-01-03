class Buffer2D:
    def __init__(self, rows: int, cols: int, init: int = 0) -> None:
        if rows < 0 or cols < 0:
            raise ValueError("rows/cols must be non-negative")
        self.rows = rows
        self.cols = cols
        self._data = [[init for _ in range(cols)] for _ in range(rows)]

    def get(self, row: int, col: int) -> int:
        return self._data[row][col]

    def set(self, row: int, col: int, value: int) -> None:
        self._data[row][col] = value


def new(rows: int, cols: int, init: int = 0) -> Buffer2D:
    return Buffer2D(rows, cols, init)


def get(buf: Buffer2D, row: int, col: int) -> int:
    return buf.get(row, col)


def set(buf: Buffer2D, row: int, col: int, value: int) -> Buffer2D:
    buf.set(row, col, value)
    return buf


def matmul(a: Buffer2D, b: Buffer2D) -> Buffer2D:
    if a.cols != b.rows:
        raise ValueError("matmul dimension mismatch")
    out = Buffer2D(a.rows, b.cols, 0)
    for i in range(a.rows):
        for j in range(b.cols):
            acc = 0
            for k in range(a.cols):
                acc = acc + a.get(i, k) * b.get(k, j)
            out.set(i, j, acc)
    return out
