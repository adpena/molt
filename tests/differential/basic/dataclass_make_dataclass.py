"""Purpose: differential coverage for dataclasses.make_dataclass."""

from dataclasses import asdict, field, make_dataclass


Point = make_dataclass(
    "Point",
    [("x", int), ("y", int, field(default=2))],
    order=True,
)
p = Point(1)
print("point", p.x, p.y, p == Point(1, 2), Point(0, 0) < Point(1, 0))
print("asdict", asdict(p))

Base = make_dataclass("Base", [("a", int)])
Child = make_dataclass("Child", [("b", int)], bases=(Base,))
c = Child(1, 2)
print("child", c.a, c.b, Child.__mro__[1].__name__)

WithModule = make_dataclass("WithModule", [("v", int)], module="demo.module")
print("module", WithModule.__module__)

try:
    make_dataclass("Bad", [("x", int, 0)])
except Exception as exc:
    print("bad_field", type(exc).__name__)

try:
    make_dataclass("BadName", [("class", int)])
except Exception as exc:
    print("bad_name", type(exc).__name__)
