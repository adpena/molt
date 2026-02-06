"""Minimal shlex support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["quote", "split", "shlex"]

_molt_shlex_quote = _require_intrinsic("molt_shlex_quote", globals())
_molt_shlex_split = _require_intrinsic("molt_shlex_split", globals())


def quote(s: str) -> str:
    if not isinstance(s, str):
        raise TypeError("shlex.quote argument must be str")
    return _molt_shlex_quote(s)


def split(s: str, posix: bool = True) -> list[str]:
    del posix
    if not isinstance(s, str):
        raise TypeError("shlex.split argument must be str")
    return _molt_shlex_split(s, " \t\r\n")


class shlex:
    def __init__(self, s: str, posix: bool = True) -> None:
        self._source = s
        self.posix = posix
        self.whitespace = " \t\r\n"
        self.whitespace_split = False
        self._tokens: list[str] | None = None
        self._index = 0

    def _ensure_tokens(self) -> None:
        if self._tokens is None:
            self._tokens = _molt_shlex_split(self._source, self.whitespace)

    def __iter__(self):
        self._ensure_tokens()
        self._index = 0
        return self

    def __next__(self) -> str:
        self._ensure_tokens()
        assert self._tokens is not None
        if self._index >= len(self._tokens):
            raise StopIteration
        token = self._tokens[self._index]
        self._index += 1
        return token
