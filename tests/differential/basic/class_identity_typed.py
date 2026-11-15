"""Purpose: class and function identity is the definition, never its name
(HF-847 slice 1). Two modules each define a class `Point` and functions
`make` and `norm`; calls and allocations must reach the right definition.
A class whose `__new__` returns an instance of another class shows that the
result of `C(...)` is not proven to be a `C` (condition C1): only the raw
allocation is exact."""

from class_identity_support import shapes_a, shapes_b


a = shapes_a.make(3, -4)
b = shapes_b.make(2)
print(type(a).__module__.rsplit(".", 1)[-1], type(a).__qualname__, a.norm())
print(type(b).__module__.rsplit(".", 1)[-1], type(b).__qualname__, b.norm())
print(type(a) is type(b), isinstance(a, shapes_b.Point), isinstance(b, shapes_a.Point))
print(shapes_a.norm(a), shapes_b.norm(b))

points = [shapes_a.Point(1, 1), shapes_b.Point(5), shapes_a.make(0, 2)]
print([type(p).__module__.rsplit(".", 1)[-1] for p in points])
print([p.norm() for p in points])


class Other:
    def __init__(self, tag):
        self.tag = tag

    def norm(self):
        return "other:" + self.tag


class Shape:
    def __new__(cls, kind):
        if kind == "other":
            return Other(kind)
        return super().__new__(cls)

    def __init__(self, kind):
        self.kind = kind

    def norm(self):
        return "shape:" + self.kind


for kind in ["square", "other", "circle"]:
    made = Shape(kind)
    print(type(made).__name__, isinstance(made, Shape), made.norm())

# `__init__` does not run when `__new__` returns a non-instance.
odd = Shape("other")
print(hasattr(odd, "kind"), odd.tag)
