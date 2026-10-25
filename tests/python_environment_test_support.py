"""Canonical synthetic Python environment custody for cross-subsystem tests."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import shutil
import sys
from typing import Iterable, Sequence, Mapping

import venv

from molt import python_environment_identity
from molt.python_runtime_identity import _NATIVE_DEPENDENCY_POLICIES
from molt.cli import source_build_environment_schema
from molt.exact_json import canonical_json_sha256
from molt.python_external_custody import empty_external_import_custody
from tests.process_guard_common import run_guarded_test_process


Package = tuple[str, str]
_READ_ONLY_EXECUTABLE = {
    "readable": True,
    "writable": False,
    "executable": True,
}
_EMPTY_FILE_SHA256 = hashlib.sha256(b"").hexdigest()


def _sealed(material: dict[str, object], digest_field: str) -> dict[str, object]:
    return {**material, digest_field: canonical_json_sha256(material)}


def environment_location_manifest(
    *,
    prefix: Path,
    selected_executable: Path,
    base_executable: Path,
    base_executable_selector: Path,
    roots: Sequence[Path],
    external_roots: Sequence[Path],
    file_paths: Sequence[Path],
) -> dict[str, object]:
    """Build the current closed locator contract from explicit fixture facts."""
    material = {
        "schema": python_environment_identity.PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        "prefix": str(prefix),
        "selected_executable": str(selected_executable),
        "base_executable": str(base_executable),
        "base_executable_selector": str(base_executable_selector),
        "roots": [str(path) for path in roots],
        "external_roots": [str(path) for path in external_roots],
        "file_paths": [str(path) for path in file_paths],
    }
    return python_environment_identity.validate_python_environment_location(
        _sealed(material, "identity_sha256")
    )


def _host_target() -> tuple[str, str, list[str], str]:
    marker = source_build_environment_schema.canonical_source_marker_environment()
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
    dependency_policy = _NATIVE_DEPENDENCY_POLICIES[operating_system]
    return operating_system, architecture, root_roles, dependency_policy


def runtime_identity_manifest() -> dict[str, object]:
    """Return one production-validated synthetic CPython runtime identity."""

    marker = source_build_environment_schema.canonical_source_marker_environment()
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
        "executable_component": "native-component-0",
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


def wheel_target_manifest(
    runtime: Mapping[str, object] | None = None,
) -> dict[str, object]:
    from molt.python_uv_lock_identity import PYTHON_WHEEL_TARGET_SCHEMA

    selected = runtime_identity_manifest() if runtime is None else runtime
    return {
        "schema": PYTHON_WHEEL_TARGET_SCHEMA,
        "runtime_closure_sha256": selected["runtime_closure_sha256"],
        "packaging_sources": {
            name + ".py": {"size": 1, "sha256": "e" * 64}
            for name in ("__init__", "_elffile", "_manylinux", "_musllinux", "tags")
        },
        "tags": ["py3-none-any"],
    }


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
        "dependency_groups": [dependency_group],
        "requirements": list(requirements),
        "project_requirements": [],
        "marker_environment": (
            source_build_environment_schema.canonical_source_marker_environment()
        ),
        "wheel_target": wheel_target_manifest(),
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


def copy_python_capture_authority(source_root: Path) -> None:
    """Give a disposable checkout the actual selected reader implementation."""
    sources = python_environment_identity.python_capture_authority_paths()
    destinations = python_environment_identity.python_capture_authority_paths(
        source_root=source_root
    )
    for source, destination in zip(sources, destinations, strict=True):
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


def create_test_venv(environment: Path) -> Path:
    """Create a runnable venv the way the platform's own tooling does.

    ``venv.EnvBuilder`` defaults to copied launchers. A copied launcher of a
    relocatable CPython (python-build-standalone, which uv manages on macOS)
    resolves ``libpython`` relative to its own location and cannot start, so
    POSIX uses symlinks like ``python -m venv`` and ``uv venv``; Windows keeps
    copies because symlinks need a privilege there. Returns the launcher.
    """
    venv.EnvBuilder(symlinks=os.name != "nt", with_pip=False).create(environment)
    return environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def create_owned_python_venv(root: Path) -> Path:
    """Create a venv whose base interpreter the caller owns; return its launcher.

    A test that hardlinks or otherwise touches interpreter files must not touch
    the host's shared installation: a hardlink changes that file's link count
    and change time, so every concurrent build or proof that captured its
    identity fails. The base installation is copied whole (python-build-
    standalone resolves libpython relative to its own location), and the venv
    is made from the copy the way ``python -m venv`` makes one.
    """
    base = Path(sys.base_prefix).resolve()
    install = root / "python"
    shutil.copytree(base, install, symlinks=True)
    base_executable = install / Path(sys._base_executable).resolve().relative_to(base)
    environment = root / "venv"
    completed = run_guarded_test_process(
        [str(base_executable), "-m", "venv", "--without-pip", str(environment)],
        timeout=120,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"owned interpreter venv failed: {completed.stderr}")
    return environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def realized_environment_manifest(
    runtime: dict[str, object], packages: Sequence[Package]
) -> dict[str, object]:
    """Return a production-validated isolated environment using ``runtime``."""

    selected = "Scripts/python.exe" if os.name == "nt" else "bin/python"
    scripts_root = "Scripts" if os.name == "nt" else "bin"
    site_root = (
        "Lib/site-packages"
        if os.name == "nt"
        else f"lib/python{sys.version_info.major}.{sys.version_info.minor}/site-packages"
    )
    ordered_packages = sorted(packages)
    installed_paths = [
        f"{site_root}/{name}/__init__.py" for name, _ in ordered_packages
    ]
    wheel_paths = [
        f"{site_root}/{name}-{version}.dist-info/WHEEL"
        for name, version in ordered_packages
    ]
    file_paths = sorted(
        [selected, "pyvenv.cfg", *installed_paths, *wheel_paths],
        key=lambda value: (value.casefold(), value),
    )
    nodes_by_path = {
        path: f"file-node-{index}" for index, path in enumerate(file_paths)
    }
    sha_by_path = {
        selected: "1" * 64,
        "pyvenv.cfg": "2" * 64,
        **{path: "4" * 64 for path in installed_paths},
        **{path: "5" * 64 for path in wheel_paths},
    }
    file_nodes = [
        {"id": nodes_by_path[path], "size": 1, "sha256": sha_by_path[path]}
        for path in file_paths
    ]
    distributions: list[dict[str, object]] = []
    for (name, version), installed_path in zip(
        ordered_packages, installed_paths, strict=True
    ):
        wheel_path = f"{site_root}/{name}-{version}.dist-info/WHEEL"
        installed = [
            {
                "path": installed_path,
                "node": nodes_by_path[installed_path],
                "declared": None,
            }
        ]
        installed.append(
            {"path": wheel_path, "node": nodes_by_path[wheel_path], "declared": None}
        )
        installed.sort(key=lambda row: (row["path"].casefold(), row["path"]))
        distributions.append(
            {
                "name": name,
                "version": version,
                "record_sha256": "b" * 64,
                "direct_url_sha256": _EMPTY_FILE_SHA256,
                "installer_sha256": "d" * 64,
                "installed_file_count": len(installed),
                "file_manifest_sha256": canonical_json_sha256(installed),
                "installed_files": installed,
                "entry_points": [],
                "entry_points_sha256": canonical_json_sha256([]),
                "console_scripts": {},
                "external_source": None,
                "wheel": {
                    "path": wheel_path,
                    "sha256": sha_by_path[wheel_path],
                    "tags": ["py3-none-any"],
                    "build": None,
                },
            }
        )
    # Declared import roots exist even when no distribution supplies a file.
    directory_paths = {scripts_root, site_root}
    for file_path in [site_root, selected, *installed_paths, *wheel_paths]:
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
            for path in [*installed_paths, *wheel_paths]
        ],
    ]
    entries.sort(key=lambda row: (str(row["path"]).casefold(), str(row["path"])))
    material = {
        "schema": python_environment_identity.PYTHON_ENVIRONMENT_IDENTITY_SCHEMA,
        "implementation": "cpython",
        "version": runtime["version"],
        "cache_tag": runtime["cache_tag"],
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
    marker = source_build_environment_schema.canonical_source_marker_environment()
    runtime = runtime_identity_manifest()
    lock_closure = lock_closure_manifest(
        selected_requirements,
        selected_packages,
        dependency_group=dependency_group,
    )
    realized = realized_environment_manifest(runtime, selected_packages)
    selected_executable = realized.get("selected_executable")
    assert isinstance(selected_executable, dict)
    selected_executable_path = selected_executable.get("path")
    assert isinstance(selected_executable_path, str)
    realized_executable = Path(selected_executable_path).name
    address = {
        "schema_version": source_build_environment_schema.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_groups": [dependency_group],
        "editable_binding": None,
        "bytecode_policy": source_build_environment_schema.ORDINARY_BYTECODE_POLICY,
        "dependency_group_requirements": selected_requirements,
        "lock_closure": lock_closure,
        "python_runtime": runtime,
        "uv": {
            "executable": "uv.exe",
            "version": "uv 0.12.23",
            "sha256": "d" * 64,
        },
    }
    payload = {
        "python": {
            "implementation": marker["implementation_name"],
            "version": marker["python_full_version"],
            "executable": realized_executable,
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
    problems = source_build_environment_schema.source_build_environment_problems(
        payload
    )
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


def materialize_development_environment(
    repo_root: Path,
    artifact_root: Path,
    *,
    python: str | None = None,
    groups: tuple[str, ...] = (),
    runnable: bool = False,
    select: bool = False,
    purpose: str = "dx",
) -> source_build_environment_schema.LockedSourceBuildEnvironment:
    """A real recipe/manifest fixture for selectors, never live-byte evidence.

    The real cheap loader remains active. The runtime/file nodes are the existing
    synthetic custody oracle; tests claiming actual admission must use a real
    captured image instead. Optional runnable Python is a local no-pip venv.
    """
    import json
    import tomllib

    from molt.cli import source_build_environment as owner
    from molt.dx import checkout_component
    from molt.python_environment_location import (
        environment_bytecode_prefix,
        environment_python_path,
    )
    from molt.python_uv_lock_identity import selected_uv_lock_group_closure
    from molt.target_python import _parse_target_python_version

    repo_root.mkdir(parents=True, exist_ok=True)
    repo_root = repo_root.resolve()
    groups = tuple(sorted(set(groups)))
    project = repo_root / "pyproject.toml"
    if not project.exists():
        project.write_text(
            "[project]\nname='selector-fixture'\nversion='1.0'\n"
            "requires-python='>=3.12'\ndependencies=[]\n[dependency-groups]\n"
            + "".join(f"{group}=['fixture-{group}==1.0']\n" for group in groups)
            + "[tool.molt.dx]\ndependency_groups="
            + json.dumps(list(groups))
            + "\n",
            encoding="utf-8",
        )
    if not (repo_root / "uv.lock").exists():
        lock = (
            "version=1\nrevision=1\nrequires-python='>=3.12'\n"
            "[[package]]\nname='selector-fixture'\nversion='1.0'\nsource={editable='.'}\n"
            "[package.dev-dependencies]\n"
            + "".join(f"{group}=[{{name='fixture-{group}'}}]\n" for group in groups)
        )
        for group in groups:
            lock += (
                f"[[package]]\nname='fixture-{group}'\nversion='1.0'\n"
                "source={registry='https://example.invalid/simple'}\n"
                f"wheels=[{{url='https://example.invalid/fixture_{group}-1.0-py3-none-any.whl',"
                f"hash='sha256:{'a' * 64}',size=1}}]\n"
            )
        (repo_root / "uv.lock").write_text(lock, encoding="utf-8")
    runtime = runtime_identity_manifest()
    if python is not None:
        target = _parse_target_python_version(python)
        runtime.pop("runtime_closure_sha256")
        runtime["version"] = f"{target.major}.{target.minor}.{target.micro}"
        runtime["cache_tag"] = f"cpython-{target.major}{target.minor}"
        runtime["capabilities"]["version_series"] = target.short
        runtime = python_environment_identity.validate_python_runtime_identity(
            _sealed(runtime, "runtime_closure_sha256")
        )
    if not (repo_root / ".python-version").exists():
        (repo_root / ".python-version").write_text(
            str(runtime["version"]) + "\n", encoding="utf-8"
        )
    markers = source_build_environment_schema.canonical_source_marker_environment()
    markers["python_full_version"] = str(runtime["version"])
    markers["python_version"] = ".".join(str(runtime["version"]).split(".")[:2])
    markers["implementation_version"] = str(runtime["version"])
    document = tomllib.loads(project.read_text(encoding="utf-8"))
    requirements = tuple(
        value for group in groups for value in document["dependency-groups"][group]
    )
    wheel_target = wheel_target_manifest(runtime)
    if runnable:
        if not str(runtime["version"]).startswith(
            f"{sys.version_info.major}.{sys.version_info.minor}."
        ):
            raise ValueError("runnable fixture must use the running interpreter series")
        from packaging.tags import sys_tags

        # This fixture's runnable venv uses this very interpreter. Copied real
        # projects can lock ABI wheels, unlike the minimal pure-Python fixture.
        # The surrounding runtime/source nodes remain explicitly synthetic.
        wheel_target["tags"] = [str(tag) for tag in sys_tags()]
    closure = selected_uv_lock_group_closure(
        repo_root,
        None,
        requirements,
        wheel_target=wheel_target,
        marker_environment=markers,
        dependency_groups=groups,
    )
    recipe = {
        "schema_version": source_build_environment_schema.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_groups": list(groups),
        "editable_binding": owner._editable_binding(repo_root),
        "bytecode_policy": source_build_environment_schema.ORDINARY_BYTECODE_POLICY,
        "dependency_group_requirements": list(requirements),
        "lock_closure": closure,
        "python_runtime": runtime,
        "uv": {"executable": "uv", "version": "fixture uv", "sha256": "d" * 64},
    }
    identity = canonical_json_sha256(recipe)
    root = (
        artifact_root.resolve()
        / "build-environments"
        / "development"
        / checkout_component(repo_root)
        / identity
    )
    manifest_path = (
        root / source_build_environment_schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST
    )
    if not manifest_path.exists():
        if runnable:
            create_test_venv(root)
        else:
            executable = environment_python_path(root)
            executable.parent.mkdir(parents=True, exist_ok=True)
            executable.write_bytes(b"")
            (root / "pyvenv.cfg").write_text(
                "home = synthetic-selector-fixture\n", encoding="utf-8"
            )
        environment_bytecode_prefix(root).mkdir()
        realized = realized_environment_manifest(
            runtime,
            [(row["name"], row["version"]) for row in closure["packages"]]
            + [
                (
                    recipe["editable_binding"]["project"],
                    recipe["editable_binding"]["version"],
                )
            ],
        )
        from packaging.utils import parse_wheel_filename

        artifacts = {row["name"]: row["artifact"] for row in closure["packages"]}
        tree_nodes = {row["id"]: row for row in realized["tree"]["file_nodes"]}
        for distribution in realized["distributions"]:
            artifact = artifacts.get(distribution["name"])
            if artifact is None:
                continue  # The editable project is not a locked registry wheel.
            _name, _version, build, tags = parse_wheel_filename(artifact["filename"])
            wheel = distribution["wheel"]
            selected_tags = sorted(str(tag) for tag in tags)
            selected_build = None if not build else str(build[0]) + build[1]
            if wheel["tags"] == selected_tags and wheel["build"] == selected_build:
                continue
            wheel["tags"] = selected_tags
            wheel["build"] = selected_build
            wheel_bytes = (
                "Wheel-Version: 1.0\n"
                + "".join(f"Tag: {tag}\n" for tag in wheel["tags"])
                + ("" if wheel["build"] is None else f"Build: {wheel['build']}\n")
            ).encode()
            wheel["sha256"] = hashlib.sha256(wheel_bytes).hexdigest()
            wheel_file = next(
                row
                for row in distribution["installed_files"]
                if row["path"] == wheel["path"]
            )
            tree_nodes[wheel_file["node"]].update(
                size=len(wheel_bytes), sha256=wheel["sha256"]
            )
        realized["external_roots"] = [{"id": "external-root-0", "path": str(repo_root)}]
        role = "external-import-0"
        realized["active_import_roots"].append(
            {
                "role": role,
                "owner": "external",
                "kind": "directory",
                "root": "external-root-0",
                "path": ".",
            }
        )
        realized["capabilities"]["required_active_import_roles"] = sorted(
            row["role"] for row in realized["active_import_roots"]
        )
        site = realized["site_roots"][0]
        metadata = f"{site}/selector_fixture-1.0.dist-info"
        pth_path = f"{site}/selector_fixture.pth"
        pth_content = str(repo_root) + "\n"
        direct_url = json.dumps(
            {"url": repo_root.as_uri(), "dir_info": {"editable": True}}
        ).encode()
        tree = realized["tree"]
        tree["entries"].append(
            {
                "path": metadata,
                "kind": "directory",
                "access": _READ_ONLY_EXECUTABLE,
            }
        )
        additions = []
        for path, data in (
            (pth_path, pth_content.encode()),
            (f"{metadata}/direct_url.json", direct_url),
        ):
            node = f"file-node-{len(tree['file_nodes'])}"
            tree["file_nodes"].append(
                {
                    "id": node,
                    "size": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
            )
            tree["entries"].append(
                {
                    "path": path,
                    "kind": "file",
                    "node": node,
                    "access": _READ_ONLY_EXECUTABLE,
                }
            )
            additions.append({"path": path, "node": node, "declared": None})
        tree["entries"].sort(key=lambda row: (row["path"].casefold(), row["path"]))
        tree["file_count"] += len(additions)
        tree["node_ids"] = [row["id"] for row in tree["file_nodes"]]
        tree["manifest_sha256"] = canonical_json_sha256(tree["entries"])
        for distribution in realized["distributions"]:
            if distribution["name"] == recipe["editable_binding"]["project"]:
                distribution["external_source"] = {
                    "root": "external-root-0",
                    "path": ".",
                    "import_roles": [role],
                }
                distribution["direct_url_sha256"] = hashlib.sha256(
                    direct_url
                ).hexdigest()
                distribution["installed_files"].extend(additions)
                distribution["installed_files"].sort(
                    key=lambda row: (row["path"].casefold(), row["path"])
                )
                distribution["installed_file_count"] = len(
                    distribution["installed_files"]
                )
                distribution["file_manifest_sha256"] = canonical_json_sha256(
                    distribution["installed_files"]
                )
        realized["external_import_custody"]["trees"] = [
            {
                "id": "external-tree-0",
                "source_root": "external-root-0",
                "source_path": ".",
                "roles": [{"role": role, "path": "."}],
                "file_count": 0,
                "node_ids": [],
                "file_nodes": [],
                "entries": [],
                "manifest_sha256": canonical_json_sha256([]),
            }
        ]
        realized["external_import_custody"]["path_declarations"] = [
            {
                "path": pth_path,
                "node": additions[0]["node"],
                "content": pth_content,
                "roles": [role],
            }
        ]
        realized["distribution_inventory_sha256"] = canonical_json_sha256(
            realized["distributions"]
        )
        realized.pop("environment_closure_sha256")
        realized = python_environment_identity.validate_python_environment_identity(
            _sealed(realized, "environment_closure_sha256")
        )
        manifest = owner._attested_environment(
            {"environment_id": identity, **recipe}, realized
        )
        manifest_path = (
            root / source_build_environment_schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST
        )
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    if select:
        from molt import checkout_provisioning as provisioning
        from molt.custody_layout import git_directories
        from molt.exact_json import write_exact
        from molt.process_environment import ProcessEnvironment

        if git_directories(repo_root) is None:
            initialized = run_guarded_test_process(
                ["git", "init", "-q", "-b", "main"],
                cwd=repo_root,
                capture_output=True,
                text=True,
                check=True,
            )
            assert initialized.returncode == 0
        spec = provisioning.DevelopmentEnvironment(
            "uv-environment",
            repo_root,
            ProcessEnvironment(
                {
                    "MOLT_EXT_ROOT": str(artifact_root),
                    "MOLT_UV_PROJECT_PURPOSE": purpose,
                }
            ),
            python=python,
            groups=groups,
        )
        intent = provisioning._development_intent(spec)
        write_exact(
            provisioning._development_selection_path(spec, intent),
            {
                "schema": provisioning.SCHEMA,
                "input": "uv-environment",
                "checkout": str(repo_root),
                "intent": intent,
                "locks": provisioning._lock_digests(spec),
                "generation": str(root),
            },
        )
    return owner.load_development_environment(
        root, repo_root=repo_root, python=python, groups=groups
    )
