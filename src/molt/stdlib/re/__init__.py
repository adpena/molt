"""Regex support for Molt stdlib — thin wrapper around Rust intrinsics."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from typing import Any, Iterator
import warnings as _warnings

# Reachability-driven feature elimination
# (docs/design/foundation/feature_reachability_tree_shaking.md, Option b): the
# regex intrinsic handles are bound lazily inside the functions that use them,
# NOT eagerly in the always-run module body. The frontend lowers
# ``_require_intrinsic("molt_re_compile")`` to a ``builtin_func`` op that directly
# link-references the ``molt_re_compile`` symbol; binding every handle at module
# top-level made all ~15 regex symbols a hard link dependency the instant ``re``
# is imported (even transitively via ``warnings``/``typing``, even when no regex
# operation is ever executed), forcing the ``stdlib_regex`` feature (an
# undefined-symbol link failure) onto lean profiles. Moving each binding into
# its single consuming function makes the symbol reach the reached SimpleIR only
# on a code path that actually performs that regex operation, so
# ``molt.cli.required_features`` (which scans the reached SimpleIR for
# ``builtin_func``/``const_str`` intrinsic names) requires ``stdlib_regex`` only
# when a regex op is genuinely reached, and an ``import re`` with no reached regex
# call links zero ``molt_re_*`` symbols.
#
# The module-body probe stays: ``molt_stdlib_probe`` is a core, ungated,
# always-linkable intrinsic that marks ``re`` as a real intrinsic-backed stdlib
# module for the import-policy classifier; it carries no regex link dependency.
_require_intrinsic("molt_stdlib_probe")

__all__ = [
    "NOFLAG",
    "ASCII",
    "A",
    "DOTALL",
    "S",
    "IGNORECASE",
    "I",
    "LOCALE",
    "L",
    "MULTILINE",
    "M",
    "UNICODE",
    "U",
    "VERBOSE",
    "X",
    "RegexFlag",
    "Pattern",
    "Match",
    "compile",
    "error",
    "escape",
    "purge",
    "findall",
    "finditer",
    "fullmatch",
    "match",
    "search",
    "split",
    "sub",
    "subn",
]

# TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P1, status:planned): complete native re parity and continue migrating parser/matcher execution into Rust (named-group edge cases, verbose-mode parser details, and full Unicode class/casefold semantics).

# Flags — CPython 3.12 values
NOFLAG = 0
ASCII = 256
DOTALL = 16
IGNORECASE = 2
LOCALE = 4
MULTILINE = 8
UNICODE = 32
VERBOSE = 64
A = ASCII
I = IGNORECASE  # noqa: E741
L = LOCALE
M = MULTILINE
S = DOTALL
U = UNICODE
X = VERBOSE
RegexFlag = int
_META_CHARS = set(".^$*+?{}[]\\|()")


class error(Exception):
    def __init__(
        self, msg: str = "", pattern: str | None = None, pos: int | None = None
    ) -> None:
        self.msg, self.pattern, self.pos = msg, pattern, pos
        super().__init__(msg)


# Pattern cache
_cache: dict[tuple[str, int], "Pattern"] = {}
_MAXCACHE = 512


def purge() -> None:
    """Clear the regular expression cache."""
    _cache.clear()


# ---------------------------------------------------------------------------
# Match — thin wrapper around intrinsic result tuple
# ---------------------------------------------------------------------------
# Intrinsic result: (match_start, match_end, group_spans)
# group_spans is 1-indexed; group 0 = overall match from start/end.


class Match:
    __slots__ = (
        "_pattern",
        "_string",
        "_pos",
        "_endpos",
        "_start",
        "_end",
        "_group_spans",
    )

    def __init__(
        self,
        pattern: "Pattern",
        string: str,
        pos: int,
        endpos: int,
        start: int,
        end: int,
        group_spans: tuple[tuple[int, int] | None, ...],
    ) -> None:
        self._pattern = pattern
        self._string = string
        self._pos = pos
        self._endpos = endpos
        self._start = start
        self._end = end
        self._group_spans = group_spans

    def group(self, *indices: int | str) -> Any:
        _molt_re_match_group = _require_intrinsic("molt_re_match_group")
        if not indices:
            indices = (0,)
        match_tuple = (self._start, self._end, self._group_spans)
        return _molt_re_match_group(
            self._string, match_tuple, indices, self._pattern.groupindex
        )

    def groups(self, default: Any = None) -> tuple[Any, ...]:
        _molt_re_match_groups = _require_intrinsic("molt_re_match_groups")
        match_tuple = (self._start, self._end, self._group_spans)
        return _molt_re_match_groups(self._string, match_tuple, default)

    def groupdict(self, default: Any = None) -> dict[str, Any]:
        _molt_re_match_groupdict = _require_intrinsic("molt_re_match_groupdict")
        match_tuple = (self._start, self._end, self._group_spans)
        return _molt_re_match_groupdict(
            self._string, match_tuple, default, self._pattern.groupindex
        )

    def start(self, group: int | str = 0) -> int:
        return self._group_span(group)[0]

    def end(self, group: int | str = 0) -> int:
        return self._group_span(group)[1]

    def span(self, group: int | str = 0) -> tuple[int, int]:
        return self._group_span(group)

    def expand(self, template: str) -> str:
        return _expand_replacement(template, self)

    def __getitem__(self, g: int | str) -> Any:
        return self.group(g)

    def __bool__(self) -> bool:
        return True

    def __repr__(self) -> str:
        return f"<re.Match object; span={self.span()!r}, match={self.group()!r}>"

    @property
    def re(self) -> "Pattern":
        return self._pattern

    @property
    def string(self) -> str:
        return self._string

    @property
    def pos(self) -> int:
        return self._pos

    @property
    def endpos(self) -> int:
        return self._endpos

    @property
    def lastindex(self) -> int | None:
        last: int | None = None
        for i in range(len(self._group_spans)):
            if self._group_spans[i] is not None:
                last = i + 1
        return last

    @property
    def lastgroup(self) -> str | None:
        li = self.lastindex
        if li is None:
            return None
        for name, idx in self._pattern.groupindex.items():
            if idx == li:
                return name
        return None

    def _group_span(self, index: int | str) -> tuple[int, int]:
        if isinstance(index, str):
            gi = self._pattern.groupindex
            if index not in gi:
                raise IndexError("no such group")
            index = gi[index]
        if index == 0:
            return (self._start, self._end)
        if index < 0 or index > len(self._group_spans):
            raise IndexError("no such group")
        span = self._group_spans[index - 1]
        return (-1, -1) if span is None else span

    def _group_value(self, index: int | str) -> Any:
        span = self._group_span(index)
        if span[0] == -1 and span[1] == -1:
            return None
        return self._string[span[0] : span[1]]


# ---------------------------------------------------------------------------
# Pattern — thin wrapper around a compiled intrinsic handle
# ---------------------------------------------------------------------------


class Pattern:
    __slots__ = ("pattern", "flags", "groups", "groupindex", "_handle")

    def __init__(
        self,
        pattern: str,
        flags: int,
        handle: int,
        groups: int,
        groupindex: dict[str, int],
    ) -> None:
        self.pattern = pattern
        self.flags = flags
        self._handle = handle
        self.groups = groups
        self.groupindex = dict(groupindex)

    def search(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return self._execute(string, pos, endpos, "search")

    def match(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return self._execute(string, pos, endpos, "match")

    def fullmatch(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return self._execute(string, pos, endpos, "fullmatch")

    def finditer(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Iterator[Match]:
        _molt_re_finditer_collect = _require_intrinsic("molt_re_finditer_collect")
        text = _ensure_text(string)
        start, end = _clamp_span(len(text), pos, endpos)
        raw = _molt_re_finditer_collect(self._handle, text, start, end)
        if raw is None:
            return
        for item in raw:
            yield Match(self, text, start, end, item[0], item[1], item[2])

    def findall(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> list[Any]:
        results: list[Any] = []
        for m in self.finditer(string, pos, endpos):
            if self.groups == 0:
                results.append(m.group(0))
            elif self.groups == 1:
                results.append(m.group(1))
            else:
                results.append(m.groups())
        return results

    def split(self, string: str, maxsplit: int = 0) -> list[str | Any]:
        _molt_re_split = _require_intrinsic("molt_re_split")
        text = _ensure_text(string)
        return _molt_re_split(self._handle, text, maxsplit)

    def sub(self, repl: object, string: str, count: int = 0) -> str:
        return self.subn(repl, string, count=count)[0]

    def subn(self, repl: object, string: str, count: int = 0) -> tuple[str, int]:
        text = _ensure_text(string)
        if count < 0:
            raise ValueError("count must be non-negative")
        if callable(repl):
            # Callable replacement must stay in Python — iterate via finditer.
            return _subn_callable(self, repl, text, count=count)
        if not isinstance(repl, str):
            repl = str(repl)
        _molt_re_sub = _require_intrinsic("molt_re_sub")
        return _molt_re_sub(self._handle, repl, text, count)

    def __repr__(self) -> str:
        return f"re.compile({self.pattern!r}, {self.flags!r})"

    def _execute(
        self, string: str, pos: int, endpos: int | None, mode: str
    ) -> Match | None:
        _molt_re_execute = _require_intrinsic("molt_re_execute")
        text = _ensure_text(string)
        start, end = _clamp_span(len(text), pos, endpos)
        raw = _molt_re_execute(self._handle, text, start, end, mode)
        if raw is None:
            return None
        return Match(self, text, start, end, raw[0], raw[1], raw[2])


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _ensure_text(string: Any) -> str:
    if not isinstance(string, str):
        raise TypeError("expected string or bytes-like object")
    return string


def _clamp_span(length: int, pos: int, endpos: int | None) -> tuple[int, int]:
    start = max(pos, 0)
    end = length if endpos is None else max(endpos, 0)
    if end > length:
        end = length
    if start > end:
        start = end
    return start, end


def _match_group_values(match_obj: Match) -> tuple[object, ...]:
    _molt_re_group_values = _require_intrinsic("molt_re_group_values")
    group0 = (match_obj._start, match_obj._end)
    all_spans = (group0,) + match_obj._group_spans
    return _molt_re_group_values(match_obj._string, all_spans)


def _expand_replacement(repl: object, match_obj: Match) -> str:
    if callable(repl):
        return str(repl(match_obj))
    if not isinstance(repl, str):
        repl = str(repl)
    _molt_re_expand_replacement = _require_intrinsic("molt_re_expand_replacement")
    return _molt_re_expand_replacement(repl, _match_group_values(match_obj))


def _compile(pattern: str, flags: int) -> Pattern:
    if flags & LOCALE:
        raise ValueError("cannot use LOCALE flag with a str pattern")
    if flags & ASCII and flags & UNICODE:
        raise ValueError("ASCII and UNICODE flags are incompatible")
    if not (flags & ASCII):
        flags |= UNICODE
    effective_pattern = pattern
    if flags & VERBOSE:
        _molt_re_strip_verbose = _require_intrinsic("molt_re_strip_verbose")
        effective_pattern = _molt_re_strip_verbose(pattern, flags)
    _molt_re_compile = _require_intrinsic("molt_re_compile")
    handle = _molt_re_compile(effective_pattern, flags)
    _molt_re_pattern_info = _require_intrinsic("molt_re_pattern_info")
    info = _molt_re_pattern_info(handle)
    groups, groupindex, effective_flags, warn_pos = info[0], info[1], info[2], info[3]
    if warn_pos is not None:
        _warnings.warn(
            f"Possible nested set at position {warn_pos}", FutureWarning, stacklevel=3
        )
    return Pattern(pattern, effective_flags, handle, groups, groupindex)


def _coerce_pattern(pattern: Any, flags: int) -> Pattern:
    if isinstance(pattern, Pattern):
        if flags:
            raise error("cannot specify flags with a compiled pattern")
        return pattern
    if hasattr(pattern, "search") and hasattr(pattern, "match"):
        if flags:
            raise error("cannot specify flags with a compiled pattern")
        return pattern  # type: ignore[return-value]
    if not isinstance(pattern, str):
        raise TypeError("pattern must be a string")
    key = (pattern, flags)
    cached = _cache.get(key)
    if cached is not None:
        return cached
    compiled = _compile(pattern, flags)
    if len(_cache) >= _MAXCACHE:
        _cache.clear()
    _cache[key] = compiled
    return compiled


def _subn_callable(
    pattern: Pattern, repl: object, string: str, *, count: int = 0
) -> tuple[str, int]:
    """sub/subn with a callable replacement.

    The replacement callable must receive a real ``Match`` object, so this path
    stays in the Python shim and iterates over the intrinsic-backed
    ``Pattern.finditer`` results.
    """
    if count < 0:
        raise ValueError("count must be non-negative")
    text = _ensure_text(string)
    limit = None if count == 0 else count
    out: list[str] = []
    last = 0
    replaced = 0
    for match_obj in pattern.finditer(text):
        if limit is not None and replaced >= limit:
            break
        start, end = match_obj.span()
        out.append(text[last:start])
        out.append(str(repl(match_obj)))
        last = end
        replaced += 1
    out.append(text[last:])
    return ("".join(out), replaced)


# ---------------------------------------------------------------------------
# Module-level convenience functions
# ---------------------------------------------------------------------------


def compile(pattern: str, flags: int = 0) -> Pattern:
    """Compile a regular expression pattern, returning a Pattern object."""
    return _coerce_pattern(pattern, flags)


def search(pattern: str, string: str, flags: int = 0) -> Match | None:
    return _coerce_pattern(pattern, flags).search(string)


def match(pattern: str, string: str, flags: int = 0) -> Match | None:
    return _coerce_pattern(pattern, flags).match(string)


def fullmatch(pattern: str, string: str, flags: int = 0) -> Match | None:
    return _coerce_pattern(pattern, flags).fullmatch(string)


def finditer(pattern: str, string: str, flags: int = 0) -> Iterator[Match]:
    return _coerce_pattern(pattern, flags).finditer(string)


def findall(pattern: str, string: str, flags: int = 0) -> list[Any]:
    return _coerce_pattern(pattern, flags).findall(string)


def split(pattern: str, string: str, maxsplit: int = 0, flags: int = 0) -> list[str]:
    return _coerce_pattern(pattern, flags).split(string, maxsplit=maxsplit)


def sub(pattern: str, repl: object, string: str, count: int = 0, flags: int = 0) -> str:
    return _coerce_pattern(pattern, flags).sub(repl, string, count=count)


def subn(
    pattern: str, repl: object, string: str, count: int = 0, flags: int = 0
) -> tuple[str, int]:
    return _coerce_pattern(pattern, flags).subn(repl, string, count=count)


def escape(pattern: object) -> str:
    """Escape special characters in pattern."""
    _molt_re_escape = _require_intrinsic("molt_re_escape")
    if not isinstance(pattern, str):
        pattern = str(pattern)
    return _molt_re_escape(pattern)
