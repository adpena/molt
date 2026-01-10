"""Capability-gated threading stubs for Molt."""

from __future__ import annotations

from typing import Any, Callable

__all__ = ["Thread"]


class Thread:
    def __init__(
        self,
        _group: Any | None = None,
        target: Callable[..., Any] | None = None,
        name: str | None = None,
        args: tuple[Any, ...] = (),
        kwargs: dict[str, Any] | None = None,
    ) -> None:
        self._target = target
        self._args = args
        self._kwargs = kwargs or {}
        self.name = name
        self.daemon = False

    def start(self) -> None:
        if self._target is None:
            return None
        if self._kwargs:
            self._target(*self._args, **self._kwargs)
            return None
        if self._args:
            self._target(*self._args)
            return None
        self._target()
        return None

    def join(self, _timeout: float | None = None) -> None:
        return None
