from __future__ import annotations

from pathlib import Path

import pytest

from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
from molt.stdlib_intrinsic_policy import (
    STATUS_INTRINSIC,
    STATUS_INTRINSIC_SUPPORT,
    STATUS_PYTHON_ONLY,
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
        "READY = require_intrinsic('molt_import_smoke_runtime_ready')\n"
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
    assert statuses["pkg._guessed"] == STATUS_PYTHON_ONLY
    assert statuses["pkg.wrapper"] == STATUS_PYTHON_ONLY


def test_filename_literal_is_not_an_intrinsic_dependency(tmp_path: Path) -> None:
    owner, helper = tmp_path / "owner.py", tmp_path / "_helper.py"
    owner.write_text(
        "from _intrinsics import require_intrinsic\n"
        "READY = require_intrinsic('molt_import_smoke_runtime_ready')\n"
        "DOCUMENTATION_EXAMPLE = '_helper.py'\n",
        encoding="utf-8",
    )
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    classification = classify_stdlib_module_statuses(
        {"pkg.owner": owner, "pkg._helper": helper},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert classification.statuses["pkg.owner"] == STATUS_INTRINSIC
    assert classification.statuses["pkg._helper"] == STATUS_PYTHON_ONLY
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
