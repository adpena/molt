"""Type flow (design 78 section 4): a function object that escapes keeps a
correct generic body, and a global rebound after the first call is honoured.

`scale` is called with ints at its direct sites, but its object also escapes
into a list, through `map` and through a computed-name read of the module, so
code the analysis cannot see calls it with str and float arguments.
"""

import sys


def scale(x, k):
    return x * k


def twice(x):
    return x + x


def call_twice(x):
    return twice(x)


print(scale(3, 4), scale(5, 6))
table = [scale]
print(table[0]("ab", 3), table[0](1.5, 2))
print(list(map(scale, ["x", "y"], [2, 3])))
name = "".join(["sc", "ale"])
print(getattr(sys.modules[__name__], name)([1], 2))

print(call_twice(21))


def twice(x):  # noqa: F811 - the rebinding is the case under test
    return [x, x]


print(call_twice(21))
