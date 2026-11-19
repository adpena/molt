"""Terminal release of dicts and exceptions: finalization count and order.

CPython oracle: Objects/dictobject.c dict_dealloc releases each entry's key,
then its value, in entry order; Objects/exceptions.c BaseException_dealloc
clears the exception's fields, then subtype_dealloc drops a heap exception
class after the free. Molt releases an exact dict's rows and an exception's
field slots in place (heap_lifecycle::release_owned_in_place); this program
counts every finalizer that runs and records its order.
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


# A dict's keys and values, in insertion order, with a deleted row between.
d = {Probe("k1"): Probe("v1"), Probe("k2"): Probe("v2"), "gap": Probe("v-gap")}
del d["gap"]
drain("after row delete")
d["k3"] = Probe("v3")
del d
drain("dict free")

# A dict reached only through another dict's value.
outer = {"inner": {"a": Probe("inner-a"), "b": Probe("inner-b")}, "c": Probe("outer-c")}
del outer
drain("nested dict free")

# A finalizer that mutates a live dict while a dead one releases its rows.
live = {}


class Mutator:
    def __del__(self):
        live["added"] = Probe("added-later")
        events.append("mutator")


d = {"m": Mutator(), "p": Probe("after-mutator")}
del d
drain("mutating finalizer")
del live
drain("live dict free")


# An exception's args, attributes, context and cause.
def make_error():
    try:
        try:
            raise KeyError(Probe("context-arg"))
        except KeyError:
            raise ValueError(Probe("arg0"), Probe("arg1"))
    except ValueError as exc:
        exc.note = Probe("attr")
        exc.__cause__ = OSError(Probe("cause-arg"))
        return exc


err = make_error()
drain("exception built")
del err
drain("exception free")


# A user exception class kept alive only by its last instance.
def make_class():
    class Custom(Exception):
        pass

    return Custom


cls = make_class()
class_ref = weakref.ref(cls)
inst = cls(Probe("custom-arg"))
del cls
gc.collect()
print("class alive while its instance lives", class_ref() is not None)
del inst
drain("custom exception free")
gc.collect()
print("class released with its last instance", class_ref() is None)

# Many short-lived dicts and exceptions: every finalizer runs once.
for i in range(500):
    row = {"value": Probe(i)}
    try:
        raise ValueError(Probe(-i - 1))
    except ValueError:
        pass
    del row
print("loop finalizers", len(events), sorted(events)[:3], sorted(events)[-3:])
events.clear()
