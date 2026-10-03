"""Purpose: differential coverage for builtin reductions.

Builtin sum() is not an explicit `+=` loop: from an exact int start it adds
C longs into a Py_ssize_t total (the target's C data model), from an exact
float start (or once an int total meets a float) it compensates float
additions and only then falls back to generic `+`; 3.14 also compensates the
ints it meets in float mode and sums complex numbers in a phase of their own.
min() and max() compare each value to the best with one rich comparison;
sorted() is list.sort() on a new list, which asks the target version's `<`
comparisons in its order (run detection changed in 3.13) and leaves the
permutation it reached when one fails; all three, like sum(), hold their
iterator only while they use it. The explicit-loop counterparts live in
vec_reduction_in_function.py.
"""

import struct
import sys

print(sum([1, 2, 3]))
print(sum((1, 2), 10))
print(sum(range(4), 1))
print(sum([1, 2], start=10))
print(sum([], 5))
print(sum(i * i for i in range(100000) if (i * i) % 2 == 0))
print(sum(v for v in {str(i): i * i for i in range(100000)}.values() if v % 2 == 0))
print(type(sum([1.0, 2.0])).__name__, sum([1.0, 2.0]))
print(sum([0.1] * 10) == 1.0)
big_start = 10**100
print(sum([], big_start) is big_start)
print(sum((), big_start) is big_start)
print(sum((i for i in []), big_start) is big_start)
float_start = float("nan")
print(sum([], float_start) is float_start)


class MyInt(int):
    pass


subclass_start = MyInt(7)
subclass_empty = sum([], subclass_start)
print(type(subclass_empty).__name__, subclass_empty is subclass_start)

try:
    sum([], "")
except TypeError as exc:
    print(f"sum-str:{exc}")

try:
    sum([], b"")
except TypeError as exc:
    print(f"sum-bytes:{exc}")

try:
    sum([], bytearray(b""))
except TypeError as exc:
    print(f"sum-bytearray:{exc}")

print(min([3, 1, 2]))
print(max([3, 1, 2]))
print(min(3, 1, 2))
print(max(3, 1, 2))


def neg(x):
    return -x


print(min([1, 2, 3], key=neg))
print(max([1, 2, 3], key=neg))
print(min([], default=9))
print(max([], default=9))
print(min([], default=9, key=abs))
print(max([], default=9, key=abs))

try:
    min([])
except ValueError as exc:
    print(f"min-empty:{exc}")

try:
    max([])
except ValueError as exc:
    print(f"max-empty:{exc}")

try:
    min()
except TypeError as exc:
    print(f"min-noargs:{exc}")

try:
    max()
except TypeError as exc:
    print(f"max-noargs:{exc}")

try:
    min(1, 2, default=0)
except TypeError as exc:
    print(f"min-default-multi:{exc}")

try:
    max(1, 2, default=0)
except TypeError as exc:
    print(f"max-default-multi:{exc}")


# --- builtin sum()'s own algorithm, not a running `+` ---
print("sum compensated:", repr(sum([0.1, 0.2, 0.3])), repr(sum([0.1] * 10, 0.0)))
print(
    "sum after int mode:", repr(sum([1, 0.1, 0.2, 0.3])), repr(sum([2**62, 2**62, 0.5]))
)
print("sum int in float mode:", repr(sum([1e16, 1, 1.0, -1e16])))
print(
    "sum specials:",
    sum([float("inf"), 1.0]),
    sum([1.0, float("inf")]),
    sum([float("inf"), -float("inf")]),
    sum([1e308, 1e308, -1e308]),
    sum([float("nan"), 1.0]),
)
print(
    "sum signed zero:", repr(sum([-0.0], -0.0)), repr(sum([-0.0])), repr(sum([], -0.0))
)
print(
    "sum ints:",
    sum([2**62, 2**62, 2**62]),
    sum([True, True, 2]),
    type(sum([True, False])).__name__,
    sum([10**30, -(10**30), 7]),
)
print("sum bool start:", sum([1, 2], True), type(sum([], True)).__name__)
try:
    sum([1.0, 10**400])
except OverflowError as exc:
    print(f"sum-overflow:{exc}")
try:
    sum(5, "")
except TypeError as exc:
    print(f"sum-not-iterable-before-str-start:{exc}")


class FailingIter:
    def __iter__(self):
        raise ValueError("iteration refused")


try:
    sum(FailingIter())
except ValueError as exc:
    print(f"sum-iter-error:{exc}")

sum_events = []


class TracedInt(int):
    def __add__(self, other):
        sum_events.append(("TracedInt.__add__", int(self), other))
        return TracedInt(int(self) + other)


class TracedFloat(float):
    def __radd__(self, other):
        sum_events.append(("TracedFloat.__radd__", other, float(self)))
        return other + float(self)


print("sum subclass start:", sum([1, 2], TracedInt(10)), sum_events)
sum_events.clear()
print("sum subclass float item:", sum([1.5, TracedFloat(2.5), 0.1]), sum_events)
sum_events.clear()
print("sum int subclass item:", sum([1, TracedInt(2), 3]), sum_events)
sum_events.clear()


class Appender:
    def __init__(self, items):
        self.items = items

    def __radd__(self, other):
        self.items.append(100)
        return other + 1


def sum_live():
    items = [1]
    items.append(Appender(items))
    items.append(2)
    return sum(items), len(items)


print("sum reads the list live:", sum_live())


# sum() owns its iterator and each item only while it uses them.
def closing_items(log):
    try:
        yield 1
        yield "x"
    finally:
        log.append("closed")


release_log = []
held = closing_items(release_log)
try:
    sum(held)
except TypeError:
    release_log.append("caught")
release_log.append("before del")
del held
release_log.append("after del")
print("sum releases its iterator:", release_log)


class Tracked:
    def __init__(self, value, log):
        self.value = value
        self.log = log

    def __radd__(self, other):
        return other + self.value

    def __del__(self):
        self.log.append(("del", self.value))


def tracked_items(log):
    for value in (1, 2, 3):
        yield Tracked(value, log)


item_log = []
print("sum releases each item:", sum(tracked_items(item_log)), item_log)


# --- sum()'s C long and Py_ssize_t are the target's C data model ---
LONG64 = struct.calcsize("l") == 8
SSIZE64 = sys.maxsize > 2**32
# An item that is a C long keeps sum() in its int phase, and a float then
# enters the compensated float phase; any other int goes generic, and generic
# float `+` does not compensate. The totals below are exact on each model.
print(
    "sum long width:",
    sum([2**53, 1.0, 1.0, 1.0])
    == (9007199254740994.0 if LONG64 else 9007199254740992.0),
)
# The int phase's total is a Py_ssize_t: 2**30 + 2**30 stays in it only where
# Py_ssize_t is wider than 32 bits.
print(
    "sum ssize width:",
    sum([2**30, 2**30, 2**-22, 2**-22, 2**-22])
    == (2147483648.0000005 if SSIZE64 else 2147483648.0),
)

# --- 3.14: the float phase compensates ints, and a complex phase follows ---
print("sum int in float phase:", repr(sum([1e16, 1, 1, 1])))
print("sum complex phase:", repr(sum([1j, 1, 10e100j, 1j, 1.0, -10e100j])))
print(
    "sum imaginary zero:",
    repr(sum([complex(1, -0.0), 1])),
    repr(sum([1, complex(1, -0.0)])),
    repr(sum([1.0, complex(1, -0.0)])),
)
try:
    sum([1j, 10**1000])
except OverflowError as exc:
    print(f"sum-complex-overflow:{exc}")
try:
    sum([10**1000, 1j])
except OverflowError as exc:
    print(f"sum-int-to-complex-overflow:{exc}")

# --- complex arithmetic with a real operand (3.14: mixed-mode rules) ---
INF = float("inf")
print(
    "complex mixed:",
    repr(complex(-0.0, -0.0) + (-0.0)),
    repr(-0.0 + complex(-0.0, -0.0)),
    repr(-0.0 - complex(0.0, 0.0)),
    repr(complex(-0.0, -0.0) - 0.0),
    repr(complex(INF, 1) * 2),
    repr(2 * complex(INF, 1)),
    repr(complex(INF, 1) / 2),
    repr(1 / complex(1, 2)),
)
print(
    "complex division:",
    repr(complex(1e200, 1e200) / complex(1e200, 1e200)),
    repr(complex(1, 1) / complex(INF, INF)),
    repr(complex(INF, -INF) / complex(1, 0)),
)
try:
    1 / complex(0, 0)
except ZeroDivisionError:
    print("complex-zero-division")

# --- min()/max(): one rich comparison per item, value against the best ---
calls = []


class Ordered:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        calls.append(("lt", self.value, other.value))
        return self.value < other.value

    def __gt__(self, other):
        calls.append(("gt", self.value, other.value))
        return self.value > other.value

    def __repr__(self):
        return f"O({self.value})"


print("min protocol:", min([Ordered(3), Ordered(1), Ordered(2)]), calls)
calls.clear()
print("max protocol:", max([Ordered(3), Ordered(1), Ordered(2)]), calls)
calls.clear()
print("min key protocol:", min([3, 1, 2], key=Ordered), calls)
calls.clear()


class Verdict:
    def __init__(self, value):
        self.value = value

    def __bool__(self):
        calls.append(("bool", self.value))
        return self.value


class Judged:
    def __init__(self, name, verdict):
        self.name = name
        self.verdict = verdict

    def __lt__(self, other):
        return Verdict(self.verdict)

    def __repr__(self):
        return self.name


print("min truth:", min([Judged("a", False), Judged("b", True)]), calls)
calls.clear()
print("min nan:", min([float("nan"), 1.0]), min([1.0, float("nan")]))
for reduce in (min, max):
    try:
        reduce([1, "a"])
    except TypeError as exc:
        print(f"{reduce.__name__}-unorderable:{exc}")


def refusing_key(value):
    if value == 2:
        raise LookupError("key refused")
    return value


try:
    max([1, 2, 3], key=refusing_key)
except LookupError as exc:
    print(f"max-key-error:{exc}")


# min(), max() and sorted() own their iterator only while they use it.
def closing_values(log):
    try:
        yield 1
        yield "x"
        yield 2
    finally:
        log.append("closed")


for reduce in (min, max, sorted):
    custody_log = []
    held = closing_values(custody_log)
    try:
        reduce(held)
    except TypeError:
        custody_log.append("caught")
    custody_log.append("before del")
    del held
    custody_log.append("after del")
    print(f"{reduce.__name__} releases its iterator:", custody_log)

print("sorted stable:", sorted([(1, "b"), (0, "z"), (1, "a")], key=lambda t: t[0]))
print(
    "sorted reverse stable:",
    sorted([(1, "b"), (0, "z"), (1, "a")], key=lambda t: t[0], reverse=True),
)
print("sorted big:", sorted(range(5000, 0, -1))[:3], sorted([3.0, 1, 2.5, True]))
try:
    sorted([2, "b", 1])
except TypeError as exc:
    print(f"sorted-unorderable:{exc}")

# --- list.sort(): the target CPython's comparisons, in its order ---
sort_log = []


class Keyed:
    def __init__(self, key, tag):
        self.key = key
        self.tag = tag

    def __lt__(self, other):
        sort_log.append((self.tag, other.tag))
        return self.key < other.key


def trace_sort(keys, **options):
    sort_log.clear()
    items = [Keyed(key, tag) for tag, key in enumerate(keys)]
    items.sort(**options)
    return [item.tag for item in items], list(sort_log)


def digest(numbers):
    total = 0
    for number in numbers:
        total = (total * 1000003 + number) % (2**61 - 1)
    return total


for keys in (
    [2, 1, 3],
    [5, 5, 4, 4, 3, 3],
    [3, 2, 3, 4, 1],
    [1, 1, 1],
    [2, 2, 1, 1, 3],
):
    print("sort trace:", keys, *trace_sort(keys))
print("sort trace reverse:", *trace_sort([2, 1, 3, 3, 1], reverse=True))
sort_log.clear()
print(
    "sort trace key:",
    sorted(range(5), key=lambda tag: Keyed([2, 1, 3, 3, 1][tag], tag), reverse=True),
    sort_log,
)
# Longer inputs merge runs and gallop; each prints its comparison count and
# digests of the result and of the comparison sequence.
for name, keys in (
    ("shuffled 65", [(i * 41) % 101 for i in range(65)]),
    ("shuffled 127", [(i * 41) % 131 for i in range(127)]),
    ("interleaved runs", [*range(0, 300, 3), *range(1, 300, 3), *range(2, 300, 3)]),
    ("galloping runs", [*range(100, 200), *range(100)]),
    ("equal descents", [i // 3 for i in range(200, 0, -1)]),
):
    tags, log = trace_sort(keys)
    pairs = digest(left * 4099 + right for left, right in log)
    print("sort trace:", name, len(log), digest(tags), pairs)


class Refusing(Keyed):
    def __lt__(self, other):
        if len(sort_log) == 4:
            raise LookupError("refused")
        return super().__lt__(other)


refused = [Refusing(key, tag) for tag, key in enumerate([5, 5, 4, 4, 3, 3, 9, 1])]
sort_log.clear()
try:
    refused.sort()
except LookupError:
    print("sort failure state:", [item.tag for item in refused], sort_log)


# CPython's generic sum lane keeps owned objects. Only its ordered optimized
# phases may replace them with scalar totals; entry depends on version and ABI.
identity_wide_start = int(str(2**62))
identity_big_start = int("1" + "0" * 100)
identity_complex_start = complex(2, -0.0)
identity_py314 = sys.version_info >= (3, 14)
for identity_source, empty in (
    ("list", []),
    ("tuple", ()),
    ("iterator", iter(())),
    ("generator", (value for value in ())),
):
    print(
        "sum empty identity:", identity_source,
        sum(empty, identity_big_start) is identity_big_start,
        (sum(empty, identity_wide_start) is identity_wide_start) == (struct.calcsize("l") < 8),
        (sum(empty, identity_complex_start) is identity_complex_start) == (not identity_py314),
    )


class IdentitySumStart:
    def __add__(self, value):
        return value


class IdentitySumItem:
    def __init__(self, result):
        self.result = result

    def __radd__(self, left):
        return self.result


class ObserveSumIdentity:
    def __init__(self, expected, observations):
        self.expected = expected
        self.observations = observations

    def __radd__(self, left):
        self.observations.append(left is self.expected)
        return left


identity_results = (
    ("int", identity_big_start),
    ("float", float("nan")),
    ("complex", complex(3, 4)),
)
for identity_kind, identity_result in identity_results:
    print(
        "sum generic __add__ identity:", identity_kind,
        sum([identity_result], IdentitySumStart()) is identity_result,
    )
    long_expected = identity_kind == "int" or (identity_kind == "complex" and not identity_py314)
    float_expected = identity_kind != "complex" or not identity_py314
    print(
        "sum phase exit identity:", identity_kind,
        (sum([IdentitySumItem(identity_result)], 0) is identity_result) == long_expected,
        (sum([IdentitySumItem(identity_result)], 0.0) is identity_result) == float_expected,
        sum([IdentitySumItem(identity_result)], 0j) is identity_result,
    )
    observations = []
    total = sum(
        [identity_result, ObserveSumIdentity(identity_result, observations)],
        IdentitySumStart(),
    )
    print("sum generic next operand:", identity_kind, observations, total is identity_result)

# Once generic, a later exact float result must not reenter compensation.
print("sum generic phase stays generic:", sum([1e16, 1.0, 1.0, 1.0], IdentitySumStart()) == 1e16)

