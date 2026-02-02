"""Minimal shlex support for Molt."""

from __future__ import annotations

__all__ = ["quote", "split", "shlex"]

# Mirror CPython's shlex.quote escaping rules without regex support.
_SAFE_CHARS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_@%+=:,./-"
)


def _is_safe(s: str) -> bool:
    for ch in s:
        if ch not in _SAFE_CHARS:
            return False
    return True


def quote(s: str) -> str:
    if not s:
        return "''"
    if _is_safe(s):
        return s
    return "'" + s.replace("'", "'\"'\"'") + "'"


def _split(s: str, whitespace: str) -> list[str]:
    tokens: list[str] = []
    buf: list[str] = []
    quote_char: str | None = None
    escape = False
    for ch in s:
        if escape:
            buf.append(ch)
            escape = False
            continue
        if ch == "\\" and quote_char != "'":
            escape = True
            continue
        if quote_char is not None:
            if ch == quote_char:
                quote_char = None
            else:
                buf.append(ch)
            continue
        if ch in "'\"":
            quote_char = ch
            continue
        if ch in whitespace:
            if buf:
                tokens.append("".join(buf))
                buf = []
            continue
        buf.append(ch)
    if buf:
        tokens.append("".join(buf))
    return tokens


def split(s: str, posix: bool = True) -> list[str]:
    del posix
    return _split(s, " \t\r\n")


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
            self._tokens = _split(self._source, self.whitespace)

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
