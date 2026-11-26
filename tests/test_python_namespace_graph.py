from __future__ import annotations

from dataclasses import FrozenInstanceError, replace
from itertools import permutations
from pathlib import Path

import pytest

from molt.compiler_analysis.python_identity_catalog_generated import (
    CANONICAL_IMPORT_IDENTITIES,
    ALL_INVALID_MEMBERS,
    NO_INVALID_MEMBERS,
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
from molt.compiler_analysis.python_imports import ImportScanMode
from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite

# A global import redirect makes every canonical provider noncanonical (M7).
ALL_CANONICAL = tuple(sorted(CANONICAL_IMPORT_IDENTITIES))


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
    # An unknown namespace write may rebind ``sys.modules`` itself.
    noncanonical = ALL_CANONICAL if computed_target else ()
    assert graph.premise(first, frozenset({"child"})) == PythonNamespacePremise(
        None, members, members, noncanonical
    )
    assert graph.premise(second, frozenset()) == PythonNamespacePremise(
        None if computed_target else ("other",), members, members, noncanonical
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
        ALL_CANONICAL,
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
        PythonNamespaceWrite("sys.modules", "typing", "import_binding", 6),
        PythonNamespaceWrite("fast", None, "guarded_import", 7),
    )
    oracle = graph_python_namespace_writes(writes, topology.get)
    assert oracle.import_redirects.redirect("typing")
    assert not oracle.import_redirects.redirect("first")
    assert oracle.premise(first, frozenset()).noncanonical_imports == ("typing",)
    assert oracle.import_redirects.swallowers("fast", ())
    for order in permutations(writes):
        index = PythonGraphNamespaceIndex()
        for write in order:
            batch = graph_python_namespace_writes((write,), topology.get)
            index.merge(batch)
            assert index.merge(batch) == frozenset()
        # The graph fact is stored once and joins to the same value.
        assert index.import_redirects == oracle.import_redirects
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
        ALL_CANONICAL,
        namespace_reflected=True,
        code_rebinding=True,
        instance_class_writer="an unknown graph",
        frame_locals_writer="an unknown graph",
        cell_writer="an unknown graph",
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


def test_function_exposure_and_foreign_globals_are_independent() -> None:
    import ast

    from molt.compiler_analysis.python_binding_flow import analyze_python_bindings
    from molt.compiler_analysis.python_identity_catalog_generated import PythonIdentity
    from molt.compiler_analysis.python_source_keys import (
        python_ast_digest,
        python_node_source_key,
    )

    source = ast.parse(
        "from _intrinsics import require_intrinsic as require\n"
        "read = require('molt_namespace_entry_get')\n"
        "def reader(owner):\n"
        "    return read(owner, 'member', None)\n"
    )
    function = source.body[-1]
    assert isinstance(function, ast.FunctionDef)
    returned = function.body[0]
    assert isinstance(returned, ast.Return) and isinstance(returned.value, ast.Call)
    exposed = (python_node_source_key(function),)
    for rebinding in [False, True]:
        premise = PythonNamespacePremise(
            (), 0, 0, (), opaque_activation_sources=exposed, code_rebinding=rebinding
        )
        policy = python_namespace_binding_policy(
            PythonBindingPolicy(), premise.scan_view()
        )
        assert policy.opaque_activation_sources == exposed
        index = analyze_python_bindings(
            source, source_digest=python_ast_digest(source), policy=policy
        )
        fact = index.call_fact(returned.value)
        assert fact is not None
        assert (
            fact.callee_is(PythonIdentity.INTRINSIC_NAMESPACE_ENTRY_GET)
            is not rebinding
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
    assert dict(batch.computed_member_writes) == {
        "__class__": "second:4",
        "cell_contents": "second:4",
    }
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


def test_computed_spelling_limits_which_identity_facts_are_invalidated() -> None:
    import ast

    from molt.compiler_analysis.python_namespace_writes import (
        decode_python_namespace_write,
        python_namespace_summary,
        python_namespace_write_payload,
    )

    source = Path("writer.py")
    index = PythonGraphNamespaceIndex()

    def apply(statement: str) -> frozenset[Path] | None:
        rows = python_namespace_summary(
            ast.parse(statement), package=None
        ).foreign_writes
        writes = [
            decode_python_namespace_write(python_namespace_write_payload(row))
            for row in rows
        ]
        return index.merge(
            graph_python_namespace_writes(
                writes, lambda _name: None, writer=("writer", source)
            )
        )

    assert apply('setattr(obj, f"field_{key}", value)') == frozenset()
    premise = index.premise(source, frozenset())
    assert premise.instance_class_writer is None
    assert premise.cell_writer is None

    assert apply('\nobject.__setattr__(obj, f"__{key}__", value)') is None
    premise = index.premise(source, frozenset())
    assert premise.instance_class_writer == "writer:2"
    assert premise.cell_writer is None

    assert apply('\n\ndelattr(obj, f"cell_{key}")') is None
    premise = index.premise(source, frozenset())
    assert premise.instance_class_writer == "writer:2"
    assert premise.cell_writer == "writer:3"


def test_cell_writers_reach_every_premise(tmp_path: Path) -> None:
    """A method's `__class__` cell holds its class only while no code in the
    graph can write a cell behind its owner: a named `cell_contents` store, a
    computed attribute store, a frame-locals writer, or a native extension
    (design 78, 5.10 step 4). A late writer changes every premise."""
    first, second = tmp_path / "first.py", tmp_path / "second.py"
    sources = {"first": first, "second": second}
    index = PythonGraphNamespaceIndex()
    quiet = PythonNamespaceWrite(None, "x", "member_store", 3)
    index.merge(
        graph_python_namespace_writes([quiet], sources.get, writer=("first", first))
    )
    assert index.premise(second, frozenset()).cell_writer is None
    assert (
        index.premise(second, frozenset(), native_code=True).cell_writer
        == "a native extension module"
    )
    writer = PythonNamespaceWrite(None, "cell_contents", "member_store", 7)
    batch = graph_python_namespace_writes(
        [writer], sources.get, writer=("second", second)
    )
    assert index.merge(batch) is None, "a cell writer is a global change"
    assert index.premise(first, frozenset()).cell_writer == "second:7"
    # The scan view drops it: a scan never reads the cell writer.
    assert index.premise(first, frozenset()).scan_view().cell_writer is None

    index = PythonGraphNamespaceIndex()
    computed = PythonNamespaceWrite(None, None, "computed_member_store", 5)
    index.merge(
        graph_python_namespace_writes([computed], sources.get, writer=("first", first))
    )
    assert index.premise(second, frozenset()).cell_writer == "first:5"


def test_frame_locals_writer_reaches_every_premise(tmp_path: Path) -> None:
    """A 3.13+ callback may rebind a running function's locals only through a
    PEP 667 frame proxy. The graph names the program's first `f_locals` use
    that may write (or a native extension) in every module's premise; with none, binding
    flow keeps locals across callbacks (HF-901)."""
    first, second = tmp_path / "first.py", tmp_path / "second.py"
    sources = {"first": first, "second": second}
    index = PythonGraphNamespaceIndex()
    quiet = PythonNamespaceWrite(None, "x", "member_store", 3)
    index.merge(
        graph_python_namespace_writes([quiet], sources.get, writer=("first", first))
    )
    assert index.premise(second, frozenset()).frame_locals_writer is None
    assert (
        index.premise(second, frozenset(), native_code=True).frame_locals_writer
        == "a native extension module"
    )

    reads = [
        PythonNamespaceWrite(None, None, "frame_locals", 9),
        PythonNamespaceWrite(None, None, "frame_locals", 4),
    ]
    batch = graph_python_namespace_writes(reads, sources.get, writer=("second", second))
    assert batch.frame_locals_writer == "second:4"
    # A new writer changes every premise: a global change.
    assert index.merge(batch) is None
    assert index.premise(first, frozenset()).frame_locals_writer == "second:4"
    # A later writer does not replace the first, and wakes nothing.
    later = graph_python_namespace_writes(
        [PythonNamespaceWrite(None, None, "frame_locals", 1)],
        sources.get,
        writer=("first", first),
    )
    assert index.merge(later) == frozenset()
    assert index.premise(first, frozenset()).frame_locals_writer == "second:4"

    assert NO_GRAPH_NAMESPACE_WRITES.frame_locals_writer is None
    assert UNKNOWN_GRAPH_NAMESPACE_WRITES.frame_locals_writer == "an unknown graph"


def test_frame_locals_writer_joins_and_binds_the_policy() -> None:
    closed = PythonNamespacePremise((), 0, 0, ())
    written = replace(closed, frame_locals_writer="m:7")
    assert join_python_namespace_premises(closed, closed) == closed
    joined = join_python_namespace_premises(closed, written)
    assert joined is not None and joined.frame_locals_writer == "m:7"

    from molt.compiler_analysis.python_binding_flow import (
        NO_CLOSED_GRAPH_FRAME_LOCALS_WRITER,
        frame_locals_exposed,
    )

    standalone = PythonBindingPolicy(target_python=(3, 15))
    # Without a graph, any code may hold a frame proxy.
    assert standalone.frame_locals_writer == NO_CLOSED_GRAPH_FRAME_LOCALS_WRITER
    assert python_namespace_binding_policy(standalone, None) == standalone
    bound = python_namespace_binding_policy(standalone, closed)
    assert bound.frame_locals_writer is None
    assert bound.flow_policy().frame_locals_writer is None
    assert (
        python_namespace_binding_policy(standalone, written).frame_locals_writer
        == "m:7"
    )
    assert not frame_locals_exposed((3, 15), None)
    assert frame_locals_exposed((3, 13), "m:7")
    # Before PEP 667 a frame proxy never writes a fast local.
    assert not frame_locals_exposed((3, 12), "m:7")


@pytest.mark.parametrize(
    ("source", "rows"),
    [
        # Uses that may write through the proxy, or hand it to code that may.
        ("import sys\ndef f():\n    sys._getframe(1).f_locals['x'] = 1\n", 1),
        ("def f(frame):\n    return getattr(frame, 'f_locals')\n", 1),
        (
            "import operator\nread = operator.attrgetter('f_locals')\n",
            1,
        ),
        ("def f(frame):\n    return frame.f_locals\n", 1),
        ("def f(frame):\n    del frame.f_locals['x']\n", 1),
        ("def f(frame):\n    frame.f_locals.update(x=1)\n", 1),
        ("def f(frame, sink):\n    sink(frame.f_locals)\n", 1),
        ("def f(frame):\n    proxy = frame.f_locals\n    proxy['x'] = 1\n", 1),
        # Reads: no local can change through them.
        ("import sys\ndef f():\n    return sys._getframe(1).f_locals.get('x')\n", 0),
        ("def f(frame):\n    return frame.f_locals['x']\n", 0),
        ("def f(frame):\n    return 'x' in frame.f_locals\n", 0),
        ("def f(frame):\n    return dict(frame.f_locals)\n", 0),
        ("def f(frame):\n    return sorted(frame.f_locals.items())\n", 0),
        ("def f(frame):\n    for name in frame.f_locals:\n        print(name)\n", 0),
        ("def f(frame):\n    return getattr(frame, 'f_locals').copy()\n", 0),
        ("def f(frame):\n    names = frame.f_locals\n    return names.get('x')\n", 0),
        ("def f(frame):\n    return frame.f_globals, frame.f_code.co_name\n", 0),
        ("def f():\n    return locals()\n", 0),
    ],
)
def test_frame_locals_writes_are_namespace_rows(source: str, rows: int) -> None:
    import ast

    from molt.compiler_analysis.python_namespace_writes import (
        python_namespace_summary,
    )

    summary = python_namespace_summary(ast.parse(source), target_python=(3, 15))
    found = [
        write for write in summary.foreign_writes if write.construct == "frame_locals"
    ]
    assert len(found) == rows
    assert all(write.module is None and write.name is None for write in found)


def test_name_universe_keeps_module_scope_names_only() -> None:
    from molt.compiler_analysis.python_namespace_graph import (
        python_namespace_name_universe,
    )

    universe = python_namespace_name_universe(
        "from __future__ import annotations\n"
        "import os.path\n"
        "KEYS = ('\\x41BC',)\n"
        "def f(x: Foo, y: 'Str[int]') -> Bar:\n"
        "    local = obj.attribute + f'{inner}'\n"
        "    return local\n"
    )
    assert universe is not None
    # Module bindings, global reads from nested scopes, PEP 563 annotation
    # names and decoded string identifiers stay.
    assert {"os", "KEYS", "f", "Foo", "Str", "Bar", "obj", "inner", "ABC"} <= universe
    # The binding analysis also reads loader metadata and builtins unspelled.
    assert {"__name__", "__spec__", "__builtins__", "__all__"} <= universe
    # Locals, parameters and attribute names never resolve in the namespace.
    assert not {"x", "y", "local", "attribute", "path"} & universe
    assert python_namespace_name_universe("def broken(:\n") is None
    assert python_namespace_name_universe(None) is None


def test_scan_view_drops_lowering_facts_and_unnamed_foreign_writes() -> None:
    premise = PythonNamespacePremise(
        ("Any", "attribute", "value"),
        NO_INVALID_MEMBERS,
        NO_INVALID_MEMBERS,
        (),
        namespace_reflected=True,
        code_rebinding=True,
        escape_writes=(("value", "writer:3"),),
        instance_class_writer="writer:4",
    )
    universe = frozenset({"Any", "value"})
    view = premise.scan_view(universe)
    assert (view.foreign_writes, view.foreign_universe) == (("Any", "value"), universe)
    assert not view.namespace_reflected
    assert view.code_rebinding
    assert (view.escape_writes, view.instance_class_writer) == ((), None)
    # The analysis learns the universe, so a computed key outside it stays
    # foreign-written.
    policy = python_namespace_binding_policy(PythonBindingPolicy(), view)
    assert policy.foreign_namespace_universe == universe
    # Without a universe only the lowering facts go; an open premise stays open.
    assert premise.scan_view().foreign_writes == premise.foreign_writes
    assert premise.scan_view().foreign_universe is None
    open_premise = replace(premise, foreign_writes=None)
    assert open_premise.scan_view(universe).foreign_universe is None
    # A view is a scan key, never a graph premise.
    with pytest.raises(ValueError, match="not a whole premise"):
        view.scan_view(universe)
    with pytest.raises(ValueError, match="do not join"):
        join_python_namespace_premises(view, premise)
    with pytest.raises(ValueError, match="bounds a closed foreign write set"):
        PythonBindingPolicy(
            foreign_namespace_writes=("Any",),
            foreign_namespace_universe=frozenset({"value"}),
        )


_SCAN_VIEW_MODULES = (
    ("asyncio", "asyncio/__init__.py"),
    ("asyncio.events", "asyncio/events.py"),
    ("asyncio.tasks", "asyncio/tasks.py"),
    ("collections", "collections/__init__.py"),
    ("logging", "logging/__init__.py"),
    ("typing", "typing.py"),
    ("os", "os.py"),
    ("enum", "enum.py"),
    ("dataclasses", "dataclasses.py"),
)


@pytest.mark.parametrize(("module", "relative"), _SCAN_VIEW_MODULES)
@pytest.mark.parametrize("mode", ["module_init", "full"])
def test_scan_view_yields_the_same_scan_as_the_whole_premise(
    module: str, relative: str, mode: ImportScanMode
) -> None:
    """HF-926 oracle: scan real stdlib sources under a whole premise that
    writes every identifier they contain, locals and attribute names included,
    plus names they never mention, and under its scan view.

    The scans agree except for one kind of row: the whole premise gives each
    foreign-only name a module slot, so the module namespace lists it with no
    value origins. Such a row carries no value: namespace consumers
    (``python_source_demands``) read origin sets, and presence only for
    ``__getattr__``, which every universe holds."""
    import ast
    import io
    import tokenize

    from molt.cli.module_resolution import _stdlib_root_path
    from molt.compiler_analysis.python_import_scan import _collect_imports_for_graph
    from molt.compiler_analysis.python_namespace_graph import (
        python_namespace_name_universe,
    )

    path = _stdlib_root_path() / relative
    text = path.read_text(encoding="utf-8")
    universe = python_namespace_name_universe(text)
    assert universe is not None and "__getattr__" in universe
    spelled = {
        token.string
        for token in tokenize.generate_tokens(io.StringIO(text).readline)
        if token.type == tokenize.NAME
    }
    foreign = tuple(sorted(spelled | universe | {f"unrelated_{n}" for n in range(64)}))
    # Every lowering-only fact is set: the view drops them, so equal scans
    # prove no such fact can change what a scan returns (its key omits them).
    premise = PythonNamespacePremise(
        foreign,
        NO_INVALID_MEMBERS,
        NO_INVALID_MEMBERS,
        (),
        namespace_reflected=True,
        code_rebinding=True,
        escape_writes=tuple((name, "writer:1") for name in foreign[:8]),
        instance_class_writer="writer:2",
        exact_receivers=(((10, 4, 12, 8, "FunctionDef"), (9, 0, 20, 0, "ClassDef")),),
    )
    view = premise.scan_view(universe)
    assert view == replace(
        premise,
        foreign_writes=view.foreign_writes,
        foreign_universe=universe,
        namespace_reflected=False,
        escape_writes=(),
        instance_class_writer=None,
        exact_receivers=(),
    )
    assert premise.foreign_writes is not None and view.foreign_writes is not None
    dropped = frozenset(premise.foreign_writes) - frozenset(view.foreign_writes)
    assert dropped - {f"unrelated_{n}" for n in range(64)}

    def scan(selected: PythonNamespacePremise) -> object:
        result = _collect_imports_for_graph(
            ast.parse(text, filename=str(path)),
            module,
            relative.endswith("__init__.py"),
            import_scan_mode=mode,
            namespace_premise=selected,
        )
        values = result.source_values
        namespaces = tuple(
            (
                owner,
                tuple(
                    (name, origins)
                    for name, origins in names
                    if origins or owner[-1] != "Module" or name not in dropped
                ),
                exposure,
            )
            for owner, names, exposure in values.namespaces
        )
        return result._replace(source_values=values._replace(namespaces=namespaces))

    assert scan(view) == scan(premise)


@pytest.mark.parametrize(
    ("module", "relative"),
    (
        ("collections", "collections/__init__.py"),
        ("enum", "enum.py"),
        ("dataclasses", "dataclasses.py"),
    ),
)
def test_premise_read_log_cuts_off_only_unchanged_scans(
    module: str, relative: str
) -> None:
    """HF-926 early-cutoff oracle: scan real stdlib module code and function
    bodies under one premise with the read log on, then scan each again,
    in full, under premises that write one module name, activate one
    function from unknown code, or write every name. Wherever the log says
    its answers hold, the full scan must equal the logged one; the cases must
    include a premise whose scan really changes, so the check is not vacuous."""
    import ast

    from molt.cli.module_resolution import _stdlib_root_path
    from molt.compiler_analysis.python_import_scan import (
        SCAN_PREMISE_READS,
        ScanPremiseReads,
        _collect_imports_for_graph,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    path = _stdlib_root_path() / relative
    text = path.read_text(encoding="utf-8")
    tree = ast.parse(text, filename=str(path))
    functions = [node for node in tree.body if isinstance(node, ast.FunctionDef)][:3]
    names = sorted(
        {
            target.id
            for node in tree.body
            if isinstance(node, ast.Assign)
            for target in node.targets
            if isinstance(target, ast.Name)
        }
        | {
            node.name
            for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.ClassDef))
        }
    )[:10]
    base = PythonNamespacePremise(
        (), NO_INVALID_MEMBERS, NO_INVALID_MEMBERS, (), opaque_activation_sources=()
    )
    alternatives = (
        [replace(base, foreign_writes=(name,)) for name in names]
        + [
            replace(base, opaque_activation_sources=(python_node_source_key(node),))
            for node in functions
        ]
        + [replace(base, foreign_writes=None)]
    )

    def scan(
        premise: PythonNamespacePremise,
        owner: object,
        log: ScanPremiseReads | None = None,
    ) -> object:
        token = SCAN_PREMISE_READS.set(log)
        try:
            return _collect_imports_for_graph(
                ast.parse(text, filename=str(path)),
                module,
                relative.endswith("__init__.py"),
                import_scan_mode="module_init",
                source_owner=owner,
                namespace_premise=premise,
            )
        finally:
            SCAN_PREMISE_READS.reset(token)

    kept = changed = 0
    for owner in [None, *(python_node_source_key(node) for node in functions)]:
        log = ScanPremiseReads()
        logged = scan(base, owner, log)
        assert log.usable
        for premise in alternatives:
            full = scan(premise, owner)
            if log.unchanged_under(premise):
                assert full == logged, (owner, premise)
                kept += 1
            else:
                changed += full != logged
    assert kept and changed


def test_unread_slots_keep_scans_with_an_explicit_global_writer() -> None:
    import ast

    from molt.compiler_analysis.python_import_scan import (
        SCAN_PREMISE_READS,
        ScanPremiseReads,
        _collect_imports_for_graph,
    )
    from molt.compiler_analysis.python_source_keys import python_node_source_key

    source = (
        "def selected(): pass\n"
        "def local_only(): pass\n"
        "def subject():\n"
        "    global exported\n"
        "    exported = selected\n"
        "    local = local_only\n"
    )
    tree = ast.parse(source)
    selected, _, subject = map(python_node_source_key, tree.body)
    base = PythonNamespacePremise(
        (),
        NO_INVALID_MEMBERS,
        NO_INVALID_MEMBERS,
        (),
        opaque_activation_sources=(),
        foreign_universe=frozenset(
            {"independent", "exported", "selected", "local_only"}
        ),
    )
    revised = replace(base, foreign_writes=("independent",))
    log = ScanPremiseReads()
    token = SCAN_PREMISE_READS.set(log)
    try:
        logged = _collect_imports_for_graph(
            tree,
            "provider",
            False,
            import_scan_mode="module_init",
            source_owner=subject,
            namespace_premise=base,
        )
    finally:
        SCAN_PREMISE_READS.reset(token)
    actual = _collect_imports_for_graph(
        ast.parse(source),
        "provider",
        False,
        import_scan_mode="module_init",
        source_owner=subject,
        namespace_premise=revised,
    )
    assert {
        demand.source for demand in logged.source_demands if demand.phase == "expose"
    } == {selected}
    assert actual == logged
    assert log.unchanged_under(revised)


_ESCAPED_MODULE = "import sys\nkeep(sys.modules[__name__])\n"
_DEFINITION_STORES = (
    "def deco(f):\n"
    "    def wrapper():\n"
    "        return f()\n"
    "    wrapper.__doc__ = f.__doc__\n"
    "    setattr(wrapper, '__annotate__', None)\n"
    "    return wrapper\n"
)


def _graph_premise(
    tmp_path: Path, sources: dict[str, str], target: str
) -> PythonNamespacePremise:
    import ast

    from molt.compiler_analysis.python_namespace_writes import (
        python_namespace_summary,
    )

    paths = {name: tmp_path / f"{name}.py" for name in sources}
    index = PythonGraphNamespaceIndex()
    for name, source in sources.items():
        writes = python_namespace_summary(ast.parse(source), package=None)
        index.merge(
            graph_python_namespace_writes(
                writes.foreign_writes, paths.get, writer=(name, paths[name])
            )
        )
    return index.premise(paths[target], frozenset())


def test_member_stores_on_function_objects_leave_escaped_modules_closed(
    tmp_path: Path,
) -> None:
    """HF-926 differential: a store on a local that holds only a function
    object writes no module, while a store through a module alias or a
    parameter that may receive a module still names its writer."""
    closed = _graph_premise(
        tmp_path, {"m": _ESCAPED_MODULE, "w": _DEFINITION_STORES}, "m"
    )
    assert closed.foreign_writes is not None
    assert not {"__doc__", "__annotate__"} & set(closed.foreign_writes)

    alias = _graph_premise(
        tmp_path,
        {
            "m": _ESCAPED_MODULE + "def f():\n    a = sys.modules[__name__]\n"
            "    a.x = 1\n",
            "p": "def set_it(mod):\n    mod.y = 1\n",
            "w": _DEFINITION_STORES,
        },
        "m",
    )
    assert alias.escape_writer("x") == "m:5"
    assert alias.escape_writer("y") == "p:2"
    assert alias.escape_writer("__doc__") is None

    # A frame-locals writer can rebind the local to a module: the stores count.
    rebinding = _graph_premise(
        tmp_path,
        {
            "m": _ESCAPED_MODULE,
            "w": _DEFINITION_STORES,
            "r": "import sys\nsys._getframe(1).f_locals['wrapper'] = sys\n",
        },
        "m",
    )
    assert rebinding.frame_locals_writer is not None
    assert rebinding.escape_writer("__doc__") == "w:4"
    assert rebinding.escape_writer("__annotate__") == "w:5"
