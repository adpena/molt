"""Purpose: differential coverage proving the in-place dunder routing fix does
NOT change builtin-type augmented-assignment semantics. Builtin int/float/str/
list/bytearray/set define no numeric in-place dunders (or, for list/set, define
ones whose result equals the binary form), so //=, /=, %=, **=, <<=, >>=, @=, and
the already-correct +=, -=, *=, |=, &=, ^= must remain byte-identical.

This is the perf-lane guard: the fast int/float lanes are reused unchanged by the
inplace ops, so their results must match exactly (including BigInt promotion,
negative floor semantics, float division, and overflow).
"""


def show(label, value):
    print(label, repr(value))


# ---- int: floor division, modulo, power, shifts (incl. negatives). ----
a = 17
a //= 3
show("int //=", a)

b = -17
b //= 3
show("int //= neg", b)

c = 17
c %= 5
show("int %=", c)

d = -17
d %= 5
show("int %= neg", d)

e = 2
e **= 10
show("int **=", e)

# Shifts, including the bigint-promotion boundary of `<<` (shift past i64). The
# in-place `<<=`/`>>=` fast lane is the SAME emitter as the binary `<<`/`>>`, so
# these results must be byte-identical including BigInt promotion. (The raw I64
# shift lane is now gated on the value-range RawI64Safe proof — count proven in
# [0, 63] AND result fits inline — so an overflowing `<<=` bails to the
# BigInt-correct boxed runtime; see shift_overflow_matrix.py for the full lane
# contract. This deliberately exercises that boundary in the in-place spelling.)
f = 1
f <<= 20
show("int <<=", f)

# In-place `<<=` PAST the i64 window: must promote to a bigint, not wrap.
fbig = 1
fbig <<= 80
show("int <<= bigint", fbig)

fbig2 = 0xFF
fbig2 <<= 100
show("int <<= bigint 2", fbig2)

g = 1024
g >>= 4
show("int >>=", g)

# In-place `>>=` of a bigint back down into the i64 window.
gbig = 1 << 90
gbig >>= 85
show("int >>= from bigint", gbig)

h = 0xFF
h <<= 8
h >>= 4
show("int <<= then >>=", h)

# `<<=` then `>>=` straddling the i64 boundary in both directions.
hbig = 1
hbig <<= 70
hbig >>= 5
show("int <<= then >>= across i64", hbig)

# Accumulating loop to exercise the hot int fast lane for //= and %=.
acc = 1_000_000
loop_sum = 0
for i in range(1, 50):
    acc //= 1
    loop_sum += acc % 97
show("int loop //= acc", acc)
show("int loop %= sum", loop_sum)


# ---- float: true division, floor division, modulo, power. ----
fa = 7.0
fa /= 2.0
show("float /=", fa)

fb = 7.5
fb //= 2.0
show("float //=", fb)

fc = 7.5
fc %= 2.0
show("float %=", fc)

fd = 2.0
fd **= 0.5
show("float **=", fd)

fe = 10
fe /= 4  # int /= int -> float in CPython
show("int /= int -> float", fe)


# ---- str: += stays correct (already-wired inplace op, regression guard). ----
s = "a"
s += "bc"
show("str +=", s)
s *= 3
show("str *=", s)


# ---- list: += extends in place (list.__iadd__), *= repeats. ----
lst = [1, 2]
lst += [3, 4]
show("list +=", lst)
lst *= 2
show("list *=", lst)


# ---- set: |=, &=, ^= in place. ----
st = {1, 2, 3}
st |= {3, 4}
show("set |=", sorted(st))
st &= {2, 3, 4}
show("set &=", sorted(st))
st ^= {3, 99}
show("set ^=", sorted(st))


# ---- bool participates as int subtype. ----
bt = True
bt <<= 3
show("bool <<=", bt)

# Declaring-parent descriptors consume int storage even for bool receivers;
# bool's own overrides preserve bool only when both operands are bool.
for name in ("__and__", "__or__", "__xor__"):
    for owner in (int, bool):
        for left, right in ((True, True), (True, False), (True, 2)):
            result = getattr(owner, name)(left, right)
            print("declared-bit", owner.__name__, name, type(result).__name__, result)

import warnings

for action in ("ignore", "error"):
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter(action, DeprecationWarning)
        for owner in (int, bool):
            try:
                result = owner.__invert__(True)
                print("declared-invert", action, owner.__name__, type(result).__name__, result)
            except DeprecationWarning:
                print("declared-invert", action, owner.__name__, "DeprecationWarning")
        print("captured-invert", len(caught))

# A set slot declines a view; fallback produces a new set and retains the alias.
import operator

for operation, expected in ((operator.ior, {1, 2, 3}),
                            (operator.iand, {2}),
                            (operator.isub, {1}),
                            (operator.ixor, {1, 3})):
    for make in (set, frozenset):
        original = make((1, 2))
        alias = original
        result = operation(original, {2: None, 3: None}.keys())
        assert result == expected and result is not alias
        assert alias == make((1, 2))
        print("view-iop-alias", make.__name__, operation.__name__, sorted(result),
              type(result).__name__, sorted(alias))

original = {"before": 1}
alias = original
result = operator.ior(original, [("next", 2)])
assert result is alias and alias == {"before": 1, "next": 2}
print("dict-pairs-alias", result is alias, sorted(alias.items()))
try:
    operator.ior(original, [("committed", 3), ("broken",)])
except ValueError:
    assert original == {"before": 1, "next": 2, "committed": 3}
    print("dict-pairs-partial", sorted(original.items()))
else:
    raise AssertionError("late malformed pair must fail")
assert dict.__or__({}, []) is NotImplemented

# View intersection/xor must cancel before hashing unhashable item values.
view_left = {1: []}.items()
view_right = {1: []}.items()
assert view_left & [] == set()
assert view_left ^ view_right == set()
print("items-cancel-unhashable", len(view_left & []), len(view_left ^ view_right))
for left, right, expected in (({1: 0, 2: 0}.keys(), [2, 3], {1}),
                              ([1, 3], {1: 0, 2: 0}.keys(), {3})):
    result = left - right
    assert result == expected
    print("view-sub-order", sorted(result))
try:
    {1: 2}.values() | set()
except TypeError:
    print("values-no-numeric")
else:
    raise AssertionError("dict values do not declare set arithmetic")

hash_events = []


class ViewKey:
    def __hash__(self):
        hash_events.append("hash")
        return 17


view_key = ViewKey()
first = {view_key: []}
second = {view_key: []}
hash_events.clear()
assert first.items() ^ second.items() == set()
assert hash_events == []
print("items-xor-known-hash", hash_events)
