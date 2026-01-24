"""Purpose: differential coverage for PEP 526 variable annotations."""

from __future__ import annotations


x: int = 1
y: "str"


class Box:
    value: int = 42
    note: "Box"


def probe() -> dict[str, object]:
    local_int: int = 3
    local_text: "int"
    return dict(__annotations__)


print(__annotations__)
print(Box.__annotations__)
print(probe())
