"""Shared namespace premises for closed Python source graphs (HF-226).

The syntax authority is ``python_namespace_writes``. This module projects those
summaries onto a caller-owned module topology; it never discovers modules or
reads source paths. Tooling discovery and fixed source inventories share this
aggregation, while each graph owner supplies package-child bindings and decides
when its premise has converged.
"""

from __future__ import annotations

from collections.abc import Callable, Collection, Iterable, Mapping
from dataclasses import dataclass, field, replace
from pathlib import Path
from types import MappingProxyType

from molt.compiler_analysis.python_identity_catalog_generated import (
    MemberMask,
    NO_INVALID_MEMBERS,
    ALL_INVALID_MEMBERS,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingFlowPolicy,
    python_canonical_member_writes,
)
from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite
from molt.compiler_analysis.python_source_keys import PythonSourceKey
from molt.compiler_analysis.python_value_identity import (
    canonical_import_binding_write_mask,
)


@dataclass(frozen=True, slots=True)
class PythonNamespacePremise:
    """One module's namespace premise, verified over a closed graph (HF-226).

    ``foreign_writes`` lists, sorted, the names graph code outside the module
    writes into its namespace: named writes and submodule bindings; ``None``
    when a written name is computed. ``member_writes`` lists the canonical
    members code anywhere in the graph writes. The binding flow keeps every
    other global and member fact across callbacks. ``namespace_reflected``
    holds when activated graph code may reach the module's namespace mapping
    by identity (a ``reflection`` row naming it or any module), so the mapping
    must hold every global. ``code_rebinding`` holds when activated graph code
    reads a code object (a ``code`` row) or a native extension can: then
    ``FunctionType`` may run any function over another globals mapping, so no
    function's global read is its module's. ``escape_writes`` pairs, sorted,
    each name some code stores as an attribute while the module's value has
    escaped (call_anatomy.md 4.5a) with its first writer site; those names are
    also in ``foreign_writes``.
    """

    foreign_writes: tuple[str, ...] | None
    member_writes: MemberMask
    entry_member_writes: MemberMask
    noncanonical_imports: tuple[str, ...]
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None
    namespace_reflected: bool = False
    code_rebinding: bool = False
    escape_writes: tuple[tuple[str, str], ...] = ()

    def escape_writer(self, name: str) -> str | None:
        """The first site that may write ``name`` through an escaped value."""
        return dict(self.escape_writes).get(name)


def python_namespace_binding_policy[P: PythonBindingFlowPolicy](
    policy: P, premise: PythonNamespacePremise | None, *, lowering: bool = False
) -> P:
    """Apply a closed graph's initial and future uncertainty together.

    A reached writer may already have run before the first acquisition. Its
    entry mask must travel with callback writes through every graph consumer.
    Standalone callers keep their explicit policy when no graph was proved.

    Exposure lets unknown code activate a definition; only a code rebinder
    (``code_rebinding``) can make that activation read another globals
    mapping. Lowering therefore sees the exposed set only when the graph holds
    a rebinder, so a method's global and builtin reads keep their proofs.
    Graph discovery keeps the exposed set: its demand worklist activates the
    bodies a value with a known origin can reach, and a precise origin for a
    whole module's escape would activate every function of that module (the
    runtime closure's escape of ``types`` pulls in 17 modules).
    """
    if premise is None:
        return policy
    opaque = premise.opaque_activation_sources
    if lowering and opaque is not None and not premise.code_rebinding:
        opaque = ()
    return replace(
        policy,
        foreign_namespace_writes=premise.foreign_writes,
        callback_member_writes=premise.member_writes,
        entry_invalid_members=premise.entry_member_writes,
        noncanonical_imports=premise.noncanonical_imports,
        opaque_activation_sources=opaque,
    )


@dataclass(frozen=True, slots=True)
class PythonGraphNamespaceWrites:
    """Named namespace writes found in one walk of a closed graph."""

    # Target source -> names written into it; None when a name is computed.
    by_source: Mapping[Path, frozenset[str] | None]
    # Names written into a namespace reached by a computed ``sys.modules`` key.
    any_module: frozenset[str] | None
    members: MemberMask
    entry_members: MemberMask
    # Sources whose namespace mapping a reflection row reaches; None when one
    # reaches any module's (a computed key, ``__globals__``, ``f_globals``).
    reflected: frozenset[Path] | None = frozenset()
    # Whether some activated code reads a code object (a ``code`` row).
    code_rebinding: bool = False
    # Named attribute stores on receivers that are not module references,
    # each with its first writer site (``member_store`` rows).
    member_stores: Mapping[str, str] = field(default_factory=dict)
    # Sources whose module value escapes; None when a computed key's does.
    escaped: frozenset[Path] | None = frozenset()

    def premise(
        self,
        source: Path,
        children: frozenset[str],
        *,
        noncanonical_imports: tuple[str, ...] = (),
        opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None,
        native_code: bool = False,
    ) -> PythonNamespacePremise:
        return _namespace_premise(
            self.by_source.get(source, frozenset()),
            self.any_module,
            self.members,
            self.entry_members,
            children,
            noncanonical_imports,
            opaque_activation_sources,
            self.reflected is None or source in self.reflected,
            self.code_rebinding or native_code,
            _escape_writes(source, self.escaped, self.member_stores),
        )


def _escape_writes(
    source: Path, escaped: Collection[Path] | None, stores: Mapping[str, str]
) -> tuple[tuple[str, str], ...]:
    if escaped is not None and source not in escaped:
        return ()
    return tuple(sorted(stores.items()))


NO_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}), frozenset(), NO_INVALID_MEMBERS, NO_INVALID_MEMBERS
)
UNKNOWN_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}), None, ALL_INVALID_MEMBERS, ALL_INVALID_MEMBERS, None, True
)


def _namespace_premise(
    named: frozenset[str] | set[str] | None,
    any_module: frozenset[str] | set[str] | None,
    members: MemberMask,
    entry_members: MemberMask,
    children: frozenset[str],
    noncanonical_imports: tuple[str, ...],
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None,
    reflected: bool = False,
    code_rebinding: bool = False,
    escape_writes: tuple[tuple[str, str], ...] = (),
) -> PythonNamespacePremise:
    return PythonNamespacePremise(
        None
        if named is None or any_module is None
        else tuple(
            sorted(children | named | any_module | {name for name, _ in escape_writes})
        ),
        members,
        entry_members,
        noncanonical_imports,
        opaque_activation_sources,
        reflected,
        code_rebinding,
        escape_writes,
    )


def join_python_namespace_premises(
    left: PythonNamespacePremise | None, right: PythonNamespacePremise | None
) -> PythonNamespacePremise | None:
    """Join scan authorities; an unverified namespace cannot grant precision."""
    if left is None or right is None:
        return None
    names = (
        None
        if left.foreign_writes is None or right.foreign_writes is None
        else tuple(sorted(set(left.foreign_writes) | set(right.foreign_writes)))
    )
    return PythonNamespacePremise(
        names,
        left.member_writes | right.member_writes,
        left.entry_member_writes | right.entry_member_writes,
        tuple(sorted(set(left.noncanonical_imports) | set(right.noncanonical_imports))),
        None
        if left.opaque_activation_sources is None
        or right.opaque_activation_sources is None
        else tuple(
            sorted(
                set(left.opaque_activation_sources)
                | set(right.opaque_activation_sources)
            )
        ),
        left.namespace_reflected or right.namespace_reflected,
        left.code_rebinding or right.code_rebinding,
        tuple(
            sorted(
                {
                    **dict(right.escape_writes),
                    **dict(left.escape_writes),
                }.items()
            )
        ),
    )


class PythonGraphNamespaceIndex:
    """One mutable writer index for an operation's monotone discovery queue.

    Merging a source visits only that source's writes. A global delta is
    reported separately so graph owners can wake affected scans without copying
    or visiting every previously indexed target on each module admission.
    """

    def __init__(self) -> None:
        self.by_source: dict[Path, set[str] | None] = {}
        self.any_module: set[str] | None = set()
        self.members = NO_INVALID_MEMBERS
        self.entry_members = NO_INVALID_MEMBERS
        self.reflected: set[Path] | None = set()
        self.code_rebinding = False
        self.member_stores: dict[str, str] = {}
        self.escaped: set[Path] | None = set()

    def merge(self, batch: PythonGraphNamespaceWrites) -> frozenset[Path] | None:
        changed: set[Path] = set()
        new_stores = {
            name: site
            for name, site in batch.member_stores.items()
            if name not in self.member_stores
        }
        self.member_stores.update(new_stores)
        escape_global = False
        if self.escaped is not None:
            if batch.escaped is None:
                self.escaped = None
                escape_global = bool(self.member_stores)
            else:
                added_sources = batch.escaped - self.escaped
                self.escaped.update(added_sources)
                if self.member_stores:
                    changed.update(added_sources)
        if new_stores:
            if self.escaped is None:
                escape_global = True
            else:
                changed.update(self.escaped)
        members = self.members | batch.members
        entry_members = self.entry_members | batch.entry_members
        global_change = (
            escape_global
            or members != self.members
            or entry_members != self.entry_members
        )
        self.members = members
        self.entry_members = entry_members
        if batch.code_rebinding and not self.code_rebinding:
            self.code_rebinding = True
            global_change = True
        if self.reflected is not None:
            if batch.reflected is None:
                self.reflected = None
                global_change = True
            else:
                added = batch.reflected - self.reflected
                self.reflected.update(added)
                changed.update(added)
        if self.any_module is not None:
            if batch.any_module is None:
                self.any_module = None
                global_change = True
            else:
                added = batch.any_module - self.any_module
                if added:
                    self.any_module.update(added)
                    global_change = True
        if self.any_module is None:
            # Unknown is absorbing for every target in this operation. Named
            # additions cannot change any premise, so retain no dead index.
            self.by_source.clear()
            return None if global_change else frozenset(changed)
        for source, names in batch.by_source.items():
            if source not in self.by_source:
                if names is None or names:
                    self.by_source[source] = None if names is None else set(names)
                    changed.add(source)
                continue
            previous = self.by_source[source]
            if previous is None:
                continue
            if names is None:
                self.by_source[source] = None
                changed.add(source)
            else:
                added = names - previous
                if added:
                    previous.update(added)
                    self.by_source[source] = previous
                    changed.add(source)
        return None if global_change else frozenset(changed)

    def premise(
        self,
        source: Path,
        children: frozenset[str],
        *,
        noncanonical_imports: tuple[str, ...] = (),
        opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None,
        native_code: bool = False,
    ) -> PythonNamespacePremise:
        return _namespace_premise(
            self.by_source.get(source, frozenset()),
            self.any_module,
            self.members,
            self.entry_members,
            children,
            noncanonical_imports,
            opaque_activation_sources,
            self.reflected is None or source in self.reflected,
            self.code_rebinding or native_code,
            _escape_writes(source, self.escaped, self.member_stores),
        )


def bind_namespace_writer(
    write: PythonNamespaceWrite, module: str
) -> PythonNamespaceWrite:
    """Name the writing module of an own-module escape for a flattened graph walk.

    An ``own_module_escape`` row means the writer's own module value escaped;
    a caller that aggregates many modules' rows at once names it here.
    """
    if write.construct != "own_module_escape":
        return write
    return replace(write, module=module, construct="module_escape")


def graph_python_namespace_writes(
    writes: Iterable[PythonNamespaceWrite],
    resolve_module: Callable[[str], Path | None],
    *,
    entry: bool = True,
    writer: tuple[str, Path] | None = None,
) -> PythonGraphNamespaceWrites:
    """Aggregate named writes against the caller's admitted source topology.

    Source capture, package children, graph discovery and fixpoint scheduling
    remain with the graph owner; this function performs no filesystem work.
    """
    by_source: dict[Path, set[str] | None] = {}
    any_module: set[str] | None = set()
    reflected: set[Path] | None = set()
    code_rebinding = False
    members = NO_INVALID_MEMBERS
    member_stores: dict[str, str] = {}
    escaped: set[Path] | None = set()
    for write in writes:
        if write.construct == "import_binding_disjoint":
            continue
        if write.construct == "member_store":
            assert write.name is not None
            label = writer[0] if writer is not None else "<module>"
            member_stores.setdefault(write.name, f"{label}:{write.line}")
            continue
        if write.construct in {"module_escape", "own_module_escape"}:
            if escaped is None:
                continue
            spelling = write.module
            if write.construct == "own_module_escape":
                # Without its writer's source the value is any module's.
                if writer is None:
                    escaped = None
                else:
                    escaped.add(writer[1])
            elif spelling is None:
                escaped = None
            elif (target := resolve_module(spelling)) is not None:
                escaped.add(target)
            continue
        if write.construct == "code":
            code_rebinding = True
            continue
        if write.construct == "reflection":
            if write.module is None:
                reflected = None
            elif reflected is not None:
                target = resolve_module(write.module)
                if target is not None:
                    reflected.add(target)
            continue
        if write.construct == "import_binding":
            members |= canonical_import_binding_write_mask(write.name)
            continue
        members |= python_canonical_member_writes(write.module, write.name)
        if write.module is None:
            if write.name is None or any_module is None:
                any_module = None
            else:
                any_module.add(write.name)
            continue
        target = resolve_module(write.module)
        if target is None:
            # Not a local module: an attribute of some object, or a module
            # outside this graph's sources.
            continue
        names = by_source.setdefault(target, set())
        if write.name is None or names is None:
            by_source[target] = None
        else:
            names.add(write.name)
    return PythonGraphNamespaceWrites(
        MappingProxyType(
            {
                source: None if names is None else frozenset(names)
                for source, names in by_source.items()
            }
        ),
        None if any_module is None else frozenset(any_module),
        members,
        members if entry else NO_INVALID_MEMBERS,
        None if reflected is None else frozenset(reflected),
        code_rebinding,
        MappingProxyType(member_stores),
        None if escaped is None else frozenset(escaped),
    )
