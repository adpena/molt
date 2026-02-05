"""Purpose: dict subclass __slots__ with __dict__ keeps mapping separate."""


class D(dict):
    __slots__ = ("__dict__", "b")


d = D()

print("has __dict__", hasattr(d, "__dict__"))
print(sorted(d.__dict__.items()))


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
