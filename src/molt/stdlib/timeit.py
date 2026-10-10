"""Minimal `timeit` subset for Molt."""

from __future__ import annotations


def timeit(
    _stmt: str = "pass", setup: str = "pass", timer=None, number: int = 1
) -> float:
    _ = setup, timer, number
    return 0.0


class Timer:
    def __init__(self, stmt: str = "pass", setup: str = "pass", timer=None) -> None:
        self.stmt = stmt
        self.setup = setup
        self.timer = timer

    def timeit(self, number: int = 1) -> float:
        _ = number
        return timeit(self.stmt, self.setup, self.timer, number)


__all__ = ["Timer", "timeit"]
