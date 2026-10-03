"""Purpose: differential guard for IN-FUNCTION vectorized reductions (vec_* ops).

Regression anchor: commit 8b5773878 ("Extract arithmetic codegen handler")
dropped the 24 `vec_*` reduction kinds from the native backend's dispatch arm
(they are handled inside `fc::arith::handle_arith_op` via delegation to
`fc::vec_reductions`). The dropped kinds fell through the silent `_ => {}`
catch-all: no codegen emitted, the result SSA value left undefined (resolved to
the None sentinel), and every in-function accumulator loop silently miscompiled
(`TypeError: 'NoneType' object is not subscriptable` downstream). Fixed in
0323ad28c; the dispatch<->handler mirror is now derived from a single source of
truth (`fc::op_family`), but this test pins the behavior so any future drop of a
vec_* family fails the differential suite loudly rather than miscompiling.

Each accumulator loop below is the exact AST shape the frontend recognizes as a
vector reduction (single-statement body), so these functions exercise the
vec_sum/vec_prod/vec_min/vec_max kernels. A kernel must return exactly what the
loop computes -- IEEE float additions in iteration order (not builtin sum()'s
compensated total), arbitrary-precision ints, the loop's own int/float
promotion, the element object a min/max keeps, the loop target's final value --
over each bounded chunk it runs, or decline an item, which the ordinary loop
then runs, with every later item, callback, destructor and exception in
CPython's order, on the same iterator.

A fused reduction reads its accumulator only as the loop would: never when the
loop runs zero times, and an unbound one raises at the first iteration.
"""


# --- sum, int, over range(n) (the exact `total += i` bug repro) ---
def sum_range_aug(n):
    total = 0
    for i in range(n):
        total += i
    return total


def sum_range_assign(n):
    total = 0
    for i in range(n):
        total = total + i
    return total


# --- sum, int, over a list (iterator reduction) ---
def sum_list(xs):
    total = 0
    for v in xs:
        total += v
    return total


# --- sum, int, indexed over range(len(xs)) ---
def sum_indexed(xs):
    total = 0
    for i in range(len(xs)):
        total += xs[i]
    return total


# --- sum, float, over a list ---
def sum_float_list(xs):
    total = 0.0
    for v in xs:
        total += v
    return total


# --- sum, float, over range(n) (float accumulator over int range) ---
def sum_float_range(n):
    total = 0.0
    for i in range(n):
        total += i
    return total


# --- product, int, over range(1, n) ---
def prod_range(n):
    p = 1
    for i in range(1, n):
        p *= i
    return p


# --- product, int, over a list ---
def prod_list(xs):
    p = 1
    for v in xs:
        p = p * v
    return p


# --- min, int, over a list ---
def min_list(xs):
    m = xs[0]
    for v in xs:
        if v < m:
            m = v
    return m


# --- max, int, over a list ---
def max_list(xs):
    m = xs[0]
    for v in xs:
        if m < v:
            m = v
    return m


# A downstream index of a reduction result: this is precisely what crashed in
# the original bug (the None-sentinel result was indexed next).
def sum_then_index(n, table):
    total = sum_range_aug(n)
    return table[total % len(table)]


# --- a loop that never runs never reads its (unbound) accumulator ---
def sum_list_unbound_empty():
    xs = []
    for v in xs:
        total += v
    return "list"


def sum_range_unbound_empty():
    for i in range(0):
        total += i
    return "range"


def sum_indexed_unbound_empty():
    xs = []
    for i in range(len(xs)):
        total += xs[i]
    return "indexed"


def sum_list_unbound_first_item():
    xs = [1]
    try:
        for v in xs:
            total += v
    except UnboundLocalError:
        return "UnboundLocalError"
    return "no error"


nums = [3, 1, 4, 1, 5, 9, 2, 6]
floats = [1.0, 2.0, 3.0, 4.0, 5.0]  # sum 15.0, order-independent in f64
table = [10, 20, 30, 40, 50]

print(
    "sum_range_aug:",
    sum_range_aug(0),
    sum_range_aug(1),
    sum_range_aug(10),
    sum_range_aug(100),
)
print(
    "sum_range_assign:", sum_range_assign(0), sum_range_assign(1), sum_range_assign(100)
)
print("sum_list:", sum_list([]), sum_list([7]), sum_list(nums))
print("sum_indexed:", sum_indexed([]), sum_indexed(nums))
print("sum_float_list:", sum_float_list([]), sum_float_list(floats))
print("sum_float_range:", sum_float_range(0), sum_float_range(101))
print("prod_range:", prod_range(1), prod_range(2), prod_range(6))
print("prod_list:", prod_list([5]), prod_list([1, 2, 3, 4]))
print("min_list:", min_list([7]), min_list(nums))
print("max_list:", max_list([7]), max_list(nums))
print("sum_then_index:", sum_then_index(10, table), sum_then_index(7, table))
print(
    "unbound accumulator:",
    sum_list_unbound_empty(),
    sum_range_unbound_empty(),
    sum_indexed_unbound_empty(),
    sum_list_unbound_first_item(),
)

# Type fidelity: a float reduction must stay float, an int reduction int.
print("types:", type(sum_list(nums)).__name__, type(sum_float_list(floats)).__name__)


# --- The kernels compute exactly what the loop computes ---


def sum_from(xs, start):
    total = start
    for v in xs:
        total += v
    return total


def sum_reversed(xs, start):
    total = start
    for v in xs:
        total = v + total
    return total


def prod_from(xs, start):
    p = start
    for v in xs:
        p *= v
    return p


def min_from(xs, start):
    m = start
    for v in xs:
        if v < m:
            m = v
    return m


def max_from(xs, start):
    m = start
    for v in xs:
        if v > m:
            m = v
    return m


def sum_or_error(xs, start):
    try:
        return sum_from(xs, start)
    except OverflowError as exc:
        return "OverflowError: " + str(exc)


print(
    "float order:",
    repr(sum_from([0.1] * 10, 0.0)),
    repr(sum([0.1] * 10)),
    repr(sum_reversed([0.1, 0.2, 0.3], 0.0)),
    repr(sum_from([1e16, 1.0, -1e16], 0.0)),
    repr(sum_from([1.0, 1e100, 1.0, -1e100], 0.0)),
)
print(
    "signed zero:",
    repr(sum_from([-0.0], -0.0)),
    repr(sum_from([-0.0], 0)),
    repr(sum_from([0.0], -0.0)),
    repr(prod_from([-1.0, 0.0], 1)),
    repr(sum_from((), -0.0)),
)
print(
    "specials:",
    sum_from([float("inf"), 1.0], 0.0),
    sum_from([float("inf"), -float("inf")], 0.0),
    sum_from([1e308, 1e308], 0.0),
    sum_from([float("nan"), 1.0], 0.0),
    prod_from([1e200, 1e200], 1),
)
big = 2**62
print(
    "int overflow:",
    sum_from([big, big, big], 0),
    sum_from([2**46 - 1, 1], 0),
    sum_from([-(2**63), -1], 0),
    sum_from([10**30, -(10**30), 5], 0),
    sum_reversed([big, big], big),
)
print(
    "int types:",
    type(sum_from([big, big], 0)).__name__,
    type(sum_from([True, True, False], 0)).__name__,
    sum_from([True, True, False], 0),
    sum_from([True], False),
)
print(
    "product overflow:",
    prod_from([2**40, 2**40, 2**40], 1),
    prod_from([-(2**32)] * 3, 1),
    prod_from([0, 10**50], 1),
    prod_from([True, 2], 3),
)
print(
    "promotion:",
    repr(sum_from([1, 2.5, 3], 0)),
    repr(sum_from([2.5, 3], 1)),
    repr(sum_from([2**53, 1, 1.0], 0)),
    repr(prod_from([3, 0.5], 1)),
)
print(
    "big int meets float:",
    sum_or_error([10**400, 1.0], 0),
    sum_or_error([1.0, 10**400], 0),
    repr(sum_or_error([10**300, 1.5], 0)),
)

nan = float("nan")
a, b = 10**30, 10**30
print(
    "min/max:",
    min_from([3, 1, 2], 10),
    max_from([3, 1, 2], -10),
    min_from([nan, 1.0], 5.0),
    min_from([1.0, nan, 0.5], 5.0),
    max_from([1, 1.0], 0),
    max_from([1.0, 1], 0),
    min_from([2**70, -(2**70)], 0),
    max_from([], nan),
)
print(
    "min/max keep objects:",
    max_from([a, b], 0) is a,
    min_from([b, a], 10**31) is b,
    type(max_from([True, 1], 0)).__name__,
    type(min_from([0.0, 0], 1)).__name__,
)


# --- The loop target keeps the loop's final binding ---


def sum_with_target(xs):
    v = "before"
    total = 0
    for v in xs:
        total += v
    return total, v


def sum_indexed_target(xs):
    i = "before"
    total = 0
    for i in range(len(xs)):
        total += xs[i]
    return total, i


def sum_range_target(start, stop, step, acc):
    i = "before"
    total = acc
    for i in range(start, stop, step):
        total += i
    return total, i


def sum_unbound_target_after_empty():
    total = 0
    for v in []:
        total += v
    try:
        return total, v
    except UnboundLocalError:
        return total, "UnboundLocalError"


def target_release_order():
    seen = []
    total = 0

    class Spy:
        def __del__(self):
            seen.append(("del", total))

    v = Spy()
    for v in [1, 2, 3]:
        total += v
    return total, v, seen


print(
    "target:",
    sum_with_target([1, 2, 3]),
    sum_with_target([]),
    sum_with_target((4.5,)),
    sum_unbound_target_after_empty(),
)
print("indexed target:", sum_indexed_target([5, 6, 7]), sum_indexed_target([]))
print(
    "range target:",
    sum_range_target(3, 10, 2, 0),
    sum_range_target(10, 0, -3, 0),
    sum_range_target(0, 0, 1, 0),
    sum_range_target(0, 5, 1, 0.1),
    sum_range_target(2**62, 2**62 + 3, 1, 0),
    sum_range_target(0, 3, 1, -0.0),
)
# The previous target is released when the loop first rebinds it, before
# any later update of the accumulator.
print("target release:", target_release_order())


# --- Callbacks, subclasses and errors run where the loop runs them ---

events = []


class LoudInt(int):
    def __radd__(self, other):
        events.append(("LoudInt.__radd__", other, int(self)))
        return other + int(self)


class LoudFloat(float):
    def __radd__(self, other):
        events.append(("LoudFloat.__radd__", other, float(self)))
        return other + float(self)


class Loud:
    def __init__(self, value):
        self.value = value

    def __radd__(self, other):
        events.append(("Loud.__radd__", other, self.value))
        return other + self.value

    def __lt__(self, other):
        events.append(("Loud.__lt__", self.value, other))
        return self.value < other


def sum_until_error(xs):
    total = 0
    v = None
    try:
        for v in xs:
            total += v
    except TypeError as exc:
        return "TypeError", total, v, str(exc)
    return total, v


def sum_from_subclass_start():
    total = LoudInt(10)
    for v in [1, 2]:
        total += v
    return total, type(total).__name__


print("int subclass items:", sum_from([1, LoudInt(2), 3], 0), events[:])
events.clear()
print("float subclass items:", sum_from([1.5, LoudFloat(2.5)], 0.0), events[:])
events.clear()
print("custom items:", sum_from([1, Loud(2), 3], 0), events[:])
events.clear()
print("custom min:", min_from([3, Loud(1)], 2).value, events[:])
events.clear()
print("subclass accumulator:", sum_from_subclass_start(), events[:])
events.clear()
print("error mid-loop:", sum_until_error([1, 2, "x", 4]), sum_until_error([1, None]))


# --- chunks: a long loop runs in bounded chunks, then the ordinary loop ---
def sum_items(xs):
    total = 0
    for v in xs:
        total += v
    return total, v


def product_items(xs):
    total = 1.0
    for v in xs:
        total = total * v
    return total, v


def smallest(xs):
    best = xs[0]
    for v in xs:
        if v < best:
            best = v
    return best, v


# Float additions stay sequential across chunk boundaries.
print("long float sum:", repr(sum_items([0.1] * 10000)))
print("long product:", repr(product_items([1.0001] * 9000)))
# An int total meets a float in a later chunk.
print("promotion after chunks:", repr(sum_items([1] * 5000 + [0.5] + [1] * 5000)))
# The minimum sits at the very end, several chunks in.
print("long min:", smallest(list(range(9000, 0, -1))))
# The kernel declines an item after earlier chunks: the ordinary loop runs it,
# once, and every later item; nothing consumed is read again.
print(
    "decline after chunks:",
    sum_items(list(range(5000)) + [Loud(1)] + list(range(3000))),
    events[:],
)
events.clear()


class Grower:
    def __init__(self, items):
        self.items = items

    def __radd__(self, other):
        events.append(("grow", len(self.items)))
        self.items.extend([1, 2, 3])
        return other + 100


def sum_growing():
    items = list(range(6000))
    items.append(Grower(items))
    total = 0
    for v in items:
        total += v
    return total, v, len(items)


# The ordinary loop continues on the loop's own iterator, so items the list
# gains while it runs are still iterated.
print("list grows after chunks:", sum_growing(), events[:])
events.clear()
print("tuple chunks:", sum_items(tuple(range(20000))))
print("range chunks:", sum_items(range(10**6, 10**6 + 9000)))
print("bool chunks:", sum_items([True, False] * 5000))


# Selected iteration identity must survive materialization and chunk publication.
# Each body retains the frontend's fused min/max shape, with both bindings live.
def minimum_with_identity(items, start):
    best = start
    value = None
    for value in items:
        if value < best:
            best = value
    return best, value


def maximum_with_identity(items, start):
    best = start
    value = None
    for value in items:
        if best < value:
            best = value
    return best, value


identity_base = 2**62
for identity_count in (1, 4096, 4097):
    ascending = range(identity_base, identity_base + identity_count)
    descending = range(identity_base + identity_count, identity_base, -1)
    for identity_source, highs, lows in (
        ("range", ascending, descending),
        ("list", list(ascending), list(descending)),
        ("tuple", tuple(ascending), tuple(descending)),
    ):
        maximum, last_max = maximum_with_identity(highs, identity_base - 1)
        minimum, last_min = minimum_with_identity(lows, identity_base + identity_count + 1)
        print(
            "min/max final occurrence:", identity_source, identity_count,
            maximum is last_max, minimum is last_min,
            maximum == identity_base + identity_count - 1, minimum == identity_base + 1,
        )

# Equal values cannot stand in for occurrence identity. A tied initial object
# survives unchanged; two distinct equal items stay distinct, and a repeated
# reference remains shared even when the winning and last positions differ.
identity_first = int(str(identity_base))
identity_equal = int(str(identity_base))
for identity_name, reduce_with_identity, start in (
    ("min", minimum_with_identity, identity_base + 1),
    ("max", maximum_with_identity, identity_base - 1),
):
    winner, last = reduce_with_identity((identity_first, identity_equal), start)
    print("min/max tied objects:", identity_name, winner is identity_first, winner is last)
    winner, last = reduce_with_identity((identity_first, identity_first), start)
    print("min/max repeated object:", identity_name, winner is identity_first, winner is last)
    winner, last = reduce_with_identity(range(identity_base, identity_base + 1), identity_first)
    print("min/max original accumulator:", identity_name, winner is identity_first, winner is last)

# Tie identity crosses a real chunk boundary and survives later container reads.
for identity_name, reduce_with_identity, start in (
    ("min", minimum_with_identity, identity_base + 1),
    ("max", maximum_with_identity, identity_base - 1),
):
    tied = [int(str(identity_base)) for _ in range(4097)]
    winner, last = reduce_with_identity(tied, start)
    print("min/max prior chunk tie:", identity_name,
          winner is tied[0], last is tied[-1], winner is last)
    repeated = [identity_first] * 4097
    winner, last = reduce_with_identity(repeated, start)
    print("min/max prior chunk repeated:", identity_name,
          winner is repeated[0], last is repeated[-1], winner is last)


def zero_count_bindings(items, start, original_target):
    best = start
    value = original_target
    for value in items:
        if value < best:
            best = value
    return best is start, value is original_target


print("min/max zero-count bindings:", zero_count_bindings([], identity_first, identity_equal))

