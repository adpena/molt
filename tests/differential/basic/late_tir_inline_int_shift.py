"""Purpose: the late-TIR stage shifts two exact ints inline: `>>` when the
count is in [0, 63] and the value fits the inline window, `<<` as a checked
product with a power of two. Every other case is the runtime's: a negative
count raises ValueError, a large count or product reaches a big int, a
non-int operand runs the operator protocol (HF-821)."""


class Wide(int):
    pass


class Shifty:
    def __lshift__(self, other):
        return ("lshift", other)

    def __rshift__(self, other):
        return ("rshift", other)

    def __rlshift__(self, other):
        return ("rlshift", other)

    def __rrshift__(self, other):
        return ("rrshift", other)


def shifts(value, count):
    return value << count, value >> count


def constant_counts(value):
    return (
        value << 0,
        value << 1,
        value << 3,
        value << 45,
        value << 46,
        value << 62,
        value << 63,
        value << 64,
        value >> 0,
        value >> 1,
        value >> 3,
        value >> 46,
        value >> 62,
        value >> 63,
        value >> 64,
        value >> 200,
    )


def constant_values(count):
    return 1 << count, -1 << count, 12345 >> count, -12345 >> count, 0 << count


def counter_shifts(n):
    right = left = both = 0
    for i in range(n):
        right += i >> 1
        left += i << 2
        both += (i << 3) >> 2
    return right, left, both


def counter_from(start, stop, count):
    out = []
    for i in range(start, stop):
        out.append((i >> count, i << count))
    return out


def variable_count(n):
    total = 0
    for i in range(n):
        total += (1 << i) + (-7 >> i) + (i << i)
    return total


VALUES = (
    0,
    1,
    -1,
    2,
    -2,
    7,
    -7,
    12345,
    -12345,
    2**31,
    -(2**31),
    2**45,
    2**46 - 1,
    2**46,
    -(2**46),
    -(2**46) - 1,
    2**62,
    2**63 - 1,
    2**63,
    -(2**63),
    -(2**63) - 1,
    10**30,
    -(10**30),
    True,
    False,
    Wide(5),
    Wide(-5),
)
COUNTS = (0, 1, 2, 3, 16, 17, 31, 32, 44, 45, 46, 47, 62, 63, 64, 65, 100, True, False, Wide(3))

for value in VALUES:
    print(type(value).__name__, value, constant_counts(value))
    for count in COUNTS:
        print(shifts(value, count))

for count in COUNTS:
    print(count, constant_values(count))

for value in (5, -5, 0, 2**46, 10**30, True):
    for count in (-1, -64, -(2**70), Wide(-1)):
        try:
            print(shifts(value, count))
        except ValueError as exc:
            print("ValueError", exc)
        try:
            print(value >> count)
        except ValueError as exc:
            print("ValueError", exc)

for count in (-1, -3):
    try:
        print(constant_values(count))
    except ValueError as exc:
        print("ValueError", exc)

print(counter_shifts(0), counter_shifts(1), counter_shifts(100), counter_shifts(Wide(9)))
print(counter_from(-4, 4, 1))
print(counter_from(2**46 - 2, 2**46 + 2, 1))
print(counter_from(-(2**46) - 2, -(2**46) + 2, 3))
print(counter_from(2**62 - 2, 2**62 + 2, 1))
print(counter_from(2**63 - 3, 2**63 - 1, 2))
print(counter_from(2**63 - 1, 2**63 + 2, 1))
print(counter_from(-3, 3, 45))
print(counter_from(-3, 3, 46))
print(counter_from(-3, 3, 63))
print(counter_from(-3, 3, 64))
print(variable_count(70))
try:
    print(counter_from(-3, 3, -1))
except ValueError as exc:
    print("ValueError", exc)

for bad in (1.5, "2", None, [1]):
    for value in (3, 2**70, True):
        try:
            print(shifts(value, bad))
        except TypeError as exc:
            print("TypeError", exc)
        try:
            print(shifts(bad, value))
        except TypeError as exc:
            print("TypeError", exc)
        except (OverflowError, MemoryError) as exc:
            print(type(exc).__name__)

print(shifts(Shifty(), 3), 3 << Shifty(), 3 >> Shifty())
print(2**70 >> 60, -(2**70) >> 60, 2**70 >> 70, -(2**70) >> 71, (2**70 + 1) << 3)
