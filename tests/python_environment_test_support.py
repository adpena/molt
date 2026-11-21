"""Canonical synthetic Python environment custody for cross-subsystem tests."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
from typing import Iterable, Sequence, Mapping

from molt import python_environment_contract as _contract
from molt import python_environment_identity
from molt.python_runtime_identity import _NATIVE_DEPENDENCY_POLICIES
from molt import source_build_environment_schema
from molt.exact_json import canonical_json_sha256
from molt.python_external_custody import (
    _STARTUP_CAPABILITIES,
    empty_external_import_custody,
)
from tests.process_guard_common import run_guarded_test_process


Package = tuple[str, str]


def _read_only_executable() -> dict[str, bool]:
    """Return a fresh access row; parsed manifests never share one."""
    return {"readable": True, "writable": False, "executable": True}


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
        "schema": _contract.PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
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


def runtime_identity_manifest(*, python: str | None = None) -> dict[str, object]:
    """Return one production-validated synthetic physical CPython identity."""

    marker = source_build_environment_schema.canonical_source_marker_environment()
    version = marker["python_full_version"]
    cache_tag = str(sys.implementation.cache_tag)
    if python is not None:
        from molt._python_release_generated import CPYTHON_RELEASES
        from molt.python_interpreter import parse_python_interpreter_request

        request = parse_python_interpreter_request(python)
        micro = request.micro
        if micro is None:
            # Synthetic identities need a source-pinned patch, independent of
            # guest admission. Explicit patches remain exact.
            micro = CPYTHON_RELEASES[request.series][2]
        version = f"{request.major}.{request.minor}.{micro}"
        cache_tag = f"cpython-{request.major}{request.minor}"
    operating_system, architecture, root_roles, dependency_policy = _host_target()
    nodes = [{"id": "file-node-0", "size": 1, "sha256": "a" * 64}]
    entries = [
        {
            "path": "os.py",
            "kind": "file",
            "node": "file-node-0",
            "access": _read_only_executable(),
        }
    ]
    capabilities = {
        "schema": python_environment_identity.PYTHON_RUNTIME_CAPABILITY_SCHEMA,
        "implementation_policy": "cpython>=3.12",
        "version_series": ".".join(version.split(".")[:2]),
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
        "schema": _contract.PYTHON_RUNTIME_IDENTITY_SCHEMA,
        "implementation": "cpython",
        "version": version,
        "cache_tag": cache_tag,
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

    selected = runtime_identity_manifest() if runtime is None else runtime
    return {
        "schema": _contract.PYTHON_WHEEL_TARGET_SCHEMA,
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
        "schema": _contract.UV_LOCK_GROUP_CLOSURE_SCHEMA,
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
                    "kind": "wheel",
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
    """Create a runnable venv without links into a shared Python installation."""
    environment.parent.mkdir(parents=True, exist_ok=True)
    owned = Path(tempfile.mkdtemp(prefix="fixture-python-", dir=environment.parent))
    return create_owned_python_venv(owned, environment=environment)


def create_owned_python_venv(root: Path, *, environment: Path | None = None) -> Path:
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
    # Bytecode is a cache of the shared install; a fixture copy carries none, so a
    # test that asserts a child wrote no bytecode sees only that child's writes.
    shutil.copytree(
        base, install, symlinks=True, ignore=shutil.ignore_patterns("__pycache__")
    )
    base_executable = install / Path(sys._base_executable).resolve().relative_to(base)
    if environment is None:
        environment = root / "venv"
    link_mode = "--copies" if os.name == "nt" else "--symlinks"
    completed = run_guarded_test_process(
        [
            str(base_executable),
            # The copied base is a fixture; building its venv writes no bytecode,
            # so tests that assert a child wrote none observe only that child.
            "-B",
            "-m",
            "venv",
            "--without-pip",
            link_mode,
            str(environment),
        ],
        timeout=python_environment_identity.PYTHON_IDENTITY_PROBE_TIMEOUT_S,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"owned interpreter venv failed: {completed.stderr}")
    return environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def realized_environment_manifest(
    runtime: dict[str, object],
    packages: Sequence[Package],
    *,
    legacy_uv_lock: bool = False,
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
    # Every managed image carries its publisher's no-bytecode site hook.
    hook_path = f"{site_root}/{_contract.IMAGE_NO_BYTECODE_HOOK}"
    hook_bytes = _contract.IMAGE_NO_BYTECODE_HOOK_SOURCE
    file_paths = [selected, "pyvenv.cfg", hook_path, *installed_paths, *wheel_paths]
    if legacy_uv_lock:
        file_paths.append(".lock")
    file_paths.sort(key=lambda value: (value.casefold(), value))
    nodes_by_path = {
        path: f"file-node-{index}" for index, path in enumerate(file_paths)
    }
    sha_by_path = {
        ".lock": _EMPTY_FILE_SHA256,
        selected: "1" * 64,
        "pyvenv.cfg": "2" * 64,
        hook_path: hashlib.sha256(hook_bytes).hexdigest(),
        **{path: "4" * 64 for path in installed_paths},
        **{path: "5" * 64 for path in wheel_paths},
    }
    file_nodes = [
        {
            "id": nodes_by_path[path],
            "size": 0
            if path == ".lock"
            else len(hook_bytes)
            if path == hook_path
            else 1,
            "sha256": sha_by_path[path],
        }
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
            {"path": path, "kind": "directory", "access": _read_only_executable()}
            for path in sorted(
                directory_paths, key=lambda value: (value.casefold(), value)
            )
        ],
        {
            "path": selected,
            "kind": "file",
            "node": nodes_by_path[selected],
            "access": _read_only_executable(),
        },
        {
            "path": "pyvenv.cfg",
            "kind": "file",
            "node": nodes_by_path["pyvenv.cfg"],
            "access": _read_only_executable(),
        },
        *[
            {
                "path": path,
                "kind": "file",
                "node": nodes_by_path[path],
                "access": _read_only_executable(),
            }
            for path in [hook_path, *installed_paths, *wheel_paths]
        ],
    ]
    if legacy_uv_lock:
        entries.append(
            {
                "path": ".lock",
                "kind": "file",
                "node": nodes_by_path[".lock"],
                "access": {"readable": True, "writable": True, "executable": False},
            }
        )
    entries.sort(key=lambda row: (str(row["path"]).casefold(), str(row["path"])))
    material = {
        "schema": _contract.PYTHON_ENVIRONMENT_IDENTITY_SCHEMA,
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
        "external_import_custody": {
            **empty_external_import_custody(),
            "path_declarations": [
                {
                    "path": hook_path,
                    "node": nodes_by_path[hook_path],
                    "content": hook_bytes.decode("utf-8"),
                    "roles": [],
                }
            ],
            "reviewed_startup": [
                {
                    "capability": "molt-no-bytecode.v1",
                    "declaration": hook_path,
                    "declaration_provenance": _STARTUP_CAPABILITIES[
                        "molt-no-bytecode.v1"
                    ]["declaration_artifacts"][0],
                    "module": None,
                    "gate": {},
                }
            ],
        },
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
        "site_bootstrap_files": [{"path": hook_path, "node": nodes_by_path[hook_path]}],
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
        "producer_contract": _contract.source_build_environment_contract(),
        "declared_inputs": {
            "pyproject.toml": "a" * 64,
            "uv.lock": "b" * 64,
            ".python-version": None,
        },
        "schema_version": _contract.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_groups": [dependency_group],
        "editable_binding": None,
        "bytecode_policy": _contract.IMAGE_BYTECODE_POLICY,
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
