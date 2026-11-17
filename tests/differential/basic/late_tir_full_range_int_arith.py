"""Purpose: a range counter whose bound is unknown is a full-range raw int.
The late-TIR stage adds, subtracts and multiplies it with an overflow check
and leaves an overflowing result, a float, a big int and every other operand
kind to the runtime (HF-821)."""


class Wide(int):
    pass


class Adder:
    def __add__(self, other):
        return ("add", other)

    def __radd__(self, other):
        return ("radd", other)

    def __sub__(self, other):
        return ("sub", other)

    def __rsub__(self, other):
        return ("rsub", other)

    def __mul__(self, other):
        return ("mul", other)

    def __rmul__(self, other):
        return ("rmul", other)


def constants(start, stop):
    out = []
    for i in range(start, stop):
        out.append(
            (
                i + 1,
                i - 1,
                1 - i,
                i * 3,
                i + (2**46 - 1),
                i - (-(2**46)),
                i - 2**46,
                i + 2**62,
                i - 2**62,
                i * -1,
                i * 2**40,
                i + i,
                i - i,
                i * i,
            )
        )
    return out


def with_operand(start, stop, other):
    out = []
    for i in range(start, stop):
        out.append((i + other, other + i, i - other, other - i, i * other, other * i))
    return out


def inplace(n, step):
    total = 0
    down = 0
    product = 1
    for i in range(n):
        total += i - 1
        down -= i + step
        product *= i + 2
    return total, down, product


def accumulate(n):
    total = 0
    for i in range(n):
        total = total + i * 3 - 1
    return total


print(constants(-3, 3))
print(constants(2**46 - 2, 2**46 + 2))
print(constants(-(2**46) - 2, -(2**46) + 2))
print(constants(2**62 - 2, 2**62 + 2))
print(constants(-(2**62) - 2, -(2**62) + 2))
print(constants(2**63 - 3, 2**63 - 1))
print(constants(-(2**63), -(2**63) + 3))
print(constants(2**63 - 1, 2**63 + 2))

for other in (
    0,
    1,
    -1,
    7,
    2**46 - 1,
    2**46,
    -(2**46),
    2**62,
    2**63 - 1,
    -(2**63),
    2**63,
    10**30,
    2.5,
    -0.0,
    float("inf"),
    float("nan"),
    True,
    False,
    Wide(4),
    Adder(),
):
    print(type(other).__name__, with_operand(-2, 3, other))
    print(with_operand(2**63 - 3, 2**63 - 1, other))
    print(with_operand(-(2**63), -(2**63) + 2, other))

print(inplace(0, 1), inplace(1, 1), inplace(25, 2**62), inplace(Wide(6), Wide(2)))
print(accumulate(0), accumulate(10), accumulate(1000))

for bad in ("5", None, [1]):
    try:
        with_operand(0, 2, bad)
    except TypeError as exc:
        print("TypeError", exc)
    print(with_operand(0, 0, bad))
