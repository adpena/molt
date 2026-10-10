"""Minimal `pyexpat` subset for Molt."""

from __future__ import annotations


class _Parser:
    def Parse(self, _data: bytes | str, _isfinal: bool = False) -> int:
        return 1


def ParserCreate(*_args, **_kwargs) -> _Parser:
    return _Parser()


__all__ = ["ParserCreate"]
