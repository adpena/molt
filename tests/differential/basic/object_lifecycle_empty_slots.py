"""Purpose: a class declaring `__slots__ = ()` constructs from its static
empty layout (HF-202, lifecycle S7) and keeps CPython's slot semantics: no
attribute store, no `__dict__`, no weak reference, the class's own type, GC
membership, and a subclass without `__slots__` that regains a dictionary.
Each check separates the correct object from a plausible defect of
allocating a plain instance with a dictionary."""

import gc
import weakref


class Plain:
    __slots__ = ()


class Listed:
    __slots__ = []


class Storing:
    __slots__ = ()

    def fill(self):
        self.value = 1


class Open(Plain):
    pass


p = Plain()
print("type", type(p) is Plain, type(Listed()) is Listed, isinstance(p, Plain))
for label, action in (
    ("store", lambda: setattr(p, "x", 1)),
    ("dict", lambda: p.__dict__),
    ("weakref", lambda: weakref.ref(p)),
    ("method store", lambda: Storing().fill()),
):
    try:
        action()
        print(label, "allowed")
    except (AttributeError, TypeError) as exc:
        print(label, type(exc).__name__)
print("tracked", gc.is_tracked(Plain()), gc.is_tracked(Listed()))
o = Open()
o.x = 2
print("subclass dict", o.x, o.__dict__)
hits = 0
for _ in range(10000):
    value = Plain()
    hits += type(value) is Plain
print("loop", hits)
print("slots attr", Plain.__slots__, Listed.__slots__)
