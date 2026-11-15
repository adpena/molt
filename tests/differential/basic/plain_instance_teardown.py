"""A plain instance's teardown releases its class edge and nothing else.

CPython oracle: Objects/typeobject.c subtype_dealloc clears the instance's
slots and dictionary, frees it, then drops its reference to the type, so the
last instance of a class can be what keeps a deleted class alive. Molt frees
an instance whose fields hold no heap reference and which has no dictionary
without its edge sink (heap_lifecycle::retires_without_edges); every other
instance keeps the full transaction. The differential runner supplies
matching CPython target expectations.
"""

import gc
import weakref


def make_class(tag):
    class Plain:
        __slots__ = ("x", "y")

        def __init__(self, x, y):
            self.x = x
            self.y = y

    Plain.__qualname__ = "Plain_" + tag
    return Plain


# The last instance holds the class's last reference.
cls = make_class("last")
class_ref = weakref.ref(cls)
only = cls(1, 2)
del cls
gc.collect()
print("class alive while an instance lives", class_ref() is not None)
del only
gc.collect()
print("class released with its last instance", class_ref() is None)


# Many int-field instances: values survive and nothing leaks a class.
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


total = 0
for i in range(2000):
    p = Point(i, i + 1)
    p.x = p.x + p.y
    total += p.x
print("int fields", total)


# A heap field, a dictionary entry and a cycle keep the full path.
class Holder:
    def __init__(self, value):
        self.value = value


holder = Holder([1, 2, 3])
held = weakref.ref(holder)
print("heap field", holder.value)
del holder
print("heap-field instance freed", held() is None)

with_dict = Point(1, 2)
with_dict.extra = "attribute in the dictionary"
probe = weakref.ref(with_dict)
del with_dict
print("dict instance freed", probe() is None)

node = Holder(None)
node.value = node
node_ref = weakref.ref(node)
del node
gc.collect()
print("cycle collected", node_ref() is None)


# Finalizers and weak references keep their order and their callbacks.
events = []


class Finalized:
    def __init__(self, n):
        self.n = n

    def __del__(self):
        events.append(("del", self.n))


f = Finalized(7)
callback_ref = weakref.ref(f, lambda _r: events.append(("weakref callback",)))
del f
print("finalizer order", events)


# A finalizer that resurrects its instance.
saved = []


class Phoenix:
    def __init__(self, n):
        self.n = n

    def __del__(self):
        saved.append(self)


Phoenix(3)
print("resurrected", [p.n for p in saved])
saved.clear()
print("resurrected instance released", saved)


# Big ints are heap references; bools and small ints are not.
class Mixed:
    def __init__(self, a, b, c):
        self.a = a
        self.b = b
        self.c = c


big = 1 << 100
m = Mixed(True, 5, big)
del m
print("big int kept", big == 1 << 100)
print("done")
