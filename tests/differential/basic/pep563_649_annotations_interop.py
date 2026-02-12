"""Purpose: differential coverage for PEP 563/649 annotations interop."""

from __future__ import annotations

import typing


class Widget:
    pass


def build(item: Widget, count: int) -> list[Widget]:
    return [item] * count


print({name: type(value).__name__ for name, value in build.__annotations__.items()})
print(typing.get_type_hints(build))
