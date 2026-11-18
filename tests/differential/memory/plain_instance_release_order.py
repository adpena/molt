"""Terminal release of plain instances: finalization count and order.

CPython oracle: Objects/typeobject.c subtype_dealloc clears an instance's
attributes in insertion order, then frees it and drops its type. Molt
releases a plain instance's fields, then its dictionary, in place
(heap_lifecycle::release_plain_instance_in_place); the field values wait in
a 16-word buffer, so the wide class below also covers its spill.
"""

import gc
import weakref

events = []


class Probe:
    def __init__(self, tag):
        self.tag = tag

    def __del__(self):
        events.append(self.tag)


def drain(label):
    print(label, events)
    events.clear()


class Pair:
    def __init__(self, a, b):
        self.a = a
        self.b = b


p = Pair(Probe("a"), Probe("b"))
del p
drain("pair")


class Wide:
    def __init__(self):
        self.f00 = Probe("f00")
        self.f01 = Probe("f01")
        self.f02 = Probe("f02")
        self.f03 = Probe("f03")
        self.f04 = 4
        self.f05 = Probe("f05")
        self.f06 = Probe("f06")
        self.f07 = Probe("f07")
        self.f08 = Probe("f08")
        self.f09 = Probe("f09")
        self.f10 = Probe("f10")
        self.f11 = "eleven"
        self.f12 = Probe("f12")
        self.f13 = Probe("f13")
        self.f14 = Probe("f14")
        self.f15 = Probe("f15")
        self.f16 = Probe("f16")
        self.f17 = Probe("f17")
        self.f18 = Probe("f18")
        self.f19 = Probe("f19")


w = Wide()
del w
drain("wide")

# Attributes added after construction, and a deleted one.
p = Pair(Probe("a2"), Probe("b2"))
p.extra = Probe("extra")
del p.a
drain("after delete")
del p
drain("late attribute")

# An instance whose dictionary was exposed.
p = Pair(Probe("a3"), Probe("b3"))
d = p.__dict__
d["more"] = Probe("more")
del d
del p
drain("exposed dict")


# A field whose finalizer reads another live instance.
class Holder:
    def __init__(self, value):
        self.value = value


shared = Holder(Probe("shared"))


class Reader:
    def __del__(self):
        events.append(("reader", shared.value.tag))


p = Pair(Reader(), Probe("after-reader"))
del p
drain("reading finalizer")
del shared
drain("shared")


# The last instance keeps its class alive.
def make_class():
    class Local:
        def __init__(self):
            self.x = Probe("local-x")

    return Local


cls = make_class()
ref = weakref.ref(cls)
inst = cls()
del cls
gc.collect()
print("class alive", ref() is not None)
del inst
drain("local instance")
gc.collect()
print("class released", ref() is None)

for i in range(300):
    p = Pair(Probe(i), Probe(-i - 1))
del p
print("loop", len(events), sorted(events)[:2], sorted(events)[-2:])
events.clear()
