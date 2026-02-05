from dataclasses import dataclass


@dataclass(frozen=True)
class C:
    x: str


obj = C("hi")
print("isinstance", isinstance(obj, C))
print("class", obj.__class__)
print("dict", getattr(obj, "__dict__", None))
print("x", getattr(obj, "x", None))
