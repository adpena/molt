"""Purpose: differential coverage for dataclass slots."""

from dataclasses import dataclass


@dataclass(slots=True)
class Point:
    x: int
    y: int = 2


p = Point(1)
print(p)
print(p.x, p.y)
print(p == Point(1, 2))
p.y = 5
print(p.y)


class InheritedSlot:
    __slots__ = ("x",)


@dataclass(slots=True)
class InheritedData(InheritedSlot):
    x: object
    y: object


inherited = InheritedData("base x", "data y")
print("dataclass inherited slot", inherited.x, inherited.y, InheritedSlot.x.__get__(inherited))
print("dataclass selected slots", InheritedData.__slots__)


class ShadowData(InheritedData):
    __slots__ = ("x",)


shadowed = ShadowData("visible x", "visible y")
InheritedSlot.x.__set__(shadowed, "hidden x")
print("dataclass slot owners", shadowed.x, shadowed.y, InheritedSlot.x.__get__(shadowed))
print("dataclass visible state", object.__getstate__(shadowed))
del shadowed.x
try:
    shadowed.x
except AttributeError:
    print("dataclass deleted visible", "AttributeError", InheritedSlot.x.__get__(shadowed))
shadowed.x = "new visible"
print("dataclass restored visible", shadowed.x, InheritedSlot.x.__get__(shadowed))


import gc
import weakref


class SlotPayload:
    pass


def dataclass_slot_owners():
    value = ShadowData(SlotPayload(), "retained y")
    hidden = SlotPayload()
    InheritedSlot.x.__set__(value, hidden)
    hidden_ref = weakref.ref(hidden)
    visible_ref = weakref.ref(value.x)
    return hidden_ref, visible_ref


hidden_ref, visible_ref = dataclass_slot_owners()
gc.collect()
print("dataclass physical owners released", hidden_ref() is None, visible_ref() is None)
