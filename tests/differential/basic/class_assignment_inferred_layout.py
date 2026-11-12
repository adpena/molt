"""Purpose: differential coverage for __class__ assignment between plain classes.

Each class's __init__ writes its attributes, so Molt keeps them in compiler-
inferred field slots. CPython accepts the assignment because the visible layouts
agree (no __slots__, both have a __dict__); the inferred slots must not change
that answer, and every attribute must read back after the move, in both
directions, including an instance allocated by the class with fewer slots.
"""

import copy
import gc


class Wide:
    def __init__(self, a, b, c):
        self.a = a
        self.b = b
        self.c = c

    def total(self):
        return self.a + self.b + self.c

    def bump(self, step):
        for _ in range(3):
            self.a += step
        return self.a


class Narrow:
    def __init__(self, z):
        self.z = z

    def doubled(self):
        return self.z * 2


class Empty:
    pass


class Slotted:
    __slots__ = ("a",)


def state(obj):
    return type(obj).__name__, sorted(vars(obj).items())


# A wide instance moves to the narrow class and back.
wide = Wide(1, 2, 3)
wide.a += 10
wide.__class__ = Narrow
print("wide-as-narrow", state(wide))
print("wide-as-narrow fields", wide.a, wide.b, wide.c)
wide.z = 7
print("wide-as-narrow doubled", wide.doubled())
wide.__class__ = Wide
print("wide-back total", wide.total(), wide.z)
print("wide-back bump", wide.bump(5))
print("wide-back", state(wide))

# A narrow instance takes the wider class, whose slots it never allocated.
narrow = Narrow(5)
narrow.__class__ = Wide
narrow.a = 1
narrow.b = 2
narrow.c = 3
print("narrow-as-wide total", narrow.total(), narrow.z)
print("narrow-as-wide bump", narrow.bump(2))
gc.collect()
print("narrow-as-wide", state(narrow))
clone = copy.copy(narrow)
print("narrow-as-wide copy", state(clone), clone.total())
narrow.__class__ = Narrow
print("narrow-back", state(narrow), narrow.doubled())

# An empty instance takes the wide class.
empty = Empty()
empty.__class__ = Wide
empty.a = empty.b = empty.c = 4
print("empty-as-wide", empty.total(), state(empty))

# The instance dictionary keeps its identity across the move.
held = Wide(7, 8, 9)
namespace = held.__dict__
held.__class__ = Narrow
print("dict identity", namespace is held.__dict__, sorted(namespace))
namespace["z"] = 21
print("dict write", held.doubled())

# Deletion and absent attributes follow the dictionary.
del held.b
print("deleted", hasattr(held, "b"), sorted(vars(held)))
try:
    held.missing
except AttributeError as exc:
    print("absent", type(exc).__name__)

# Equal __slots__ that include __dict__, with different inferred attributes.
class SlotsA:
    __slots__ = ("s", "__dict__")

    def __init__(self):
        self.s = 1
        self.x = 2
        self.y = 3


class SlotsB:
    __slots__ = ("s", "__dict__")

    def __init__(self):
        self.s = 10
        self.q = 20


slots_a = SlotsA()
slots_a.__class__ = SlotsB
print("slots a->b", slots_a.s, slots_a.x, slots_a.y, sorted(vars(slots_a)))
slots_b = SlotsB()
slots_b.__class__ = SlotsA
print("slots b->a", slots_b.s, slots_b.q, sorted(vars(slots_b)))

# A differing visible layout is still refused, in both directions.
for source, target in ((Wide(1, 2, 3), Slotted), (Slotted(), Wide)):
    try:
        source.__class__ = target
    except TypeError as exc:
        print("refused", exc)
    else:
        print("accepted", type(source).__name__)
