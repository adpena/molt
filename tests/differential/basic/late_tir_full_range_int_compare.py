"""Purpose: a range counter whose bound is unknown is a full-range raw int.
The late-TIR stage compares it with a boxed word in an int arm and leaves
every other operand kind (float, NaN, big int, bool, int subclass, a type
with its own comparison, a type that raises) to the runtime (HF-821)."""


class Wide(int):
    pass


class Always:
    def __init__(self, answer):
        self.answer = answer

    def __gt__(self, other):
        return self.answer

    def __lt__(self, other):
        return self.answer

    def __ge__(self, other):
        return self.answer

    def __le__(self, other):
        return self.answer

    def __eq__(self, other):
        return self.answer

    def __ne__(self, other):
        return self.answer


def count(n):
    total = 0
    for i in range(n):
        total += i
    return total


def compare_all(n, bound):
    lt = le = gt = ge = eq = ne = 0
    for i in range(n):
        if i < bound:
            lt += 1
        if i <= bound:
            le += 1
        if i > bound:
            gt += 1
        if i >= bound:
            ge += 1
        if i == bound:
            eq += 1
        if i != bound:
            ne += 1
    return lt, le, gt, ge, eq, ne


def reversed_operands(n, bound):
    lt = ge = eq = 0
    for i in range(n):
        if bound < i:
            lt += 1
        if bound >= i:
            ge += 1
        if bound == i:
            eq += 1
    return lt, ge, eq


def from_start(start, stop, bound):
    hits = 0
    last = None
    for i in range(start, stop):
        if i < bound:
            hits += 1
        last = i
    return hits, last


print(count(0), count(1), count(10), count(True), count(Wide(7)), count(-5))
for bound in (
    5,
    -1,
    0,
    2**46 - 1,
    2**46,
    -(2**46),
    2**63 - 1,
    2**63,
    -(2**63) - 1,
    10**30,
    4.5,
    5.0,
    -0.0,
    float("inf"),
    float("-inf"),
    float("nan"),
    True,
    False,
    Wide(5),
    Always(True),
    Always(False),
):
    print(type(bound).__name__, compare_all(10, bound), reversed_operands(10, bound))

print(from_start(2**46 - 3, 2**46 + 3, 2**46))
print(from_start(2**62, 2**62 + 4, 2**62 + 2))
print(from_start(-(2**46) - 2, -(2**46) + 2, -(2**46)))
print(from_start(2**63 - 4, 2**63 - 1, 2**63 - 2))
print(from_start(2**63 - 2, 2**63 + 2, 2**63))
print(from_start(-3, 3, 0.5))

for bad in ("5", None, [1], 1j):
    try:
        compare_all(3, bad)
    except TypeError as exc:
        print("TypeError", exc)
    print(compare_all(0, bad))
    try:
        reversed_operands(3, bad)
    except TypeError as exc:
        print("TypeError", exc)
