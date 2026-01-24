"""Purpose: differential coverage for PEP 563 future annotations."""

from __future__ import annotations


class Box:
    value: "int"

    def method(self, other: "Box") -> "Box":
        return other


def annotated(x: "int") -> "str":
    return str(x)


module_value: "Box" = Box()

print("module", __annotations__)
print("func", annotated.__annotations__)
print("class", Box.__annotations__)
