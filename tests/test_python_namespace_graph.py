from __future__ import annotations

from dataclasses import FrozenInstanceError
from itertools import permutations
from pathlib import Path

import pytest

from molt.compiler_analysis.python_identity_catalog_generated import (
    ALL_INVALID_MEMBERS,
    PythonMember,
)
from molt.compiler_analysis.python_namespace_graph import (
    NO_GRAPH_NAMESPACE_WRITES,
    PythonNamespacePremise,
    graph_python_namespace_writes,
)
from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite


def test_namespace_graph_groups_source_aliases_and_package_bindings() -> None:
    shared = Path("shared.py")
    untouched = Path("untouched.py")
    topology = {"first": shared, "alias": shared, "other": untouched}
    writes = (
        PythonNamespaceWrite("first", "z", "attribute", 1),
        PythonNamespaceWrite("alias", "a", "setattr", 2),
        PythonNamespaceWrite(None, "shared_name", "namespace", 3),
    )
    for order in permutations(writes):
        graph = graph_python_namespace_writes(order, topology.get)
        assert graph.premise(shared, frozenset({"child"})) == PythonNamespacePremise(
            ("a", "child", "shared_name", "z"), 0
        )
        assert graph.premise(untouched, frozenset()) == PythonNamespacePremise(
            ("shared_name",), 0
        )
        assert graph.by_source == {shared: frozenset({"a", "z"})}


@pytest.mark.parametrize("computed_target", [False, True])
def test_computed_name_widens_its_target_without_losing_later_writes(
    computed_target: bool,
) -> None:
    first, second = Path("first.py"), Path("second.py")
    writes = (
        PythonNamespaceWrite(
            None if computed_target else "first", None, "namespace", 1
        ),
        PythonNamespaceWrite("first", "later", "attribute", 2),
        PythonNamespaceWrite("second", "other", "attribute", 3),
    )
    graph = graph_python_namespace_writes(
        writes, {"first": first, "second": second}.get
    )
    members = ALL_INVALID_MEMBERS if computed_target else 0
    assert graph.premise(first, frozenset({"child"})) == PythonNamespacePremise(
        None, members
    )
    assert graph.premise(second, frozenset()) == PythonNamespacePremise(
        None if computed_target else ("other",), members
    )


def test_non_source_canonical_writes_still_invalidate_members() -> None:
    graph = graph_python_namespace_writes(
        (
            PythonNamespaceWrite("typing", "TYPE_CHECKING", "attribute", 1),
            PythonNamespaceWrite("sys", "meta_path", "import_hooks", 2),
            PythonNamespaceWrite("unrelated.object", "value", "attribute", 3),
        ),
        lambda _name: None,
    )
    assert not graph.by_source
    assert graph.premise(Path("entry.py"), frozenset()) == PythonNamespacePremise(
        (), int(PythonMember.TYPING_TYPE_CHECKING | PythonMember.IMPORT_HOOKS)
    )


def test_aggregation_and_empty_premise_are_immutable() -> None:
    source = Path("entry.py")
    assert NO_GRAPH_NAMESPACE_WRITES.premise(
        source, frozenset({"child"})
    ) == PythonNamespacePremise(("child",), 0)
    graph = graph_python_namespace_writes(
        (PythonNamespaceWrite("entry", "flag", "attribute", 1),),
        {"entry": source}.get,
    )
    with pytest.raises(TypeError):
        graph.by_source[source] = frozenset()  # type: ignore[index]
    with pytest.raises(FrozenInstanceError):
        graph.premise(source, frozenset()).foreign_writes = ()  # type: ignore[misc]
