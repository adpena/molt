"""Purpose: a field read or store at a fixed offset (`obj.x` on a receiver
whose class the compiler proved) keeps CPython's attribute semantics when it
compiles to machine code (HF-847, `molt_codegen_abi::field_slot`). Each block
separates the correct lowering from a plausible defect: reading the retired
inline word after an instance dictionary took the field, reading the empty
(missing) word as a value, a wrong reference count on the old or new value,
a store that forgets the object now holds pointers, and the receivers the
inline path must hand back to the runtime (`__class__` assignment,
subclasses, data descriptors, `__slots__`)."""

import gc
import weakref


class Point:
    x: int
    y: int

    def __init__(self, x, y):
        self.x = x
        self.y = y


class Token:
    """A heap value whose release is observable through a weak reference."""

    def __init__(self, name):
        self.name = name


def fresh_stores():
    # Immediates, then pointers, into fresh objects; then pointer over
    # pointer and immediate over pointer. The old value must be released
    # exactly once and the new one kept alive by the field alone.
    out = []
    for value in (1, -7, 2.5, True, None, 2**70, "text", [1, 2], (3, 4)):
        p = Point(0, 0)
        p.x = value
        p.y = p.x
        out.append((p.x, p.y))
    print(out)
    p = Point(0, 0)
    first = Token("first")
    probe = weakref.ref(first)
    p.x = first
    del first
    gc.collect()
    print(probe() is not None, p.x.name)
    p.x = Token("second")
    gc.collect()
    print(probe() is None, p.x.name)
    p.x = 5
    gc.collect()
    print(p.x)
    # The same value stored twice keeps one reference per store target.
    shared = Token("shared")
    q = Point(shared, shared)
    q.x = shared
    q.y = shared
    probe = weakref.ref(shared)
    del shared
    q.x = 0
    gc.collect()
    print(probe() is not None, q.y.name)
    q.y = 0
    gc.collect()
    print(probe() is None)


def stores_in_a_loop(n):
    keep = []
    i = 0
    while i < n:
        p = Point(i, i)
        p.x = str(i)
        p.y = [i]
        if i % 50 == 0:
            keep.append(p)
        i += 1
    gc.collect()
    print([(k.x, k.y) for k in keep[:3]], len(keep))


def dictionary_takes_the_field():
    # Reading `__dict__` materializes the instance dictionary: the inferred
    # fields move into it, and a later read at the same site must see the
    # dictionary's value, not the retired inline word.
    reads = []
    for step in range(3):
        p = Point(1, 2)
        if step == 1:
            p.__dict__["x"] = 10
        if step == 2:
            vars(p)["y"] = 20
            p.extra = 3
        reads.append((p.x, p.y))
        p.x = p.x + 100
        reads.append((p.x, sorted(vars(p).items())))
    print(reads)


def dictionary_from_a_new_attribute():
    p = Point(4, 5)
    p.other = "o"
    p.x = 6
    print(p.x, p.y, p.other, sorted(vars(p)))


def deleted_field_is_missing():
    p = Point(1, 2)
    del p.x
    try:
        print(p.x)
    except AttributeError as exc:
        print("AttributeError", "x" in str(exc))
    p.x = 9
    print(p.x)


class WithDefault:
    x = "class default"

    def __init__(self, x):
        self.x = x


def deleted_field_falls_back_to_the_class():
    w = WithDefault("instance")
    print(w.x)
    del w.x
    print(w.x)
    w.x = "again"
    print(w.x)


class Other:
    x: int
    y: int

    def __init__(self, x, y):
        self.x = x
        self.y = y

    def describe(self):
        return "Other", self.x, self.y


def class_assignment():
    p = Point(1, 2)
    p.__class__ = Other
    print(type(p).__name__, p.x, p.y, p.describe())
    p.x = 7
    print(p.x)


class Sub(Point):
    def __init__(self, x, y):
        super().__init__(x, y)
        self.z = x + y


def total(points):
    t = 0
    for p in points:
        t += p.x * 10 + p.y
    return t


def subclasses_at_the_site():
    mixed = [Point(1, 2), Sub(3, 4), Point(5, 6), Sub(7, 8)]
    print(total(mixed), [getattr(p, "z", None) for p in mixed])
    for p in mixed:
        p.x = p.y
    print([(p.x, p.y) for p in mixed])


class Guarded:
    def __init__(self, value):
        self._value = value

    @property
    def value(self):
        return self._value * 2

    @value.setter
    def value(self, v):
        self._value = v + 1


def data_descriptor():
    g = Guarded(3)
    print(g.value, g._value)
    g.value = 10
    print(g.value, g._value)


class Slotted:
    __slots__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


def slots():
    s = Slotted("a", 2)
    s.x = s.x + "b"
    print(s.x, s.y)
    del s.y
    try:
        print(s.y)
    except AttributeError:
        print("AttributeError y")
    try:
        s.z = 1
    except AttributeError:
        print("AttributeError z")


fresh_stores()
stores_in_a_loop(400)
dictionary_takes_the_field()
dictionary_from_a_new_attribute()
deleted_field_is_missing()
deleted_field_falls_back_to_the_class()
class_assignment()
subclasses_at_the_site()
data_descriptor()
slots()
