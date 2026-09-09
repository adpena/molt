"""Canonical synthetic Python environment custody for cross-subsystem tests."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import sys
from typing import Iterable, Sequence

from molt import python_environment_identity
from molt.cli import source_build_environment
from molt.exact_json import canonical_json_sha256
from molt.python_external_custody import empty_external_import_custody


Package = tuple[str, str]
_READ_ONLY_EXECUTABLE = {
    "readable": True,
    "writable": False,
    "executable": True,
}
_EMPTY_FILE_SHA256 = hashlib.sha256(b"").hexdigest()


def _sealed(material: dict[str, object], digest_field: str) -> dict[str, object]:
    return {**material, digest_field: canonical_json_sha256(material)}


def _host_target() -> tuple[str, str, list[str], str]:
    marker = source_build_environment.canonical_source_marker_environment()
    operating_system = {"win32": "windows", "darwin": "macos"}.get(
        sys.platform, "linux"
    )
    architecture = (
        "arm64"
        if marker["platform_machine"].casefold() in {"arm64", "aarch64"}
        else "x86_64"
    )
    root_roles = {
        "windows": ["base-dlls", "platstdlib", "stdlib"],
        "macos": ["base-lib-dynload", "platstdlib", "stdlib"],
        "linux": ["base-lib-dynload", "platstdlib", "stdlib"],
    }[operating_system]
    dependency_policy = {
        "windows": "pe-loaded-import-closure-v2",
        "macos": "mach-o-loaded-dylib-closure-v2",
        "linux": "elf-loaded-needed-closure-v2",
    }[operating_system]
    return operating_system, architecture, root_roles, dependency_policy


def runtime_identity_manifest() -> dict[str, object]:
    """Return one production-validated synthetic CPython runtime identity."""

    marker = source_build_environment.canonical_source_marker_environment()
    operating_system, architecture, root_roles, dependency_policy = _host_target()
    nodes = [{"id": "file-node-0", "size": 1, "sha256": "a" * 64}]
    entries = [
        {
            "path": "os.py",
            "kind": "file",
            "node": "file-node-0",
            "access": _READ_ONLY_EXECUTABLE,
        }
    ]
    capabilities = {
        "schema": python_environment_identity.PYTHON_RUNTIME_CAPABILITY_SCHEMA,
        "implementation_policy": "cpython>=3.12",
        "version_series": ".".join(marker["python_full_version"].split(".")[:2]),
        "operating_system": operating_system,
        "architecture": architecture,
        "linkage": "static",
        "unicodedata_linkage": "built-in",
        "scanner_policy": "no-follow-handle-two-snapshot-sha256-v1",
        "import_root_policy": "isolated-active-prefix-roots-v1",
        "native_dependency_policy": dependency_policy,
        "required_root_roles": root_roles,
        "required_explicit_roles": ["base-executable"],
    }
    dependency_material = {
        "policy": dependency_policy,
        "root_components": ["native-component-0"],
        "observed_components": ["native-component-0"],
        "observed_contracts": [],
        "deferred_imports": [],
        "components": [
            {
                "id": "native-component-0",
                "filename": Path(sys.executable).name,
                "node": "file-node-0",
                "roles": ["base-executable"],
            }
        ],
        "contracts": [],
        "edges": [],
    }
    material = {
        "schema": python_environment_identity.PYTHON_RUNTIME_IDENTITY_SCHEMA,
        "implementation": "cpython",
        "version": marker["python_full_version"],
        "cache_tag": str(sys.implementation.cache_tag),
        "soabi": "test-soabi",
        "abi_flags": "",
        "multiarch": "test-multiarch",
        "py_debug": False,
        "gil_disabled": False,
        "operating_system": operating_system,
        "architecture": architecture,
        "pointer_bits": 64,
        "byteorder": sys.byteorder,
        "capabilities": capabilities,
        "file_nodes": nodes,
        "native_dependency_closure": {
            "status": "closed",
            **dependency_material,
            "closure_sha256": canonical_json_sha256(dependency_material),
        },
        "explicit_files": [
            {
                "role": "base-executable",
                "kind": "node-reference",
                "filename": Path(sys.executable).name,
                "node": "file-node-0",
            }
        ],
        "import_roots": [{"kind": "directory", "root": "runtime-root-0", "path": "."}],
        "runtime_root_roles": [
            {"role": role, "root": "runtime-root-0", "path": "."} for role in root_roles
        ],
        "runtime_roots": [
            {
                "id": "runtime-root-0",
                "file_count": 1,
                "node_ids": ["file-node-0"],
                "entries": entries,
                "manifest_sha256": canonical_json_sha256(entries),
            }
        ],
    }
    identity = _sealed(material, "runtime_closure_sha256")
    return python_environment_identity.validate_python_runtime_identity(identity)


def lock_closure_manifest(
    requirements: Sequence[str],
    packages: Sequence[Package],
    *,
    dependency_group: str = "source-build-scipy",
) -> dict[str, object]:
    """Return one production-validated locked dependency-group closure."""

    material = {
        "schema": python_environment_identity.UV_LOCK_GROUP_CLOSURE_SCHEMA,
        "lock_version": 1,
        "lock_revision": 3,
        "requires_python": ">=3.12",
        "dependency_group": dependency_group,
        "requirements": list(requirements),
        "project_requirements": [],
        "marker_environment": (
            source_build_environment.canonical_source_marker_environment()
        ),
        "packages": [
            {
                "name": name,
                "version": version,
                "source": {"registry": "https://example.invalid/simple"},
                "extras": [],
                "dependencies": [],
                "artifact": {
                    "filename": f"{name.replace('-', '_')}-{version}-py3-none-any.whl",
                    "size": 1,
                    "sha256": "a" * 64,
                },
            }
            for name, version in sorted(packages)
        ],
    }
    closure = _sealed(material, "closure_sha256")
    return python_environment_identity.validate_uv_lock_group_closure(closure)


def realized_environment_manifest(
    runtime: dict[str, object], packages: Sequence[Package]
) -> dict[str, object]:
    """Return a production-validated isolated environment using ``runtime``."""

    marker = source_build_environment.canonical_source_marker_environment()
    selected = "Scripts/python.exe" if os.name == "nt" else "bin/python"
    scripts_root = "Scripts" if os.name == "nt" else "bin"
    site_root = (
        "Lib/site-packages"
        if os.name == "nt"
        else f"lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages"
    )
    installed_paths = [f"{site_root}/{name}/__init__.py" for name, _ in packages]
    file_paths = sorted(
        [selected, "pyvenv.cfg", *installed_paths],
        key=lambda value: (value.casefold(), value),
    )
    nodes_by_path = {
        path: f"file-node-{index}" for index, path in enumerate(file_paths)
    }
    sha_by_path = {
        selected: "1" * 64,
        "pyvenv.cfg": "2" * 64,
        **{path: "4" * 64 for path in installed_paths},
    }
    file_nodes = [
        {"id": nodes_by_path[path], "size": 1, "sha256": sha_by_path[path]}
        for path in file_paths
    ]
    distributions: list[dict[str, object]] = []
    for (name, version), installed_path in zip(packages, installed_paths, strict=True):
        installed = [
            {
                "path": installed_path,
                "node": nodes_by_path[installed_path],
                "declared": None,
            }
        ]
        distributions.append(
            {
                "name": name,
                "version": version,
                "record_sha256": "b" * 64,
                "direct_url_sha256": _EMPTY_FILE_SHA256,
                "installer_sha256": "d" * 64,
                "installed_file_count": 1,
                "file_manifest_sha256": canonical_json_sha256(installed),
                "installed_files": installed,
                "entry_points": [],
                "entry_points_sha256": canonical_json_sha256([]),
                "console_scripts": {},
                "external_source": None,
            }
        )
    # Declared import roots exist even when no distribution supplies a file.
    directory_paths = {scripts_root, site_root}
    for file_path in [site_root, selected, *installed_paths]:
        parts = file_path.split("/")
        directory_paths.update(
            "/".join(parts[:index]) for index in range(1, len(parts))
        )
    entries: list[dict[str, object]] = [
        *[
            {"path": path, "kind": "directory", "access": _READ_ONLY_EXECUTABLE}
            for path in sorted(
                directory_paths, key=lambda value: (value.casefold(), value)
            )
        ],
        {
            "path": selected,
            "kind": "file",
            "node": nodes_by_path[selected],
            "access": _READ_ONLY_EXECUTABLE,
        },
        {
            "path": "pyvenv.cfg",
            "kind": "file",
            "node": nodes_by_path["pyvenv.cfg"],
            "access": _READ_ONLY_EXECUTABLE,
        },
        *[
            {
                "path": path,
                "kind": "file",
                "node": nodes_by_path[path],
                "access": _READ_ONLY_EXECUTABLE,
            }
            for path in installed_paths
        ],
    ]
    entries.sort(key=lambda row: (str(row["path"]).casefold(), str(row["path"])))
    material = {
        "schema": python_environment_identity.PYTHON_ENVIRONMENT_IDENTITY_SCHEMA,
        "implementation": "cpython",
        "version": marker["python_full_version"],
        "cache_tag": str(sys.implementation.cache_tag),
        "soabi": "test-soabi",
        "abi_flags": "",
        "multiarch": "test-multiarch",
        "py_debug": False,
        "gil_disabled": False,
        "operating_system": runtime["operating_system"],
        "architecture": runtime["architecture"],
        "pointer_bits": 64,
        "byteorder": sys.byteorder,
        "capabilities": {
            "schema": (
                python_environment_identity.PYTHON_ENVIRONMENT_CAPABILITY_SCHEMA
            ),
            "runtime": runtime["capabilities"],
            "isolated": True,
            "ignore_environment": True,
            "no_user_site": True,
            "safe_path": True,
            "environment_root_active": False,
            "required_active_import_roles": ["runtime-import-0", "site-root-0"],
        },
        "runtime": runtime,
        "selected_executable": {
            "path": selected,
            "kind": "tree-reference",
            "node": nodes_by_path[selected],
        },
        "pyvenv_config": {
            "path": "pyvenv.cfg",
            "node": nodes_by_path["pyvenv.cfg"],
        },
        "scripts_root": scripts_root,
        "site_roots": [site_root],
        "active_import_roots": [
            {
                "role": "runtime-import-0",
                "owner": "runtime",
                **runtime["import_roots"][0],  # type: ignore[index]
            },
            {
                "role": "site-root-0",
                "owner": "environment",
                "kind": "directory",
                "path": site_root,
            },
        ],
        "external_roots": [],
        "external_import_custody": empty_external_import_custody(),
        "tree": {
            "id": "environment-root",
            "file_count": sum(row["kind"] == "file" for row in entries),
            "node_ids": [node["id"] for node in file_nodes],
            "file_nodes": file_nodes,
            "entries": entries,
            "manifest_sha256": canonical_json_sha256(entries),
        },
        "distributions": distributions,
        "distribution_inventory_sha256": canonical_json_sha256(distributions),
        "site_bootstrap_files": [],
        "console_scripts": {},
        "native_modules": [],
    }
    identity = _sealed(material, "environment_closure_sha256")
    return python_environment_identity.validate_python_environment_identity(identity)


def package_rows_from_requirements(requirements: Iterable[str]) -> list[Package]:
    """Project exact pinned requirements into normalized test package rows."""

    packages: list[Package] = []
    for requirement in requirements:
        name, version = requirement.split("==", 1)
        packages.append((re.sub(r"[-_.]+", "-", name).casefold(), version))
    return packages


def build_environment_manifest(
    requirements: Sequence[str] | None = None,
    packages: Sequence[Package] | None = None,
    *,
    dependency_group: str = "source-build-scipy",
) -> dict[str, object]:
    """Return one exact source-build environment and validate every projection."""

    selected_requirements = list(
        ("meson==1.9.0", "ninja==1.13.0") if requirements is None else requirements
    )
    selected_packages = list(
        package_rows_from_requirements(selected_requirements)
        if packages is None
        else packages
    )
    if len(selected_requirements) != len(selected_packages):
        raise ValueError("requirements and package rows must have equal length")
    marker = source_build_environment.canonical_source_marker_environment()
    runtime = runtime_identity_manifest()
    lock_closure = lock_closure_manifest(
        selected_requirements,
        selected_packages,
        dependency_group=dependency_group,
    )
    realized = realized_environment_manifest(runtime, selected_packages)
    address = {
        "schema_version": source_build_environment.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_group": dependency_group,
        "dependency_group_requirements": selected_requirements,
        "lock_closure": lock_closure,
        "python_runtime": runtime,
        "uv": {
            "executable": "uv.exe",
            "version": "uv 0.11.24",
            "sha256": "d" * 64,
        },
    }
    payload = {
        "python": {
            "implementation": marker["implementation_name"],
            "version": marker["python_full_version"],
            "executable": Path(sys.executable).name,
        },
        "requirements": selected_requirements,
        "marker_environment": marker,
        "active_requirements": selected_requirements,
        "resolved": [
            {
                "requirement": requirement,
                "distribution": package[0],
                "version": package[1],
            }
            for requirement, package in zip(
                selected_requirements, selected_packages, strict=True
            )
        ],
        "custody": {
            "environment_id": canonical_json_sha256(address),
            **address,
            "realized_environment": realized,
        },
    }
    problems = source_build_environment.source_build_environment_problems(payload)
    if problems:
        raise AssertionError(
            "invalid synthetic build environment: " + "; ".join(problems)
        )
    return payload


def reseal_build_environment_custody(payload: dict[str, object]) -> None:
    """Reseal a deliberately mutated build-environment test payload."""

    custody = payload["custody"]
    assert isinstance(custody, dict)
    lock_closure = custody["lock_closure"]
    assert isinstance(lock_closure, dict)
    lock_closure["closure_sha256"] = canonical_json_sha256(
        {key: value for key, value in lock_closure.items() if key != "closure_sha256"}
    )
    address = {
        key: value
        for key, value in custody.items()
        if key not in {"environment_id", "realized_environment"}
    }
    custody["environment_id"] = canonical_json_sha256(address)
