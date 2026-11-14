"""Purpose: reads of module globals stored once at initialization (HF-846).
A read that runs only after the global's one store is a plain load; every
other read keeps the unbound test. Each case checks that a read never sees an
unbound global as a value: a function called before the store raises
NameError, a function defined after the store reads it, recursion reads the
function's own name, a class body and a method read it, a value escapes by
return, storage, capture and argument."""

import math

LIMIT = 10


def scaled(x):
    return x * LIMIT


def too_early():
    return LATE


try:
    too_early()
except NameError as exc:
    print("before the store", type(exc).__name__)

LATE = "late"
print("after the store", too_early())


def fact(n):
    return 1 if n <= 1 else n * fact(n - 1)


class Box:
    size = LIMIT

    def __init__(self, value):
        self.value = value + LIMIT

    def area(self):
        return self.value * Box.size


def returns_module():
    return math


def keeps_in_list():
    return [math, LIMIT]


def captures():
    def inner():
        return math.floor(LIMIT / 3)

    return inner


def passes_on(callback):
    return callback(math)


total = 0
for i in range(LIMIT):
    total += scaled(i) + Box(i).area() + int(math.sqrt(4.0))
print("total", total, fact(6))
print("escapes", returns_module().pi > 3, keeps_in_list()[1], captures()())
print("argument", passes_on(lambda m: m.e > 2))

STATE = {"count": 0}


def bump():
    STATE["count"] += 1
    return STATE["count"]


for _ in range(3):
    bump()
print("state", STATE["count"])
