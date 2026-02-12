"""Purpose: differential coverage for typing protocol."""

from typing import Protocol, runtime_checkable


@runtime_checkable
class HasLen(Protocol):
    def __len__(self) -> int: ...


class C:
    def __len__(self) -> int:
        return 1


obj = C()
print(isinstance(obj, HasLen))
