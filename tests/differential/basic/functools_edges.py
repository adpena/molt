"""Purpose: differential coverage for functools edges."""

import functools


def section(name):
    print(f"--- {name} ---")


section("Partial Arg Merging")


def func(a, b, c, d=4):
    print(f"a={a}, b={b}, c={c}, d={d}")


p1 = functools.partial(func, 1, c=3)
try:
    p1(2)  # a=1, b=2, c=3, d=4
except Exception as e:
    print(e)

p2 = functools.partial(func, 1, 2)
p2(3, d=5)  # a=1, b=2, c=3, d=5

# Overriding keywords
p3 = functools.partial(func, d=10)
p3(1, 2, 3, d=20)  # d=20 wins

section("Partial Recursion")
# partial of a partial
p4 = functools.partial(p2, 3)
p4()

section("Reduce Edge Cases")
try:
    print(functools.reduce(lambda x, y: x + y, []))
except TypeError:
    print("TypeError caught (empty list, no init)")

print(functools.reduce(lambda x, y: x + y, [], 0))
print(functools.reduce(lambda x, y: x + y, [1], 0))
print(functools.reduce(lambda x, y: x + y, [1]))
