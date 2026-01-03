"""Context manager helpers for Molt (capability-safe stubs)."""

from __future__ import annotations

from typing import Any


class _NullContext:
    def __init__(self, value: Any = None) -> None:
        self._value = value

    def __enter__(self) -> Any:
        return self._value

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        return False


def nullcontext(value: Any = None) -> _NullContext:
    return _NullContext(value)


class _Closing:
    def __init__(self, thing: Any) -> None:
        self._thing = thing

    def __enter__(self) -> Any:
        return self._thing

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        close = getattr(self._thing, "close", None)
        if callable(close):
            close()
        return False


def closing(thing: Any) -> _Closing:
    return _Closing(thing)
