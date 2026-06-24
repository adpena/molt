"""Purpose: a chained int-zero init (a = b = 0) whose targets later accumulate
floats must promote every target to float, including the SECOND+ chained
targets that lower through a `binding_alias` seed.

Regression for a native-backend silent-wrong-answer: the second chained target's
`binding_alias` seed was an int-primary raw-i64 carrier, but the alias codegen
stored a NaN-boxed value into that raw-i64 Variable (it only honored the
float-primary lane, not int/bool). Every downstream raw read then reinterpreted
the NaN-box bits as a scalar, so the accumulator froze near 2^63 instead of
tracking the float sum. spectral_norm's `vv` printed ~9.2e18 instead of its true
value (the `sqrt(vBv/vv)` ratio collapsed to ~1e-6).
"""


def chained_pair():
    a = b = 0
    data = [1.5, 2.5, 3.5, 4.5]
    other = [10.0, 20.0, 30.0, 40.0]
    for x, y in zip(data, other):
        a += x * y
        b += y * y
    return a, b


def chained_triple():
    a = b = c = 0
    for y in [10.0, 20.0, 30.0, 40.0]:
        a += y * y
        b += y * y
        c += y * y
    return a, b, c


def separate_pair():
    a = 0
    b = 0
    for y in [10.0, 20.0, 30.0, 40.0]:
        a += y * y
        b += y * y
    return a, b


def chained_int_stays_int():
    # The same chained-init shape but accumulating ints must remain exact ints
    # (no float promotion, no representation drift).
    a = b = 0
    for n in range(5):
        a += n
        b += n * 2
    return a, b


print(chained_pair())
print(chained_triple())
print(separate_pair())
print(chained_int_stays_int())

# Direct spectral_norm-shaped accumulator: vBv (first target) and vv (second
# chained target) accumulate floats from a zip; the ratio must be finite.
from math import sqrt  # noqa: E402

vBv = vv = 0
us = [4.0, 9.0, 16.0, 25.0]
vs = [2.0, 3.0, 4.0, 5.0]
for ue, ve in zip(us, vs):
    vBv += ue * ve
    vv += ve * ve
print("%0.9f" % sqrt(vBv / vv))
print(vBv, vv)
