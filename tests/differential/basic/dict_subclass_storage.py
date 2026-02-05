"""Purpose: dict subclass __dict__ should not expose mapping entries."""


class D(dict):
    pass


d = D()
d["a"] = 1
d.b = 2

print(d["a"])
print(d.b)
print("a" in d.__dict__)
print(sorted(d.__dict__.items()))

try:
    print(d["b"])
    print("mapping-attr-ok")
except KeyError:
    print("mapping-attr-keyerror")
