"""Purpose: dict subclass __weakref__ slots should permit weakref.ref()."""

import weakref


class D(dict):
    __slots__ = ("__dict__", "__weakref__", "b")


d = D()
w = weakref.ref(d)
print("weakref returns", w() is d)
print("has __weakref__", hasattr(d, "__weakref__"))


d.c = 3

d["a"] = 1
setattr(d, "b", 2)

print(sorted(d.__dict__.items()))
print(d["a"])
print(d.b)
print("b" in d.__dict__)
print("c" in d.__dict__)

try:
    print(d["b"])
    print("mapping-attr-ok")
except KeyError:
    print("mapping-attr-keyerror")
