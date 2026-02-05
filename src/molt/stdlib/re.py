"""Regex support for Molt stdlib."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_stdlib_probe", globals())

from dataclasses import dataclass
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

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete native re parity (lookarounds, backreferences, named groups, verbose/ASCII flags, and Unicode casefold/class semantics).

ASCII = 256
DOTALL = 16
IGNORECASE = 2
LOCALE = 4
MULTILINE = 8
VERBOSE = 64

_META_CHARS = set(".^$*+?{}[]\\|()")
_SUPPORTED_FLAGS = IGNORECASE | MULTILINE | DOTALL


class error(Exception):
    pass


@dataclass(frozen=True)
class _Empty:
    pass


@dataclass(frozen=True)
class _Literal:
    text: str


@dataclass(frozen=True)
class _Any:
    pass


@dataclass(frozen=True)
class _Anchor:
    kind: str  # "start" or "end"


@dataclass(frozen=True)
class _CharClass:
    negated: bool
    ranges: tuple[tuple[str, str], ...]
    chars: tuple[str, ...]
    categories: tuple[str, ...]


@dataclass(frozen=True)
class _Concat:
    nodes: tuple[Any, ...]


@dataclass(frozen=True)
class _Alt:
    options: tuple[Any, ...]


@dataclass(frozen=True)
class _Repeat:
    node: Any
    min_count: int
    max_count: int | None
    greedy: bool


@dataclass(frozen=True)
class _Group:
    node: Any
    index: int


class _Parser:
    def __init__(self, pattern: str) -> None:
        self.pattern = pattern
        self.pos = 0
        self.group_count = 0

    def parse(self) -> tuple[Any, int]:
        node = self._parse_expr()
        if self.pos != len(self.pattern):
            raise error("unexpected pattern text")
        return node, self.group_count

    def _peek(self) -> str | None:
        if self.pos >= len(self.pattern):
            return None
        return self.pattern[self.pos]

    def _next(self) -> str:
        if self.pos >= len(self.pattern):
            raise error("unexpected end of pattern")
        ch = self.pattern[self.pos]
        self.pos += 1
        return ch

    def _parse_expr(self) -> Any:
        terms = [self._parse_term()]
        while self._peek() == "|":
            self._next()
            terms.append(self._parse_term())
        if len(terms) == 1:
            return terms[0]
        return _Alt(tuple(terms))

    def _parse_term(self) -> Any:
        nodes: list[Any] = []
        while True:
            ch = self._peek()
            if ch is None or ch in ")|":
                break
            node = self._parse_factor()
            if isinstance(node, _Literal) and nodes and isinstance(nodes[-1], _Literal):
                prev = nodes.pop()
                nodes.append(_Literal(prev.text + node.text))
            else:
                nodes.append(node)
        if not nodes:
            return _Empty()
        if len(nodes) == 1:
            return nodes[0]
        return _Concat(tuple(nodes))

    def _parse_factor(self) -> Any:
        node = self._parse_atom()
        ch = self._peek()
        if ch is None:
            return node
        if ch in "*+?":
            self._next()
            min_count, max_count = (0, None) if ch == "*" else (1, None)
            if ch == "?":
                min_count, max_count = (0, 1)
            greedy = True
            if self._peek() == "?":
                self._next()
                greedy = False
            return _Repeat(node, min_count, max_count, greedy)
        if ch == "{":
            start = self.pos
            self._next()
            min_count = self._parse_number()
            max_count = min_count
            if self._peek() == ",":
                self._next()
                if self._peek() == "}":
                    max_count = None
                else:
                    max_count = self._parse_number()
            if self._peek() != "}":
                self.pos = start
                return node
            self._next()
            if max_count is not None and max_count < min_count:
                raise error("invalid quantifier range")
            greedy = True
            if self._peek() == "?":
                self._next()
                greedy = False
            return _Repeat(node, min_count, max_count, greedy)
        return node

    def _parse_number(self) -> int:
        digits = []
        while True:
            ch = self._peek()
            if ch is None or not ch.isdigit():
                break
            digits.append(self._next())
        if not digits:
            raise error("expected number")
        return int("".join(digits))

    def _parse_atom(self) -> Any:
        ch = self._next()
        if ch == ".":
            return _Any()
        if ch == "^":
            return _Anchor("start")
        if ch == "$":
            return _Anchor("end")
        if ch == "(":
            if self._peek() == "?":
                raise NotImplementedError(
                    "group flags and non-capturing groups unsupported"
                )
            node = self._parse_expr()
            if self._peek() != ")":
                raise error("missing )")
            self._next()
            self.group_count += 1
            return _Group(node, self.group_count)
        if ch == "[":
            return self._parse_class()
        if ch == "\\":
            return self._parse_escape()
        if ch in _META_CHARS:
            raise error(f"unexpected character '{ch}'")
        return _Literal(ch)

    def _parse_escape(self) -> Any:
        ch = self._next()
        if ch in "dDsSwW":
            negated = ch.isupper()
            category = ch.lower()
            return _CharClass(negated, (), (), (category,))
        if ch in "ntrfv":
            mapped = {
                "n": "\n",
                "t": "\t",
                "r": "\r",
                "f": "\f",
                "v": "\v",
            }
            return _Literal(mapped[ch])
        if ch.isdigit():
            raise NotImplementedError("backreferences are not supported")
        if ch in "AbBzZ":
            raise NotImplementedError("escape anchors are not supported")
        return _Literal(ch)

    def _parse_class(self) -> Any:
        negated = False
        chars: list[str] = []
        ranges: list[tuple[str, str]] = []
        categories: list[str] = []
        if self._peek() == "^":
            self._next()
            negated = True
        if self._peek() == "]":
            chars.append(self._next())
        while True:
            ch = self._peek()
            if ch is None:
                raise error("unterminated character class")
            if ch == "]":
                self._next()
                break
            item = self._class_item()
            if isinstance(item, tuple) and item[0] == "range":
                ranges.append((item[1], item[2]))
                continue
            if isinstance(item, tuple) and item[0] == "category":
                categories.append(item[1])
                continue
            chars.append(item)
        return _CharClass(negated, tuple(ranges), tuple(chars), tuple(categories))

    def _class_item(self) -> Any:
        ch = self._next()
        if ch == "\\":
            esc = self._next()
            if esc in "dDsSwW":
                return ("category", esc.lower())
            if esc in "ntrfv":
                mapped = {
                    "n": "\n",
                    "t": "\t",
                    "r": "\r",
                    "f": "\f",
                    "v": "\v",
                }
                return mapped[esc]
            if esc.isdigit():
                raise NotImplementedError("backreferences are not supported")
            return esc
        if ch == "-" or ch == "]":
            return ch
        if self._peek() == "-":
            start_pos = self.pos
            self._next()
            next_ch = self._peek()
            if next_ch is None or next_ch == "]":
                self.pos = start_pos
                return ch
            end_item = self._class_item()
            if isinstance(end_item, tuple):
                raise NotImplementedError("ranges over categories are not supported")
            return ("range", ch, end_item)
        return ch


class Match:
    def __init__(
        self,
        pattern: "Pattern",
        string: str,
        start: int,
        end: int,
        groups: tuple[tuple[int, int] | None, ...],
    ) -> None:
        self._pattern = pattern
        self._string = string
        self._start = start
        self._end = end
        self._groups = groups

    def group(self, *indices: int) -> Any:
        if not indices:
            indices = (0,)
        if len(indices) == 1:
            return self._group_value(indices[0])
        return tuple(self._group_value(idx) for idx in indices)

    def groups(self) -> tuple[Any, ...]:
        return tuple(self._group_value(idx) for idx in range(1, len(self._groups)))

    def groupdict(self) -> dict[str, Any]:
        return {}

    def start(self, index: int = 0) -> int:
        span = self._group_span(index)
        return span[0]

    def end(self, index: int = 0) -> int:
        span = self._group_span(index)
        return span[1]

    def span(self, index: int = 0) -> tuple[int, int]:
        return self._group_span(index)

    def _group_span(self, index: int) -> tuple[int, int]:
        if index < 0 or index >= len(self._groups):
            raise IndexError("no such group")
        span = self._groups[index]
        if span is None:
            return (-1, -1)
        return span

    def _group_value(self, index: int) -> Any:
        span = self._group_span(index)
        if span == (-1, -1):
            return None
        return self._string[span[0] : span[1]]


class Pattern:
    def __init__(self, pattern: str, node: Any, groups: int, flags: int) -> None:
        self.pattern = pattern
        self.flags = flags
        self.groups = groups
        self._node = node

    def search(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return _search(self, string, pos, endpos)

    def match(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return _match(self, string, pos, endpos)

    def fullmatch(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Match | None:
        return _fullmatch(self, string, pos, endpos)


def _clamp_span(length: int, pos: int, endpos: int | None) -> tuple[int, int]:
    start = max(0, pos)
    end = length if endpos is None else max(0, endpos)
    if end > length:
        end = length
    if start > end:
        start = end
    return start, end


def _ensure_text(string: Any) -> str:
    if not isinstance(string, str):
        raise TypeError("expected string")
    return string


def _casefold(text: str) -> str:
    casefold = getattr(text, "casefold", None)
    if casefold is not None:
        return casefold()
    return text.lower()


def _class_matches(node: _CharClass, ch: str, flags: int) -> bool:
    hit = False
    if flags & IGNORECASE:
        ch_cmp = _casefold(ch)
        for item in node.chars:
            if ch_cmp == _casefold(item):
                hit = True
                break
        if not hit:
            for start, end in node.ranges:
                if _casefold(start) <= ch_cmp <= _casefold(end):
                    hit = True
                    break
    else:
        if ch in node.chars:
            hit = True
        if not hit:
            for start, end in node.ranges:
                if start <= ch <= end:
                    hit = True
                    break
    if not hit:
        for category in node.categories:
            if category == "d" or category == "digit":
                if ch.isdigit():
                    hit = True
                    break
            elif category == "w" or category == "word":
                if ch.isalnum() or ch == "_":
                    hit = True
                    break
            elif category == "s" or category == "space":
                if ch.isspace():
                    hit = True
                    break
    if node.negated:
        return not hit
    return hit


def _match_empty(
    _node: _Empty,
    _text: str,
    pos: int,
    _end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    _flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    return [(pos, groups)]


def _match_literal(
    node: _Literal,
    text: str,
    pos: int,
    end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    results: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
    length = len(node.text)
    if pos + length <= end:
        segment = text[pos : pos + length]
        if flags & IGNORECASE:
            if segment.casefold() == node.text.casefold():
                results.append((pos + length, groups))
        else:
            if segment == node.text:
                results.append((pos + length, groups))
    return results


def _match_any(
    _node: _Any,
    text: str,
    pos: int,
    end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    if pos < end and (flags & DOTALL or text[pos] != "\n"):
        return [(pos + 1, groups)]
    return []


def _match_anchor(
    node: _Anchor,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    results: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
    if node.kind == "start":
        if pos == origin or (
            flags & MULTILINE and pos > origin and text[pos - 1] == "\n"
        ):
            results.append((pos, groups))
    else:
        if flags & MULTILINE:
            if pos == end or (pos < end and text[pos] == "\n"):
                results.append((pos, groups))
        else:
            if pos == end or (pos == end - 1 and end > origin and text[pos] == "\n"):
                results.append((pos, groups))
    return results


def _match_charclass(
    node: _CharClass,
    text: str,
    pos: int,
    end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    if pos < end and _class_matches(node, text[pos], flags):
        return [(pos + 1, groups)]
    return []


def _match_group(
    node: _Group,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    results: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
    for new_pos, new_groups in _match_node(
        node.node, text, pos, end, origin, groups, flags
    ):
        updated = list(new_groups)
        updated[node.index] = (pos, new_pos)
        results.append((new_pos, tuple(updated)))
    return results


def _match_alt(
    node: _Alt,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    results: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
    for option in node.options:
        results.extend(_match_node(option, text, pos, end, origin, groups, flags))
    return results


def _match_node(
    node: Any,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    if isinstance(node, _Empty):
        return _match_empty(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Literal):
        return _match_literal(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Any):
        return _match_any(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Anchor):
        return _match_anchor(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _CharClass):
        return _match_charclass(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Group):
        return _match_group(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Concat):
        return _match_concat(node.nodes, text, pos, end, origin, groups, flags)
    if isinstance(node, _Alt):
        return _match_alt(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Repeat):
        return _match_repeat(node, text, pos, end, origin, groups, flags)
    raise error("unsupported pattern node")


def _match_concat(
    nodes: tuple[Any, ...],
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    if not nodes:
        return [(pos, groups)]
    first = nodes[0]
    rest = nodes[1:]
    results: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
    for new_pos, new_groups in _match_node(
        first, text, pos, end, origin, groups, flags
    ):
        results.extend(
            _match_concat(rest, text, new_pos, end, origin, new_groups, flags)
        )
    return results


def _match_repeat(
    node: _Repeat,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    def rec(
        count: int,
        cur_pos: int,
        cur_groups: tuple[tuple[int, int] | None, ...],
    ) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
        results: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
        if node.max_count is not None and count == node.max_count:
            if count >= node.min_count:
                results.append((cur_pos, cur_groups))
            return results
        if node.greedy:
            for next_pos, next_groups in _match_node(
                node.node, text, cur_pos, end, origin, cur_groups, flags
            ):
                if next_pos == cur_pos:
                    continue
                results.extend(rec(count + 1, next_pos, next_groups))
            if count >= node.min_count:
                results.append((cur_pos, cur_groups))
        else:
            if count >= node.min_count:
                results.append((cur_pos, cur_groups))
            for next_pos, next_groups in _match_node(
                node.node, text, cur_pos, end, origin, cur_groups, flags
            ):
                if next_pos == cur_pos:
                    continue
                results.extend(rec(count + 1, next_pos, next_groups))
        return results

    return rec(0, pos, groups)


def _compile_native(pattern: str, flags: int) -> Pattern:
    if flags & ~_SUPPORTED_FLAGS:
        raise NotImplementedError("regex flags are not supported yet")
    parser = _Parser(pattern)
    node, groups = parser.parse()
    return Pattern(pattern, node, groups, flags)


def _coerce_pattern(pattern: Any, flags: int) -> Any:
    if isinstance(pattern, Pattern):
        if flags:
            raise error("cannot specify flags with a compiled pattern")
        return pattern
    if hasattr(pattern, "search") and hasattr(pattern, "match"):
        if flags:
            raise error("cannot specify flags with a compiled pattern")
        return pattern
    if not isinstance(pattern, str):
        raise TypeError("pattern must be a string")
    try:
        return _compile_native(pattern, flags)
    except NotImplementedError:
        raise


def _match(
    pattern: Pattern,
    string: str,
    pos: int = 0,
    endpos: int | None = None,
) -> Match | None:
    text = _ensure_text(string)
    start, end = _clamp_span(len(text), pos, endpos)
    groups = tuple([None] * (pattern.groups + 1))
    flags = pattern.flags
    for new_pos, new_groups in _match_node(
        pattern._node, text, start, end, start, groups, flags
    ):
        updated = list(new_groups)
        updated[0] = (start, new_pos)
        return Match(pattern, text, start, new_pos, tuple(updated))
    return None


def _fullmatch(
    pattern: Pattern,
    string: str,
    pos: int = 0,
    endpos: int | None = None,
) -> Match | None:
    text = _ensure_text(string)
    start, end = _clamp_span(len(text), pos, endpos)
    groups = tuple([None] * (pattern.groups + 1))
    flags = pattern.flags
    for new_pos, new_groups in _match_node(
        pattern._node, text, start, end, start, groups, flags
    ):
        if new_pos != end:
            continue
        updated = list(new_groups)
        updated[0] = (start, new_pos)
        return Match(pattern, text, start, new_pos, tuple(updated))
    return None


def _search(
    pattern: Pattern,
    string: str,
    pos: int = 0,
    endpos: int | None = None,
) -> Match | None:
    text = _ensure_text(string)
    start, end = _clamp_span(len(text), pos, endpos)
    groups_template = tuple([None] * (pattern.groups + 1))
    flags = pattern.flags
    anchored = _is_anchored_start(pattern._node) and not (flags & MULTILINE)
    if anchored:
        for new_pos, new_groups in _match_node(
            pattern._node, text, start, end, start, groups_template, flags
        ):
            updated = list(new_groups)
            updated[0] = (start, new_pos)
            return Match(pattern, text, start, new_pos, tuple(updated))
        return None
    for offset in range(start, end + 1):
        for new_pos, new_groups in _match_node(
            pattern._node, text, offset, end, start, groups_template, flags
        ):
            updated = list(new_groups)
            updated[0] = (offset, new_pos)
            return Match(pattern, text, offset, new_pos, tuple(updated))
    return None


def _is_anchored_start(node: Any) -> bool:
    if isinstance(node, _Anchor) and node.kind == "start":
        return True
    if isinstance(node, _Concat):
        return bool(node.nodes) and _is_anchored_start(node.nodes[0])
    if isinstance(node, _Alt):
        return all(_is_anchored_start(option) for option in node.options)
    if isinstance(node, _Group):
        return _is_anchored_start(node.node)
    if isinstance(node, _Repeat):
        if node.min_count <= 0:
            return False
        return _is_anchored_start(node.node)
    return False


def compile(pattern: str, flags: int = 0) -> Pattern:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return compiled
    return compiled


def search(pattern: str, string: str, flags: int = 0) -> Match | None:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _search(compiled, string, 0, None)
    return compiled.search(string)


def match(pattern: str, string: str, flags: int = 0) -> Match | None:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _match(compiled, string, 0, None)
    return compiled.match(string)


def fullmatch(pattern: str, string: str, flags: int = 0) -> Match | None:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _fullmatch(compiled, string, 0, None)
    return compiled.fullmatch(string)
