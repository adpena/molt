"""Purpose: differential coverage for typing get type hints."""

from typing import Optional, get_type_hints


class C:
    value: Optional[int] = None


def foo(x: int) -> Optional[int]:
    return x


print(get_type_hints(C))
print(get_type_hints(foo))
