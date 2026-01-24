"""Purpose: differential coverage for annotated assignment and __annotations__."""

from __future__ import annotations

module_only: int
module_value: "str" = "hello"


def annotated_locals():
    local_only: int
    local_value: "float" = 1.5
    return __annotations__


def annotated_signature(x: int, y: "bytes") -> "str":
    return "ok"


class Box:
    member_only: int
    member_value: "bytes" = b"data"


print("module", __annotations__)
print("func_locals", annotated_locals())
print("func_sig", annotated_signature.__annotations__)
print("class", Box.__annotations__)
