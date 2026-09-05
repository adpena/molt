"""Pure selected-lock graph proofs; no uv install or interpreter subprocesses."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any

import pytest
from packaging.markers import default_environment

from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_uv_lock_identity import (
    environment_matches_lock_closure,
    selected_uv_lock_group_closure,
    validate_uv_lock_group_closure,
)


def _edge(name: str, *, extras: tuple[str, ...] = (), marker: str | None = None) -> str:
    fields = [f"name = {json.dumps(name)}"]
    if extras:
        fields.append(f"extra = {json.dumps(extras)}")
    if marker is not None:
        fields.append(f"marker = {json.dumps(marker)}")
    return "{ " + ", ".join(fields) + " }"


def _package(
    name: str,
    dependencies: tuple[str, ...] = (),
    optional: dict[str, tuple[str, ...]] | None = None,
    *,
    size: str = "1",
) -> str:
    filename = f"{name.replace('-', '_')}-1.0-py3-none-any.whl"
    result = (
        f"\n[[package]]\nname = {json.dumps(name)}\nversion = '1.0'\n"
        "source = { registry = 'https://example.invalid/simple' }\n"
        f"dependencies = [{', '.join(dependencies)}]\n"
        "wheels = [{ url = 'https://example.invalid/"
        f"{filename}', hash = 'sha256:{'a' * 64}', size = {size} }}]\n"
    )
    if optional:
        result += "[package.optional-dependencies]\n"
        result += "".join(
            f"{json.dumps(extra)} = [{', '.join(edges)}]\n"
            for extra, edges in optional.items()
        )
    return result


def _project(
    root: Path,
    *,
    requirements: tuple[str, ...] = ("alpha==1.0",),
    project_requirements: tuple[str, ...] = (),
    group_edges: tuple[str, ...] | None = None,
    project_edges: tuple[str, ...] = (),
    packages: str | None = None,
    lock_version: str = "1",
) -> tuple[str, ...]:
    (root / "pyproject.toml").write_text(
        "[project]\nname = 'fixture'\n"
        f"dependencies = {json.dumps(project_requirements)}\n"
        f"[dependency-groups]\nsource-build = {json.dumps(requirements)}\n",
        encoding="utf-8",
    )
    edges = (_edge("alpha"),) if group_edges is None else group_edges
    (root / "uv.lock").write_text(
        f"version = {lock_version}\nrevision = 1\nrequires-python = '>=3.12'\n"
        "[[package]]\nname = 'fixture'\nversion = '1.0'\n"
        "source = { editable = '.' }\n"
        f"dependencies = [{', '.join(project_edges)}]\n"
        f"[package.dev-dependencies]\nsource-build = [{', '.join(edges)}]\n"
        + (_package("alpha") if packages is None else packages),
        encoding="utf-8",
    )
    return requirements


def _select(root: Path, requirements: tuple[str, ...]) -> dict[str, Any]:
    return selected_uv_lock_group_closure(root, "source-build", requirements)


def _reseal(payload: dict[str, Any]) -> None:
    payload["closure_sha256"] = canonical_json_sha256(
        {key: value for key, value in payload.items() if key != "closure_sha256"}
    )


def test_sync_closure_includes_project_dependencies_and_late_extra_fixed_point(
    tmp_path: Path,
) -> None:
    requirements = _project(
        tmp_path,
        requirements=("late==1.0", "alpha==1.0", "absent; python_version < '3'"),
        project_requirements=("cli-base==1.0",),
        project_edges=(_edge("cli-base"),),
        group_edges=(
            _edge("late"),
            _edge("alpha"),
            _edge("absent", marker="python_version < '3'"),
        ),
        packages=(
            _package("cli-base")
            + _package("late", (_edge("alpha", extras=("fast",)),))
            + _package(
                "alpha",
                (_edge("conditional", marker="extra == 'fast'"),),
                {"fast": (_edge("leaf", extras=("nested",)),)},
            )
            + _package("conditional")
            + _package("leaf", optional={"nested": (_edge("alpha"),)})
        ),
    )
    closure = _select(tmp_path, requirements)
    rows = {row["name"]: row for row in closure["packages"]}
    assert set(rows) == {"alpha", "cli-base", "conditional", "late", "leaf"}
    assert rows["alpha"]["extras"] == ["fast"]
    assert rows["leaf"]["extras"] == ["nested"]
    assert rows["alpha"]["dependencies"] == [
        {"name": "conditional", "extras": [], "when_extra": "fast"},
        {"name": "leaf", "extras": ["nested"], "when_extra": "fast"},
    ]
    assert closure["project_requirements"] == ["cli-base==1.0"]
    actual = {"distributions": [{"name": name, "version": "1.0"} for name in rows]}
    assert environment_matches_lock_closure(actual, closure)
    actual["distributions"] = [
        row for row in actual["distributions"] if row["name"] != "cli-base"
    ]
    assert not environment_matches_lock_closure(actual, closure)


def test_project_requirement_extras_are_bound_to_selected_lock(tmp_path: Path) -> None:
    requirements = _project(
        tmp_path,
        project_requirements=("cli-base[fast]==1.0",),
        project_edges=(_edge("cli-base", extras=("fast",)),),
        packages=_package("alpha") + _package("cli-base", optional={"fast": ()}),
    )
    closure = _select(tmp_path, requirements)
    assert closure["packages"][1]["extras"] == ["fast"]
    closure["packages"][1]["extras"] = []
    _reseal(closure)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="missing package or extra"
    ):
        validate_uv_lock_group_closure(closure)


def test_requested_extra_with_no_optional_lock_edges_remains_grounded(
    tmp_path: Path,
) -> None:
    requirements = _project(
        tmp_path,
        requirements=("alpha[empty]==1.0",),
        group_edges=(_edge("alpha", extras=("empty",)),),
    )
    closure = _select(tmp_path, requirements)
    assert closure["packages"][0]["extras"] == ["empty"]
    assert closure["packages"][0]["dependencies"] == []


@pytest.mark.parametrize(
    "mutation", ["orphan", "extra-cycle", "missing-edge", "duplicate-edge"]
)
def test_resealed_graph_cannot_gain_ungrounded_packages_or_extras(
    tmp_path: Path,
    mutation: str,
) -> None:
    closure = _select(tmp_path, _project(tmp_path))
    alpha = closure["packages"][0]
    if mutation == "orphan":
        orphan = copy.deepcopy(alpha)
        orphan["name"] = "orphan"
        orphan["artifact"]["filename"] = "orphan-1.0-py3-none-any.whl"
        closure["packages"].append(orphan)
    elif mutation == "extra-cycle":
        alpha["extras"] = ["fast"]
        alpha["dependencies"] = [
            {"name": "alpha", "extras": ["fast"], "when_extra": "fast"}
        ]
    elif mutation == "missing-edge":
        alpha["dependencies"] = [{"name": "absent", "extras": [], "when_extra": ""}]
    else:
        edge = {"name": "alpha", "extras": [], "when_extra": ""}
        alpha["dependencies"] = [edge, copy.deepcopy(edge)]
    _reseal(closure)
    with pytest.raises(PythonEnvironmentIdentityError):
        validate_uv_lock_group_closure(closure)


@pytest.mark.parametrize(
    "malformation",
    [
        "lock-bool",
        "size-bool",
        "version-number",
        "source-string",
        "extras-string",
        "marker-type",
    ],
)
def test_selector_rejects_malformed_authority_before_returning_recipe(
    tmp_path: Path,
    malformation: str,
) -> None:
    edge = _edge("alpha")
    if malformation == "version-number":
        edge = "{ name = 'alpha', version = 1 }"
    elif malformation == "source-string":
        edge = "{ name = 'alpha', source = 'registry' }"
    elif malformation == "extras-string":
        edge = "{ name = 'alpha', extra = 'fast' }"
    elif malformation == "marker-type":
        edge = "{ name = 'alpha', marker = true }"
    requirements = _project(
        tmp_path,
        lock_version="true" if malformation == "lock-bool" else "1",
        group_edges=(edge,),
        packages=_package("alpha", size="true" if malformation == "size-bool" else "1"),
    )
    with pytest.raises(PythonEnvironmentIdentityError):
        _select(tmp_path, requirements)


def test_selector_rejects_stale_frozen_requirement_before_returning_recipe(
    tmp_path: Path,
) -> None:
    requirements = _project(tmp_path, requirements=("alpha==2.0",))
    with pytest.raises(PythonEnvironmentIdentityError, match="does not satisfy"):
        _select(tmp_path, requirements)


def test_selector_rejects_non_string_marker_environment(tmp_path: Path) -> None:
    requirements = _project(tmp_path)
    environment: dict[str, Any] = dict(default_environment())
    environment["platform_version"] = 1
    with pytest.raises(
        PythonEnvironmentIdentityError, match="marker environment shape"
    ):
        selected_uv_lock_group_closure(
            tmp_path, "source-build", requirements, marker_environment=environment
        )


def test_all_inactive_roots_have_an_empty_realized_closure(tmp_path: Path) -> None:
    requirements = _project(
        tmp_path,
        requirements=("alpha; python_version < '3'",),
        project_requirements=("cli-base; python_version < '3'",),
        project_edges=(_edge("cli-base", marker="python_version < '3'"),),
        group_edges=(_edge("alpha", marker="python_version < '3'"),),
    )
    closure = _select(tmp_path, requirements)
    assert closure["packages"] == []
    assert environment_matches_lock_closure({"distributions": []}, closure)
