"""Pure selected-lock graph proofs; no uv install or interpreter subprocesses."""

from __future__ import annotations

import copy
import json
import tomllib
from pathlib import Path
from typing import Any

import pytest
from packaging.markers import default_environment
from tests.python_environment_test_support import wheel_target_manifest

from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_uv_lock_identity import (
    _locked_artifact,
    environment_matches_lock_closure,
    selected_uv_lock_group_closure,
    validate_uv_lock_group_closure,
)


def _wheel(filename: str, digest: str = "a") -> dict[str, object]:
    return {
        "url": f"https://example.invalid/{filename}",
        "hash": f"sha256:{digest * 64}",
        "size": 123,
    }


@pytest.mark.parametrize("reader_minor", ["312", "313"])
@pytest.mark.parametrize(
    "abi,platform",
    [
        ("cp313", "macosx_11_0_arm64"),
        ("cp313t", "win_arm64"),
        ("cp313", "manylinux_2_17_x86_64"),
    ],
)
def test_selected_target_wheel_is_independent_of_reader_tags(
    monkeypatch, reader_minor, abi, platform
):
    from packaging.tags import Tag

    monkeypatch.setattr(
        "packaging.tags.sys_tags",
        lambda: iter([Tag("cp" + reader_minor, "cp" + reader_minor, "win_amd64")]),
    )
    expected = f"msgpack-1.2.3-cp313-{abi}-{platform}.whl"
    row = {
        "name": "msgpack",
        "version": "1.2.3",
        "wheels": [
            _wheel("msgpack-1.2.3-cp312-cp312-win_amd64.whl", "b"),
            _wheel(expected, "c"),
        ],
    }
    assert _locked_artifact(row, [f"cp313-{abi}-{platform}"]) == {
        "filename": expected,
        "sha256": "c" * 64,
        "size": 123,
    }
    with pytest.raises(PythonEnvironmentIdentityError, match="no wheel"):
        _locked_artifact(row, ["cp314-cp314t-win_arm64"])


def test_installed_wheel_tags_and_runtime_must_match_recipe(tmp_path):
    closure = _select(tmp_path, _project(tmp_path))
    actual = {
        "runtime": {
            "runtime_closure_sha256": closure["wheel_target"]["runtime_closure_sha256"]
        },
        "distributions": [
            {
                "name": "alpha",
                "version": "1.0",
                "wheel": {"tags": ["py3-none-any"], "build": None},
            }
        ],
    }
    assert environment_matches_lock_closure(actual, closure)
    actual["distributions"][0]["wheel"]["tags"] = ["cp312-cp312-win_amd64"]
    assert not environment_matches_lock_closure(actual, closure)
    actual["distributions"][0]["wheel"]["tags"] = ["py3-none-any"]
    actual["distributions"][0]["wheel"]["build"] = "1"
    assert not environment_matches_lock_closure(actual, closure)
    actual["distributions"][0]["wheel"]["build"] = None
    actual["runtime"]["runtime_closure_sha256"] = "0" * 64
    assert not environment_matches_lock_closure(actual, closure)


def test_installed_compressed_wheel_may_record_narrower_supported_tags(tmp_path):
    closure = _select(tmp_path, _project(tmp_path))
    closure["packages"][0]["artifact"]["filename"] = "alpha-1.0-py2.py3-none-any.whl"
    actual = {
        "runtime": {
            "runtime_closure_sha256": closure["wheel_target"]["runtime_closure_sha256"]
        },
        "distributions": [
            {
                "name": "alpha",
                "version": "1.0",
                "wheel": {"tags": ["py3-none-any"], "build": None},
            }
        ],
    }
    assert environment_matches_lock_closure(actual, closure)
    actual["distributions"][0]["wheel"]["tags"] = ["py2-none-any"]
    assert not environment_matches_lock_closure(actual, closure)


def test_tag_capture_executes_bound_packaging_source_not_imported_parent(monkeypatch):
    from packaging.tags import sys_tags
    from molt.python_file_node_custody import PythonFileCaptureContext
    from molt.python_uv_lock_identity import (
        capture_python_wheel_target,
        packaging_tag_source,
    )
    from tests.python_environment_test_support import runtime_identity_manifest

    expected = [str(tag) for tag in sys_tags()]
    monkeypatch.setattr(
        "packaging.tags.sys_tags",
        lambda: (_ for _ in ()).throw(AssertionError("parent tag implementation used")),
    )
    runtime = runtime_identity_manifest()
    with PythonFileCaptureContext() as context:
        target = capture_python_wheel_target(packaging_tag_source(), runtime, context)
        context.verify()
    assert target["tags"] == expected
    assert target["runtime_closure_sha256"] == runtime["runtime_closure_sha256"]
    assert set(target["packaging_sources"]) == {
        "__init__.py",
        "tags.py",
        "_manylinux.py",
        "_musllinux.py",
        "_elffile.py",
    }


@pytest.mark.parametrize("failure", ["source-change", "algorithm"])
def test_tag_capture_source_custody_and_private_namespace_cleanup(
    tmp_path, monkeypatch, failure
):
    import shutil
    import sys
    from molt.python_file_node_custody import PythonFileCaptureContext, _FileNodePool
    from molt.python_uv_lock_identity import (
        capture_python_wheel_target,
        packaging_tag_source,
    )
    from tests.python_environment_test_support import runtime_identity_manifest

    for name in ("__init__", "_elffile", "_manylinux", "_musllinux", "tags"):
        shutil.copyfile(
            packaging_tag_source() / (name + ".py"), tmp_path / (name + ".py")
        )
    target = tmp_path / "tags.py"
    if failure == "algorithm":
        target.write_text(
            "raise RuntimeError('independent algorithm failure')\n", encoding="utf-8"
        )
    else:
        original = _FileNodePool.read_bound

        def read(pool, node, *, label):
            contents = original(pool, node, label=label)
            if b"def sys_tags(" in contents:
                target.write_bytes(contents + b"\n# changed after binding\n")
            return contents

        monkeypatch.setattr(_FileNodePool, "read_bound", read)
    with pytest.raises(
        (PythonEnvironmentIdentityError, RuntimeError, ValueError)
    ) as caught:
        with PythonFileCaptureContext() as context:
            capture_python_wheel_target(tmp_path, runtime_identity_manifest(), context)
            context.verify()
    if failure == "algorithm":
        assert str(caught.value) == "independent algorithm failure"
    assert not any(
        name == "_molt_selected_wheel_packaging"
        or name.startswith("_molt_selected_wheel_packaging.")
        for name in sys.modules
    )


@pytest.mark.parametrize("reverse", [False, True])
def test_locked_compressed_platform_tie_retains_uv_lock_order(monkeypatch, reverse):
    from packaging.tags import Tag

    monkeypatch.setattr(
        "packaging.tags.sys_tags",
        lambda: iter([Tag("py3", "none", "macosx_11_0_arm64")]),
    )
    # The real py-spy publication has these two distinct archives. Both expose
    # the same best platform tag; uv keeps the first equally ranked lock entry.
    names = [
        "py_spy-0.4.2-py2.py3-none-macosx_10_12_x86_64.macosx_11_0_arm64.macosx_10_12_universal2.whl",
        "py_spy-0.4.2-py2.py3-none-macosx_11_0_arm64.whl",
    ]
    wheels = [_wheel(names[0], "a"), _wheel(names[1], "b")]
    if reverse:
        wheels.reverse()
    artifact = _locked_artifact(
        {"name": "py-spy", "version": "0.4.2", "wheels": wheels},
        ["py3-none-macosx_11_0_arm64"],
    )
    assert artifact == {
        "filename": names[1 if reverse else 0],
        "size": 123,
        "sha256": ("b" if reverse else "a") * 64,
    }


@pytest.mark.parametrize(
    "filenames,expected",
    [
        (["alpha-1.0-99-py3-none-any.whl", "alpha-1.0-py3-none-win_amd64.whl"], 1),
        (["alpha-1.0-9-py3-none-any.whl", "alpha-1.0-10-py3-none-any.whl"], 1),
        (["alpha-1.0-10b-py3-none-any.whl", "alpha-1.0-10a-py3-none-any.whl"], 0),
        (["alpha-1.0-py3-none-any.whl", "alpha-1.0-0-py3-none-any.whl"], 1),
    ],
)
def test_locked_wheel_tag_priority_precedes_numeric_then_text_build_tag(
    monkeypatch, filenames, expected
):
    from packaging.tags import Tag

    monkeypatch.setattr(
        "packaging.tags.sys_tags",
        lambda: iter([Tag("py3", "none", "win_amd64"), Tag("py3", "none", "any")]),
    )
    artifact = _locked_artifact(
        {
            "name": "alpha",
            "version": "1.0",
            "wheels": [_wheel(name) for name in filenames],
        },
        ["py3-none-win_amd64", "py3-none-any"],
    )
    assert artifact["filename"] == filenames[expected]


def test_locked_wheel_winner_with_invalid_identity_never_falls_back(monkeypatch):
    from packaging.tags import Tag

    monkeypatch.setattr(
        "packaging.tags.sys_tags", lambda: iter([Tag("py3", "none", "any")])
    )
    with pytest.raises(
        PythonEnvironmentIdentityError, match="wheel identity is invalid"
    ):
        _locked_artifact(
            {
                "name": "alpha",
                "version": "1.0",
                "wheels": [
                    _wheel("alpha-1.0-10-py3-none-any.whl", "invalid"),
                    _wheel("alpha-1.0-9-py3-none-any.whl"),
                ],
            },
            ["py3-none-any"],
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
    return selected_uv_lock_group_closure(
        root, "source-build", requirements, wheel_target=wheel_target_manifest()
    )


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
    actual = {
        "runtime": {
            "runtime_closure_sha256": closure["wheel_target"]["runtime_closure_sha256"]
        },
        "distributions": [
            {
                "name": name,
                "version": "1.0",
                "wheel": {"tags": ["py3-none-any"], "build": None},
            }
            for name in rows
        ],
    }
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
            tmp_path,
            "source-build",
            requirements,
            marker_environment=environment,
            wheel_target=wheel_target_manifest(),
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
    assert environment_matches_lock_closure(
        {
            "distributions": [],
            "runtime": {
                "runtime_closure_sha256": closure["wheel_target"][
                    "runtime_closure_sha256"
                ]
            },
        },
        closure,
    )


def test_repository_lock_revision_is_supported_and_unknown_ones_fail_closed(
    tmp_path: Path,
) -> None:
    from molt import python_uv_lock_identity as identity

    repository_lock = tomllib.loads(
        (Path(__file__).resolve().parents[1] / "uv.lock").read_text(encoding="utf-8")
    )
    assert repository_lock["revision"] in identity.SUPPORTED_UV_LOCK_REVISIONS

    requirements = _project(tmp_path)
    lock = tmp_path / "uv.lock"
    lock.write_text(
        lock.read_text(encoding="utf-8").replace(
            "revision = 1",
            f"revision = {max(identity.SUPPORTED_UV_LOCK_REVISIONS) + 1}",
        ),
        encoding="utf-8",
    )
    with pytest.raises(identity.PythonEnvironmentIdentityError, match="schema"):
        _select(tmp_path, requirements)
