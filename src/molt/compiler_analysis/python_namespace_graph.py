"""Shared namespace premises for closed Python source graphs (HF-226).

The syntax authority is ``python_namespace_writes``. This module projects those
summaries onto a caller-owned module topology; it never discovers modules or
reads source paths. Tooling discovery and fixed source inventories share this
aggregation, while each graph owner supplies package-child bindings and decides
when its premise has converged.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, replace
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
    other global and member fact across callbacks.
    """

    foreign_writes: tuple[str, ...] | None
    member_writes: MemberMask
    entry_member_writes: MemberMask
    noncanonical_imports: tuple[str, ...]
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None


def python_namespace_binding_policy[P: PythonBindingFlowPolicy](
    policy: P, premise: PythonNamespacePremise | None
) -> P:
    """Apply a closed graph's initial and future uncertainty together.

    A reached writer may already have run before the first acquisition. Its
    entry mask must travel with callback writes through every graph consumer.
    Standalone callers keep their explicit policy when no graph was proved.
    """
    if premise is None:
        return policy
    return replace(
        policy,
        foreign_namespace_writes=premise.foreign_writes,
        callback_member_writes=premise.member_writes,
        entry_invalid_members=premise.entry_member_writes,
        noncanonical_imports=premise.noncanonical_imports,
        opaque_activation_sources=premise.opaque_activation_sources,
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

    def premise(
        self,
        source: Path,
        children: frozenset[str],
        *,
        noncanonical_imports: tuple[str, ...] = (),
        opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None,
    ) -> PythonNamespacePremise:
        return _namespace_premise(
            self.by_source.get(source, frozenset()),
            self.any_module,
            self.members,
            self.entry_members,
            children,
            noncanonical_imports,
            opaque_activation_sources,
        )


NO_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}), frozenset(), NO_INVALID_MEMBERS, NO_INVALID_MEMBERS
)
UNKNOWN_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}), None, ALL_INVALID_MEMBERS, ALL_INVALID_MEMBERS
)


def _namespace_premise(
    named: frozenset[str] | set[str] | None,
    any_module: frozenset[str] | set[str] | None,
    members: MemberMask,
    entry_members: MemberMask,
    children: frozenset[str],
    noncanonical_imports: tuple[str, ...],
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None,
) -> PythonNamespacePremise:
    if named is None or any_module is None:
        return PythonNamespacePremise(
            None,
            members,
            entry_members,
            noncanonical_imports,
            opaque_activation_sources,
        )
    return PythonNamespacePremise(
        tuple(sorted(children | named | any_module)),
        members,
        entry_members,
        noncanonical_imports,
        opaque_activation_sources,
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

    def merge(self, batch: PythonGraphNamespaceWrites) -> frozenset[Path] | None:
        changed: set[Path] = set()
        members = self.members | batch.members
        entry_members = self.entry_members | batch.entry_members
        global_change = members != self.members or entry_members != self.entry_members
        self.members = members
        self.entry_members = entry_members
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
            return None if global_change else frozenset()
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
    ) -> PythonNamespacePremise:
        return _namespace_premise(
            self.by_source.get(source, frozenset()),
            self.any_module,
            self.members,
            self.entry_members,
            children,
            noncanonical_imports,
            opaque_activation_sources,
        )


def graph_python_namespace_writes(
    writes: Iterable[PythonNamespaceWrite],
    resolve_module: Callable[[str], Path | None],
    *,
    entry: bool = True,
) -> PythonGraphNamespaceWrites:
    """Aggregate named writes against the caller's admitted source topology.

    Source capture, package children, graph discovery and fixpoint scheduling
    remain with the graph owner; this function performs no filesystem work.
    """
    by_source: dict[Path, set[str] | None] = {}
    any_module: set[str] | None = set()
    members = NO_INVALID_MEMBERS
    for write in writes:
        if write.construct == "import_binding_disjoint":
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
    )
