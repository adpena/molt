"""Purpose: differential coverage for PEP 563 annotation stringization types."""

from __future__ import annotations


x: "int"


class Node:
    next: "Node | None"


def fn(arg: "Node") -> "list[Node]":
    return [arg]


print({name: type(value).__name__ for name, value in __annotations__.items()})
print({name: type(value).__name__ for name, value in Node.__annotations__.items()})
print({name: type(value).__name__ for name, value in fn.__annotations__.items()})
