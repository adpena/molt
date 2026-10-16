"""Shared namespace premises for closed Python source graphs (HF-226).

The syntax authority is ``python_namespace_writes``. This module projects those
summaries onto a caller-owned module topology; it never discovers modules or
reads source paths. Tooling discovery and fixed source inventories share this
aggregation, while each graph owner supplies package-child bindings and decides
when its premise has converged.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

from molt.compiler_analysis.python_binding_facts import NO_INVALID_MEMBERS, MemberMask
from molt.compiler_analysis.python_binding_flow import python_canonical_member_writes
from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite


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


@dataclass(frozen=True, slots=True)
class PythonGraphNamespaceWrites:
    """Named namespace writes found in one walk of a closed graph."""

    # Target source -> names written into it; None when a name is computed.
    by_source: Mapping[Path, frozenset[str] | None]
    # Names written into a namespace reached by a computed ``sys.modules`` key.
    any_module: frozenset[str] | None
    members: MemberMask

    def premise(self, source: Path, children: frozenset[str]) -> PythonNamespacePremise:
        named = self.by_source.get(source, frozenset())
        if named is None or self.any_module is None:
            return PythonNamespacePremise(None, self.members)
        return PythonNamespacePremise(
            tuple(sorted(children | named | self.any_module)), self.members
        )


NO_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}), frozenset(), NO_INVALID_MEMBERS
)


def graph_python_namespace_writes(
    writes: Iterable[PythonNamespaceWrite],
    resolve_module: Callable[[str], Path | None],
) -> PythonGraphNamespaceWrites:
    """Aggregate named writes against the caller's admitted source topology.

    Source capture, package children, graph discovery and fixpoint scheduling
    remain with the graph owner; this function performs no filesystem work.
    """
    by_source: dict[Path, set[str] | None] = {}
    any_module: set[str] | None = set()
    members = NO_INVALID_MEMBERS
    for write in writes:
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
    )
