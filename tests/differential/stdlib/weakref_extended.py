"""Purpose: differential coverage for weakref extended behavior."""

import atexit
import gc
import types
import weakref
from dataclasses import dataclass


class Thing:
    pass


class Hashy:
    def __hash__(self):
        return 42


class Key:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return 1

    def __eq__(self, other):
        return isinstance(other, Key) and self.v == other.v


class Unhash:
    def __eq__(self, other):
        return isinstance(other, Unhash)

    __hash__ = None


class Value:
    pass


class Obj:
    def method(self):
        return 42


class ProxyTarget:
    def __init__(self, v):
        self.v = v

    def bump(self):
        self.v += 1
        return self.v


class SlotsOnly:
    __slots__ = ()


class SlotsWeak:
    __slots__ = ("__weakref__",)


class InheritedWeak(SlotsWeak):
    __slots__ = ()


class DefaultBase:
    pass


class InheritedDefaultWeak(DefaultBase):
    __slots__ = ()


class SlotsBase:
    __slots__ = ()


class InheritedSlotsOnly(SlotsBase):
    __slots__ = ()


class WeakDict(dict):
    __slots__ = ("__weakref__",)


@dataclass
class DefaultDataclass:
    value: int = 1


@dataclass(slots=True)
class SlotsDataclass:
    value: int = 1


@dataclass(slots=True, weakref_slot=True)
class WeakSlotsDataclass:
    value: int = 1


def supports_weakref(value):
    try:
        weakref.ref(value)
    except TypeError:
        return False
    return True


print("hash-cached")
obj = Hashy()
ref = weakref.ref(obj)
print("hash", hash(ref))
del obj
gc.collect()
print("hash-dead", hash(ref))

print("hash-late")
obj = Hashy()
ref = weakref.ref(obj)
del obj
gc.collect()
try:
    print("hash-late", hash(ref))
except TypeError as exc:
    print("hash-late-err", type(exc).__name__, exc)

print("counts")
obj = Thing()
ref1 = weakref.ref(obj)
ref2 = weakref.ref(obj)
print("count", weakref.getweakrefcount(obj))
print("refs", len(weakref.getweakrefs(obj)))

print(
    "weakrefability-user",
    supports_weakref(Thing()),
    supports_weakref(SlotsOnly()),
    supports_weakref(SlotsWeak()),
    supports_weakref(InheritedWeak()),
    supports_weakref(InheritedDefaultWeak()),
    supports_weakref(InheritedSlotsOnly()),
    supports_weakref(WeakDict()),
    supports_weakref(DefaultDataclass()),
    supports_weakref(SlotsDataclass()),
    supports_weakref(WeakSlotsDataclass()),
)
bound_owner = Obj()
generator = (value for value in ())
print(
    "weakrefability-builtins-yes",
    supports_weakref(supports_weakref),
    supports_weakref(bound_owner.method),
    supports_weakref(types.ModuleType("weakref_probe")),
    supports_weakref(Thing),
    supports_weakref(generator),
    supports_weakref(set()),
    supports_weakref(frozenset()),
    supports_weakref(supports_weakref.__code__),
)
print(
    "weakrefability-builtins-no",
    supports_weakref("text"),
    supports_weakref([]),
    supports_weakref({}),
    supports_weakref(()),
    supports_weakref(b"bytes"),
    supports_weakref(bytearray()),
    supports_weakref(range(1)),
    supports_weakref(slice(1)),
    supports_weakref(Exception()),
    supports_weakref(enumerate(())),
    supports_weakref(zip(())),
    supports_weakref(map(int, ())),
    supports_weakref(filter(None, ())),
    supports_weakref([].__str__),
    supports_weakref(classmethod(lambda cls: None)),
    supports_weakref(staticmethod(lambda: None)),
    supports_weakref(property(lambda self: None)),
    supports_weakref(1 + 2j),
)

print("weakkey")
k1 = Key(1)
k2 = Key(1)
store = weakref.WeakKeyDictionary()
store[k1] = "a"
store[k2] = "b"
print("wk-len", len(store))
print("wk-value", store[k1], store[k2])

print("weakvalue")
value = Value()
values = weakref.WeakValueDictionary()
values["x"] = value
print("wvd-has", "x" in values)
del value
gc.collect()
print("wvd-has", "x" in values)

print("weakset")
ws = weakref.WeakSet()
try:
    ws.add(Unhash())
    print("ws-add", True)
except TypeError as exc:
    print("ws-err", type(exc).__name__, exc)

print("weakmethod")
obj = Obj()
wm = weakref.WeakMethod(obj.method)
print("wm-alive", wm() is not None)
del obj
gc.collect()
print("wm-dead", wm() is None)

print("finalize")
calls = []
obj = Thing()
fin = weakref.finalize(obj, calls.append, "done")
print("fin-alive", fin.alive)
print("fin-peek", fin.peek() is not None)
del obj
gc.collect()
print("fin-calls", calls)
print("fin-alive", fin.alive)
print("fin-peek", fin.peek() is not None)

clear_calls = []
clear_obj = Thing()
clear_fin = weakref.finalize(clear_obj, clear_calls.append, "before-clear")
print("fin-clear-before", atexit._ncallbacks())
atexit._clear()
print("fin-clear-after", atexit._ncallbacks())
post_clear_obj = Thing()
post_clear_fin = weakref.finalize(post_clear_obj, clear_calls.append, "after-clear")
print("fin-clear-after-new", atexit._ncallbacks())
del post_clear_obj
gc.collect()
print("fin-clear-object-death", clear_calls, post_clear_fin.alive)

print("proxy")
obj = ProxyTarget(1)
proxy = weakref.proxy(obj)
print("proxy-v", proxy.v)
print("proxy-bump", proxy.bump())
