"""Purpose: dict subclass __slots__ should block __dict__ and keep mapping separate."""


class D(dict):
    __slots__ = ("b",)


d = D()

print("has __dict__", hasattr(d, "__dict__"))
try:
    print(d.__dict__)
    print("dict-attr-ok")
except AttributeError:
    print("dict-attr-error")

try:
    d.c = 3
    print("c-set-ok")
except AttributeError:
    print("c-set-attr-error")


d["a"] = 1
setattr(d, "b", 2)

print(d["a"])
print(d.b)

try:
    print(d["b"])
    print("mapping-attr-ok")
except KeyError:
    print("mapping-attr-keyerror")
