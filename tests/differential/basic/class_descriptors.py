"""Purpose: differential coverage for class descriptors."""


class Widget:
    kind = "widget"

    def __init__(self, x: int) -> None:
        self.x = x

    @classmethod
    def make(cls, x: int) -> "Widget":
        return cls(x)

    @staticmethod
    def add(a: int, b: int) -> int:
        return a + b

    @property
    def value(self) -> int:
        return self.x + 1


w = Widget(2)
print(Widget.kind)
print(w.kind)
print(Widget.make(3).x)
print(w.make(4).x)
print(Widget.add(1, 2))
print(w.add(2, 3))
print(w.value)

print(getattr(w, "x"))
setattr(w, "x", 9)
print(hasattr(w, "x"))
print(getattr(w, "missing", "fallback"))

try:
    w.value = 10
except Exception as exc:
    print(str(exc))

print(getattr(Widget, "kind"))
print(getattr(Widget, "make")(5).x)
print(getattr(Widget, "add")(4, 6))
print(hasattr(Widget, "value"))
setattr(Widget, "kind", "gizmo")
print(Widget.kind)
setattr(Widget, "extra", 7)
print(hasattr(Widget, "extra"))
print(getattr(Widget, "extra"))
