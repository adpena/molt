"""Purpose: solid base selection and physical fields through legal native mixins.

CPython independently specifies which bases compose; equal layout roots retain
first direct base, neutral mixins do not replace the native structural base.
"""

class IntA(int): pass
class IntB(int): pass
class ListA(list): pass
class ListB(list): pass
class SlotA(list): __slots__ = ("x",)
class SlotB(list): __slots__ = ("x",)
class SlotC(SlotA): __slots__ = ()
class OtherSlot(list): __slots__ = ("y",)
class PlainA: payload: int
class PlainB: other: int
class DictOnly: __slots__ = ("__dict__",)
class WeakOnly: __slots__ = ("__weakref__",)
class Spoof: pass
Spoof.__name__ = "int"
cases = [
    ("list-int", (list, int)), ("int-list", (int, list)),
    ("list-float", (list, float)), ("float-list", (float, list)),
    ("list-dict", (list, dict)), ("dict-list", (dict, list)),
    ("list-str", (list, str)), ("list-tuple", (list, tuple)),
    ("int-float", (int, float)), ("int-exception", (int, Exception)),
    ("list-oserror", (list, OSError)), ("same-int-root", (IntA, IntB)),
    ("same-list-root", (ListA, ListB)), ("unrelated-same-slot", (SlotA, SlotB)),
    ("unrelated-other-slot", (SlotA, OtherSlot)), ("slot-and-neutral", (SlotA, ListB)),
    ("derived-slot-root", (SlotC, ListB)), ("plain-native", (PlainA, list)),
    ("native-plain", (list, PlainA)), ("plain-plain", (PlainA, PlainB)),
    ("dict-only-native", (DictOnly, list)), ("weak-only-native", (WeakOnly, list)),
    ("dict-weak", (DictOnly, WeakOnly)), ("spoof-native", (list, Spoof)),
    ("exception-siblings", (ValueError, TypeError)), ("oserror-exception", (OSError, ValueError)),
]
for label, bases in cases:
    try:
        result = type("Composed", bases, {})
    except TypeError:
        print(label, "TypeError")
    else:
        print(label, "accepted", result.__base__.__name__)


class StaticPlainList(PlainA, list):
    pass


class StaticListPlain(list, PlainA):
    pass


class StaticSlotMixin(PlainA, SlotA):
    pass


for factory in (StaticPlainList, StaticListPlain,
                type("DynamicPlainList", (PlainA, list), {}),
                type("DynamicListPlain", (list, PlainA), {})):
    value = factory([1, 2])
    value.payload = "payload"
    value.extra = "extra"
    print("native mixin fields", factory.__name__, factory.__base__.__name__,
          value, value.payload, value.extra, value.__dict__)
    list.append(value, 3)
    value.payload = "changed"
    print("native mixin mutation", value, value.payload, value.__dict__)


for factory in (StaticSlotMixin, type("DynamicSlotMixin", (PlainA, SlotA), {})):
    value = factory([4])
    value.x = "slot"
    value.payload = "field"
    print("slot mixin fields", factory.__name__, factory.__base__.__name__,
          value, value.x, value.payload, value.__dict__)


import gc
import weakref


class Payload:
    pass


class GcMixin:
    payload: object


class GcList(GcMixin, list):
    pass


def make_mixin_cycle():
    value = GcList([5])
    payload = Payload()
    payload.owner = value
    value.payload = payload
    value.extra = value
    return weakref.ref(value), weakref.ref(payload)


owner_ref, field_ref = make_mixin_cycle()
gc.collect()
print("mixin cycle collected", owner_ref() is None, field_ref() is None)


class IntPayloadSpelling(int):
    __molt_int_value__: object


class FloatPayloadSpelling(float):
    __slots__ = ("__molt_float_value__",)


int_value = IntPayloadSpelling(123)
print("int payload private", int(int_value), int_value.__dict__)
int_value.__molt_int_value__ = "ordinary attribute"
print("int payload spelling", int(int_value), int_value.__molt_int_value__, int_value.__dict__)
float_value = FloatPayloadSpelling(1.25)
print("float payload private", float(float_value), hasattr(float_value, "__molt_float_value__"))
float_value.__molt_float_value__ = "ordinary slot"
print("float payload spelling", float(float_value), float_value.__molt_float_value__)


class RedeclaredSlot(SlotA):
    __slots__ = ("x",)


slot_value = RedeclaredSlot([6])
slot_value.x = "derived slot"
SlotA.x.__set__(slot_value, "base slot")
print("redeclared slot identities", slot_value.x, SlotA.x.__get__(slot_value), slot_value)


RepeatedSlot = type("RepeatedSlot", (list,), {"__slots__": ("x", "x")})
repeated_slot = RepeatedSlot([7])
repeated_slot.x = "last slot"
print("repeated slot name", repeated_slot.x, repeated_slot)


# Source lookup and explicit descriptor addressing have distinct, observable roles.
base_descriptor = SlotA.x
print("slot descriptor class access", base_descriptor.__get__(None, SlotA) is base_descriptor)
base_descriptor.__delete__(slot_value)
try:
    base_descriptor.__get__(slot_value)
except AttributeError:
    print("deleted base slot", "AttributeError", slot_value.x)
try:
    base_descriptor.__set__([], "invalid receiver")
except TypeError:
    print("wrong slot receiver", "TypeError")
base_descriptor.__set__(slot_value, "restored base slot")
SlotA.x = "rebound class attribute"
print("slot namespace rebound", slot_value.x, base_descriptor.__get__(slot_value))
SlotA.x = base_descriptor


class ShadowedSlot(SlotA):
    x = "visible class attribute"


shadowed_slot = ShadowedSlot([8])
base_descriptor.__set__(shadowed_slot, "hidden slot")
shadowed_slot.x = "dictionary attribute"
print("slot shadow dictionary", shadowed_slot.x, base_descriptor.__get__(shadowed_slot), shadowed_slot.__dict__)
del shadowed_slot.x
print("slot shadow deletion", shadowed_slot.x, base_descriptor.__get__(shadowed_slot), shadowed_slot.__dict__)


class PrivateSlots(list):
    __slots__ = ("__hidden",)

    def set_hidden(self, value):
        self.__hidden = value


private_slots = PrivateSlots([9])
private_slots.set_hidden("private value")
print("private slot owner", PrivateSlots._PrivateSlots__hidden.__get__(private_slots), hasattr(private_slots, "__hidden"))


class CompatibleLeft:
    __slots__ = ("a", "b")


class CompatibleRight:
    __slots__ = ("b", "a")


class IncompatibleNames:
    __slots__ = ("a", "c")


compatible = CompatibleLeft()
compatible.a = "first"
compatible.b = "second"
compatible.__class__ = CompatibleRight
print("compatible slot transfer", type(compatible).__name__, compatible.a, compatible.b)
try:
    compatible.__class__ = IncompatibleNames
except TypeError:
    print("incompatible slot transfer", "TypeError", type(compatible).__name__, compatible.a, compatible.b)


events = []


class SlotReentry:
    def __init__(self, owner):
        self.owner = owner

    def __del__(self):
        self.owner.__class__ = CompatibleRight
        events.append((type(self.owner).__name__, self.owner.a, self.owner.b))


reentry = CompatibleLeft()
reentry.b = "retained field"
reentry.a = SlotReentry(reentry)
reentry.a = "replacement field"
print("slot finalizer reentry", events, type(reentry).__name__, reentry.a, reentry.b)
