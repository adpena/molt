"""Purpose: differential coverage for container methods."""

lst = [1, 2, 3]
print(lst.reverse(), lst)
lst.clear()
print(lst)
lst2 = [1, 2]
lst3 = lst2.copy()
lst2.append(3)
print(lst2, lst3)
lst4 = [0]
lst4.extend(range(3))
print(lst4)

d = {"a": 1}
print(d.setdefault("a", 5), d)
print(d.setdefault("b", 2), d)
d.update({"c": 3})
print(d)
d.update([("d", 4), ("e", 5)])
print(d)
u = d.update
print(u(), d)

s = "AbC"
print(s.lower(), s.upper())


class StoredList(list):
    def __init__(self, values, label="default"):
        super().__init__(values)
        self.label = label


class InheritedList(StoredList):
    pass


for factory in (StoredList, InheritedList):
    value = factory([1, 2], label="retained")
    print("stored list", type(value).__name__, value, value.label, value.__dict__)
    list.__init__(value, [3])
    print("reinitialized list", value, value.label)
    blank = list.__new__(factory, "ignored", ignored=True)
    print("list new", type(blank).__name__, list.__len__(blank), blank.__dict__)
    print("instance new is static", type(value.__new__(factory)).__name__)
    try:
        object.__new__(factory)
    except TypeError:
        print("unsafe object new rejected")


class SlotList(list):
    __slots__ = ("label", "__weakref__")


slot_list = SlotList([4, 5])
slot_list.label = "slot"
print("slotted list", slot_list, slot_list.label, hasattr(slot_list, "__dict__"))


class MoreSlots(SlotList):
    __slots__ = ("extra",)


more = MoreSlots([6])
more.label = "inherited"
more.extra = "own"
print("inherited slots", more, more.label, more.extra)


class ExplicitNew(list):
    def __new__(cls, values, *, label):
        result = super().__new__(cls)
        result.label = label
        return result

    def __init__(self, values, *, label):
        super().__init__(values)


custom = ExplicitNew([7], label="custom")
print("custom new init", custom, custom.label)


class ForeignNew(list):
    def __new__(cls):
        return ("foreign",)

    def __init__(self):
        print("incorrect foreign init")


print("foreign new", ForeignNew())


def partial_values():
    yield 8
    raise ValueError("partial")


partial = StoredList([], "kept")
try:
    list.__init__(partial, partial_values())
except ValueError:
    print("partial list init", partial, partial.label)

for other in (dict, int):
    try:
        type("ConflictingList", (list, other), {})
    except TypeError:
        print("list layout conflict")


import weakref

print("list weakref", weakref.ref(slot_list)() is slot_list)
try:
    weakref.ref([])
except TypeError:
    print("exact list weakref rejected")



class EmptySlotsList(list):
    __slots__ = ()


for exact in ([], list.__new__(list)):
    print("exact list state", hasattr(exact, "__dict__"), type(exact).__name__)
    for target in (EmptySlotsList, list):
        try:
            exact.__class__ = target
        except TypeError:
            print("exact list class rejected", type(exact).__name__)
    try:
        exact.extra = "forbidden"
    except AttributeError:
        print("exact list attribute rejected")
    try:
        weakref.ref(exact)
    except TypeError:
        print("exact new list weakref rejected")


class SwapListA(list):
    __slots__ = ("slot", "__dict__", "__weakref__")


class SwapListB(list):
    __slots__ = ("slot", "__dict__", "__weakref__")


class WrongSwapList(list):
    __slots__ = ("other", "__dict__", "__weakref__")


swapped = SwapListA([9])
swapped.slot = "slot value"
swapped.dynamic = "dict value"
swapped_ref = weakref.ref(swapped)
swapped.__class__ = SwapListB
print("list class transfer", type(swapped).__name__, swapped,
      swapped.slot, swapped.dynamic, swapped_ref() is swapped)
for target in (WrongSwapList, list, dict):
    try:
        swapped.__class__ = target
    except TypeError:
        print("list class unchanged", type(swapped).__name__, swapped.slot, swapped)


class AnnotatedList(list):
    payload: int

    def __init__(self, values):
        super().__init__(values)
        self.payload = 12


class AnnotatedDescendant(AnnotatedList):
    extra: int


annotated = AnnotatedDescendant([10, 11])
annotated.extra = 13
print("list inferred fields", annotated, annotated.payload, annotated.extra)



class UnslottedSwapA(list):
    pass


class UnslottedSwapB(UnslottedSwapA):
    __slots__ = ()


inherited_state = UnslottedSwapA([14])
inherited_state.label = "inherited dict"
inherited_state.__class__ = UnslottedSwapB
print("list inherited state transfer", type(inherited_state).__name__,
      inherited_state, inherited_state.label)
