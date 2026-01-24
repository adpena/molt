"""Minimal regex support for Molt stdlib."""

from __future__ import annotations

from typing import Any

__all__ = [
    "ASCII",
    "DOTALL",
    "IGNORECASE",
    "LOCALE",
    "MULTILINE",
    "VERBOSE",
    "Pattern",
    "Match",
    "compile",
    "error",
    "fullmatch",
    "match",
    "search",
]

ASCII = 256
DOTALL = 16
IGNORECASE = 2
LOCALE = 4
MULTILINE = 8
VERBOSE = 64

_META_CHARS = set(".^$*+?{}[]\\|()")


class error(Exception):
    pass


class Match:
    def __init__(self, pattern: str, string: str, start: int, end: int) -> None:
        self._pattern = pattern
        self._string = string
        self._start = start
        self._end = end

    def group(self, index: int = 0) -> str:
        if index != 0:
            raise IndexError("no such group")
        return self._string[self._start : self._end]

    def start(self, index: int = 0) -> int:
        if index != 0:
            raise IndexError("no such group")
        return self._start

    def end(self, index: int = 0) -> int:
        if index != 0:
            raise IndexError("no such group")
        return self._end

    def span(self, index: int = 0) -> tuple[int, int]:
        if index != 0:
            raise IndexError("no such group")
        return (self._start, self._end)


class Pattern:
    def __init__(self, pattern: str, flags: int = 0) -> None:
        _ensure_literal_pattern(pattern, flags)
        self.pattern = pattern
        self.flags = flags

    def search(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return _search_literal(self.pattern, string, pos, endpos)

    def match(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return _match_literal(self.pattern, string, pos, endpos)

    def fullmatch(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return _fullmatch_literal(self.pattern, string, pos, endpos)


def _ensure_literal_pattern(pattern: str, flags: int) -> None:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full re syntax/flags with CPython-compatible matching and groups.
    if flags:
        raise NotImplementedError("regex flags are not supported yet")
    if any(ch in _META_CHARS for ch in pattern):
        raise NotImplementedError("regex syntax is not supported yet")


def _validate_text(pattern: Any, string: Any) -> tuple[str, str]:
    if not isinstance(pattern, str) or not isinstance(string, str):
        raise TypeError("pattern and string must be str")
    return pattern, string


def _clamp_span(length: int, pos: int, endpos: int | None) -> tuple[int, int]:
    start = max(0, pos)
    end = length if endpos is None else max(0, endpos)
    if end > length:
        end = length
    if start > end:
        start = end
    return start, end


def _search_literal(
    pattern: str, string: str, pos: int, endpos: int | None
) -> Match | None:
    pat, text = _validate_text(pattern, string)
    start, end = _clamp_span(len(text), pos, endpos)
    idx = _find_literal(text, pat, start, end)
    if idx == -1:
        return None
    return Match(pat, text, idx, idx + len(pat))


def _match_literal(
    pattern: str, string: str, pos: int, endpos: int | None
) -> Match | None:
    pat, text = _validate_text(pattern, string)
    start, end = _clamp_span(len(text), pos, endpos)
    if start + len(pat) > end:
        return None
    if text[start : start + len(pat)] == pat:
        return Match(pat, text, start, start + len(pat))
    return None


def _fullmatch_literal(
    pattern: str, string: str, pos: int, endpos: int | None
) -> Match | None:
    pat, text = _validate_text(pattern, string)
    start, end = _clamp_span(len(text), pos, endpos)
    if end - start != len(pat):
        return None
    if text[start:end] == pat:
        return Match(pat, text, start, end)
    return None


def _find_literal(text: str, pat: str, start: int, end: int) -> int:
    if pat == "":
        return start
    last = end - len(pat)
    if last < start:
        return -1
    for idx in range(start, last + 1):
        if text[idx : idx + len(pat)] == pat:
            return idx
    return -1


def compile(pattern: str, flags: int = 0) -> Pattern:
    return Pattern(pattern, flags)


def search(pattern: str, string: str, flags: int = 0) -> Match | None:
    _ensure_literal_pattern(pattern, flags)
    return _search_literal(pattern, string, 0, None)


def match(pattern: str, string: str, flags: int = 0) -> Match | None:
    _ensure_literal_pattern(pattern, flags)
    return _match_literal(pattern, string, 0, None)


def fullmatch(pattern: str, string: str, flags: int = 0) -> Match | None:
    _ensure_literal_pattern(pattern, flags)
    return _fullmatch_literal(pattern, string, 0, None)
