"""Purpose: module-level code passes heap ints and heap floats to calls safely.

Module bodies compile as module chunks. An int past the inline window but
inside the i64 range boxes to a heap int, and a NaN float boxes to a heap
float. A call adopts its argument, so each call site must hand it its own
reference. A chunk that handed one hoisted literal to several calls freed it
after the first (use-after-free at the next release). Int and float identity
across a call is a CPython detail, so the test compares values.
"""


def ident(x):
    return x


def keep_first(items, start):
    best = start
    for value in items:
        if best < value:
            best = value
    return best, value


s = 2**46
for _ in range(5):
    ident(s)
print(s)

wide = 2**62 - 1
t = ident(wide)
t = ident(wide)
print(t, t == wide)

base = 2**62
for count in (1, 2, 2):
    highs = list(range(base, base + count))
    m, last = keep_first(highs, base - 1)
    print(count, m is last, m == base + count - 1)

nan = float("nan")
for _ in range(3):
    ident(nan)
print(nan != nan, ident(nan) != ident(nan))

for value in (2**46, -(2**46) - 1, 2**63 - 1, -(2**63)):
    print(ident(value), ident(value) == value)
