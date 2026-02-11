"""Regex support for Molt stdlib."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from dataclasses import dataclass
from typing import Any, Iterator
import warnings as _warnings

_require_intrinsic("molt_stdlib_probe", globals())


def _require_callable_intrinsic(name: str):
    value = _require_intrinsic(name, globals())
    if not callable(value):
        raise RuntimeError(f"{name} intrinsic unavailable")
    return value


_molt_re_literal_advance = _require_callable_intrinsic("molt_re_literal_advance")
_molt_re_any_advance = _require_callable_intrinsic("molt_re_any_advance")
_molt_re_anchor_matches = _require_callable_intrinsic("molt_re_anchor_matches")
_molt_re_group_is_set = _require_callable_intrinsic("molt_re_group_is_set")
_molt_re_backref_group_advance = _require_callable_intrinsic(
    "molt_re_backref_group_advance"
)
_molt_re_apply_scoped_flags = _require_callable_intrinsic("molt_re_apply_scoped_flags")
_molt_re_group_capture = _require_callable_intrinsic("molt_re_group_capture")
_molt_re_charclass_advance = _require_callable_intrinsic("molt_re_charclass_advance")
_molt_re_group_values = _require_callable_intrinsic("molt_re_group_values")
_molt_re_expand_replacement = _require_callable_intrinsic("molt_re_expand_replacement")


__all__ = [
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
    "Pattern",
    "Match",
    "compile",
    "error",
    "escape",
    "findall",
    "finditer",
    "fullmatch",
    "match",
    "search",
    "split",
    "sub",
    "subn",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): complete native re parity and continue migrating parser/matcher execution into Rust (remaining lookaround variants, named-group edge cases, verbose-mode parser details, and full Unicode class/casefold semantics).

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

_META_CHARS = set(".^$*+?{}[]\\|()")
_SUPPORTED_FLAGS = IGNORECASE | MULTILINE | DOTALL | ASCII | UNICODE | VERBOSE


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


@dataclass(frozen=True)
class _Backref:
    index: int


@dataclass(frozen=True)
class _Look:
    node: Any
    behind: bool
    positive: bool


@dataclass(frozen=True)
class _ScopedFlags:
    node: Any
    add_flags: int
    clear_flags: int


@dataclass(frozen=True)
class _Conditional:
    group_index: int
    yes: Any
    no: Any


class _Parser:
    def __init__(self, pattern: str) -> None:
        self.pattern = pattern
        self.pos = 0
        self.group_count = 0
        self.group_names: dict[str, int] = {}
        self.inline_flags = 0
        self.nested_set_warning_pos: int | None = None

    def parse(self) -> tuple[Any, int, dict[str, int], int]:
        node = self._parse_expr()
        if self.pos != len(self.pattern):
            raise error("unexpected pattern text")
        return node, self.group_count, dict(self.group_names), self.inline_flags

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
                self._next()
                marker = self._peek()
                if marker == "=":
                    self._next()
                    node = self._parse_expr()
                    if self._peek() != ")":
                        raise error("missing )")
                    self._next()
                    return _Look(node, behind=False, positive=True)
                if marker == "!":
                    raise NotImplementedError("negative lookahead is not supported")
                if marker == "<":
                    self._next()
                    look_kind = self._peek()
                    if look_kind == "=":
                        self._next()
                        node = self._parse_expr()
                        if self._peek() != ")":
                            raise error("missing )")
                        self._next()
                        if _fixed_width(node) is None:
                            raise error("look-behind requires fixed-width pattern")
                        return _Look(node, behind=True, positive=True)
                    if look_kind == "!":
                        raise NotImplementedError(
                            "negative lookbehind is not supported"
                        )
                    raise error("unknown extension")
                if marker == "(":
                    self._next()
                    digits: list[str] = []
                    while True:
                        token = self._peek()
                        if token is None:
                            raise error("missing )")
                        if not ("0" <= token <= "9"):
                            break
                        digits.append(self._next())
                    if not digits or self._peek() != ")":
                        raise error("bad character in group name")
                    self._next()
                    group_index = int("".join(digits))
                    yes_node = self._parse_term()
                    no_node: Any = _Empty()
                    if self._peek() == "|":
                        self._next()
                        no_node = self._parse_term()
                    if self._peek() != ")":
                        raise error("missing )")
                    self._next()
                    return _Conditional(group_index, yes_node, no_node)
                if marker == ":":
                    self._next()
                    node = self._parse_expr()
                    if self._peek() != ")":
                        raise error("missing )")
                    self._next()
                    return node
                if marker == "P":
                    self._next()
                    if self._next() != "<":
                        raise error("bad character in group name")
                    name_chars: list[str] = []
                    while True:
                        token = self._peek()
                        if token is None:
                            raise error("unterminated group name")
                        if token == ">":
                            self._next()
                            break
                        if token in _META_CHARS or token in "<>":
                            raise error("bad character in group name")
                        name_chars.append(self._next())
                    if not name_chars:
                        raise error("missing group name")
                    name = "".join(name_chars)
                    if name in self.group_names:
                        raise error("redefinition of group name")
                    node = self._parse_expr()
                    if self._peek() != ")":
                        raise error("missing )")
                    self._next()
                    self.group_count += 1
                    self.group_names[name] = self.group_count
                    return _Group(node, self.group_count)
                # Inline flags: (?i), (?s), (?x), (?-i:...), (?i:...)
                flags = 0
                clear_flags = 0
                seen_minus = False
                while True:
                    token = self._peek()
                    if token is None:
                        raise error("unterminated inline flag")
                    if token == "-":
                        seen_minus = True
                        self._next()
                        continue
                    if token in "imsxaLu":
                        self._next()
                        bit = 0
                        if token in ("i", "I"):
                            bit = IGNORECASE
                        elif token in ("m", "M"):
                            bit = MULTILINE
                        elif token in ("s", "S"):
                            bit = DOTALL
                        elif token in ("x", "X"):
                            bit = VERBOSE
                        elif token in ("a", "A"):
                            bit = ASCII
                        elif token in ("u", "U"):
                            bit = UNICODE
                        elif token in ("L", "l"):
                            bit = LOCALE
                        if seen_minus:
                            clear_flags |= bit
                        else:
                            flags |= bit
                        continue
                    break
                token = self._peek()
                if token == ")":
                    self._next()
                    self.inline_flags |= flags
                    self.inline_flags &= ~clear_flags
                    return _Empty()
                if token == ":":
                    self._next()
                    node = self._parse_expr()
                    if self._peek() != ")":
                        raise error("missing )")
                    self._next()
                    return _ScopedFlags(node, flags, clear_flags)
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
            negated = "A" <= ch <= "Z"
            if negated:
                category = chr(ord(ch) + 32)
            else:
                category = ch
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
        if "0" <= ch <= "9":
            digits = [ch]
            while True:
                nxt = self._peek()
                if nxt is None or not ("0" <= nxt <= "9"):
                    break
                digits.append(self._next())
            return _Backref(int("".join(digits)))
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
                if "A" <= esc <= "Z":
                    esc = chr(ord(esc) + 32)
                return ("category", esc)
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
        if ch == "[" and self._peek() == ":":
            if self.nested_set_warning_pos is None:
                self.nested_set_warning_pos = self.pos - 1
            self._next()
            name_chars: list[str] = []
            while True:
                token = self._peek()
                if token is None:
                    raise error("unterminated character class")
                if token == ":":
                    self._next()
                    if self._peek() != "]":
                        name_chars.append(":")
                        continue
                    self._next()
                    break
                name_chars.append(self._next())
            return ("category", "posix:" + "".join(name_chars))
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


def _fixed_width(node: Any) -> int | None:
    if isinstance(node, _Empty):
        return 0
    if isinstance(node, _Literal):
        return len(node.text)
    if isinstance(node, _Any):
        return 1
    if isinstance(node, _Anchor):
        return 0
    if isinstance(node, _CharClass):
        return 1
    if isinstance(node, _Backref):
        return None
    if isinstance(node, _Group):
        return _fixed_width(node.node)
    if isinstance(node, _Look):
        return 0
    if isinstance(node, _ScopedFlags):
        return _fixed_width(node.node)
    if isinstance(node, _Conditional):
        yes_width = _fixed_width(node.yes)
        no_width = _fixed_width(node.no)
        if yes_width is None or no_width is None:
            return None
        if yes_width != no_width:
            return None
        return yes_width
    if isinstance(node, _Concat):
        total = 0
        for item in node.nodes:
            width = _fixed_width(item)
            if width is None:
                return None
            total += width
        return total
    if isinstance(node, _Alt):
        if not node.options:
            return 0
        first = _fixed_width(node.options[0])
        if first is None:
            return None
        for item in node.options[1:]:
            width = _fixed_width(item)
            if width is None or width != first:
                return None
        return first
    if isinstance(node, _Repeat):
        width = _fixed_width(node.node)
        if width is None:
            return None
        if node.max_count is None:
            return None
        if node.min_count != node.max_count:
            return None
        return width * node.min_count
    return None


class Match:
    def __init__(
        self,
        pattern: "Pattern",
        string: str,
        start: int,
        end: int,
        groups: tuple[tuple[int, int] | None, ...],
        group_names: dict[str, int],
    ) -> None:
        self._pattern = pattern
        self._string = string
        self._start = start
        self._end = end
        self._groups = groups
        self._group_names = group_names

    def group(self, *indices: int | str) -> Any:
        if not indices:
            indices = (0,)
        if len(indices) == 1:
            return self._group_value(indices[0])
        return tuple(self._group_value(idx) for idx in indices)

    def groups(self, default: Any = None) -> tuple[Any, ...]:
        out = []
        for idx in range(1, len(self._groups)):
            value = self._group_value(idx)
            if value is None:
                value = default
            out.append(value)
        return tuple(out)

    def groupdict(self, default: Any = None) -> dict[str, Any]:
        out: dict[str, Any] = {}
        for name, idx in self._group_names.items():
            value = self._group_value(idx)
            if value is None:
                value = default
            out[name] = value
        return out

    def start(self, index: int = 0) -> int:
        span = self._group_span(index)
        return span[0]

    def end(self, index: int = 0) -> int:
        span = self._group_span(index)
        return span[1]

    def span(self, index: int = 0) -> tuple[int, int]:
        return self._group_span(index)

    def _group_span(self, index: int | str) -> tuple[int, int]:
        if isinstance(index, str):
            if index not in self._group_names:
                raise IndexError("no such group")
            index = self._group_names[index]
        if index < 0 or index >= len(self._groups):
            raise IndexError("no such group")
        span = self._groups[index]
        if span is None:
            return (-1, -1)
        return span

    def _group_value(self, index: int | str) -> Any:
        span = self._group_span(index)
        if span == (-1, -1):
            return None
        return self._string[span[0] : span[1]]


class Pattern:
    def __init__(
        self,
        pattern: str,
        node: Any,
        groups: int,
        flags: int,
        group_names: dict[str, int] | None = None,
    ) -> None:
        self.pattern = pattern
        self.flags = flags
        self.groups = groups
        self._node = node
        self._group_names = dict(group_names or {})

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

    def finditer(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> Iterator[Match]:
        return _finditer(self, string, pos, endpos)

    def findall(
        self, string: str, pos: int = 0, endpos: int | None = None
    ) -> list[Any]:
        return _findall(self, string, pos, endpos)

    def split(self, string: str, maxsplit: int = 0) -> list[str]:
        return _split(self, string, maxsplit=maxsplit)

    def sub(self, repl: object, string: str, count: int = 0) -> str:
        return _sub(self, repl, string, count=count)

    def subn(self, repl: object, string: str, count: int = 0) -> tuple[str, int]:
        return _subn(self, repl, string, count=count)


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
    new_pos = _molt_re_literal_advance(text, pos, end, node.text, flags)
    if new_pos < 0:
        return []
    return [(new_pos, groups)]


def _match_any(
    _node: _Any,
    text: str,
    pos: int,
    end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    new_pos = _molt_re_any_advance(text, pos, end, flags)
    if new_pos >= 0:
        return [(new_pos, groups)]
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
    if _molt_re_anchor_matches(node.kind, text, pos, end, origin, flags):
        return [(pos, groups)]
    return []


def _match_charclass(
    node: _CharClass,
    text: str,
    pos: int,
    end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    new_pos = _molt_re_charclass_advance(
        text, pos, end, node.negated, node.chars, node.ranges, node.categories, flags
    )
    if new_pos < 0:
        return []
    return [(new_pos, groups)]


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
        updated = _molt_re_group_capture(new_groups, node.index, pos, new_pos)
        results.append((new_pos, updated))
    return results


def _match_backref(
    node: _Backref,
    text: str,
    pos: int,
    end: int,
    _origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    _flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    new_pos = _molt_re_backref_group_advance(text, pos, end, groups, node.index)
    if new_pos < 0:
        return []
    return [(new_pos, groups)]


def _match_look(
    node: _Look,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    if node.behind:
        width = _fixed_width(node.node)
        if width is None:
            raise error("look-behind requires fixed-width pattern")
        start = pos - width
        if start < 0:
            return [] if node.positive else [(pos, groups)]
        matches: list[tuple[int, tuple[tuple[int, int] | None, ...]]] = []
        for end_pos, new_groups in _match_node(
            node.node, text, start, pos, start, groups, flags
        ):
            if end_pos == pos:
                matches.append((pos, new_groups))
        if node.positive:
            return matches
        return [] if matches else [(pos, groups)]
    matches = _match_node(node.node, text, pos, end, origin, groups, flags)
    if node.positive:
        return [(pos, new_groups) for _, new_groups in matches]
    if matches:
        return []
    return [(pos, groups)]


def _match_scoped_flags(
    node: _ScopedFlags,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    scoped = _molt_re_apply_scoped_flags(flags, node.add_flags, node.clear_flags)
    return _match_node(node.node, text, pos, end, origin, groups, scoped)


def _match_conditional(
    node: _Conditional,
    text: str,
    pos: int,
    end: int,
    origin: int,
    groups: tuple[tuple[int, int] | None, ...],
    flags: int,
) -> list[tuple[int, tuple[tuple[int, int] | None, ...]]]:
    matched = _molt_re_group_is_set(groups, node.group_index)
    branch = node.yes if matched else node.no
    return _match_node(branch, text, pos, end, origin, groups, flags)


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
    if isinstance(node, _Backref):
        return _match_backref(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Look):
        return _match_look(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _ScopedFlags):
        return _match_scoped_flags(node, text, pos, end, origin, groups, flags)
    if isinstance(node, _Conditional):
        return _match_conditional(node, text, pos, end, origin, groups, flags)
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
    if flags & LOCALE:
        raise ValueError("cannot use LOCALE flag with a str pattern")
    if flags & ASCII and flags & UNICODE:
        raise ValueError("ASCII and UNICODE flags are incompatible")
    if not (flags & ASCII):
        flags |= UNICODE
    parser = _Parser(pattern)
    node, groups, group_names, inline_flags = parser.parse()
    effective_flags = (flags | inline_flags) & ~(LOCALE)
    if parser.nested_set_warning_pos is not None:
        _warnings.warn(
            f"Possible nested set at position {parser.nested_set_warning_pos}",
            FutureWarning,
            stacklevel=2,
        )
    return Pattern(pattern, node, groups, effective_flags, group_names)


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
        return Match(
            pattern, text, start, new_pos, tuple(updated), pattern._group_names
        )
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
        return Match(
            pattern, text, start, new_pos, tuple(updated), pattern._group_names
        )
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
            return Match(
                pattern, text, start, new_pos, tuple(updated), pattern._group_names
            )
        return None
    for offset in range(start, end + 1):
        for new_pos, new_groups in _match_node(
            pattern._node, text, offset, end, start, groups_template, flags
        ):
            updated = list(new_groups)
            updated[0] = (offset, new_pos)
            return Match(
                pattern, text, offset, new_pos, tuple(updated), pattern._group_names
            )
    return None


def _finditer(
    pattern: Pattern,
    string: str,
    pos: int = 0,
    endpos: int | None = None,
) -> Iterator[Match]:
    text = _ensure_text(string)
    start, end = _clamp_span(len(text), pos, endpos)
    cursor = start
    while cursor <= end:
        match_obj = _search(pattern, text, cursor, end)
        if match_obj is None:
            break
        yield match_obj
        m_start, m_end = match_obj.span()
        if m_end > m_start:
            cursor = m_end
        else:
            # Zero-width match: force forward progress.
            if cursor >= end:
                break
            cursor = m_start + 1


def _findall(
    pattern: Pattern,
    string: str,
    pos: int = 0,
    endpos: int | None = None,
) -> list[Any]:
    out: list[Any] = []
    for match_obj in _finditer(pattern, string, pos, endpos):
        if pattern.groups == 0:
            out.append(match_obj.group(0))
        elif pattern.groups == 1:
            out.append(match_obj.group(1))
        else:
            out.append(match_obj.groups())
    return out


def _match_group_values(match_obj: Match) -> tuple[object, ...]:
    return _molt_re_group_values(match_obj._string, match_obj._groups)


def _expand_replacement(repl: object, match_obj: Match) -> str:
    if callable(repl):
        return str(repl(match_obj))
    if not isinstance(repl, str):
        repl = str(repl)
    return _molt_re_expand_replacement(repl, _match_group_values(match_obj))


def _subn(
    pattern: Pattern,
    repl: object,
    string: str,
    *,
    count: int = 0,
) -> tuple[str, int]:
    if count < 0:
        raise ValueError("count must be non-negative")
    text = _ensure_text(string)
    parts: list[str] = []
    last = 0
    replaced = 0
    limit = None if count == 0 else count
    for match_obj in _finditer(pattern, text, 0, None):
        if limit is not None and replaced >= limit:
            break
        m_start, m_end = match_obj.span()
        parts.append(text[last:m_start])
        parts.append(_expand_replacement(repl, match_obj))
        last = m_end
        replaced += 1
    parts.append(text[last:])
    return ("".join(parts), replaced)


def _sub(pattern: Pattern, repl: object, string: str, *, count: int = 0) -> str:
    return _subn(pattern, repl, string, count=count)[0]


def _split(pattern: Pattern, string: str, *, maxsplit: int = 0) -> list[str]:
    if maxsplit < 0:
        raise ValueError("maxsplit must be non-negative")
    text = _ensure_text(string)
    out: list[str] = []
    last = 0
    splits = 0
    limit = None if maxsplit == 0 else maxsplit
    for match_obj in _finditer(pattern, text, 0, None):
        if limit is not None and splits >= limit:
            break
        m_start, m_end = match_obj.span()
        out.append(text[last:m_start])
        if pattern.groups:
            for value in match_obj.groups():
                out.append("" if value is None else value)
        last = m_end
        splits += 1
    out.append(text[last:])
    return out


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


def finditer(pattern: str, string: str, flags: int = 0) -> Iterator[Match]:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _finditer(compiled, string, 0, None)
    return compiled.finditer(string)


def findall(pattern: str, string: str, flags: int = 0) -> list[Any]:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _findall(compiled, string, 0, None)
    return compiled.findall(string)


def split(pattern: str, string: str, maxsplit: int = 0, flags: int = 0) -> list[str]:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _split(compiled, string, maxsplit=maxsplit)
    return compiled.split(string, maxsplit=maxsplit)


def sub(
    pattern: str,
    repl: object,
    string: str,
    count: int = 0,
    flags: int = 0,
) -> str:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _sub(compiled, repl, string, count=count)
    return compiled.sub(repl, string, count=count)


def subn(
    pattern: str,
    repl: object,
    string: str,
    count: int = 0,
    flags: int = 0,
) -> tuple[str, int]:
    compiled = _coerce_pattern(pattern, flags)
    if isinstance(compiled, Pattern):
        return _subn(compiled, repl, string, count=count)
    return compiled.subn(repl, string, count=count)


def escape(pattern: object) -> str:
    if not isinstance(pattern, str):
        pattern = str(pattern)
    out: list[str] = []
    for ch in pattern:
        if ("a" <= ch <= "z") or ("A" <= ch <= "Z") or ("0" <= ch <= "9") or ch == "_":
            out.append(ch)
        else:
            out.append("\\")
            out.append(ch)
    return "".join(out)
