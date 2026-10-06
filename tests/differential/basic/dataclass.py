"""Purpose: differential coverage for dataclass."""

from dataclasses import dataclass


@dataclass
class Point:
    x: int
    y: int = 2


p = Point(1)
print(p.x)
print(p.y)
print(p)
print(p == Point(1, 2))
print(p == Point(2, 2))
p.y = 5
print(p.y)
p.z = 9
print(p.z)

# The decorator reads cls.__dict__.get/items and must see a live namespace.
from dataclasses import fields, is_dataclass


class RuntimePoint:
    __annotations__ = {"x": int, "y": int}
    y = 4


namespace = RuntimePoint.__dict__
RuntimePoint = dataclass(RuntimePoint)
runtime_point = RuntimePoint(6)
print(runtime_point.x, runtime_point.y)
print([field.name for field in fields(RuntimePoint)], is_dataclass(RuntimePoint))
print("__dataclass_fields__" in namespace, "__molt_dataclass__" in namespace)
print("__molt_field_offsets__" in namespace, "__molt_layout_size__" in namespace)
print(namespace.get("y"), vars(RuntimePoint).get("y"))


def annotated(value):
    return value


annotated.__annotations__ = {"value": int}
annotated.extra = 7
print(annotated.__annotations__["value"] is int, annotated.extra)
del annotated.extra
print(hasattr(annotated, "extra"))
