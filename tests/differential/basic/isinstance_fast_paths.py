"""Purpose: isinstance fast paths keep CPython's answers.

The exact-type match answers before `__instancecheck__`; an immediate value
against a class whose metaclass is exactly `type` answers by the real-type
subtype test; every other case keeps the full protocol.
"""

import numbers


class Meta(type):
    def __instancecheck__(cls, instance):
        print("Meta.__instancecheck__", type(instance).__name__)
        return False


class Guarded(metaclass=Meta):
    pass


class Plain:
    pass


class Child(Plain):
    pass


class LiesAboutClass:
    @property
    def __class__(self):
        return Plain


class StrSub(str):
    pass


values = ["s", 7, True, 2.5, None, StrSub("t"), Plain(), Child(), LiesAboutClass()]
classes = [str, int, bool, float, object, type(None), Plain, Child, numbers.Number]
for value in values:
    print(type(value).__name__, [isinstance(value, cls) for cls in classes])

# The exact match never consults the metaclass hook; a subclass instance does.
print(isinstance(Guarded(), Guarded))


class GuardedChild(Guarded):
    pass


print(isinstance(GuardedChild(), Guarded))
print(isinstance(1, Guarded))

# Tuples of classes and nested tuples keep the full protocol.
print(isinstance(3, (str, (float, int))), isinstance("x", (int, float)))

# An isinstance inside a handler leaves the handled exception in place.
try:
    raise KeyError("k")
except KeyError as error:
    print(isinstance(error, KeyError), isinstance(5, str), isinstance(5, int))
    print(repr(error))

# A loop that mixes hits and misses.
count = 0
for index in range(1000):
    item = "s" if index % 3 else index
    if isinstance(item, str):
        count += 1
print(count)
