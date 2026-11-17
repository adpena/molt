"""Who can change what ``import M`` yields: one program fact per registry row.

Molt compiles the whole program, so whether code can make ``import M`` yield
something other than the registry's ``M`` is a program fact (bedrock design
§13, D1). The namespace graph (``python_namespace_graph``) produces one
``ImportRedirects`` per program from the activated ``import_binding``,
``import_hooks`` and ``guarded_import`` rows of ``python_namespace_writes``.
Its readers are the closed-world receiver proof (F3), static module cells (C4)
and the canonical-import identity of the binding flow (M7, through the
premise's ``noncanonical_imports``). No member mask or spelling decides it.

A row is ``Fixed`` when ``closed_writers`` returns no writer: ``redirect``
names none, and no spec writer can change the answer of the import protocol's
``__spec__._initializing`` probe or ``_handle_fromlist`` for the row (D2b): a
store of the row's ``__spec__``, ``__class__`` or ``__path__`` (named, or a
computed name on the row itself), a reflection of its namespace, or a store of
any ``_initializing``.
Only a Fixed row's ready word is set, and only a site whose rows are all Fixed
skips the protocol (IC3). A write of
``sys.modules[K]`` redirects ``K`` and every ``K.*`` row: importing ``K.x``
first acquires ``K``. A computed key carries its bound (prefix, suffix,
length), and the bound redirects every row it admits. Hook mutation, a
``builtins.__import__`` write, an unbounded key, an escape of an import-hook
object and a native extension module redirect every row.

A ``guarded_import`` is an import under a handler that can swallow its
failure (an ``except`` clause or a ``with`` statement). A swallowed body
failure re-runs the body on the next import, so its module and everything the
import runs first lose the facts that assume one execution (static cells, C4).
"""

from __future__ import annotations

import ast
from collections.abc import Collection, Iterable
from dataclasses import dataclass

from molt.compiler_analysis.python_identity_catalog_generated import (
    CANONICAL_IMPORT_IDENTITIES,
)


@dataclass(frozen=True, slots=True, order=True)
class PythonImportKeyBound:
    """What a computed module name may spell: it starts with ``prefix``, ends
    with ``suffix`` and has at least ``minimum`` characters. A key with no
    literal part has no bound; its row is a global writer instead."""

    prefix: str
    suffix: str
    minimum: int

    def admits(self, name: str) -> bool:
        return (
            len(name) >= self.minimum
            and name.startswith(self.prefix)
            and name.endswith(self.suffix)
        )

    def payload(self) -> list[object]:
        return [self.prefix, self.suffix, self.minimum]


def decode_python_import_key_bound(raw: object) -> PythonImportKeyBound:
    if (
        not isinstance(raw, list)
        or len(raw) != 3
        or type(raw[0]) is not str
        or type(raw[1]) is not str
        or type(raw[2]) is not int
        or raw[2] < max(len(raw[0]), len(raw[1]))
        or not (raw[0] or raw[1])
    ):
        raise ValueError("invalid import key bound")
    return PythonImportKeyBound(raw[0], raw[1], raw[2])


def _string_parts(node: ast.expr) -> list[str | None] | None:
    """The literal and computed parts of a string expression, in order, or
    None when it is not a concatenation of string parts."""
    if isinstance(node, ast.Constant):
        return [node.value] if type(node.value) is str else None
    if isinstance(node, ast.JoinedStr):
        parts: list[str | None] = []
        for part in node.values:
            if isinstance(part, ast.Constant) and type(part.value) is str:
                parts.append(part.value)
            elif isinstance(part, ast.FormattedValue):
                parts.append(None)
            else:
                return None
        return parts
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        left = _string_parts(node.left)
        right = _string_parts(node.right)
        if left is None and right is None:
            return None
        # A non-literal operand is a computed part: ``"encodings." + name``.
        return (left if left is not None else [None]) + (
            right if right is not None else [None]
        )
    return None


def python_import_key(node: ast.expr | str | None) -> str | PythonImportKeyBound | None:
    """The module name an expression spells without running it: the exact
    name, a bound on it, or None when nothing bounds it.

    Requires: nothing; any expression is accepted.
    Ensures: an exact result equals every value the expression can have; a
    bound admits every value; None makes no claim.
    """
    if isinstance(node, str):
        return node
    if node is None:
        return None
    parts = _string_parts(node)
    if parts is None:
        return None
    if all(part is not None for part in parts):
        return "".join(part for part in parts if part is not None)
    prefix = ""
    for part in parts:
        if part is None:
            break
        prefix += part
    suffix = ""
    for part in reversed(parts):
        if part is None:
            break
        suffix = part + suffix
    if not (prefix or suffix):
        return None
    minimum = sum(len(part) for part in parts if part is not None)
    return PythonImportKeyBound(prefix, suffix, minimum)


@dataclass(frozen=True, slots=True, order=True)
class ImportWriter:
    """One site that may change what an import yields: ``site`` is
    ``module:line`` and ``construct`` says what it does. Diagnostics print
    it; no consumer parses it."""

    site: str
    construct: str

    def __str__(self) -> str:
        return f"{self.construct} at {self.site}"


def _ancestors(module: str) -> Iterable[str]:
    """``module`` and each dotted ancestor: ``a.b.c``, ``a.b``, ``a``."""
    name = module
    while True:
        yield name
        head, dot, _tail = name.rpartition(".")
        if not dot:
            return
        name = head


def _first(writers: Iterable[ImportWriter]) -> tuple[ImportWriter, ...]:
    return tuple(sorted(set(writers)))


@dataclass(frozen=True, slots=True)
class ImportRedirects:
    """The program's one answer to "can ``import M`` yield something other
    than the registry's ``M``?" (bedrock §13, IC1).

    ``global_writers`` redirect every row. ``key_writers`` pairs an exact
    ``sys.modules`` key with its writer; ``bound_writers`` pairs a computed
    key's bound with its writer. ``guarded`` pairs a guarded import's module
    name (or bound, or None for an unbounded name) with its handler site.
    Every tuple is sorted, so equal facts compare equal.
    """

    global_writers: tuple[ImportWriter, ...] = ()
    key_writers: tuple[tuple[str, ImportWriter], ...] = ()
    bound_writers: tuple[tuple[PythonImportKeyBound, ImportWriter], ...] = ()
    guarded: tuple[tuple[str | PythonImportKeyBound | None, ImportWriter], ...] = ()
    # D2b: a spec writer of one module (its name), or of every module (None).
    spec_writers: tuple[tuple[str | None, ImportWriter], ...] = ()
    # D2b: named stores of a spec attribute on a receiver that is not
    # syntactically a module; each writes the modules whose values escaped
    # (``escaped``; None: some value escaped unbounded).
    spec_member_stores: tuple[ImportWriter, ...] = ()
    escaped: tuple[str, ...] | None = ()

    def redirect(self, module: str) -> tuple[ImportWriter, ...]:
        """The writers of ``module``'s import result; empty means Fixed.

        Requires: ``module`` is a canonical dotted module name.
        Ensures: the result holds every global writer, every writer of a key
        equal to ``module`` or one of its dotted ancestors, and every bound
        that admits one of them; it is empty only when all three are.
        """
        names = tuple(_ancestors(module))
        found = list(self.global_writers)
        found.extend(writer for key, writer in self.key_writers if key in names)
        found.extend(
            writer
            for bound, writer in self.bound_writers
            if any(bound.admits(name) for name in names)
        )
        return _first(found)

    def closed_writers(self, module: str) -> tuple[ImportWriter, ...]:
        """The writers that keep ``module`` from the closed lowering; empty
        means the row is Fixed (``MODULE_FLAG_FIXED``, bedrock §13 D2b).

        Ensures: the result holds ``redirect(module)``, every spec writer of
        ``module`` or of every module, and every spec member store when
        ``module``'s value may have escaped to it.
        """
        found = list(self.redirect(module))
        found.extend(
            writer for key, writer in self.spec_writers if key in (None, module)
        )
        if self.escaped is None or module in self.escaped:
            found.extend(self.spec_member_stores)
        return _first(found)

    def swallowers(
        self, module: str, importers: Collection[str]
    ) -> tuple[ImportWriter, ...]:
        """Handlers that can swallow a failure of ``module``'s body (M11).

        ``importers`` are the modules whose import runs ``module``'s body
        first (its transitive importers). A guarded import of ``module``, of
        a dotted descendant of it, or of an importer swallows its failure.

        Requires: ``importers`` holds every module whose dependency closure
        contains ``module``.
        Ensures: the result is empty only when no guarded import can run
        ``module``'s body.
        """
        targets = {module, *importers}
        found: list[ImportWriter] = []
        for key, writer in self.guarded:
            if key is None:
                found.append(writer)
            elif isinstance(key, PythonImportKeyBound):
                if any(key.admits(name) for name in targets) or any(
                    key.prefix.startswith(name + ".") for name in targets
                ):
                    found.append(writer)
            elif any(name in targets for name in _ancestors(key)):
                found.append(writer)
        return _first(found)

    def canonical_redirected(self) -> tuple[str, ...]:
        """The canonical providers (``CANONICAL_IMPORT_IDENTITIES``) some
        writer redirects; the binding flow treats them as noncanonical (M7)."""
        return tuple(
            sorted(name for name in CANONICAL_IMPORT_IDENTITIES if self.redirect(name))
        )

    def join(self, other: ImportRedirects) -> ImportRedirects:
        if other == self or other == NO_IMPORT_REDIRECTS:
            return self
        if self == NO_IMPORT_REDIRECTS:
            return other
        return import_redirects(
            (*self.global_writers, *other.global_writers),
            (*self.key_writers, *other.key_writers),
            (*self.bound_writers, *other.bound_writers),
            (*self.guarded, *other.guarded),
            (*self.spec_writers, *other.spec_writers),
            (*self.spec_member_stores, *other.spec_member_stores),
            None
            if self.escaped is None or other.escaped is None
            else (*self.escaped, *other.escaped),
        )

    def with_global(self, writer: ImportWriter) -> ImportRedirects:
        return self.join(import_redirects((writer,)))


def _guard_order(key: str | PythonImportKeyBound | None) -> tuple[int, str, str, int]:
    if key is None:
        return (0, "", "", 0)
    if isinstance(key, str):
        return (1, key, "", 0)
    return (2, key.prefix, key.suffix, key.minimum)


def import_redirects(
    global_writers: Iterable[ImportWriter] = (),
    key_writers: Iterable[tuple[str, ImportWriter]] = (),
    bound_writers: Iterable[tuple[PythonImportKeyBound, ImportWriter]] = (),
    guarded: Iterable[tuple[str | PythonImportKeyBound | None, ImportWriter]] = (),
    spec_writers: Iterable[tuple[str | None, ImportWriter]] = (),
    spec_member_stores: Iterable[ImportWriter] = (),
    escaped: Iterable[str] | None = (),
) -> ImportRedirects:
    """The one constructor that sorts every part, so equal facts compare equal."""
    return ImportRedirects(
        _first(global_writers),
        tuple(sorted(set(key_writers))),
        tuple(sorted(set(bound_writers))),
        tuple(sorted(set(guarded), key=lambda row: (_guard_order(row[0]), row[1]))),
        tuple(sorted(set(spec_writers), key=lambda row: (row[0] or "", row[1]))),
        _first(spec_member_stores),
        None if escaped is None else tuple(sorted(set(escaped))),
    )


NO_IMPORT_REDIRECTS = ImportRedirects()
# No verified graph: every row may be redirected.
UNKNOWN_IMPORT_REDIRECTS = ImportRedirects(
    global_writers=(ImportWriter("<no closed graph>", "an unverified program"),),
    guarded=((None, ImportWriter("<no closed graph>", "an unverified program")),),
    spec_writers=((None, ImportWriter("<no closed graph>", "an unverified program")),),
    escaped=None,
)
# The module attributes the protocol reads when ``import M`` finds M done:
# ``__spec__`` (and its ``_initializing``), the module's class (which owns
# attribute lookup) and ``__path__`` (``_handle_fromlist``).
IMPORT_SPEC_ATTRIBUTES = frozenset({"__spec__", "__class__", "__path__"})
# A native extension module's C code can write the module dictionary
# (``PyImport_GetModuleDict``) and swallow any import failure (B2c).
NATIVE_EXTENSION_WRITER = ImportWriter(
    "<native extension>", "a native extension module"
)
