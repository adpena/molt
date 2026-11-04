"""Purpose: materialized __dict__ stays coherent with field-backed attribute writes."""

class Demo:
    def __init__(self) -> None:
        self.note = "live"
        self.value = 7


item = Demo()
first_dict = item.__dict__
item.note = "updated"
print(item.note, sorted(first_dict.items()), sorted(item.__dict__.items()), first_dict is item.__dict__)


# Construction starts with inline owners; exposure, replacement and deletion
# must keep Python's one public namespace while declared slots stay separate.
class Empty:
    pass


empty = Empty()
namespace = vars(empty)
namespace.update({"value": 11})
print("empty", empty.value, namespace is empty.__dict__, namespace is vars(empty))

fresh = Empty.__new__(Empty)
fresh.__dict__.update({"value": 12})
print("new-update", fresh.value, fresh.__dict__ is vars(fresh))

for number in range(3):
    current = Demo()
    current.value = number
    observed = vars(current)
    current.value += 10
    observed["note"] = "mapping"
    print("inline-to-dict", current.value, current.note, observed is vars(current))
    current.__dict__ = {"value": 20 + number, "note": "replacement"}
    current.value += 1
    print("replace", current.value, current.note, observed["value"], observed is vars(current))
    del current.__dict__
    print("delete-dict", vars(current), observed["note"])
    current.value = 30 + number
    print("after-delete", current.value, vars(current))


class SlotBase:
    __slots__ = ("value",)

    def __init__(self, value):
        self.value = value


class SlotChild(SlotBase):
    __slots__ = ("value", "other", "__dict__")

    def __init__(self, value):
        SlotBase.value.__set__(self, value)
        self.value = value + 1
        self.other = value + 2
        self.extra = value + 3


slotted = SlotChild(40)
hidden = SlotBase.value.__get__(slotted, SlotChild)
print("slots", hidden, slotted.value, slotted.other, vars(slotted))
slotted.__dict__.update({"value": 99, "other": 98, "extra": 97})
print("slot-shadow", SlotBase.value.__get__(slotted, SlotChild),
      slotted.value, slotted.other, slotted.extra)
slotted.__dict__ = {"extra": 50}
print("slot-replace", slotted.value, slotted.other, slotted.extra)
del slotted.__dict__
print("slot-delete", slotted.value, slotted.other, vars(slotted))


class NativeLeft(str):
    __slots__ = ("value", "other", "__dict__")


class NativeRight(str):
    __slots__ = ("value", "other", "__dict__")


for text in ("", "abc", "abcdefghi"):
    native = NativeLeft(text)
    native.value = [len(text)]
    native.other = len(text) + 1
    native.extra = len(text) + 2
    namespace = vars(native)
    native.__class__ = NativeRight
    namespace["extra"] += 10
    print("native-tail", str(native), native.value, native.other, native.extra,
          namespace is native.__dict__, type(native) is NativeRight)


class EmptyNative(str):
    __slots__ = ()


native_empty = EmptyNative("nonempty intrinsic tail")
print("empty-native", str(native_empty), type(native_empty) is EmptyNative)


events = []


class NewBase:
    def __new__(cls, value):
        events.append(("new", cls.__name__, value))
        return object.__new__(cls)

    def __init__(self, value):
        events.append(("init", type(self).__name__, value))
        self.value = value


class NewChild(NewBase):
    pass


constructed = NewChild(60)
print("inherited-new-init", constructed.value, events, vars(constructed))


class ForeignNew(NewBase):
    def __new__(cls, value):
        return ("foreign", value)

    def __init__(self, value):
        raise AssertionError("foreign __new__ result must skip __init__")


print("foreign-new", ForeignNew(61))


class BadInit:
    def __init__(self):
        self.value = 62
        return 1


try:
    BadInit()
except TypeError as error:
    print("init-result", type(error).__name__)


class RaisesInit:
    def __init__(self):
        self.value = 63
        raise ValueError("init failed")


try:
    RaisesInit()
except ValueError as error:
    print("init-error", str(error))


try:
    vars(SlotBase(1))
except TypeError as error:
    print("no-dict", type(error).__name__)


# Reconstruction exercises explicit __new__ and namespace restoration without
# calling __init__; it must use the same physical owner as ordinary construction.
import pickle

for protocol in (2, 4, 5):
    source = Demo()
    source.value = 70 + protocol
    restored = pickle.loads(pickle.dumps(source, protocol=protocol))
    print("pickle-namespace", protocol, type(restored) is Demo,
          sorted(vars(restored).items()), vars(restored) is restored.__dict__)


argument_events = []


class ReentrantCall:
    def __init__(self, value):
        self.value = value


class ArgumentOwner:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        argument_events.append(("release", self.label, ReentrantCall(1).value))


def argument_failure():
    raise ValueError("preparation failed")


def receive_arguments(*args, **kwargs):
    print("received-owners", [item.label for item in args],
          [(key, kwargs[key].label) for key in sorted(kwargs)])


try:
    receive_arguments(ArgumentOwner("failed"), argument_failure())
except ValueError as error:
    print("argument-abort", str(error), argument_events)

argument_events.clear()


class ExpandedOwners:
    def __iter__(self):
        argument_events.append(("iterate", ReentrantCall(2).value))
        return iter([ArgumentOwner("star")])


class KeywordOwners:
    def keys(self):
        argument_events.append(("keys", ReentrantCall(3).value))
        return ["token"]

    def __getitem__(self, key):
        argument_events.append(("getitem", key, ReentrantCall(4).value))
        return ArgumentOwner("keyword")


receive_arguments(*ExpandedOwners(), **KeywordOwners())
print("expanded-reentrant-release", argument_events)
