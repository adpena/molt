"""Purpose: differential coverage for typing runtime checkable error."""

from typing import Protocol, runtime_checkable


@runtime_checkable
class P(Protocol):
    def method(self) -> None: ...


try:
    issubclass(int, P)
except Exception as exc:
    print(type(exc).__name__)
