"""Canonical stable source identity for parsed and synthetic Python AST nodes."""

from __future__ import annotations

import ast
import hashlib
import struct
from collections.abc import Iterator
from dataclasses import dataclass, field
from typing import TypeAlias, cast


PythonSourceKey: TypeAlias = tuple[int, int, int, int, str]


def python_source_digest(source: str) -> str:
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


_AST_MISSING = object()
_AST_END = object()
_AST_BUFFER_SIZE = 16 * 1024


class _AstDigestStream:
    """Bounded framing buffer; large payloads go directly to SHA-256."""

    __slots__ = ("hash", "buffer")

    def __init__(self, domain: bytes) -> None:
        self.hash = hashlib.sha256(domain)
        self.buffer = bytearray()

    def write(self, payload: bytes) -> None:
        if len(self.buffer) + len(payload) >= _AST_BUFFER_SIZE:
            self.hash.update(self.buffer)
            self.buffer.clear()
            self.hash.update(payload)
        else:
            self.buffer.extend(payload)

    def framed(self, payload: bytes) -> None:
        length = len(payload).to_bytes(8, "big")
        if len(payload) < 256:
            self.write(length + payload)
        else:
            self.write(length)
            self.write(payload)

    def scalar(self, value: object) -> bool:
        kind = type(value)
        if value is None:
            self.write(b"n")
        elif value is Ellipsis:
            self.write(b"e")
        elif kind is bool:
            self.write(b"t" if value else b"f")
        elif kind is int:
            integer = cast(int, value)
            if -9223372036854775808 <= integer <= 9223372036854775807:
                self.write(b"i" + struct.pack("!q", integer))
            else:
                magnitude = abs(integer)
                self.write(b"-" if integer < 0 else b"+")
                self.framed(
                    magnitude.to_bytes((magnitude.bit_length() + 7) // 8, "big")
                )
        elif kind is float:
            self.write(b"d" + struct.pack("!d", cast(float, value)))
        elif kind is complex:
            number = cast(complex, value)
            self.write(b"c" + struct.pack("!dd", number.real, number.imag))
        elif kind is str:
            self.write(b"s")
            self.framed(cast(str, value).encode("utf-8", errors="surrogatepass"))
        elif kind is bytes:
            self.write(b"b")
            self.framed(cast(bytes, value))
        else:
            return False
        return True

    def finish(self) -> bytes:
        self.hash.update(self.buffer)
        self.buffer.clear()
        return self.hash.digest()


@dataclass(slots=True)
class _AstDigestFrame:
    # Retain the owner until its descendants finish: shared siblings are values,
    # but an active-path revisit is a cycle, never object-id reuse.
    owner: object
    children: Iterator[object]
    stream: _AstDigestStream
    ast_slots: bool = False
    members: list[bytes] | None = None


@dataclass(frozen=True, slots=True)
class _AstDigestSchema:
    fields: tuple[str, ...]
    attributes: tuple[str, ...]
    header: bytes
    names: tuple[str, ...]


def _ast_schema_header(
    kind: type[ast.AST], fields: tuple[str, ...], attributes: tuple[str, ...]
) -> bytes:
    parts = [b"a"]
    for name in (kind.__module__, kind.__qualname__):
        encoded = name.encode("utf-8", errors="surrogatepass")
        parts.extend((len(encoded).to_bytes(8, "big"), encoded))
    for names in (fields, attributes):
        parts.append(len(names).to_bytes(8, "big"))
        for name in names:
            encoded = name.encode("utf-8", errors="surrogatepass")
            parts.extend((len(encoded).to_bytes(8, "big"), encoded))
    return b"".join(parts)


def python_ast_digest(tree: ast.AST) -> str:
    """Hash typed AST values and spans with stack-safe, prefix-free framing.

    The v3 domain retires per-node Merkle hashing. Only unordered members need
    independent hashes; ordinary nodes stream into one root digest. Traversal
    retains O(depth) frames, a bounded byte buffer and sorted unordered member
    digests, never a serialization of the whole tree. Identity is by value, not
    filename or object id. No mutable AST identity survives this operation.
    """
    if not isinstance(tree, ast.AST):
        raise TypeError("Python AST identity requires an AST root")
    stream = _AstDigestStream(b"molt.python-ast.v3\0")
    frames: list[_AstDigestFrame] = []
    active: set[int] = set()
    schemas: dict[type[ast.AST], _AstDigestSchema] = {}
    value: object = tree
    while True:
        kind = type(value)
        if not isinstance(value, ast.AST) and kind not in (list, tuple, frozenset):
            raise TypeError(f"unsupported Python AST identity value: {kind.__name__}")
        identity = id(value)
        if identity in active:
            raise ValueError(f"cyclic Python AST identity value: {kind.__name__}")
        active.add(identity)
        if isinstance(value, ast.AST):
            schema = schemas.get(type(value))
            if (
                schema is None
                or value._fields is not schema.fields
                or value._attributes is not schema.attributes
            ):
                fields, attributes = tuple(value._fields), tuple(value._attributes)
                schema = _AstDigestSchema(
                    fields,
                    attributes,
                    _ast_schema_header(type(value), fields, attributes),
                    (*fields, *attributes),
                )
                # Instance overrides cannot reuse the class shape, and the
                # operation-local memo stays bounded for synthetic classes.
                if len(schemas) < 256:
                    schemas[type(value)] = schema
            stream.write(schema.header)
            frames.append(
                _AstDigestFrame(value, iter(schema.names), stream, ast_slots=True)
            )
        else:
            values = cast(list[object] | tuple[object, ...] | frozenset[object], value)
            tag = b"l" if kind is list else b"q" if kind is tuple else b"u"
            stream.write(tag + len(values).to_bytes(8, "big"))
            frames.append(
                _AstDigestFrame(
                    value,
                    iter(values),
                    stream,
                    members=[] if kind is frozenset else None,
                )
            )
        while frames:
            frame = frames[-1]
            if frame.members is not None and stream is not frame.stream:
                frame.members.append(stream.finish())
                stream = frame.stream
            child = next(frame.children, _AST_END)
            if child is _AST_END:
                if frame.members is not None:
                    frame.members.sort()
                    for member in frame.members:
                        stream.write(member)
                active.remove(id(frame.owner))
                frames.pop()
                continue
            if frame.ast_slots:
                child = getattr(frame.owner, cast(str, child), _AST_MISSING)
                if child is _AST_MISSING:
                    stream.write(b"0")
                    continue
                stream.write(b"1")
            elif frame.members is not None:
                stream = _AstDigestStream(b"molt.python-ast.v3.member\0")
            if stream.scalar(child):
                continue
            value = child
            break
        else:
            return stream.finish().hex()


@dataclass(frozen=True, slots=True)
class _PythonAstDigestAdmission:
    """Digest fact for one exact, read-only scan generation.

    Retaining the tree rejects substitution; this fact does not make a mutable
    AST immutable and must not outlive the non-mutating scan that created it.
    """

    tree: ast.AST = field(repr=False, compare=False)
    digest: str = field(init=False)

    def __post_init__(self) -> None:
        object.__setattr__(self, "digest", python_ast_digest(self.tree))

    @classmethod
    def for_tree(
        cls,
        tree: ast.AST,
        admission: _PythonAstDigestAdmission | None = None,
    ) -> _PythonAstDigestAdmission:
        if admission is None:
            return cls(tree)
        if admission.tree is not tree:
            raise ValueError("AST digest admission belongs to a different tree")
        return admission


def python_node_source_key(node: ast.AST) -> PythonSourceKey:
    """Return a stable key, tolerating CPython's ``None`` synthetic end spans."""
    lineno = getattr(node, "lineno", None)
    col_offset = getattr(node, "col_offset", None)
    start_line = int(lineno) if lineno is not None else 0
    start_column = int(col_offset) if col_offset is not None else 0
    end_lineno = getattr(node, "end_lineno", None)
    end_col_offset = getattr(node, "end_col_offset", None)
    return (
        start_line,
        start_column,
        int(end_lineno) if end_lineno is not None else start_line,
        int(end_col_offset) if end_col_offset is not None else start_column,
        type(node).__name__,
    )


def python_pattern_capture_names(pattern: ast.pattern) -> tuple[str, ...]:
    """Return one pattern's captures in deterministic source order."""

    names: list[str] = []
    seen: set[str] = set()

    def add(name: str | None) -> None:
        if name and name != "_" and name not in seen:
            seen.add(name)
            names.append(name)

    def visit(current: ast.pattern) -> None:
        if isinstance(current, ast.MatchAs):
            if current.pattern is not None:
                visit(current.pattern)
            add(current.name)
        elif isinstance(current, ast.MatchStar):
            add(current.name)
        elif isinstance(current, ast.MatchMapping):
            for child in current.patterns:
                visit(child)
            add(current.rest)
        elif isinstance(current, ast.MatchSequence):
            for child in current.patterns:
                visit(child)
        elif isinstance(current, ast.MatchClass):
            for child in (*current.patterns, *current.kwd_patterns):
                visit(child)
        elif isinstance(current, ast.MatchOr):
            for child in current.patterns:
                visit(child)

    visit(pattern)
    return tuple(names)


def python_pattern_irrefutable_reason(
    pattern: ast.pattern,
) -> tuple[str, str | None] | None:
    if isinstance(pattern, ast.MatchAs):
        if pattern.pattern is None:
            if pattern.name is None:
                return ("wildcard", None)
            return ("capture", pattern.name)
        inner = python_pattern_irrefutable_reason(pattern.pattern)
        if inner is None:
            return None
        if inner[0] == "wildcard":
            return ("wildcard", None)
        return inner
    if isinstance(pattern, ast.MatchOr):
        for sub in pattern.patterns:
            reason = python_pattern_irrefutable_reason(sub)
            if reason is not None:
                return reason
    return None


def python_pattern_is_capture_only(pattern: ast.pattern) -> bool:
    """Whether matching only binds names, without a value/protocol test."""
    while isinstance(pattern, ast.MatchAs):
        if pattern.pattern is None:
            return True
        pattern = pattern.pattern
    return False


__all__ = [
    "PythonSourceKey",
    "python_ast_digest",
    "python_source_digest",
    "python_node_source_key",
    "python_pattern_capture_names",
    "python_pattern_irrefutable_reason",
    "python_pattern_is_capture_only",
]
