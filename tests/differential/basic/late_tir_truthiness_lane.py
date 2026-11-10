"""Purpose: truth tests keep CPython semantics on every kind of operand (HF-821
C3's late-TIR truthiness lane): bools, ints at both inline-int window edges,
None, containers, user __bool__ and __len__, an int subclass that overrides
__bool__, and a __bool__ that raises, including inside a loop condition
(HF-829)."""

LIMIT = 2**46


def truth(x):
    return "T" if x else "F"


def negated(x):
    return not x


values = [
    True,
    False,
    0,
    1,
    -1,
    LIMIT - 1,
    -LIMIT,
    LIMIT,
    -LIMIT - 1,
    2**70,
    0.0,
    -0.0,
    2.5,
    None,
    "",
    "x",
    [],
    [0],
    (),
    {},
    {0: 0},
]
print("".join(truth(v) for v in values))
print([negated(v) for v in values[:10]])


class Sized:
    def __init__(self, n):
        self.n = n

    def __len__(self):
        return self.n


class Flag:
    def __init__(self, on):
        self.on = on

    def __bool__(self):
        return self.on


class Falsy(int):
    def __bool__(self):
        return False


print(truth(Sized(0)), truth(Sized(3)), truth(Flag(True)), truth(Flag(False)))
print(truth(Falsy(5)), truth(int(Falsy(5))), Falsy(5) + 1)


class Refuses:
    def __bool__(self):
        raise ValueError("no bool")


try:
    print(truth(Refuses()))
except ValueError as error:
    print("ValueError", error)


def countdown(start, poison_at):
    seen = []
    n = start
    while n if n != poison_at else Refuses():
        seen.append(n)
        n -= 1
    return seen


print(countdown(4, -1))
try:
    countdown(4, 2)
except ValueError as error:
    print("loop ValueError", error)

total = 0
for i in range(-3, 4):
    if i:
        total += i * 10
    elif not i:
        total += 1
print(total)
