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
