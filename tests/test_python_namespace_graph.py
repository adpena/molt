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
    UNKNOWN_GRAPH_NAMESPACE_WRITES,
    PythonGraphNamespaceIndex,
    PythonNamespacePremise,
    graph_python_namespace_writes,
    join_python_namespace_premises,
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


def test_incremental_index_matches_whole_graph_in_any_admission_order() -> None:
    first, second = Path("first.py"), Path("second.py")
    topology = {"first": first, "second": second}
    writes = (
        PythonNamespaceWrite("first", "flag", "attribute", 1),
        PythonNamespaceWrite("second", None, "namespace", 2),
        PythonNamespaceWrite(None, "shared", "attribute", 3),
        PythonNamespaceWrite("typing", "TYPE_CHECKING", "attribute", 4),
    )
    oracle = graph_python_namespace_writes(writes, topology.get)
    for order in permutations(writes):
        index = PythonGraphNamespaceIndex()
        for write in order:
            batch = graph_python_namespace_writes((write,), topology.get)
            index.merge(batch)
            assert index.merge(batch) == frozenset()
        for source in (first, second, Path("untouched.py")):
            assert index.premise(source, frozenset({"child"})) == oracle.premise(
                source, frozenset({"child"})
            )


def test_incremental_index_reports_only_changed_targets_and_preserves_snapshots() -> (
    None
):
    first, second = Path("first.py"), Path("second.py")
    topology = {"first": first, "second": second}
    index = PythonGraphNamespaceIndex()
    old = index.premise(first, frozenset())
    batch = graph_python_namespace_writes(
        (PythonNamespaceWrite("first", "flag", "attribute", 1),), topology.get
    )
    assert index.merge(batch) == frozenset({first})
    assert old == PythonNamespacePremise((), 0)
    assert index.premise(second, frozenset()) == old
    assert index.merge(NO_GRAPH_NAMESPACE_WRITES) == frozenset()
    assert index.merge(UNKNOWN_GRAPH_NAMESPACE_WRITES) is None
    assert index.premise(second, frozenset()) == PythonNamespacePremise(
        None, ALL_INVALID_MEMBERS
    )
    assert index.merge(UNKNOWN_GRAPH_NAMESPACE_WRITES) == frozenset()
    assert index.merge(batch) == frozenset()
    assert (
        index.merge(
            graph_python_namespace_writes(
                (PythonNamespaceWrite("second", "later", "attribute", 2),), topology.get
            )
        )
        == frozenset()
    )
    assert not index.by_source


@pytest.mark.parametrize(
    ("left", "right", "expected"),
    [
        (None, PythonNamespacePremise((), 0), None),
        (
            PythonNamespacePremise(("a", "shared"), 0),
            PythonNamespacePremise(("b", "shared"), int(PythonMember.IMPORT_HOOKS)),
            PythonNamespacePremise(
                ("a", "b", "shared"), int(PythonMember.IMPORT_HOOKS)
            ),
        ),
        (
            PythonNamespacePremise(None, 0),
            PythonNamespacePremise(("b",), int(PythonMember.IMPORT_HOOKS)),
            PythonNamespacePremise(None, int(PythonMember.IMPORT_HOOKS)),
        ),
    ],
)
def test_authority_join_never_grants_unverified_precision(
    left, right, expected
) -> None:
    assert join_python_namespace_premises(left, right) == expected
    assert join_python_namespace_premises(right, left) == expected
