from __future__ import annotations

from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest

from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
from molt.stdlib_intrinsic_policy import (
    STATUS_INTRINSIC,
    STATUS_INTRINSIC_SUPPORT,
    STATUS_POLICY_GATE,
    STATUS_PYTHON_COMPILED,
    StdlibIntrinsicClassification,
    classify_stdlib_module_statuses,
    stdlib_module_import_evidence,
    stdlib_module_static_imports,
)
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION
from tests.source_record_test_support import source_records


def test_intrinsic_facts_reuse_the_full_scan_namespace_authority(tmp_path):
    from molt.stdlib_intrinsic_policy import stdlib_module_intrinsic_facts

    owner = tmp_path / "owner.py"
    facts = stdlib_module_intrinsic_facts(
        "pkg.owner",
        owner,
        source="from . import target\ndef callback():\n    target.flag = 1\n",
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert (
        facts.namespace_summary is facts.import_evidence.source_scan.namespace_summary
    )
    assert {
        (write.module, write.name) for write in facts.namespace_summary.foreign_writes
    } == {("pkg.target", "flag")}


def test_intrinsic_relationships_do_not_turn_dynamic_metadata_into_static_edges(
    tmp_path: Path,
) -> None:
    owner = tmp_path / "owner.py"
    helper = tmp_path / "_helper.py"
    guessed = tmp_path / "_guessed.py"
    wrapper = tmp_path / "wrapper.py"
    owner.write_text(
        "from _intrinsics import require_intrinsic\n"
        "READY = require_intrinsic('molt_capabilities_has')\n"
        "from pkg import _helper\n"
        "FILENAME = '_guessed.py'\n"
        "__package__ = unknown_package()\n"
        "from . import _guessed\n",
        encoding="utf-8",
    )
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    guessed.write_text("VALUE = 2\n", encoding="utf-8")
    wrapper.write_text(
        "__package__ = unknown_package()\nfrom . import owner\n", encoding="utf-8"
    )
    evidence = stdlib_module_import_evidence(
        "pkg.owner", owner, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert "pkg._helper" in evidence.proven_modules
    assert "pkg._guessed" not in evidence.proven_modules
    assert len(evidence.unresolved_sites) == 1
    assert evidence.unresolved_sites[0][2].requires_runtime
    with pytest.raises(
        UnresolvedStaticImportError, match=r"pkg.owner.*runtime import custody"
    ):
        stdlib_module_static_imports(
            "pkg.owner", owner, target_python=_DEFAULT_TARGET_PYTHON_VERSION
        )
    classification = classify_stdlib_module_statuses(
        {
            "pkg.owner": owner,
            "pkg._helper": helper,
            "pkg._guessed": guessed,
            "pkg.wrapper": wrapper,
        },
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    statuses = classification.statuses
    assert {site["module"] for site in classification.unresolved_imports_payload()} == {
        "pkg.owner",
        "pkg.wrapper",
    }
    assert statuses["pkg.owner"] == STATUS_INTRINSIC
    assert statuses["pkg._helper"] == STATUS_INTRINSIC_SUPPORT
    assert statuses["pkg._guessed"] == STATUS_PYTHON_COMPILED
    assert statuses["pkg.wrapper"] == STATUS_PYTHON_COMPILED


def test_filename_literal_is_not_an_intrinsic_dependency(tmp_path: Path) -> None:
    owner, helper = tmp_path / "owner.py", tmp_path / "_helper.py"
    owner.write_text(
        "from _intrinsics import require_intrinsic\n"
        "READY = require_intrinsic('molt_capabilities_has')\n"
        "DOCUMENTATION_EXAMPLE = '_helper.py'\n",
        encoding="utf-8",
    )
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    classification = classify_stdlib_module_statuses(
        {"pkg.owner": owner, "pkg._helper": helper},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert classification.statuses["pkg.owner"] == STATUS_INTRINSIC
    assert classification.statuses["pkg._helper"] == STATUS_PYTHON_COMPILED
    assert (
        "pkg._helper" not in classification.import_evidence["pkg.owner"].proven_modules
    )


def test_real_importlib_intrinsic_evidence_preserves_unresolved_machinery_site() -> (
    None
):
    stdlib = Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib"
    graph = {
        "importlib": stdlib / "importlib" / "__init__.py",
        "importlib.util": stdlib / "importlib" / "util.py",
        "importlib.machinery": stdlib / "importlib" / "machinery.py",
    }
    evidence = stdlib_module_import_evidence(
        "importlib.machinery",
        graph["importlib.machinery"],
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert any(
        request.level == 1 and request.fromlist == ("util",) and plan.requires_runtime
        for _line, request, plan in evidence.unresolved_sites
    )
    assert "importlib.util" not in evidence.proven_modules
    classification = classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert all(
        status == STATUS_INTRINSIC for status in classification.statuses.values()
    )
    assert any(
        site["module"] == "importlib.machinery" and site["fromlist"] == ["util"]
        for site in classification.unresolved_imports_payload()
    )


@pytest.mark.parametrize("source", [None, "def broken(:\n"])
def test_unavailable_intrinsic_source_never_becomes_empty_evidence(
    tmp_path: Path, source: str | None
) -> None:
    path = tmp_path / "broken.py"
    if source is not None:
        path.write_text(source, encoding="utf-8")
    with pytest.raises(
        UnresolvedStaticImportError, match=r"pkg.broken.*Python 3.12.*cannot"
    ):
        classify_stdlib_module_statuses(
            {"pkg.broken": path}, target_python=_DEFAULT_TARGET_PYTHON_VERSION
        )


def _classify_sources(
    tmp_path: Path, sources: dict[str, str]
) -> StdlibIntrinsicClassification:
    graph: dict[str, Path] = {}
    for module, source in sources.items():
        path = tmp_path / f"{module}.py"
        path.write_text(source, encoding="utf-8")
        graph[module] = path
    return classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )


_INTRINSIC_OWNER = (
    "from _intrinsics import require_intrinsic\n"
    "Value = require_intrinsic('molt_capabilities_has')\n"
)


@pytest.mark.parametrize(
    "provider_source",
    [
        None,
        "class Value: pass\n",
        "raise ImportError('not supported')\n",
        "from _intrinsics import require_intrinsic\n"
        "_VALUE = require_intrinsic('molt_capabilities_has')\n",
        "import stream\nclass Value: pass\n",
    ],
    ids=["missing", "python", "policy", "unread", "cycle"],
)
def test_python_implementation_does_not_inherit_provider_status(
    tmp_path: Path, provider_source: str | None
) -> None:
    sources = {"stream": "import _stream\nclass Stream(_stream.Value): pass\n"}
    if provider_source is not None:
        sources["_stream"] = provider_source
    classification = _classify_sources(tmp_path, sources)

    assert classification.statuses["stream"] == STATUS_PYTHON_COMPILED


@pytest.mark.parametrize(
    "wrapper,provider",
    [
        ("stream", "_stream"),
        ("pkg.stream", "pkg.owner"),
        ("pkg.stream", "pkg.child.owner"),
        ("stream", "_stream_extra"),
        ("stream", "__stream"),
        ("stream", "_stream.child"),
        ("stream", "other._stream"),
        ("_stream", "__stream"),
        ("_stream", "stream"),
        ("pkg.stream", "_pkg"),
    ],
)
def test_imports_do_not_change_python_implementation_ownership(
    tmp_path: Path, wrapper: str, provider: str
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            wrapper: f"import {provider} as owner\nclass Stream(owner.Value): pass\n",
            provider: _INTRINSIC_OWNER,
        },
    )

    assert classification.statuses[wrapper] == STATUS_PYTHON_COMPILED


def test_private_provider_requires_a_proved_import_edge(tmp_path: Path) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "stream": (
                "__package__ = choose_package()\nfrom . import _stream\n"
                "class Stream(_stream.Value): pass\n"
            ),
            "_stream": _INTRINSIC_OWNER,
        },
    )

    assert classification.statuses["stream"] == STATUS_PYTHON_COMPILED
    assert "_stream" not in classification.import_evidence["stream"].proven_modules
    assert classification.unresolved_imports_payload()


def test_private_provider_cannot_bypass_pure_facade_all_owner_proof(
    tmp_path: Path,
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "stream": "from _stream import Value\nfrom other import Other\n",
            "_stream": _INTRINSIC_OWNER,
            "other": "class Other: pass\n",
        },
    )

    assert classification.statuses["stream"] == STATUS_PYTHON_COMPILED
    assert classification.import_evidence["stream"].facade.owners == {
        "_stream",
        "other",
    }


def test_real_io_abc_wrapper_keeps_python_implementation_ownership() -> None:
    stdlib = Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib"
    classification = classify_stdlib_module_statuses(
        {name: stdlib / f"{name}.py" for name in ("io", "_io", "abc")},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert classification.statuses["io"] == STATUS_PYTHON_COMPILED
    assert classification.statuses["_io"] == STATUS_INTRINSIC
    assert "_io" in classification.import_evidence["io"].proven_modules


def test_facade_uses_explicit_owner_not_fromlist_candidate(
    tmp_path: Path,
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "_facade": (
                '"""Forward the implementation without a marker load."""\n'
                "from __future__ import annotations\n"
                "from owner import Value as PublicValue, Other\n"
                "__all__ = ('Other', 'PublicValue')\n"
            ),
            "owner": _INTRINSIC_OWNER,
        },
    )
    assert classification.statuses["_facade"] == STATUS_INTRINSIC_SUPPORT
    evidence = classification.import_evidence["_facade"]
    assert "owner.Value" in evidence.proven_modules
    facade = evidence.facade
    assert facade is not None and facade.resolved
    assert facade.owners == frozenset({"owner"})
    assert classification.facades_payload() == [
        {
            "module": "_facade",
            "path": str(tmp_path / "_facade.py"),
            "status": STATUS_INTRINSIC_SUPPORT,
            "reason": "pure-reexport",
            "resolved": True,
            "owners": ["owner"],
            "bindings": [
                {
                    "export_name": "PublicValue",
                    "owner_module": "owner",
                    "imported_name": "Value",
                    "imported_module": None,
                    "line": 3,
                },
                {
                    "export_name": "Other",
                    "owner_module": "owner",
                    "imported_name": "Other",
                    "imported_module": None,
                    "line": 3,
                },
            ],
        }
    ]
    with pytest.raises(FrozenInstanceError):
        facade.bindings[0].owner_module = "other"  # type: ignore[misc]
    with pytest.raises(FrozenInstanceError):
        facade.bindings = ()  # type: ignore[misc]
    with pytest.raises(TypeError):
        classification.import_evidence["_facade"] = evidence  # type: ignore[index]
    payload = classification.facades_payload()
    payload_owners = payload[0]["owners"]
    assert isinstance(payload_owners, list)
    payload_owners.append("invented")
    assert facade.owners == frozenset({"owner"})


@pytest.mark.parametrize(
    "facade_source",
    [
        "from owner import Value\n__all__ = ['Value']\n",
        "from owner import Value as Renamed\n__all__ = ['Renamed']\n",
        "from . import Value\n__all__ = ['Value']\n",
    ],
)
@pytest.mark.parametrize(
    ("child_source", "expected"),
    [
        ("VALUE = 1\n", STATUS_PYTHON_COMPILED),
        ("raise ImportError('reserved')\n", STATUS_PYTHON_COMPILED),
        (
            "_VALUE = require_intrinsic('molt_capabilities_has')\n",
            STATUS_PYTHON_COMPILED,
        ),
        (_INTRINSIC_OWNER, STATUS_INTRINSIC_SUPPORT),
    ],
)
def test_real_imported_child_is_required_alongside_explicit_owner(
    tmp_path: Path, facade_source: str, child_source: str, expected: str
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "owner._facade": facade_source,
            # Defines Value, but importing owner.Value can replace that binding.
            "owner": _INTRINSIC_OWNER,
            "owner.Value": child_source,
        },
    )
    assert classification.statuses["owner._facade"] == expected
    facade = classification.import_evidence["owner._facade"].facade
    assert facade is not None
    assert facade.owners == {"owner", "owner.Value"}
    assert facade.bindings[0].owner_module == "owner"
    assert facade.bindings[0].imported_module == "owner.Value"
    row = classification.facades_payload()[0]
    assert row["owners"] == ["owner", "owner.Value"]
    assert row["reason"] == (
        "pure-reexport" if expected == STATUS_INTRINSIC_SUPPORT else None
    )


def test_facade_waits_for_real_child_support_closure(tmp_path: Path) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "_facade": "from owner import _child as Child\n__all__ = ['Child']\n",
            "owner._child": "from implementation import Value\n__all__ = ['Value']\n",
            "owner": _INTRINSIC_OWNER,
            "implementation": _INTRINSIC_OWNER,
        },
    )
    assert classification.statuses["_facade"] == STATUS_INTRINSIC_SUPPORT
    assert classification.statuses["owner._child"] == STATUS_INTRINSIC_SUPPORT
    facade = classification.import_evidence["_facade"].facade
    assert facade is not None and facade.owners == {"owner", "owner._child"}


def test_real_child_without_python_status_cannot_establish_facade_support(
    tmp_path: Path,
) -> None:
    owner, facade_path = tmp_path / "owner.py", tmp_path / "_facade.py"
    owner.write_text(_INTRINSIC_OWNER, encoding="utf-8")
    facade_path.write_text(
        "from owner import Value\n__all__ = ['Value']\n", encoding="utf-8"
    )
    classification = classify_stdlib_module_statuses(
        {
            "owner": owner,
            "_facade": facade_path,
            "owner.Value": tmp_path / "Value.pyd",
        },
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert "owner.Value" not in classification.statuses
    assert classification.statuses["_facade"] == STATUS_PYTHON_COMPILED
    facade = classification.import_evidence["_facade"].facade
    assert facade is not None and facade.owners == {"owner", "owner.Value"}


def test_real_child_facade_cycle_cannot_use_intrinsic_parent_as_anchor(
    tmp_path: Path,
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "_facade": "from owner import _child as Child\n__all__ = ['Child']\n",
            "owner._child": "from _facade import Child\n__all__ = ['Child']\n",
            "owner": _INTRINSIC_OWNER,
        },
    )
    assert classification.statuses["_facade"] == STATUS_PYTHON_COMPILED
    assert classification.statuses["owner._child"] == STATUS_PYTHON_COMPILED


def test_facade_chains_close_independent_of_graph_order(tmp_path: Path) -> None:
    sources = {
        "_outer": "from _inner import PublicValue\n__all__ = ['PublicValue']\n",
        "_inner": "from owner import Value as PublicValue\n__all__ = ['PublicValue']\n",
        "owner": _INTRINSIC_OWNER,
    }
    for ordered in (sources, dict(reversed(list(sources.items())))):
        classification = _classify_sources(tmp_path, ordered)
        assert dict(classification.statuses) == {
            "_outer": STATUS_INTRINSIC_SUPPORT,
            "_inner": STATUS_INTRINSIC_SUPPORT,
            "owner": STATUS_INTRINSIC,
        }


def test_facade_accepts_multiple_intrinsic_owners(tmp_path: Path) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "_facade": (
                "from first import Value\n"
                "from second import Value as Other\n"
                "__all__ = ['Value', 'Other']\n"
            ),
            "first": _INTRINSIC_OWNER,
            "second": _INTRINSIC_OWNER,
        },
    )
    assert classification.statuses["_facade"] == STATUS_INTRINSIC_SUPPORT
    facade = classification.import_evidence["_facade"].facade
    assert facade is not None
    assert facade.owners == {"first", "second"}


@pytest.mark.parametrize(
    ("source", "status"),
    [
        ("Value = 1\n", STATUS_PYTHON_COMPILED),
        ("raise ImportError('reserved')\n", STATUS_POLICY_GATE),
        (
            "_unread = require_intrinsic('molt_capabilities_has')\n",
            STATUS_PYTHON_COMPILED,
        ),
        (None, None),
    ],
)
@pytest.mark.parametrize("prefix", ["", "pkg."])
def test_facade_mixed_owners_fail_closed_even_in_same_package(
    tmp_path: Path, source: str | None, status: str | None, prefix: str
) -> None:
    sources = {
        f"{prefix}_facade": (
            f"from {prefix}owner import Value\n"
            f"from {prefix}other import Other\n"
            "__all__ = ['Value', 'Other']\n"
        ),
        f"{prefix}owner": _INTRINSIC_OWNER,
    }
    if source is not None:
        sources[f"{prefix}other"] = source
    classification = _classify_sources(tmp_path, sources)
    assert classification.statuses[f"{prefix}_facade"] == STATUS_PYTHON_COMPILED
    assert classification.statuses.get(f"{prefix}other") == status
    assert classification.facades_payload()[0]["reason"] is None


@pytest.mark.parametrize("prefix", ["", "pkg."])
def test_facade_cycle_cannot_seed_itself_or_use_reverse_support(
    tmp_path: Path, prefix: str
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            f"{prefix}_first": (
                f"from {prefix}_second import Value\n__all__ = ['Value']\n"
            ),
            f"{prefix}_second": (
                f"from {prefix}_first import Value\n"
                f"from {prefix}owner import Value as Other\n"
                "__all__ = ['Value', 'Other']\n"
            ),
            # In the package case the old reverse-support rule could seed the
            # cycle from this unrelated owner's import of the facade.
            f"{prefix}owner": _INTRINSIC_OWNER + f"import {prefix}_first\n",
        },
    )
    assert classification.statuses[f"{prefix}_first"] == STATUS_PYTHON_COMPILED
    assert classification.statuses[f"{prefix}_second"] == STATUS_PYTHON_COMPILED
    assert classification.statuses[f"{prefix}owner"] == STATUS_INTRINSIC


@pytest.mark.parametrize("owner_source", [None, "Value = 1\n"])
def test_intrinsic_fromlist_candidate_cannot_replace_missing_or_python_owner(
    tmp_path: Path, owner_source: str | None
) -> None:
    sources = {
        "_facade": "from owner import Value\n__all__ = ['Value']\n",
        "owner.Value": _INTRINSIC_OWNER,
    }
    if owner_source is not None:
        sources["owner"] = owner_source
    classification = _classify_sources(tmp_path, sources)
    assert classification.statuses["_facade"] == STATUS_PYTHON_COMPILED


def test_facade_relative_owner_uses_shared_import_context(
    tmp_path: Path,
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "pkg._facade": "from .owner import Value\n__all__ = ['Value']\n",
            "pkg.owner": _INTRINSIC_OWNER,
        },
    )
    assert classification.statuses["pkg._facade"] == STATUS_INTRINSIC_SUPPORT
    facade = classification.import_evidence["pkg._facade"].facade
    assert facade is not None and facade.owners == {"pkg.owner"}
    assert classification.unresolved_imports_payload() == []


@pytest.mark.parametrize(
    ("source", "owners", "error", "owner_writes_metadata"),
    [
        pytest.param(
            "from .owner import Value\n"
            "from ...other import Other\n"
            "__all__ = ['Value', 'Other']\n",
            ("pkg.owner", None),
            "unknown_package",
            True,
            id="prior-import-writes-package",
        ),
        pytest.param(
            "from .owner import Value\n"
            "from ...other import Other\n"
            "__all__ = ['Value', 'Other']\n",
            ("pkg.owner", None),
            "beyond_top",
            False,
            id="closed-graph-import-preserves-package",
        ),
        pytest.param(
            "from ...other import Other\n"
            "from pkg.owner import Value\n"
            "__all__ = ['Value', 'Other']\n",
            (None, "pkg.owner"),
            "beyond_top",
            False,
            id="known-package-beyond-top",
        ),
    ],
)
def test_unresolved_facade_owner_does_not_fall_through_to_resolved_sibling(
    tmp_path: Path,
    source: str,
    owners: tuple[str | None, ...],
    error: str,
    owner_writes_metadata: bool,
) -> None:
    # A prior import may change this namespace when graph source writes it.
    # Without that writer, the same prior import preserves the known package.
    # An absolute sibling remains resolved even after a relative-owner failure.
    owner = _INTRINSIC_OWNER
    if owner_writes_metadata:
        owner += (
            "import sys\n"
            "sys.modules['pkg._facade'].__package__ = 'different.package.root'\n"
        )
    classification = _classify_sources(
        tmp_path,
        {"pkg._facade": source, "pkg.owner": owner},
    )
    assert classification.statuses["pkg._facade"] == STATUS_PYTHON_COMPILED
    facade = classification.import_evidence["pkg._facade"].facade
    assert facade is not None and not facade.resolved
    assert tuple(binding.owner_module for binding in facade.bindings) == owners
    assert classification.unresolved_imports_payload()[0]["errors"] == [error]


@pytest.mark.parametrize(
    "source",
    [
        "from owner import *\n__all__ = ['Value']\n",
        "import owner\n__all__ = ['owner']\n",
        "from owner import Value\n__all__ = []\n",
        "from owner import Value\n__all__ = ['Value', 'Value']\n",
        "from owner import Value, Hidden\n__all__ = ['Value']\n",
        "from owner import Value\n__all__ = ['Missing']\n",
        "from owner import Value\n__all__ = list(('Value',))\n",
        "from owner import Value\n__all__ = [Value]\n",
        "from owner import Value\n__all__ = ['Value'] + []\n",
        "from owner import Value\n__all__ = alias = ['Value']\n",
        "from owner import Value\n__all__: list[str] = ['Value']\n",
        "from owner import Value\n__all__ = ['Value']\n__all__.append('Other')\n",
        "from owner import Value\nValue = 1\n__all__ = ['Value']\n",
        "from owner import Value\ndel Value\n__all__ = ['Value']\n",
        "from owner import Value\nValue()\n__all__ = ['Value']\n",
        "from owner import Value\nValue.attr = 1\n__all__ = ['Value']\n",
        "from owner import Value\nValue += 1\n__all__ = ['Value']\n",
        "from owner import Value\ndef hidden(): pass\n__all__ = ['Value']\n",
        "from owner import Value\nclass Hidden: pass\n__all__ = ['Value']\n",
        "if True:\n from owner import Value\n__all__ = ['Value']\n",
        (
            "try:\n from owner import Value\nexcept ImportError:\n pass\n"
            "__all__ = ['Value']\n"
        ),
        (
            "from owner import Value\nfrom other import Other as Value\n"
            "__all__ = ['Value']\n"
        ),
        "from owner import Value as __package__\n__all__ = ['__package__']\n",
        "from owner import Value\n__package__ = 'owner'\n__all__ = ['Value']\n",
        (
            "from __future__ import annotations as Value\nfrom owner import Value\n"
            "__all__ = ['Value']\n"
        ),
        (
            "from __future__ import annotations\n"
            "from owner import Value as annotations\n__all__ = ['annotations']\n"
        ),
    ],
)
def test_executable_or_incomplete_facade_shapes_are_not_private_forwarding(
    tmp_path: Path, source: str
) -> None:
    classification = _classify_sources(
        tmp_path, {"_facade": source, "owner": _INTRINSIC_OWNER}
    )
    assert classification.statuses["_facade"] == STATUS_PYTHON_COMPILED
    assert classification.import_evidence["_facade"].facade is None
    assert classification.facades_payload() == []


@pytest.mark.parametrize("name", ["facade", "_facade"])
@pytest.mark.parametrize(
    "source",
    [
        "from owner import Value\n",
        "from owner import Value\n__all__ = ['Value']\n",
        "from owner import *\nfrom owner import __all__ as __all__\n",
    ],
)
def test_cross_root_facade_uses_all_owner_proof_independent_of_spelling(
    tmp_path: Path,
    name: str,
    source: str,
) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            name: source,
            "owner": _INTRINSIC_OWNER,
        },
    )
    assert classification.statuses[name] == STATUS_INTRINSIC_SUPPORT
    assert classification.import_evidence[name].facade.owners == {"owner"}


@pytest.mark.parametrize("other_source", [None, "class Other: pass\n"])
def test_import_only_forwarding_cannot_hide_an_unproved_owner(tmp_path, other_source):
    sources = {
        "public": "from owner import *\nfrom other import __all__\n",
        "owner": _INTRINSIC_OWNER,
    }
    if other_source is not None:
        sources["other"] = other_source
    classification = _classify_sources(tmp_path, sources)
    assert classification.statuses["public"] == STATUS_PYTHON_COMPILED
    assert classification.import_evidence["public"].facade.owners == {"owner", "other"}


def test_star_facade_cannot_prove_a_later_relative_owner(tmp_path: Path) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "pkg.facade": "from owner import *\nfrom .child import Value\n",
            "owner": _INTRINSIC_OWNER
            + "__package__ = 'other'\n__all__ = ['__package__']\n",
            "pkg.child": _INTRINSIC_OWNER,
            "other.child": "Value = 1\n",
        },
    )
    assert classification.statuses["pkg.facade"] == STATUS_PYTHON_COMPILED
    evidence = classification.import_evidence["pkg.facade"]
    assert "pkg.child" not in evidence.proven_modules
    assert evidence.unresolved_sites
    assert evidence.facade is not None and not evidence.facade.resolved


def test_import_sites_distinguish_required_owner_from_attribute_candidate(tmp_path):
    path = tmp_path / "consumer.py"
    path.write_text(
        "try:\n    from pkg import Value, child\nexcept ImportError:\n    pass\n"
        "try:\n    import pkg.missing\nexcept ImportError:\n    pass\n",
        encoding="utf-8",
    )
    evidence = stdlib_module_import_evidence(
        "consumer", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    first, second = evidence.sites
    assert first.owner_plan.modules == ("pkg",)
    assert first.plan.modules == ("pkg", "pkg.Value", "pkg.child")
    assert second.owner_plan.modules == ("pkg.missing",)
    assert (first.line, second.line) == (2, 6)
    assert not first.unresolved and not second.unresolved
    assert evidence.source_path == path
    assert [site.source[-1] for site in evidence.source_scan.source_sites] == [
        "ImportFrom",
        "Import",
    ]


@pytest.mark.parametrize("except_word", ["except", "except*"])
def test_import_discovery_retains_eager_and_deferred_regions(tmp_path, except_word):
    path = tmp_path / "regions.py"
    path.write_text(
        "try:\n"
        "    import immediate\n"
        "    def deferred():\n"
        "        import later\n"
        "    class C:\n"
        "        import class_body\n"
        "        def method(self):\n"
        "            import method_body\n"
        f"{except_word} ImportError:\n"
        "    import handler\n"
        "else:\n"
        "    import otherwise\n"
        "finally:\n"
        "    import cleanup\n",
        encoding="utf-8",
    )
    evidence = stdlib_module_import_evidence(
        "regions", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    observed = {}
    for site in evidence.sites:
        assert site.request is not None
        observed[site.request.name] = site.line
    assert observed == {
        "immediate": 2,
        "later": 4,
        "class_body": 6,
        "method_body": 8,
        "handler": 10,
        "otherwise": 12,
        "cleanup": 14,
    }
    assert evidence.proven_modules == {
        "immediate",
        "later",
        "class_body",
        "method_body",
        "handler",
        "otherwise",
        "cleanup",
    }
    assert not evidence.unresolved_sites


def test_import_discovery_retains_nested_handler_and_finally_bodies(tmp_path):
    path = tmp_path / "regions.py"
    path.write_text(
        "try:\n"
        "    try:\n"
        "        import inner\n"
        "    except ImportError:\n"
        "        import recovery\n"
        "    finally:\n"
        "        import cleanup\n"
        "except Exception:\n"
        "    import outside\n",
        encoding="utf-8",
    )
    evidence = stdlib_module_import_evidence(
        "regions", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    observed = {}
    for site in evidence.sites:
        assert site.request is not None
        observed[site.request.name] = site.line
    assert observed == {
        "inner": 3,
        "recovery": 5,
        "cleanup": 7,
        "outside": 9,
    }
    assert evidence.proven_modules == {"inner", "recovery", "cleanup", "outside"}
    assert not evidence.unresolved_sites


def test_alias_normalizer_keeps_bootstrap_and_exact_missing_child_distinct(tmp_path):
    from molt.compiler_analysis.python_imports import normalize_stdlib_module_name
    from molt.cli.module_dependencies import _module_dependencies_from_imports

    assert normalize_stdlib_module_name("molt.stdlib") is None
    assert normalize_stdlib_module_name("molt.stdlib.pkg.missing") == "pkg.missing"
    assert normalize_stdlib_module_name("molt.stdlib_extra") == "molt.stdlib_extra"
    paths = {name: tmp_path / name for name in ("molt.stdlib", "pkg", "pkg.child")}
    assert _module_dependencies_from_imports(
        "consumer", paths, ("molt.stdlib.pkg.child",)
    ) == {"molt.stdlib", "pkg", "pkg.child"}


def test_status_worklist_preserves_unanchored_cycles_and_all_owner_requirement(
    tmp_path,
):
    sources = {
        "anchor": "from _intrinsics import require_intrinsic\n"
        "OP = require_intrinsic('molt_time_time')\nvalue = OP\n",
        "plain": "value = 1\n",
        "left": "from right import value\n",
        "right": "from left import value\n",
        "mixed": "from anchor import value\nfrom plain import value as other\n",
    }
    # A reverse-name chain previously forced one full owner rescan per hop.
    for index in range(80):
        owner = "anchor" if index == 79 else f"chain_{index + 1:02}"
        sources[f"chain_{index:02}"] = f"from {owner} import value\n"
    graph = {}
    for name, source in sources.items():
        graph[name] = tmp_path / f"{name}.py"
        graph[name].write_text(source, encoding="utf-8")
    statuses = classify_stdlib_module_statuses(
        dict(sorted(graph.items())), target_python=_DEFAULT_TARGET_PYTHON_VERSION
    ).statuses
    assert statuses["anchor"] == STATUS_INTRINSIC
    assert all(
        statuses[f"chain_{index:02}"] == STATUS_INTRINSIC_SUPPORT for index in range(80)
    )
    assert {statuses[name] for name in ("left", "right", "mixed")} == {
        STATUS_PYTHON_COMPILED
    }


@pytest.mark.parametrize(
    "continuation",
    [
        "except Alias:\n    pass\n",
        "except choose_handler():\n    pass\n",
        "except (ImportError, Alias):\n    pass\n",
        "finally:\n    cleanup()\n",
    ],
)
def test_import_custody_is_independent_of_exception_handler_expression(
    tmp_path, continuation
):
    path = tmp_path / "consumer.py"
    path.write_text("try:\n    import provider\n" + continuation, encoding="utf-8")
    evidence = stdlib_module_import_evidence(
        "consumer", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    [site] = evidence.sites
    assert site.line == 2
    assert site.owner_plan.modules == site.plan.modules == ("provider",)
    assert not site.unresolved
    assert evidence.source_path == path


@pytest.mark.parametrize(
    "source,kind,owners,candidates",
    [
        ("__import__('builtins')\n", "dunder_import", ("builtins",), ("builtins",)),
        ("__import__(unknown_name)\n", None, (), ()),
        (
            "__import__('pkg', fromlist=('child',))\n",
            "dunder_import",
            ("pkg",),
            ("pkg", "pkg.child"),
        ),
    ],
)
def test_literal_import_calls_use_bound_requests_and_unknown_calls_remain_obligations(
    tmp_path, source, kind, owners, candidates
):
    path = tmp_path / "consumer.py"
    path.write_text(source, encoding="utf-8")
    [site] = stdlib_module_import_evidence(
        "consumer", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    ).sites
    assert (site.request.kind if site.request is not None else None) == kind
    assert site.owner_plan.modules == owners
    assert site.plan.modules == candidates
    assert site.unresolved is (kind is None)


@pytest.mark.parametrize(
    "target,future",
    [((3, 12), False), ((3, 13), False), ((3, 14), False), ((3, 12), True)],
)
def test_annotation_regions_retain_owners_under_selected_execution_policy(
    target, future
):
    import ast
    from molt.compiler_analysis.python_lexical_scope import python_execution_regions

    source = ("from __future__ import annotations\n" if future else "") + (
        "try:\n"
        "    def f[T: bound()](x: annotation() = default()):\n"
        "        local: never_executed()\n"
        "        body()\n"
        "    class C[T: class_bound()](base()):\n"
        "        class_body()\n"
        "    type Alias[T: alias_bound()] = alias_body()\n"
        "except Exception:\n    pass\n"
    )
    calls = {
        region.node.func.id: region.owner is None
        for region in python_execution_regions(
            ast.parse(source), target_python=target, include_deferred=True
        )
        if isinstance(region.node, ast.Call) and isinstance(region.node.func, ast.Name)
    }
    assert calls["default"] and calls["base"] and calls["class_body"]
    assert not any(
        calls[name]
        for name in ("bound", "body", "class_bound", "alias_bound", "alias_body")
    )
    assert "never_executed" not in calls
    if future:
        assert "annotation" not in calls
    else:
        assert calls["annotation"] is (target < (3, 14))


def test_proven_import_owner_retains_runtime_warning_protocol(tmp_path):
    path = tmp_path / "consumer.py"
    path.write_text(
        "__package__ = None\n__spec__ = None\n__name__ = 'pkg.consumer'\n"
        "try:\n    from . import child\nexcept ImportError:\n    pass\n",
        encoding="utf-8",
    )
    evidence = stdlib_module_import_evidence(
        "consumer", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    [site] = evidence.sites
    assert site.owner_plan.modules == ("pkg",)
    assert site.plan.modules == ("pkg", "pkg.child")
    assert site.plan.requires_runtime_execution
    assert not site.unresolved
    assert stdlib_module_static_imports(
        "consumer", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    ) == frozenset({"pkg", "pkg.child"})


@pytest.fixture
def premise_source_cache(tmp_path, monkeypatch):
    from molt.cli import module_graph_cache

    monkeypatch.setattr(
        module_graph_cache, "_build_state_root", lambda _root: tmp_path / "cache"
    )
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "premise"
    )
    return module_graph_cache


def test_intrinsic_source_metadata_rebinds_facts_but_reuses_syntax_summary(
    tmp_path, monkeypatch, premise_source_cache
):
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    cache = premise_source_cache
    path = tmp_path / "entry.py"
    path.write_text(
        "from typing import TYPE_CHECKING\ncallback()\n"
        "if TYPE_CHECKING:\n    import leaf\n"
        "def mutate():\n    global value\n    value = 1\n",
        encoding="utf-8",
    )
    counts = {}
    stable = PythonNamespacePremise((), 0)
    rewritten = PythonNamespacePremise(("TYPE_CHECKING",), 0)

    def facts(premise):
        return cache._stdlib_intrinsic_source_facts(
            tmp_path,
            "entry",
            path,
            premise,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            operation_counts=counts,
        )

    for premise, expected in ((None, True), (stable, False), (rewritten, True)):
        cold = facts(premise)
        assert ("leaf" in cold.import_evidence.proven_modules) is expected
        assert facts(premise) == cold
    assert counts == {
        "intrinsic_source_requests": 6,
        "intrinsic_source_misses": 3,
        "intrinsic_source_hits": 3,
    }
    assert len(source_records(tmp_path, "intrinsic-facts")) == 1
    # A graph premise changes binding facts, never syntax. The warm syntax
    # projection therefore requires neither a guessed premise nor a new parse.
    original_parse = cache._parse_source_for_target
    monkeypatch.setattr(
        cache,
        "_parse_source_for_target",
        lambda *_args, **_kwargs: pytest.fail("warm namespace summary reparsed source"),
    )
    summary = cache._stdlib_intrinsic_namespace_summary(
        tmp_path, "entry", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert summary.own_names == frozenset({"value"})
    assert summary == cold.namespace_summary
    monkeypatch.setattr(cache, "_parse_source_for_target", original_parse)
    path.write_text(
        "def mutate():\n    global newer\n    newer = 2\n", encoding="utf-8"
    )
    changed = cache._stdlib_intrinsic_namespace_summary(
        tmp_path, "entry", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert changed.own_names == frozenset({"newer"})


@pytest.mark.parametrize(
    "field,invalid",
    [
        ("schema", True),
        ("schema", 3),  # Retired summary before private-name normalization.
        ("own_names", {}),
        ("own_names", ["duplicate", "duplicate"]),
        ("own_reflection_line", True),
        ("own_reflection_line", -1),
        ("own_reflection_line", 1),
        ("foreign_writes", [[None, "name", "attribute", 0, None]]),
        ("foreign_writes", [[True, "name", "attribute", 1, None]]),
        ("foreign_writes", [[None, 1, "attribute", 1, None]]),
        ("foreign_writes", [[None, "name", "unknown", 1, None]]),
        ("foreign_writes", [[None, "name", "attribute", 1]]),
        ("foreign_writes", [[None, "name", "attribute", 1, None]] * 2),
        ("foreign_writes", [[None, "name", "attribute", 1, []]]),
        ("foreign_writes", [[None, "name", "attribute", 1, True]]),
        ("foreign_writes", [[None, "name", "attribute", 1, {}]]),
        (
            "foreign_writes",
            [[None, "name", "attribute", 1, [True, 0, 1, 1, "FunctionDef"]]],
        ),
        ("foreign_writes", [[None, "name", "attribute", 1, [1, 0, 2, 1, "Call"]]]),
    ],
)
def test_malformed_cached_namespace_summary_recomputes_canonical_syntax(
    tmp_path, monkeypatch, premise_source_cache, field, invalid
):
    import copy

    cache = premise_source_cache
    path = tmp_path / "entry.py"
    path.write_text(
        "def mutate():\n    global retained\n    retained = 1\n", encoding="utf-8"
    )
    facts = cache._stdlib_intrinsic_source_facts(
        tmp_path, "entry", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    read = cache._read_intrinsic_facts_payload
    parse = cache._parse_source_for_target
    parses = []

    def corrupt(project_root, path, key):
        payload = copy.deepcopy(read(project_root, path, key))
        assert payload is not None
        payload["facts"]["namespace_summary"][field] = invalid
        return payload

    def counted(*args, **kwargs):
        parses.append(1)
        return parse(*args, **kwargs)

    monkeypatch.setattr(cache, "_read_intrinsic_facts_payload", corrupt)
    monkeypatch.setattr(cache, "_parse_source_for_target", counted)
    summary = cache._stdlib_intrinsic_namespace_summary(
        tmp_path, "entry", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert summary == facts.namespace_summary
    assert summary.own_names == frozenset({"retained"})
    assert parses == [1]


def test_intrinsic_warmer_transports_each_modules_namespace_premise(
    tmp_path, monkeypatch
):
    import molt.host_capacity
    from molt.cli import module_graph_cache as cache
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    # Actual spawned workers prove the frozen premise crosses the process
    # boundary and populates the same identity the serial reader will admit.
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setattr(molt.host_capacity, "worker_count", lambda **_kwargs: 2)
    monkeypatch.setattr(cache, "_INTRINSIC_FACTS_MODULES_PER_WORKER", 1)
    paths = {}
    for module in ("stable", "rewritten"):
        path = tmp_path / f"{module}.py"
        path.write_text(
            "from typing import TYPE_CHECKING\ncallback()\n"
            "if TYPE_CHECKING:\n    import leaf\n",
            encoding="utf-8",
        )
        paths[module] = path
    premises = {
        "stable": PythonNamespacePremise((), 0),
        "rewritten": PythonNamespacePremise(("TYPE_CHECKING",), 0),
    }
    cache.warm_stdlib_intrinsic_source_facts(
        tmp_path,
        paths,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        namespace_premises=premises,
    )
    counts = {}
    for module, path in paths.items():
        facts = cache._stdlib_intrinsic_source_facts(
            tmp_path,
            module,
            path,
            premises[module],
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            operation_counts=counts,
        )
        assert ("leaf" in facts.import_evidence.proven_modules) is (
            module == "rewritten"
        )
    assert counts == {"intrinsic_source_requests": 2, "intrinsic_source_hits": 2}


def test_intrinsic_metadata_rejects_boolean_namespace_member_mask(
    tmp_path, monkeypatch, premise_source_cache
):
    import copy
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise

    cache = premise_source_cache
    path = tmp_path / "entry.py"
    path.write_text("pass\n", encoding="utf-8")
    premise = PythonNamespacePremise((), 0)
    facts = cache._stdlib_intrinsic_source_facts(
        tmp_path,
        "entry",
        path,
        premise,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    read = cache._read_intrinsic_facts_payload

    def corrupt(project_root, path, key):
        payload = copy.deepcopy(read(project_root, path, key))
        assert payload is not None
        payload["identity"]["namespace_premise"]["member_writes"] = False
        return payload

    monkeypatch.setattr(cache, "_read_intrinsic_facts_payload", corrupt)
    counts = {}
    assert (
        cache._stdlib_intrinsic_source_facts(
            tmp_path,
            "entry",
            path,
            premise,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            operation_counts=counts,
        )
        == facts
    )
    assert counts == {
        "intrinsic_source_requests": 1,
        "intrinsic_source_rejected": 1,
        "intrinsic_source_misses": 1,
    }
