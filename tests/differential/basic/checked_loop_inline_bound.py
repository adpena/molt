"""Purpose: a loop whose bound is a loop-invariant exact int runs a fast copy
that reads the bound's inline payload, entered by one tag test; a big-int
bound runs the slow copy from the start (HF-857). In the fast copy the
bound limits the counter and the trip count, so a bounded-step accumulator
takes no overflow check. Each case pins a value the two copies or the
range proof could get wrong: big-int and boundary bounds, accumulators that
start past the inline window or leave i64, and a bound also used as an
object."""

INLINE_TOP = 2**46 - 1


def masked_sum(n):
    i = 0
    total = 0
    while i < n:
        total += i & 7
        i += 1
    return total, i


def masked_sum_from(start, n):
    i = 0
    total = start
    while i < n:
        total += i & 7
        i += 1
    return total


def early_exit(n, stop):
    i = 0
    total = 0
    while i < n:
        total += i & 3
        if i == stop:
            break
        i += 1
    return total, i


def bound_also_object(n):
    i = 0
    total = 0
    seen = []
    while i < n:
        total += i & 1
        if i == 2:
            seen.append(n)
        i += 1
    return total, seen, type(n).__name__


def bound_in_sum(n):
    i = 0
    total = 0
    while i < n:
        total += n
        i += 1
    return total


def two_bounds(n, m):
    i = 0
    total = 0
    while i < n and i < m:
        total += i & 15
        i += 1
    return total, i


def stepped(n, step):
    i = 0
    count = 0
    while i < n:
        count += 1
        i += step
    return count, i


for n in (0, 1, 7, 8, 1000, -5, 10**5):
    print("masked", n, masked_sum(n))

# The accumulator starts past the inline window but stays in i64; and one
# that starts near the top of i64 leaves it within a few steps.
print("from 2**50", masked_sum_from(2**50, 1000))
print("from -2**50", masked_sum_from(-(2**50), 1000))
print("from i64 top", masked_sum_from(2**63 - 20, 100))
print("from i64 bottom", masked_sum_from(-(2**63), 100))
print("from big", masked_sum_from(2**80, 50))

# Bounds at and past the inline window, and big ints, exit early.
for n in (INLINE_TOP, INLINE_TOP + 1, 2**62, 2**63 - 1, 2**63, 2**70, -(2**70)):
    print("early", n, early_exit(n, 500))

print("object", bound_also_object(5))
print("bound in sum", bound_in_sum(1000), bound_in_sum(0), bound_in_sum(-3))
print("two bounds", two_bounds(100, 40), two_bounds(40, 100), two_bounds(2**70, 30))
print("stepped", stepped(1000, 3), stepped(10, 7), stepped(2**65, 2**64))
print("float bound", masked_sum(10.5))
print("bool bound", masked_sum(True))


# A range loop's stop is `n.__index__()`, an exact int of any magnitude: the
# same guard applies, and in the fast copy `i * 3 - (i >> 1)` leaves the
# inline window but not i64 (a subtraction with no checked form).
def range_mix(n, stop):
    total = 0
    for i in range(n):
        total += i * 3 - (i >> 1)
        total += i - 1
        if i == stop:
            break
    return total


def range_from(start, n):
    total = start
    for i in range(n):
        total -= i * 5 - 7
    return total


for n in (0, 1, 2, -3, 1000, 10**5):
    print("range", n, range_mix(n, -1))
for n in (INLINE_TOP, INLINE_TOP + 1, 2**63, 2**80):
    print("range early", n, range_mix(n, 300))
print("range from", range_from(2**62, 1000), range_from(-(2**63), 1000), range_from(2**90, 10))
