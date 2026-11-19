"""Merging a mappingproxy keeps CPython's observable behavior on every path.

An exact mappingproxy over an exact dict with str keys (every `type.__dict__`
of an ordinary class) may be merged without dispatching through the proxy; any
other shape takes the generic keys()/__getitem__ path. Both must agree with
CPython: same keys, values and order, same errors.
"""

import types


class Base:
    base_attr = 1

    def method(self):
        return "m"


class Child(Base):
    child_attr = 2


# dict.update from a class's proxy: same keys, values and insertion order.
merged = {}
merged.update(Child.__dict__)
print("update keys", [k for k in merged if not k.startswith("__")])
print("update value", merged["child_attr"])
print("same as dict()", dict(Child.__dict__) == merged)

# Merging into a non-empty dict overwrites shared keys and keeps the rest.
target = {"child_attr": "old", "extra": 9}
target.update(Child.__dict__)
print("overwrite", target["child_attr"], target["extra"])

# dir() merges the proxies of the whole MRO.
print("dir", [name for name in dir(Child()) if not name.startswith("__")])


# ** unpacking of a proxy into a function.
def take(**kwargs):
    return sorted(k for k in kwargs if not k.startswith("__"))


print("kwargs", take(**Child.__dict__))

# A proxy over a user mapping takes the generic path: its methods run.
calls = []


class Logged(dict):
    def keys(self):
        calls.append("keys")
        return super().keys()

    def __getitem__(self, key):
        calls.append(("getitem", key))
        return super().__getitem__(key)


proxy = types.MappingProxyType(Logged(x=1, y=2))
generic = {}
generic.update(proxy)
print("generic", generic, calls)

# A class namespace with a non-str key also merges through the generic path.
Odd = type("Odd", (), {1: "one", "a": 2})
odd = {}
odd.update(Odd.__dict__)
print("non-str key", odd[1], odd["a"])

# A proxy over a plain dict with str keys.
plain = types.MappingProxyType({"p": 1, "q": 2})
out = {"q": 0}
out.update(plain)
print("plain", out)
