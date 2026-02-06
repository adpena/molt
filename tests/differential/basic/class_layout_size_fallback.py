"""Purpose: class layout size fallback when local metadata is missing."""


class Base:
    x: int


class Child(Base):
    y: int


if hasattr(Child, "__molt_layout_size__"):
    delattr(Child, "__molt_layout_size__")

a = Child()
b = Child()

a.x = 11
a.y = 29
b.x = 3
b.y = 7

a.tag = "left"
b.tag = "right"

print(a.x, a.y, a.tag)
print(b.x, b.y, b.tag)
print(a.x + a.y, b.x + b.y)
print(a.__dict__.get("tag"), b.__dict__.get("tag"))
