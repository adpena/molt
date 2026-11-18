"""Floor division and modulo on values whose type the compiler cannot see.

The operands come out of a list, so they reach `//` and `%` as boxed words
and take the shared fast arms (inline ints, floats, mixed) or the runtime.
"""


def pairs():
    ints = [7, -7, 3, -3, 1, -1, 0, 2**46 - 1, -(2**46), 2**70, -(2**70), True, False]
    floats = [7.5, -7.5, 0.5, -0.0, 0.0, 1e308, float("inf"), float("nan")]
    values = ints + floats
    return [(a, b) for a in values for b in values]


def show(op, a, b):
    try:
        result = op(a, b)
    except ZeroDivisionError as error:
        return f"ZeroDivisionError({error})"
    return repr(result)


def floordiv(a, b):
    return a // b


def mod(a, b):
    return a % b


def inplace(a, b):
    x = a
    x //= b
    y = a
    y %= b
    return (x, y)


for a, b in pairs():
    print(repr(a), repr(b), show(floordiv, a, b), show(mod, a, b), show(inplace, a, b))

total = 0
divisors = [3, -5, 7, -11]
for i in range(-50, 50):
    d = divisors[i & 3]
    total += i // d + i % d
print(total)
