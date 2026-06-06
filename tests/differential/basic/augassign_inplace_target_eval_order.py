"""Purpose: differential coverage for augmented-assignment TARGET evaluation
order with the in-place dunders for //=, %=, **=, <<=, >>=, @=, /=.

CPython evaluates the augmented target's container/object expression EXACTLY
ONCE (it is loaded, the in-place op applied, then stored back to the same
resolved location). A naive lowering that re-evaluates the target for the load
and the store would call the side-effecting subscript/attribute twice. This test
traces every access so any divergence in count or order is visible.
"""

log = []


class Cell:
    """A value whose in-place ops are observable."""

    def __init__(self, v):
        self.v = v

    def __ifloordiv__(self, other):
        log.append(("ifloordiv", self.v, other))
        self.v //= other
        return self

    def __imod__(self, other):
        log.append(("imod", self.v, other))
        self.v %= other
        return self

    def __ipow__(self, other):
        log.append(("ipow", self.v, other))
        self.v **= other
        return self

    def __ilshift__(self, other):
        log.append(("ilshift", self.v, other))
        self.v <<= other
        return self

    def __irshift__(self, other):
        log.append(("irshift", self.v, other))
        self.v >>= other
        return self

    def __imatmul__(self, other):
        log.append(("imatmul", self.v, other))
        self.v = self.v * 100 + other
        return self

    def __itruediv__(self, other):
        log.append(("itruediv", self.v, other))
        self.v = self.v / other
        return self

    def __repr__(self):
        return "Cell(%r)" % (self.v,)


# ---- Subscript target: container.__getitem__ / __setitem__ counted. ----
print("=== subscript target ===")


class TracedList:
    def __init__(self, items):
        self._items = list(items)

    def __getitem__(self, idx):
        log.append(("getitem", idx))
        return self._items[idx]

    def __setitem__(self, idx, value):
        log.append(("setitem", idx))
        self._items[idx] = value


def fresh_index():
    log.append(("index-expr",))
    return 0


box = TracedList([Cell(20)])
box[fresh_index()] //= 3
print("after subscript //=", box._items, "log", log)
log.clear()


# ---- Attribute target: object resolved once. ----
print("=== attribute target ===")


class Holder:
    def __init__(self, cell):
        self.cell = cell


def fresh_holder(h):
    log.append(("holder-expr",))
    return h


holder = Holder(Cell(2))
fresh_holder(holder).cell **= 5
print("after attribute **=", holder.cell, "log", log)
log.clear()


# ---- Each remaining operator once, on a subscript, to confirm dispatch. ----
print("=== per-op subscript dispatch ===")
ops_box = TracedList([Cell(64), Cell(20), Cell(1), Cell(256), Cell(7), Cell(3)])

ops_box[0] %= 5
ops_box[1] <<= 2
ops_box[2] @= 9
ops_box[3] >>= 3
ops_box[4] /= 2
ops_box[5] //= 2
print("ops_box", ops_box._items)
print("final log", log)
