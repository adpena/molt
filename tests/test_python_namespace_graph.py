from __future__ import annotations

from dataclasses import FrozenInstanceError, replace
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
    python_namespace_binding_policy,
)
from molt.compiler_analysis.python_binding_flow import PythonBindingPolicy
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
            ("a", "child", "shared_name", "z"), 0, 0, ()
        )
        assert graph.premise(untouched, frozenset()) == PythonNamespacePremise(
            ("shared_name",), 0, 0, ()
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
        None, members, members, ()
    )
    assert graph.premise(second, frozenset()) == PythonNamespacePremise(
        None if computed_target else ("other",), members, members, ()
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
        (),
        int(PythonMember.TYPING_TYPE_CHECKING | PythonMember.IMPORT_HOOKS),
        int(PythonMember.TYPING_TYPE_CHECKING | PythonMember.IMPORT_HOOKS),
        (),
    )


def test_aggregation_and_empty_premise_are_immutable() -> None:
    source = Path("entry.py")
    assert NO_GRAPH_NAMESPACE_WRITES.premise(
        source, frozenset({"child"})
    ) == PythonNamespacePremise(("child",), 0, 0, ())
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
        PythonNamespaceWrite("second", None, "reflection", 5),
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
    assert old == PythonNamespacePremise((), 0, 0, ())
    assert index.premise(second, frozenset()) == old
    assert index.merge(NO_GRAPH_NAMESPACE_WRITES) == frozenset()
    assert index.merge(UNKNOWN_GRAPH_NAMESPACE_WRITES) is None
    assert index.premise(second, frozenset()) == PythonNamespacePremise(
        None,
        ALL_INVALID_MEMBERS,
        ALL_INVALID_MEMBERS,
        (),
        namespace_reflected=True,
        code_rebinding=True,
        instance_class_writer="an unknown graph",
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
        (None, PythonNamespacePremise((), 0, 0, ()), None),
        (
            PythonNamespacePremise(("a", "shared"), 0, 0, ()),
            PythonNamespacePremise(
                ("b", "shared"),
                int(PythonMember.IMPORT_HOOKS),
                int(PythonMember.IMPORT_HOOKS),
                (),
            ),
            PythonNamespacePremise(
                ("a", "b", "shared"),
                int(PythonMember.IMPORT_HOOKS),
                int(PythonMember.IMPORT_HOOKS),
                (),
            ),
        ),
        (
            PythonNamespacePremise(None, 0, 0, ()),
            PythonNamespacePremise(
                ("b",),
                int(PythonMember.IMPORT_HOOKS),
                int(PythonMember.IMPORT_HOOKS),
                (),
            ),
            PythonNamespacePremise(
                None, int(PythonMember.IMPORT_HOOKS), int(PythonMember.IMPORT_HOOKS), ()
            ),
        ),
    ],
)
def test_authority_join_never_grants_unverified_precision(
    left, right, expected
) -> None:
    assert join_python_namespace_premises(left, right) == expected
    assert join_python_namespace_premises(right, left) == expected


def test_callback_inventory_does_not_claim_initializer_execution() -> None:
    path = Path("provider.py")
    write = PythonNamespaceWrite("sys", "platform", "attribute", 3)
    inventory = graph_python_namespace_writes((write,), lambda _name: None, entry=False)
    active = graph_python_namespace_writes((write,), lambda _name: None, entry=True)
    index = PythonGraphNamespaceIndex()
    index.merge(inventory)
    before = index.premise(path, frozenset())
    assert before.member_writes & int(PythonMember.SYS_PLATFORM)
    assert before.entry_member_writes == 0
    index.merge(active)
    assert index.premise(path, frozenset()).entry_member_writes & int(
        PythonMember.SYS_PLATFORM
    )
    assert before.entry_member_writes == 0


def test_code_row_makes_every_premise_code_rebinding() -> None:
    first, second = Path("first.py"), Path("second.py")
    topology = {"first": first, "second": second}
    code = graph_python_namespace_writes(
        (PythonNamespaceWrite(None, None, "code", 1),), topology.get
    )
    assert code.premise(second, frozenset()) == PythonNamespacePremise(
        (), 0, 0, (), code_rebinding=True
    )
    index = PythonGraphNamespaceIndex()
    assert index.premise(first, frozenset()).code_rebinding is False
    # A native extension's C code can read code objects the scan cannot see.
    assert index.premise(first, frozenset(), native_code=True).code_rebinding
    # The fact is global: every premise changes, so the delta is unbounded.
    assert index.merge(code) is None
    assert index.premise(first, frozenset()).code_rebinding
    assert index.merge(code) == frozenset()
    assert UNKNOWN_GRAPH_NAMESPACE_WRITES.premise(first, frozenset()).code_rebinding


def test_lowering_sees_exposed_definitions_only_with_a_code_rebinder() -> None:
    exposed = ((1, 0, 2, 4, "FunctionDef"),)
    policy = PythonBindingPolicy()
    quiet = PythonNamespacePremise((), 0, 0, (), opaque_activation_sources=exposed)
    # Lowering: without a rebinder an exposed function reads its own globals.
    lowered = python_namespace_binding_policy(policy, quiet, lowering=True)
    assert lowered.opaque_activation_sources == ()
    rebinding = replace(quiet, code_rebinding=True)
    assert (
        python_namespace_binding_policy(
            policy, rebinding, lowering=True
        ).opaque_activation_sources
        == exposed
    )
    # Discovery keeps the exposed set whatever the fact says.
    assert python_namespace_binding_policy(policy, quiet).opaque_activation_sources == (
        exposed
    )
    # An owner with no proven exposure stays wholly opaque.
    unknown = replace(quiet, opaque_activation_sources=None)
    assert (
        python_namespace_binding_policy(
            policy, unknown, lowering=True
        ).opaque_activation_sources
        is None
    )


def test_reflection_marks_only_its_target_and_writes_no_member() -> None:
    first, second = Path("first.py"), Path("second.py")
    topology = {"first": first, "second": second}
    named = graph_python_namespace_writes(
        (PythonNamespaceWrite("first", None, "reflection", 1),), topology.get
    )
    assert named.premise(first, frozenset()) == PythonNamespacePremise(
        (), 0, 0, (), namespace_reflected=True
    )
    assert named.premise(second, frozenset()) == PythonNamespacePremise((), 0, 0, ())
    # A reflection of some module's mapping (a frame's f_globals) reaches all.
    anywhere = graph_python_namespace_writes(
        (PythonNamespaceWrite(None, None, "reflection", 1),), topology.get
    )
    assert anywhere.premise(second, frozenset()) == PythonNamespacePremise(
        (), 0, 0, (), namespace_reflected=True
    )
    index = PythonGraphNamespaceIndex()
    assert index.merge(named) == frozenset({first})
    assert index.merge(anywhere) is None
    assert index.premise(second, frozenset()).namespace_reflected


def test_escaped_module_values_meet_named_member_stores(tmp_path: Path) -> None:
    """A name stored as an attribute anywhere is a write of every module whose
    value escapes, and of no other module (call_anatomy.md 4.5a)."""
    from molt.compiler_analysis.python_namespace_graph import (
        PythonGraphNamespaceIndex,
        bind_namespace_writer,
        graph_python_namespace_writes,
    )
    from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite

    warnings, other = tmp_path / "warnings.py", tmp_path / "other.py"
    sources = {"warnings": warnings, "other": other}
    store = PythonNamespaceWrite(None, "_impl", "member_store", 296)
    own = PythonNamespaceWrite(None, None, "own_module_escape", 266)
    index = PythonGraphNamespaceIndex()
    index.merge(
        graph_python_namespace_writes(
            [store, own], sources.get, writer=("warnings", warnings)
        )
    )
    escaped = index.premise(warnings, frozenset())
    assert escaped.foreign_writes == ("_impl",)
    assert escaped.escape_writes == (("_impl", "warnings:296"),)
    assert escaped.escape_writer("_impl") == "warnings:296"
    assert escaped.escape_writer("other_name") is None
    kept = index.premise(other, frozenset())
    assert kept.foreign_writes == ()
    assert kept.escape_writes == ()

    # A store admitted after the escape still reaches the escaped module.
    later = PythonNamespaceWrite(None, "late", "member_store", 7)
    changed = index.merge(
        graph_python_namespace_writes([later], sources.get, writer=("other", other))
    )
    assert changed is not None and warnings in changed
    assert "late" in index.premise(warnings, frozenset()).foreign_writes

    # A flattened walk names the writer of an own escape itself.
    bound = bind_namespace_writer(own, "warnings")
    assert (bound.module, bound.construct) == ("warnings", "module_escape")
    flat = graph_python_namespace_writes([store, bound], sources.get)
    assert flat.escaped == frozenset({warnings})
    # Without a writer an own escape is any module's value.
    assert graph_python_namespace_writes([own], sources.get).escaped is None


def test_instance_class_writers_reach_every_premise(tmp_path: Path) -> None:
    """A constructor's result is exactly its class only while no code in the
    graph can assign `__class__` on an instance: a named store, a computed
    attribute store, or a native extension (call_anatomy.md 4.5, "Implementation
    (field slots)"). The fact belongs to every module's premise."""
    from molt.compiler_analysis.python_namespace_graph import (
        PythonGraphNamespaceIndex,
        graph_python_namespace_writes,
    )
    from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite

    first, second = tmp_path / "first.py", tmp_path / "second.py"
    sources = {"first": first, "second": second}
    index = PythonGraphNamespaceIndex()
    field = PythonNamespaceWrite(None, "x", "member_store", 3)
    assert (
        index.merge(
            graph_python_namespace_writes([field], sources.get, writer=("first", first))
        )
        == frozenset()
    )
    assert index.premise(second, frozenset()).instance_class_writer is None
    assert (
        index.premise(second, frozenset(), native_code=True).instance_class_writer
        == "a native extension module"
    )

    computed = PythonNamespaceWrite(None, None, "computed_member_store", 9)
    later = PythonNamespaceWrite(None, None, "computed_member_store", 4)
    batch = graph_python_namespace_writes(
        [computed, later], sources.get, writer=("second", second)
    )
    assert batch.computed_member_store == "second:4"
    # A new writer changes every premise: a global change.
    assert index.merge(batch) is None
    assert index.premise(first, frozenset()).instance_class_writer == "second:4"

    named = PythonNamespaceWrite(None, "__class__", "member_store", 12)
    index = PythonGraphNamespaceIndex()
    assert (
        index.merge(
            graph_python_namespace_writes([named], sources.get, writer=("first", first))
        )
        is None
    )
    assert index.premise(second, frozenset()).instance_class_writer == "first:12"
