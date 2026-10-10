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


def test_public_wrapper_uses_exact_top_level_native_provider(tmp_path: Path) -> None:
    classification = _classify_sources(
        tmp_path,
        {
            "stream": "import _stream\nclass Stream(_stream.Value): pass\n",
            "_stream": _INTRINSIC_OWNER,
        },
    )

    assert classification.statuses["stream"] == STATUS_INTRINSIC
    assert classification.import_evidence["stream"].proven_modules == {"_stream"}
    assert classification.import_evidence["stream"].facade is None


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
def test_public_wrapper_requires_independently_backed_native_provider(
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
        ("stream", "_stream_extra"),
        ("stream", "__stream"),
        ("stream", "_stream.child"),
        ("stream", "other._stream"),
        ("_stream", "__stream"),
        ("_stream", "stream"),
        ("pkg.stream", "_pkg"),
    ],
)
def test_private_provider_relationship_is_exact_and_directed(
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


def test_real_io_abc_wrapper_inherits_native_provider_backing() -> None:
    stdlib = Path(__file__).resolve().parents[1] / "src" / "molt" / "stdlib"
    classification = classify_stdlib_module_statuses(
        {name: stdlib / f"{name}.py" for name in ("io", "_io", "abc")},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )

    assert classification.statuses["io"] == STATUS_INTRINSIC
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
    ("source", "owners", "error"),
    [
        pytest.param(
            "from .owner import Value\n"
            "from ...other import Other\n"
            "__all__ = ['Value', 'Other']\n",
            ("pkg.owner", None),
            "unknown_package",
            id="prior-import-taints-package",
        ),
        pytest.param(
            "from ...other import Other\n"
            "from pkg.owner import Value\n"
            "__all__ = ['Value', 'Other']\n",
            (None, "pkg.owner"),
            "beyond_top",
            id="known-package-beyond-top",
        ),
    ],
)
def test_unresolved_facade_owner_does_not_fall_through_to_resolved_sibling(
    tmp_path: Path, source: str, owners: tuple[str | None, ...], error: str
) -> None:
    # A prior import can mutate package metadata, so uncertainty precedes the
    # depth check. With no prior import, the known package proves beyond_top.
    # The absolute sibling in that case stays resolved despite import effects.
    classification = _classify_sources(
        tmp_path,
        {
            "pkg._facade": source,
            "pkg.owner": _INTRINSIC_OWNER,
        },
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
