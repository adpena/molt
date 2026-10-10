"""Minimal `pstats` subset for Molt."""

from __future__ import annotations


class Stats:
    def __init__(self, *_args, **_kwargs) -> None:
        pass

    def sort_stats(self, *_args, **_kwargs) -> "Stats":
        return self

    def print_stats(self, *_args, **_kwargs) -> "Stats":
        return self


__all__ = ["Stats"]
