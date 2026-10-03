"""Purpose: range() converts its bounds exactly as CPython does.

range() evaluates every argument, then converts start, stop and step with
operator.index, in that order, and checks the step only afterwards. Its values
are exact ints of any size, whatever the bounds' types (bools, int subclasses,
__index__ objects), and a loop over it leaves its target at the last value, or
unbound after no iteration. The loops run both in a function, where the counted
lowering may apply, and at module scope. Version-stable across CPython
3.12/3.13/3.14.
"""

events = []


class Index:
    def __init__(self, value, name):
        self.value = value
        self.name = name

    def __index__(self):
        events.append(("index", self.name))
        return self.value


class BadIndex:
    def __index__(self):
        return "not an int"


class Sub(int):
    pass


def arg(value, name):
    events.append(("arg", name))
    return value


def loop3(start, stop, step):
    values = []
    for i in range(start, stop, step):
        values.append(i)
    try:
        last = i
    except UnboundLocalError:
        last = "<unbound>"
    return values, sorted({type(v).__name__ for v in values}), last


def loop1(stop):
    count = 0
    for i in range(stop):
        count += 1
    try:
        last = i
    except UnboundLocalError:
        last = "<unbound>"
    return count, last


def report(label, thunk):
    try:
        result = thunk()
    except Exception as exc:
        result = (type(exc).__name__, str(exc))
    print(label, result, events[:])
    events.clear()


report("bool bounds", lambda: loop3(True, 4, True))
report("int subclass bounds", lambda: loop3(Sub(1), Sub(4), Sub(1)))
report(
    "index bounds", lambda: loop3(Index(1, "start"), Index(4, "stop"), Index(1, "step"))
)
report("negative step", lambda: loop3(3, 0, -1))
step = -2
report("variable step", lambda: loop3(10, 1, step))
report("empty leaves unbound", lambda: loop3(0, 0, 1))
report("big values", lambda: loop3(2**62, 2**62 + 3, 1))
report("huge values", lambda: loop3(2**70, 2**70 + 2, 1))
report("negative big", lambda: loop3(-(2**50), -(2**50) + 2, 1))
report("one bound", lambda: loop1(5))
report("one index bound", lambda: loop1(Index(3, "stop")))
report("zero bound", lambda: loop1(0))

# Every argument is evaluated before any conversion; conversions run in order.
report(
    "evaluation then conversion",
    lambda: loop3(
        arg(Index(0, "a"), "a"), arg(Index(2, "b"), "b"), arg(Index(1, "c"), "c")
    ),
)
report("start fails first", lambda: loop3(Index(1, "start"), "x", 1))
report(
    "stop fails after start", lambda: loop3(Index(1, "start"), 2.5, Index(1, "step"))
)
report(
    "zero step after conversions",
    lambda: loop3(Index(0, "a"), Index(5, "b"), Index(0, "c")),
)
report("float bound", lambda: loop1(1.5))
report("bad __index__", lambda: loop1(BadIndex()))
report("none bound", lambda: loop1(None))

# Range objects hold exact values beyond the inline int window.
big = range(2**62, 2**62 + 10)
print("list big", list(range(2**62, 2**62 + 3)))
print(
    "index big",
    big[3],
    big[-1],
    big.index(2**62 + 5),
    (2**62 + 5) in big,
    big.count(2**62 + 9),
)
print("sum big", sum(range(2**62, 2**62 + 3)))
print("iter big", [v for v in range(2**62, 2**62 + 2)], next(iter(range(2**61, 2**62))))
huge = range(2**70, 2**70 + 4, 2)
print("huge", list(huge), huge[1], len(huge))

# Module scope: the loop target is a module global.
for m in range(True, 3):
    pass
print("module target", m, type(m).__name__)
for m2 in range(Index(2, "module"), 4):
    pass
print("module index target", m2, events[:])
events.clear()
try:
    for m3 in range(0, 3, 0):
        pass
except ValueError as exc:
    print("module zero step", exc)
