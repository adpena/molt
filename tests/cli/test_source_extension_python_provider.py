from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any
from unittest.mock import patch

import pytest

from molt.cli.source_extension_python_provider import (
    source_extension_python_provider,
    validate_static_python_provider_requirements,
)
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkInput,
    SourceExtensionLinkProvider,
    SourceExtensionLinkProviderKind,
    SourceExtensionLinkRequirements,
    SourceExtensionLinkCyclicGroup,
)
from molt.cli.source_extension_set_registry import SourceExtensionVariant
from molt.exact_json import canonical_json_sha256
from molt.python_runtime_identity import validate_python_runtime_identity
from molt.target_python import TargetPythonVersion
from tests import python_environment_test_support as support


def _runtime(*, minor: int = 12, architecture: str = "x86_64") -> dict[str, Any]:
    with patch.object(
        support,
        "_host_target",
        return_value=(
            "windows",
            architecture,
            ["base-dlls", "platstdlib", "stdlib"],
            "pe-loaded-import-closure-v3",
        ),
    ):
        runtime = support.runtime_identity_manifest()
    runtime["version"] = f"3.{minor}.1"
    runtime["cache_tag"] = f"cpython-3{minor}"
    capabilities = runtime["capabilities"]
    capabilities["version_series"] = f"3.{minor}"
    capabilities["linkage"] = "shared"
    capabilities["required_explicit_roles"] = ["base-executable", "runtime-library"]
    dll = f"python3{minor}.dll"
    library = f"libs/python3{minor}.lib"
    access = {"readable": True, "writable": False, "executable": True}
    runtime["file_nodes"].extend(
        [
            {"id": "file-node-1", "size": 1, "sha256": "b" * 64},
            {"id": "file-node-2", "size": 1, "sha256": "c" * 64},
        ]
    )
    root = runtime["runtime_roots"][0]
    root["entries"].extend(
        [
            {"path": "Lib", "kind": "directory", "access": access},
            {"path": "libs", "kind": "directory", "access": access},
            {"path": library, "kind": "file", "access": access, "node": "file-node-2"},
            {"path": dll, "kind": "file", "access": access, "node": "file-node-1"},
        ]
    )
    root["entries"].sort(key=lambda row: row["path"])
    root["file_count"] = 3
    root["node_ids"] = ["file-node-0", "file-node-1", "file-node-2"]
    root["manifest_sha256"] = canonical_json_sha256(root["entries"])
    for role in runtime["runtime_root_roles"]:
        if role["role"] in {"stdlib", "platstdlib"}:
            role["path"] = "Lib"
    runtime["explicit_files"].append(
        {
            "role": "runtime-library",
            "kind": "root-reference",
            "root": root["id"],
            "path": dll,
            "node": "file-node-1",
        }
    )
    closure = runtime["native_dependency_closure"]
    closure["components"].append(
        {
            "id": "native-component-1",
            "filename": dll,
            "node": "file-node-1",
            "roles": ["runtime-library"],
        }
    )
    closure["root_components"].append("native-component-1")
    closure["observed_components"].append("native-component-1")
    closure["closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in closure.items()
            if key not in {"closure_sha256", "status"}
        }
    )
    runtime["runtime_closure_sha256"] = canonical_json_sha256(
        {
            key: value
            for key, value in runtime.items()
            if key != "runtime_closure_sha256"
        }
    )
    return validate_python_runtime_identity(runtime)


def _provider(
    tmp_path: Path,
    *,
    runtime=None,
    dependencies=None,
    minor=12,
    target="x86_64-pc-windows-msvc",
    abi="cpython-abi",
):
    path = tmp_path / "intro-dependencies.json"
    path.write_text(
        json.dumps(
            dependencies
            if dependencies is not None
            else [
                {
                    "name": "python",
                    "type": "system",
                    "version": f"3.{minor}",
                    "link_args": [f"@python-base/libs/python3{minor}.lib"],
                }
            ]
        ),
        encoding="utf-8",
    )
    return source_extension_python_provider(
        dependencies_path=path,
        runtime=runtime if runtime is not None else _runtime(minor=minor),
        variant=SourceExtensionVariant(TargetPythonVersion(3, minor, 0), abi, target),
        python_base="@python-base",
    )


@pytest.mark.parametrize(
    "minor,architecture,target",
    [
        (12, "x86_64", "x86_64-pc-windows-msvc"),
        (13, "arm64", "aarch64-pc-windows-msvc"),
        (14, "x86_64", "wasm32-wasip1"),
    ],
)
def test_provider_replaces_only_exact_interpreter_operand(
    tmp_path, minor, architecture, target
):
    provider = _provider(
        tmp_path,
        runtime=_runtime(minor=minor, architecture=architecture),
        minor=minor,
        target=target,
    )
    argument = f"@python-base/libs/python3{minor}.lib"
    foreign = f"@other/libs/python3{minor}.lib"
    remaining, receipt = provider.project((foreign, argument, "kernel32.lib", argument))
    assert remaining == (foreign, "kernel32.lib")
    assert receipt["consumed_link_args"] == [argument, argument]
    assert receipt["target_triple"] == target
    assert receipt["runtime_architecture"] == architecture
    assert receipt["import_library"]["sha256"] == "c" * 64
    assert provider.project((foreign,))[1] is None


@pytest.mark.parametrize(
    "mutation,match",
    [
        (lambda dep: dep.update(version="3.13"), "version/ABI"),
        (lambda dep: dep.update(type="pkgconfig"), "version/ABI"),
        (
            lambda dep: dep.update(link_args=["@other/libs/python312.lib"]),
            "import-library",
        ),
        (lambda dep: dep.update(link_args=["-lpython312"]), "exactly one absolute"),
        (lambda dep: dep["link_args"].append("unowned.lib"), "exactly one absolute"),
    ],
)
def test_provider_rejects_unproven_dependency(tmp_path, mutation, match):
    dependency = {
        "name": "python",
        "type": "system",
        "version": "3.12",
        "link_args": ["@python-base/libs/python312.lib"],
    }
    mutation(dependency)
    with pytest.raises(ValueError, match=match):
        _provider(tmp_path, dependencies=[dependency])


def test_provider_requires_unambiguous_dependency_and_runtime_custody(tmp_path):
    dependency = {
        "name": "python",
        "type": "system",
        "version": "3.12",
        "link_args": ["@python-base/libs/python312.lib"],
    }
    with pytest.raises(ValueError, match="ambiguous"):
        _provider(tmp_path, dependencies=[dependency, copy.deepcopy(dependency)])
    runtime = _runtime()
    runtime["runtime_closure_sha256"] = "f" * 64
    with pytest.raises(ValueError, match="digest"):
        _provider(tmp_path, runtime=runtime)


@pytest.mark.parametrize("dependencies", [[], [{"name": "python", "link_args": []}]])
def test_no_host_link_provider_needs_no_windows_layout(tmp_path, dependencies):
    provider = _provider(tmp_path, runtime={}, dependencies=dependencies)
    assert provider.project(("user.lib",)) == (("user.lib",), None)


def test_forced_library_operand_is_not_silently_consumed(tmp_path):
    provider = _provider(tmp_path)
    args = ("/WHOLEARCHIVE:@python-base/libs/python312.lib",)
    assert provider.project(args) == (args, None)


@pytest.mark.parametrize("grouped", [False, True])
@pytest.mark.parametrize("kind", ["bytes", "lookup"])
def test_consumed_provider_cannot_survive_in_final_link_requirements(
    tmp_path, grouped, kind
):
    # WASM also consumes a Windows-host provider, and permits cyclic groups.
    provider = _provider(tmp_path, target="wasm32-wasip1")
    _, receipt = provider.project((provider.argument,))
    atom = (
        SourceExtensionLinkInput("renamed.a", "c" * 64)
        if kind == "bytes"
        else SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.LIBRARY, "python312"
        )
    )
    requirements = SourceExtensionLinkRequirements(
        "wasm32-wasip1", (SourceExtensionLinkCyclicGroup((atom,)) if grouped else atom,)
    )
    with pytest.raises(ValueError, match="consumed Python provider survives"):
        validate_static_python_provider_requirements(receipt, requirements)


def test_unrelated_same_basename_input_is_not_the_consumed_provider(tmp_path):
    provider = _provider(tmp_path)
    _, receipt = provider.project((provider.argument,))
    requirements = SourceExtensionLinkRequirements(
        "x86_64-pc-windows-msvc",
        (SourceExtensionLinkInput("vendor/python312.lib", "d" * 64),),
    )
    validate_static_python_provider_requirements(receipt, requirements)
