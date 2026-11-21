"""Purpose: an instance whose fields hold no heap object frees through its
class's lifecycle plan (HF-202), so the plan must follow every change that
could make the death observable. Each block separates the correct release from
a plausible defect: a plan that missed a late `__del__` on a base (finalizer
skipped), a grandparent's finalizer missed by a grandchild, a shadowing
`__del__` replaced or removed late, a removed `__del__` that still runs, a
weak reference whose callback must fire, a
resurrection, and GC membership that must match CPython (heap-class instances
tracked from allocation, an exact `object()` never)."""

import gc
import weakref

events = []


class Base:
    pass


class Child(Base):
    pass


class Plain:
    __slots__ = ()


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


def base_finalizer(self):
    events.append(("base", type(self).__name__))


# A late finalizer on a base reaches an existing descendant instance whose
# fields hold only immediates.
held = Child()
Base.__del__ = base_finalizer
del held
print("late base", events)

# Removing it stops the finalizer for instances made before and after.
events.clear()
before = Child()
del Base.__del__
after = Child()
del before
del after
print("removed", events)


# A finalizer two levels up reaches every descendant, and replacing the
# middle class's own finalizer shadows it. (Molt refuses `__bases__`
# assignment, so declarations are the only hierarchy input that can change.)
class Grand:
    pass


class Middle(Grand):
    pass


class Leaf(Middle):
    pass


events.clear()
leaf = Leaf()
Grand.__del__ = lambda self: events.append(("grand", type(self).__name__))
del leaf
print("grandparent", events)
Middle.__del__ = lambda self: events.append(("middle", type(self).__name__))
leaf = Leaf()
del leaf
del Middle.__del__
leaf = Leaf()
del leaf
del Grand.__del__
leaf = Leaf()
del leaf
print("shadowed", events)

# A weak reference to an empty instance: its callback runs at the free.
events.clear()
target = Child()
ref = weakref.ref(target, lambda r: events.append("callback"))
del target
print("weakref", events, ref() is None)

# Resurrection through `__del__` on an instance with immediate fields.
saved = []


class Phoenix:
    def __init__(self):
        self.count = 1

    def __del__(self):
        events.append(("phoenix", self.count))
        saved.append(self)


events.clear()
bird = Phoenix()
del bird
print("resurrected", events, len(saved), saved[0].count)
saved.clear()
print("final drop", events)

# An exact object() can never become a heap-class instance, so it never needs
# GC membership.
bare = object()
try:
    bare.__class__ = Child
except TypeError as exc:
    print("object class assignment", type(exc).__name__)

# GC membership follows CPython.
print("tracked object", gc.is_tracked(object()))
print("tracked Plain", gc.is_tracked(Plain()))
print("tracked Point", gc.is_tracked(Point(1, 2)))
print("tracked Child", gc.is_tracked(Child()))

# Exact object() allocations do not count toward generation 0; heap-class
# instances do, and their frees give the count back.
gc.disable()
gc.collect()
start = gc.get_count()[0]
objects = [object() for _ in range(50)]
object_delta = gc.get_count()[0] - start
points = [Point(i, i) for i in range(50)]
point_delta = gc.get_count()[0] - start - object_delta
del points
freed = start + object_delta + point_delta - gc.get_count()[0]
# The exact counts include interpreter-private containers, so compare ranges.
print("objects uncounted", object_delta < 10)
print("points counted", point_delta >= 50)
print("frees counted", freed >= 50)
del objects
gc.enable()

# Many short-lived instances in a loop keep their values.
total = 0
for i in range(1000):
    p = Point(i, 1)
    total += p.x + p.y
    q = object()
    total += type(q) is object
print("loop", total)

# Instances compiled code admits and retires itself stay exact GC members:
# a collection finds their cycles, survivors move to older generations and
# leave from there, and get_objects sees them.


class Node:
    def __init__(self, name):
        self.name = name
        self.other = None


gc.collect()
survivors = [Point(i, i) for i in range(20)]
gc.collect()  # the survivors move to an older generation
print("survivors tracked", all(gc.is_tracked(p) for p in survivors))
print("survivor listed", any(o is survivors[3] for o in gc.get_objects()))
del survivors  # each leaves its older generation
collected = 0
for i in range(30):
    a = Node("a")
    b = Node("b")
    a.other = b
    b.other = a
del a, b
collected = gc.collect()
print("cycles collected", collected >= 60)
fresh = [Point(i, 0) for i in range(5)]
print("fresh tracked", all(gc.is_tracked(p) for p in fresh), len(fresh))
gc.freeze()
del fresh  # frozen members leave the permanent generation
gc.unfreeze()
print("after freeze", gc.collect() >= 0)

# `type(x)` reads the class an instance names, never `__class__`; the builtin
# `object` reference is the one class.


class Disguised:
    @property
    def __class__(self):
        return int


class Items(list):
    pass


class Lazy:
    def __init__(self):
        self.ready = True

    def fill(self):
        self.late = 1


print("type of instances", type(Point(1, 2)) is Point, type(Disguised()) is Disguised)
print("disguise", Disguised().__class__ is int, isinstance(Disguised(), Disguised))
print("type of builtins", type(Items()) is Items, type(object()) is object, type(3), type("s"))
print("type of classes", type(Point) is type, type(object) is type, object is type(object()))
lazy = Lazy()
try:
    lazy.late
except AttributeError as exc:
    print("unset field", type(exc).__name__)
lazy.fill()
print("filled field", lazy.late, lazy.ready)
kinds = [type(v).__name__ for v in (1, 2.0, None, True, Point(0, 0), object(), [], Items())]
print("kinds", kinds)
