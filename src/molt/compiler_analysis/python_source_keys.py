"""Canonical stable source identity for parsed and synthetic Python AST nodes."""

from __future__ import annotations

import ast
from molt.python_private_names import python_source_fields
import hashlib
import struct
from collections import deque
from collections.abc import Iterator
from dataclasses import dataclass, field
from enum import StrEnum
from threading import Lock
from typing import TYPE_CHECKING, TypeAlias, cast
from types import MappingProxyType
from collections.abc import Mapping

if TYPE_CHECKING:
    from molt.compiler_analysis.python_binding_facts import PythonOpaqueActivationReads
    from molt.compiler_analysis.python_call_arguments import CallArgumentPlan
    from molt.compiler_analysis.python_lexical_scope import PythonModuleLexicalPlan
    from molt.compiler_analysis.python_imports import SourceAllExports
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.compiler_analysis.python_namespace_writes import PythonNamespaceSummary
    from molt.target_python import TargetPythonVersion


PythonSourceKey: TypeAlias = tuple[int, int, int, int, str]


@dataclass(frozen=True, eq=False, slots=True)
class PythonSourceLocations:
    """Project source positions to their ordered AST line slots."""

    slots: Mapping[int, int] | None = None

    def line(self, line: int) -> int:
        if self.slots is None or line == 0:
            return line
        try:
            return self.slots[line]
        except KeyError as error:
            raise ValueError(f"source line outside the recorded AST: {line}") from error

    def key(self, source: PythonSourceKey) -> PythonSourceKey:
        if self.slots is None:
            return source
        line, column, end_line, end_column, kind = source
        return self.line(line), column, self.line(end_line), end_column, kind

    def optional(self, source: PythonSourceKey | None) -> PythonSourceKey | None:
        return None if source is None else self.key(source)

    def ordered(self, sources: frozenset[PythonSourceKey]) -> list[PythonSourceKey]:
        ordered = sorted(sources)
        return (
            ordered if self.slots is None else [self.key(source) for source in ordered]
        )


def python_source_contains(outer: PythonSourceKey, inner: PythonSourceKey) -> bool:
    return outer[:2] <= inner[:2] and inner[2:4] <= outer[2:4]


def decode_python_source_key(value: object) -> PythonSourceKey:
    if not isinstance(value, (list, tuple)) or len(value) != 5:
        raise ValueError("invalid source-owner identity")
    line, column, end_line, end_column, kind = value
    if (
        type(line) is not int
        or type(column) is not int
        or type(end_line) is not int
        or type(end_column) is not int
        or not isinstance(kind, str)
    ):
        raise ValueError("invalid source-owner identity")
    if (
        line < 0
        or column < 0
        or end_line < line
        or end_column < 0
        or (end_line == line and end_column < column)
        or not kind
    ):
        raise ValueError("invalid source-owner span")
    return (line, column, end_line, end_column, kind)


class PythonElementProtocol(StrEnum):
    CLASS_METHOD = "class_method"
    STATIC_METHOD = "static_method"
    # `property(fget)`: an instance attribute load calls `fget` with the
    # instance; a class attribute load returns the property itself.
    PROPERTY = "property"
    SUBSCRIPT = "subscript"
    REFLECTED_BUILTINS = "reflected_builtins"
    REFLECTED_GLOBALS = "reflected_globals"
    MAPPING_WRITE = "mapping_write"
    ENSURE_BUILTINS = "ensure_builtins"
    # Zero-argument `super()` as an attribute receiver: the origins are the
    # enclosing function's first argument. An attribute of it selects from
    # the strict ancestors of the argument's class and binds the argument.
    SUPER = "super"
    ITERATE = "iterate"
    ASYNC_ITERATE = "async_iterate"


class PythonSourceProjection(StrEnum):
    YIELD_FROM_ELEMENTS = "YieldFrom.elements"
    CALL_ATTRIBUTE = "Call.attribute"
    CALL_NATIVE_INITIALIZER = "Call.native_initializer"
    CALL_MAPPING_VALUE = "Call.mapping_value"
    CALL_NATIVE_BUILTINS = "Call.native_builtins"
    DECORATOR_APPLY = "Decorator.apply"
    CALL_REFLECTED_BUILTINS = "Call.reflected_builtins"
    ARGUMENT_REFLECTED_GLOBALS = "Argument.reflected_globals"
    MAPPING_WRITE = "Mapping.write"


def guarded_declaration_origin(
    origins: frozenset[PythonSourceKey],
) -> PythonSourceKey | None:
    """Find one declaration to try against the live callable's runtime guard.

    Decorator results are additional value-flow origins. They can change the
    callable without erasing the guarded code candidate from its declaration.
    """
    declaration = None
    for origin in origins:
        if origin[-1] == PythonSourceProjection.DECORATOR_APPLY:
            continue
        if declaration is not None:
            return None
        declaration = origin
    return declaration


def python_source_projection_key(
    node: ast.expr, projection: PythonSourceProjection
) -> PythonSourceKey:
    """Keep a projected value distinct from its expression's completion."""
    source = python_node_source_key(node)
    return (*source[:4], projection.value)


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

    def framed(self, prefix: bytes, payload: bytes) -> None:
        header = prefix + len(payload).to_bytes(8, "big")
        if len(payload) < 256:
            self.write(header + payload)
        else:
            self.write(header)
            self.write(payload)

    def scalar(self, value: object, prefix: bytes) -> bool:
        kind = type(value)
        if value is None:
            self.write(prefix + b"n")
        elif value is Ellipsis:
            self.write(prefix + b"e")
        elif kind is bool:
            self.write(prefix + (b"t" if value else b"f"))
        elif kind is int:
            integer = cast(int, value)
            if -9223372036854775808 <= integer <= 9223372036854775807:
                self.write(prefix + b"i" + struct.pack("!q", integer))
            else:
                magnitude = abs(integer)
                self.framed(
                    prefix + (b"-" if integer < 0 else b"+"),
                    magnitude.to_bytes((magnitude.bit_length() + 7) // 8, "big"),
                )
        elif kind is float:
            self.write(prefix + b"d" + struct.pack("!d", cast(float, value)))
        elif kind is complex:
            number = cast(complex, value)
            self.write(prefix + b"c" + struct.pack("!dd", number.real, number.imag))
        elif kind is str:
            self.framed(
                prefix + b"s", cast(str, value).encode("utf-8", errors="surrogatepass")
            )
        elif kind is bytes:
            self.framed(prefix + b"b", cast(bytes, value))
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


_AST_LINE_FIELDS = frozenset(("lineno", "end_lineno"))


@dataclass(frozen=True, slots=True)
class PythonAstLineIdentity:
    """AST identity in which source lines are their ordered occupied slots."""

    digest: str
    lines: tuple[int, ...]

    def locations(self) -> PythonSourceLocations:
        return PythonSourceLocations(
            {line: slot for slot, line in enumerate(self.lines, start=1)}
        )


def python_ast_line_identity(tree: ast.AST) -> PythonAstLineIdentity:
    # Keep columns, line sharing, ordering, and every value-bearing AST field.
    # In particular a moved expression can share a slot only if every node's
    # start/end line again refers to that same relative slot.
    lines = tuple(
        sorted(
            {
                line
                for node in ast.walk(tree)
                for field in _AST_LINE_FIELDS
                if type(line := getattr(node, field, None)) is int and line > 0
            }
        )
    )
    slots = {line: index for index, line in enumerate(lines, start=1)}
    return PythonAstLineIdentity(python_ast_digest(tree, line_slots=slots), lines)


def python_ast_digest(
    tree: ast.AST, *, line_slots: Mapping[int, int] | None = None
) -> str:
    """Hash typed AST values and spans with stack-safe, prefix-free framing.

    The v3 domain retires per-node Merkle hashing. Only unordered members need
    independent hashes; ordinary nodes stream into one root digest. Traversal
    retains O(depth) frames, a bounded byte buffer and sorted unordered member
    digests, never a serialization of the whole tree. Identity is by value, not
    filename or object id. No mutable AST identity survives this operation.
    """
    if not isinstance(tree, ast.AST):
        raise TypeError("Python AST identity requires an AST root")
    stream = _AstDigestStream(
        b"molt.python-ast.v3\0"
        if line_slots is None
        else b"molt.python-ast.occupied-lines.v1\0"
    )
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
            children = python_source_fields(value, schema.names, _AST_MISSING)
            if line_slots is not None:
                children = (
                    line_slots[child]
                    if name in _AST_LINE_FIELDS and type(child) is int and child > 0
                    else child
                    for name, child in zip(schema.names, children, strict=True)
                )
            frames.append(
                _AstDigestFrame(
                    value,
                    children,
                    stream,
                    ast_slots=True,
                )
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
                if child is _AST_MISSING:
                    stream.write(b"0")
                    continue
                prefix = b"1"
            else:
                prefix = b""
                if frame.members is not None:
                    stream = _AstDigestStream(b"molt.python-ast.v3.member\0")
            if stream.scalar(child, prefix):
                continue
            if prefix:
                stream.write(prefix)
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
    deferred_nodes: Mapping[PythonSourceKey, ast.AST] = field(
        init=False, repr=False, compare=False
    )
    _deferred_binding_parents: Mapping[PythonSourceKey, PythonSourceKey | None] = field(
        init=False, repr=False, compare=False
    )

    module_setup_nodes: tuple[ast.stmt, ...] = field(
        init=False, repr=False, compare=False
    )

    module_all_exports: dict[
        tuple[TargetPythonVersion, str | None, PythonNamespacePremise | None],
        tuple[SourceAllExports, PythonOpaqueActivationReads | None],
    ] = field(default_factory=dict, init=False, repr=False, compare=False)

    namespace_summaries: dict[
        tuple[tuple[int, int], str | None], PythonNamespaceSummary
    ] = field(default_factory=dict, init=False, repr=False, compare=False)

    _lexical_plans: dict[tuple[int, int], PythonModuleLexicalPlan] = field(
        default_factory=dict, init=False, repr=False, compare=False
    )
    _argument_plans: dict[ast.Call | ast.ClassDef, CallArgumentPlan] = field(
        default_factory=dict, init=False, repr=False, compare=False
    )
    _name_queries: dict[frozenset[str], bool] = field(
        default_factory=dict, init=False, repr=False, compare=False
    )
    _syntax_plan_lock: Lock = field(
        default_factory=Lock, init=False, repr=False, compare=False
    )
    _source_keys: Mapping[ast.AST, PythonSourceKey] = field(
        init=False, repr=False, compare=False
    )
    _canonical: bool = field(default=False, init=False, repr=False, compare=False)

    def lexical_plan(
        self, tree: ast.Module, *, target_python: tuple[int, int]
    ) -> PythonModuleLexicalPlan:
        from molt.compiler_analysis.python_lexical_scope import PythonModuleLexicalPlan

        self.canonical_tree(tree)
        with self._syntax_plan_lock:
            plan = self._lexical_plans.get(target_python)
            if plan is None:
                plan = PythonModuleLexicalPlan.build(tree, target_python=target_python)
                self._lexical_plans[target_python] = plan
            return plan

    def call_argument_plan(self, node: ast.Call | ast.ClassDef) -> CallArgumentPlan:
        with self._syntax_plan_lock:
            plan = self._argument_plans.get(node)
            if plan is None:
                from molt.compiler_analysis.python_call_arguments import (
                    call_argument_plan,
                )

                plan = call_argument_plan(node)
                self._argument_plans[node] = plan
            return plan

    def contains_reference_name(self, names: frozenset[str]) -> bool:
        """A cheap presence check over parsed, normalized reference spellings."""
        with self._syntax_plan_lock:
            result = self._name_queries.get(names)
            if result is None:
                result = any(
                    (isinstance(node, ast.Name) and node.id.rpartition(".")[2] in names)
                    or (isinstance(node, ast.Attribute) and node.attr in names)
                    or (
                        isinstance(node, ast.alias)
                        and node.name.rpartition(".")[2] in names
                    )
                    for node in self._source_keys
                )
                self._name_queries[names] = result
            return result

    def canonical_tree(self, tree: ast.Module) -> ast.Module:
        if self.tree is not tree:
            raise ValueError("AST digest admission belongs to a different tree")
        if not self._canonical:
            from molt.python_canonical_ast import canonical_python_module

            canonical_python_module(tree)
            object.__setattr__(self, "_canonical", True)
        return tree

    def __post_init__(self) -> None:
        if isinstance(self.tree, ast.Module):
            from molt.python_canonical_ast import canonical_module_digest

            object.__setattr__(self, "digest", canonical_module_digest(self.tree))
            object.__setattr__(self, "_canonical", True)
        else:
            object.__setattr__(self, "digest", python_ast_digest(self.tree))
        object.__setattr__(
            self,
            "module_setup_nodes",
            tuple(
                node
                for node in getattr(self.tree, "body", ())
                if isinstance(
                    node, (ast.Import, ast.ImportFrom, ast.Assign, ast.AnnAssign)
                )
            ),
        )
        deferred_kinds = (
            ast.FunctionDef,
            ast.AsyncFunctionDef,
            ast.ClassDef,
            ast.Lambda,
            ast.GeneratorExp,
            *([ast.TypeAlias] if hasattr(ast, "TypeAlias") else []),
        )
        coordinates: dict[ast.AST, PythonSourceKey] = {}
        deferred: dict[PythonSourceKey, ast.AST] = {}
        deferred_binding_parents: dict[PythonSourceKey, PythonSourceKey | None] = {}
        pending: deque[tuple[ast.AST, PythonSourceKey | None]] = deque(
            [(self.tree, None)]
        )
        while pending:
            node, parent = pending.popleft()
            if node in coordinates:
                continue
            key = coordinates[node] = python_node_source_key(node)
            if isinstance(node, deferred_kinds):
                deferred[key] = node
                deferred_binding_parents[key] = parent
                if isinstance(
                    node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)
                ):
                    parent = key
            pending.extend((child, parent) for child in ast.iter_child_nodes(node))
        object.__setattr__(self, "_source_keys", MappingProxyType(coordinates))
        object.__setattr__(self, "deferred_nodes", MappingProxyType(deferred))
        object.__setattr__(
            self,
            "_deferred_binding_parents",
            MappingProxyType(deferred_binding_parents),
        )

    def deferred_binding_path(
        self, owner: PythonSourceKey
    ) -> frozenset[PythonSourceKey]:
        """The function jobs that can establish the selected body's entry."""
        parents = self._deferred_binding_parents
        if owner not in parents:
            raise ValueError(f"deferred demand lost source owner: {owner!r}")
        path = {owner}
        parent = parents[owner]
        while parent is not None:
            path.add(parent)
            parent = parents[parent]
        return frozenset(path)

    def source_key(self, node: ast.AST) -> PythonSourceKey:
        """Read admitted coordinates, or compute them for an analysis-local node."""
        key = self._source_keys.get(node)
        return python_node_source_key(node) if key is None else key

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
    """Return a stable key, including source-located semantic AST wrappers."""
    if isinstance(node, ast.comprehension):
        # CPython gives the clause no positions. Its target and final executed
        # operand delimit a unique span even across nested clauses or reparses.
        # Preserve the wrapper kind so it cannot alias its target expression.
        start = python_node_source_key(node.target)
        end = python_node_source_key(node.ifs[-1] if node.ifs else node.iter)
        return (*start[:2], *end[2:4], type(node).__name__)
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
    "PythonSourceProjection",
    "decode_python_source_key",
    "python_source_projection_key",
    "python_ast_digest",
    "python_source_digest",
    "python_node_source_key",
    "python_pattern_capture_names",
    "python_pattern_irrefutable_reason",
    "python_pattern_is_capture_only",
]
