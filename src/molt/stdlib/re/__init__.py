"""Regex support for Molt stdlib — thin wrapper around Rust intrinsics."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from typing import Any, Iterator, Mapping
from types import MappingProxyType as _MappingProxyType

# Bind intrinsics in their consuming functions. Static link reachability still
# retains a published function's body even when no caller invokes it, so lazy
# binding alone does not make an imported regex module feature-free. The shared
# required_features authority accounts for these references and registry roots.
# This module-body probe is core/ungated and carries no regex link dependency.

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
_cache: dict[tuple[str | bytes, int], "Pattern"] = {}
_MAXCACHE = 512


def purge() -> None:
    """Clear the regular expression cache."""
    _cache.clear()


# ---------------------------------------------------------------------------
# Match — thin wrapper around intrinsic result tuple
# ---------------------------------------------------------------------------
# Intrinsic result: (match_start, match_end, group_spans, last_closed_group)
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
        "_lastindex",
    )

    def __init__(
        self,
        pattern: "Pattern",
        string: Any,
        pos: int,
        endpos: int,
        start: int,
        end: int,
        group_spans: tuple[tuple[int, int] | None, ...],
        lastindex: int | None,
    ) -> None:
        self._pattern = pattern
        self._string = string
        self._pos = pos
        self._endpos = endpos
        self._start = start
        self._end = end
        self._group_spans = group_spans
        self._lastindex = lastindex

    def group(self, *indices: int | str) -> Any:
        if not indices:
            indices = (0,)
        spans = tuple(self._group_span(index) for index in indices)
        values = _captured_values(self._string, spans)
        return values[0] if len(values) == 1 else values

    def groups(self, default: Any = None) -> tuple[Any, ...]:
        values = _captured_values(
            self._string,
            tuple(self._group_span(index) for index in range(1, len(self._group_spans) + 1)),
        )
        return tuple(default if value is None else value for value in values)

    def groupdict(self, default: Any = None) -> dict[str, Any]:
        names = self._pattern.groupindex
        values = _captured_values(
            self._string, tuple(self._group_span(index) for index in names.values())
        )
        return {
            name: default if value is None else value
            for name, value in zip(names, values, strict=True)
        }

    def start(self, group: int | str = 0) -> int:
        return self._group_span(group)[0]

    def end(self, group: int | str = 0) -> int:
        return self._group_span(group)[1]

    def span(self, group: int | str = 0) -> tuple[int, int]:
        return self._group_span(group)

    def expand(self, template: str | bytes) -> str | bytes:
        if isinstance(self._pattern.pattern, bytes):
            if not isinstance(template, bytes):
                raise TypeError("sequence item 0: expected a bytes-like object, str found")
        elif not isinstance(template, str):
            raise TypeError("sequence item 0: expected str instance, bytes found")
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
    def string(self) -> Any:
        return self._string

    @property
    def pos(self) -> int:
        return self._pos

    @property
    def endpos(self) -> int:
        return self._endpos

    @property
    def lastindex(self) -> int | None:
        return self._lastindex

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

# ---------------------------------------------------------------------------
# Pattern — thin wrapper around a compiled intrinsic handle
# ---------------------------------------------------------------------------


class Pattern:
    __slots__ = (
        "_pattern",
        "_flags",
        "_groups",
        "_groupindex",
        "_groupindex_data",
        "_handle",
        "__weakref__",
    )

    def __init__(
        self,
        pattern: str | bytes,
        flags: int,
        handle: object,
        groups: int,
        groupindex: Mapping[str, int],
    ) -> None:
        self._pattern = pattern
        self._flags = flags
        self._handle = handle
        self._groups = groups
        self._groupindex_data = dict(groupindex)
        self._groupindex = _MappingProxyType(self._groupindex_data)

    @property
    def pattern(self) -> str | bytes:
        return self._pattern

    @property
    def flags(self) -> int:
        return self._flags

    @property
    def groups(self) -> int:
        return self._groups

    @property
    def groupindex(self) -> Mapping[str, int]:
        return self._groupindex

    def search(
        self, string: Any, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return self._execute(string, pos, endpos, "search")

    def match(
        self, string: Any, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return self._execute(string, pos, endpos, "match")

    def fullmatch(
        self, string: Any, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return self._execute(string, pos, endpos, "fullmatch")

    def finditer(
        self, string: Any, pos: int = 0, endpos: int | None = None
    ) -> Iterator[Match]:
        return _RegexScanner(self, string, pos, endpos)

    def findall(
        self, string: Any, pos: int = 0, endpos: int | None = None
    ) -> list[Any]:
        results: list[Any] = []
        group_count = self.groups
        empty = b"" if isinstance(self.pattern, bytes) else ""
        for m in self.finditer(string, pos, endpos):
            if group_count == 0:
                results.append(m.group(0))
            elif group_count == 1:
                value = m.group(1)
                results.append(empty if value is None else value)
            else:
                results.append(m.groups(empty))
        return results

    def split(self, string: Any, maxsplit: int = 0) -> list[Any]:
        _molt_re_split = _require_intrinsic("molt_re_split")
        text = _subject_for(self, string)
        return _molt_re_split(self._handle, text, maxsplit)

    def sub(self, repl: object, string: Any, count: int = 0) -> Any:
        return self.subn(repl, string, count=count)[0]

    def subn(self, repl: object, string: Any, count: int = 0) -> tuple[Any, int]:
        if callable(repl):
            return _subn_callable(self, repl, string, count=count)
        text = _subject_for(self, string)
        if isinstance(self.pattern, bytes):
            if not isinstance(repl, bytes):
                raise TypeError("a bytes-like object is required, not 'str'")
            repl = _require_intrinsic("molt_re_subject_snapshot")(repl)
            if not isinstance(repl, bytes):
                raise TypeError("a bytes-like object is required for the replacement")
        elif not isinstance(repl, str):
            repl = str(repl)
        _molt_re_sub = _require_intrinsic("molt_re_sub")
        result = _molt_re_sub(self._handle, repl, text, count)
        if _is_regex_syntax_error(result):
            _raise_regex_syntax_error(result)
        return result

    def __repr__(self) -> str:
        return f"re.compile({self.pattern!r}, {self.flags!r})"

    def _execute(
        self, string: Any, pos: int, endpos: int | None, mode: str
    ) -> Match | None:
        _molt_re_execute = _require_intrinsic("molt_re_execute")
        text = _subject_for(self, string)
        start, end = _clamp_span(_subject_length(text), pos, endpos)
        raw = _molt_re_execute(self._handle, text, start, end, mode)
        if raw is None:
            return None
        return Match(self, string, start, end, raw[0], raw[1], raw[2], raw[3])


class _RegexScanner(Iterator[Match]):
    """Ordinary eager-acquisition iterator over native scanner state."""

    __slots__ = (
        "_pattern",
        "_string",
        "_subject",
        "_start",
        "_end",
        "_scanner",
        "_exhausted",
    )

    def __init__(
        self, pattern: Pattern, string: Any, pos: int, endpos: int | None
    ) -> None:
        self._pattern = pattern
        self._string = string
        if isinstance(string, str) or type(string) is bytes:
            subject = string
        else:
            subject = _require_intrinsic("molt_re_subject_view")(string)
        if isinstance(pattern.pattern, bytes):
            if isinstance(subject, str):
                raise TypeError("cannot use a bytes pattern on a string-like object")
        elif not isinstance(subject, str):
            raise TypeError("cannot use a string pattern on a bytes-like object")
        length = _subject_length(subject)
        self._subject = subject
        self._start, self._end = _clamp_span(length, pos, endpos)
        create_scanner = _require_intrinsic("molt_re_scanner_new")
        self._scanner = create_scanner(
            pattern._handle, subject, self._start, self._end
        )
        self._exhausted = False

    def __iter__(self) -> _RegexScanner:
        return self

    def __next__(self) -> Match:
        if self._exhausted:
            raise StopIteration
        next_match = _require_intrinsic("molt_re_finditer_next")
        raw = next_match(self._scanner, self._subject)
        if raw is None:
            self._exhausted = True
            self._subject = b""
            self._string = None
            self._scanner = None
            self._pattern = None
            raise StopIteration
        return Match(
            self._pattern,
            self._string,
            self._start,
            self._end,
            raw[0],
            raw[1],
            raw[2],
            raw[3],
        )


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _ensure_text(string: Any) -> str:
    if not isinstance(string, str):
        raise TypeError("expected string or bytes-like object")
    return string


def _subject_for(pattern: Pattern, string: Any) -> str | bytes:
    bytes_pattern = isinstance(pattern.pattern, bytes)
    if isinstance(string, str) or type(string) is bytes:
        snapshot = string
    elif bytes_pattern:
        snapshot = _require_intrinsic("molt_re_subject_snapshot")(string)
    else:
        # Admit the real exporter before the kind mismatch, without copying
        # a potentially large byte subject that this text pattern cannot use.
        snapshot = _require_intrinsic("molt_re_subject_view")(string)
    if bytes_pattern:
        if isinstance(snapshot, str):
            raise TypeError("cannot use a bytes pattern on a string-like object")
    elif not isinstance(snapshot, str):
        raise TypeError("cannot use a string pattern on a bytes-like object")
    return snapshot


def _clamp_span(length: int, pos: int, endpos: int | None) -> tuple[int, int]:
    start = max(pos, 0)
    end = length if endpos is None else max(endpos, 0)
    if end > length:
        end = length
    if start > end:
        start = end
    return start, end


def _subject_length(subject: str | bytes | memoryview) -> int:
    """Read physical string/bytes length without dispatching subclass hooks."""
    if isinstance(subject, str):
        return str.__len__(subject)
    if isinstance(subject, memoryview):
        return subject.nbytes
    return bytes.__len__(subject)


def _subject_slice(
    subject: str | bytes | memoryview, start: int, end: int
) -> str | bytes:
    """Slice physical subject storage without dispatching subclass hooks."""
    if isinstance(subject, str):
        return str.__getitem__(subject, slice(start, end))
    if isinstance(subject, memoryview):
        return subject[start:end].tobytes()
    return bytes.__getitem__(subject, slice(start, end))


def _captured_values(
    subject: Any, spans: tuple[tuple[int, int], ...]
) -> tuple[object, ...]:
    _molt_re_group_values = _require_intrinsic("molt_re_group_values")
    normalized = tuple(None if start < 0 else (start, end) for start, end in spans)
    return _molt_re_group_values(subject, normalized)


def _match_group_values(match_obj: Match) -> tuple[object, ...]:
    group0 = (match_obj._start, match_obj._end)
    spans = (group0,) + tuple(
        (-1, -1) if span is None else span for span in match_obj._group_spans
    )
    return _captured_values(match_obj._string, spans)


def _expand_replacement(repl: str | bytes, match_obj: Match) -> str | bytes:
    expected = bytes if isinstance(match_obj._pattern.pattern, bytes) else str
    if not isinstance(repl, expected):
        raise TypeError("replacement template must match the pattern type")
    _molt_re_expand_replacement = _require_intrinsic("molt_re_expand_replacement")
    result = _molt_re_expand_replacement(
        repl, _match_group_values(match_obj), match_obj._pattern._groupindex_data
    )
    if _is_regex_syntax_error(result):
        _raise_regex_syntax_error(result)
    return result


def _is_regex_syntax_error(value: Any) -> bool:
    return (
        isinstance(value, tuple)
        and len(value) == 3
        and value[0] is None
        and value[1] is None
        and isinstance(value[2], str)
    )


def _raise_regex_syntax_error(value: tuple[None, None, str]) -> None:
    raise error(value[2])


def _compile(pattern: str | bytes, flags: int) -> Pattern:
    is_bytes = isinstance(pattern, bytes)
    if is_bytes and type(pattern) is not bytes:
        # CPython admits bytes subclasses through their buffer slot, while the
        # compiled syntax still comes from the subclass's physical bytes.
        _require_intrinsic("molt_re_subject_snapshot")(pattern)
    if flags & LOCALE and not is_bytes:
        raise ValueError("cannot use LOCALE flag with a str pattern")
    if flags & ASCII and flags & UNICODE:
        raise ValueError("ASCII and UNICODE flags are incompatible")
    if not is_bytes and not (flags & ASCII):
        flags |= UNICODE
    if is_bytes and flags & UNICODE:
        raise ValueError("cannot use UNICODE flag with a bytes pattern")
    effective_pattern = pattern
    if flags & VERBOSE and not is_bytes:
        _molt_re_strip_verbose = _require_intrinsic("molt_re_strip_verbose")
        effective_pattern = _molt_re_strip_verbose(pattern, flags)
    _molt_re_compile = _require_intrinsic("molt_re_compile")
    handle = _molt_re_compile(effective_pattern, flags)
    if _is_regex_syntax_error(handle):
        _raise_regex_syntax_error(handle)
    _molt_re_pattern_info = _require_intrinsic("molt_re_pattern_info")
    info = _molt_re_pattern_info(handle)
    groups, groupindex, effective_flags, warn_pos = info[0], info[1], info[2], info[3]
    if warn_pos is not None:
        import warnings

        warnings.warn(
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
    if not isinstance(pattern, (str, bytes)):
        raise TypeError("first argument must be string or compiled pattern")
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
    pattern: Pattern, repl: object, string: Any, *, count: int = 0
) -> tuple[str | bytes, int]:
    scanner = pattern.finditer(string)
    subject = scanner._subject
    byte_pattern = isinstance(pattern.pattern, bytes)
    parts: list[str] | list[bytes] = []
    last = 0
    replaced = 0
    while not count or replaced < count:
        try:
            match_obj = next(scanner)
        except StopIteration:
            break
        parts.append(_subject_slice(subject, last, match_obj._start))
        value = repl(match_obj)  # type: ignore[operator]
        if byte_pattern:
            if not isinstance(value, bytes):
                raise TypeError("sequence item 1: expected a bytes-like object, str found")
        elif not isinstance(value, str):
            raise TypeError("sequence item 1: expected str instance, object found")
        parts.append(value)
        last = match_obj._end
        replaced += 1
    if not replaced:
        unchanged = (
            subject
            if isinstance(subject, (bytes, str))
            else _subject_slice(subject, 0, _subject_length(subject))
        )
        return unchanged, 0
    parts.append(_subject_slice(subject, last, _subject_length(subject)))
    return (b"".join(parts) if byte_pattern else "".join(parts)), replaced


# ---------------------------------------------------------------------------
# Module-level convenience functions
# ---------------------------------------------------------------------------


def compile(pattern: str | bytes, flags: int = 0) -> Pattern:
    """Compile a regular expression pattern, returning a Pattern object."""
    return _coerce_pattern(pattern, flags)


def search(pattern: str | bytes, string: Any, flags: int = 0) -> Match | None:
    return _coerce_pattern(pattern, flags).search(string)


def match(pattern: str | bytes, string: Any, flags: int = 0) -> Match | None:
    return _coerce_pattern(pattern, flags).match(string)


def fullmatch(pattern: str | bytes, string: Any, flags: int = 0) -> Match | None:
    return _coerce_pattern(pattern, flags).fullmatch(string)


def finditer(pattern: str | bytes, string: Any, flags: int = 0) -> Iterator[Match]:
    return _coerce_pattern(pattern, flags).finditer(string)


def findall(pattern: str | bytes, string: Any, flags: int = 0) -> list[Any]:
    return _coerce_pattern(pattern, flags).findall(string)


def split(pattern: str | bytes, string: Any, maxsplit: int = 0, flags: int = 0) -> list[Any]:
    return _coerce_pattern(pattern, flags).split(string, maxsplit=maxsplit)


def sub(pattern: str | bytes, repl: object, string: Any, count: int = 0, flags: int = 0) -> Any:
    return _coerce_pattern(pattern, flags).sub(repl, string, count=count)


def subn(
    pattern: str | bytes, repl: object, string: Any, count: int = 0, flags: int = 0
) -> tuple[Any, int]:
    return _coerce_pattern(pattern, flags).subn(repl, string, count=count)


def escape(pattern: object) -> str | bytes:
    """Escape special characters in pattern."""
    _molt_re_escape = _require_intrinsic("molt_re_escape")
    if not isinstance(pattern, str):
        pattern = _require_intrinsic("molt_re_subject_snapshot")(pattern)
    return _molt_re_escape(pattern)
