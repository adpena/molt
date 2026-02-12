"""Purpose: differential coverage for weakref extended behavior."""

import gc
import weakref


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

print("proxy")
obj = ProxyTarget(1)
proxy = weakref.proxy(obj)
print("proxy-v", proxy.v)
print("proxy-bump", proxy.bump())
