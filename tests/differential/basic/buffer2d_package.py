"""One public buffer representation through normal imports and live calls."""

import molt_buffer
import molt_buffer as buffers
import sys
from molt_buffer import Buffer2D, new as make, set as put


def snapshot(buf):
    return [[buf.get(i, j) for j in range(buf.cols)] for i in range(buf.rows)]


def report_error(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__, str(error))


a = make(2, 2, 1)
print("identity", molt_buffer is buffers, isinstance(a, Buffer2D))
print("method-set", a.set(-1, -1, 7))
print("module-set", put(a, 0, 1, 3) is a)
print("state", a.rows, a.cols, snapshot(a), buffers.get(a, -1, -1))

b = Buffer2D(2, 2, 2)
product = buffers.matmul(a, b)
print("matmul", isinstance(product, Buffer2D), snapshot(product), snapshot(a))


class VirtualBuffer(Buffer2D):
    def __init__(self, backing):
        if backing:
            super().__init__(3, 4, 99)

    @property
    def rows(self):
        return 1

    @property
    def cols(self):
        return 1

    def get(self, row, col):
        return 7


for backing in (False, True):
    virtual = VirtualBuffer(backing)
    ordinary = make(1, 1, 3)
    for left, right in ((virtual, ordinary), (ordinary, virtual)):
        virtual_product = buffers.matmul(left, right)
        print(
            "virtual-matmul",
            backing,
            type(virtual_product) is Buffer2D,
            snapshot(virtual_product),
        )

shadow_left = make(1, 2, 99)
shadow_right = make(2, 1, 5)
shadow_events = []


def second_cell(row, col):
    shadow_events.append(("second", row, col))
    return 3


def first_cell(row, col):
    shadow_events.append(("first", row, col))
    shadow_left.get = second_cell
    return 2


shadow_left.get = first_cell
print(
    "shadow-matmul", snapshot(buffers.matmul(shadow_left, shadow_right)), shadow_events
)

original_get = Buffer2D.get
original_set = Buffer2D.set
class_writes = []


def replaced_get(self, row, col):
    return original_get(self, row, col) + 1


def replaced_set(self, row, col, value):
    class_writes.append(value)
    return original_set(self, row, col, value + 10)


try:
    Buffer2D.get = replaced_get
    Buffer2D.set = replaced_set
    class_product = buffers.matmul(make(1, 1, 2), make(1, 1, 3))
    print("class-matmul", original_get(class_product, 0, 0), class_writes)
finally:
    Buffer2D.get = original_get
    Buffer2D.set = original_set

lookup_events = []
cell_failure = RuntimeError("virtual cell failed")


class ObservedBuffer(Buffer2D):
    def __getattribute__(self, name):
        if name in ("rows", "cols", "get"):
            lookup_events.append(name)
        return super().__getattribute__(name)

    def get(self, row, col):
        raise cell_failure


try:
    buffers.matmul(ObservedBuffer(1, 1), make(1, 1))
except RuntimeError as error:
    print("matmul-exception", error is cell_failure, str(error), lookup_events)

# Exact Python integers at both storage and arithmetic boundaries.
large = 1 << 90
print("large-set", buffers.set(a, 0, 0, large) is a)
print("large-get", a.get(0, 0), snapshot(buffers.matmul(a, b)))
c = make(1, 1, (1 << 62) + 1)
print("mul-overflow", snapshot(buffers.matmul(c, c)))
print("large-init", snapshot(make(1, 1, -large)))
print("empty", snapshot(make(0, 3)))
for rows, cols in ((0, sys.maxsize), (sys.maxsize, 0)):
    empty = make(rows, cols)
    print("empty-shape", empty.rows == rows, empty.cols == cols)
    report_error("empty-get", lambda: empty.get(-1, -1))
    report_error("empty-set", lambda: empty.set(-1, -1, 7))
    empty_product = buffers.matmul(make(rows, 0), make(0, cols))
    print("empty-product", empty_product.rows == rows, empty_product.cols == cols)
report_error("impossible-shape", lambda: make(sys.maxsize, sys.maxsize))
report_error("impossible-backing", lambda: make(sys.maxsize, 1))


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


indexed = make(Index(1), Index(2), Index(5))
indexed.set(Index(0), Index(-1), Index(9))
print("index-protocol", indexed.rows, indexed.cols, snapshot(indexed))
boolean = make(True, True, True)
print("bool-normalized", type(boolean.get(0, 0)).__name__, snapshot(boolean))


class ReentrantIndex:
    def __index__(self):
        indexed.set(0, 0, large)
        return 0


print("reentrant-get", indexed.get(ReentrantIndex(), 0))
indexed = make(1, 2, 3)
indexed.set(ReentrantIndex(), 1, 11)
print("reentrant-set", snapshot(indexed))


class ReinitializingIndex:
    def __index__(self):
        indexed.__init__(1, 1, 9)
        return -1


for axis in (0, 1):
    for write in (False, True):
        indexed = make(2, 3, 7)
        indices = [-1, -1]
        indices[axis] = ReinitializingIndex()
        result = indexed.set(*indices, 11) if write else indexed.get(*indices)
        print("reinit-index", axis, write, result, snapshot(indexed))

captured = buffers.new
try:
    buffers.new = lambda rows, cols, init=0: ("replacement", rows, cols, init)
    print("rebound", molt_buffer.new(1, 2, 3))
    print("captured", snapshot(captured(1, 2, 4)))
finally:
    buffers.new = captured

report_error("negative-shape", lambda: make(-1, 1))
report_error("index-high", lambda: a.get(2, 0))
report_error("index-negative", lambda: a.get(-3, 0))
report_error("index-type", lambda: a.get(0.5, 0))
report_error("slice-row", lambda: a.get(slice(None), 0))
report_error("slice-col", lambda: a.get(0, slice(None)))
report_error("slice-set-row", lambda: a.set(slice(None), 0, 8))
report_error("slice-set-col", lambda: a.set(0, slice(None), 8))
report_error("init-type", lambda: make(1, 1, 1.5))
report_error("set-type", lambda: a.set(0, 0, "not an integer"))
report_error("mismatch", lambda: buffers.matmul(a, make(3, 1)))

events = []


class OrderedIndex:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def __index__(self):
        events.append(self.name)
        return self.value


report_error(
    "row-before-col", lambda: a.get(OrderedIndex("row", 99), OrderedIndex("col", 0))
)
print("get-index-order", events)
events.clear()
report_error(
    "value-row-before-col",
    lambda: a.set(
        OrderedIndex("row", 99), OrderedIndex("col", 0), OrderedIndex("value", 8)
    ),
)
print("set-index-order", events)

index_failure = TypeError("index callback failed")


class BrokenIndex:
    def __index__(self):
        raise index_failure


for write in (False, True):
    try:
        if write:
            a.set(BrokenIndex(), 0, 9)
        else:
            a.get(0, BrokenIndex())
    except TypeError as error:
        print("index-exception", write, error is index_failure, str(error))
