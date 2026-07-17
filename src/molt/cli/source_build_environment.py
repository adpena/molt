"""Locked, addressable build-environment custody for source extensions."""

from __future__ import annotations

import hashlib
import importlib.metadata as importlib_metadata
import json
import os
import shutil
import subprocess
import sys
import sysconfig
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TypedDict, cast

from packaging.markers import default_environment
from packaging.requirements import InvalidRequirement, Requirement
from packaging.utils import canonicalize_name
from packaging.version import InvalidVersion, Version

from molt.cli.atomic_io import _atomic_write_json, _remove_file_or_tree
from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.file_hashing import _sha256_file
from molt.dx import canonical_molt_root


class SourceBuildEnvironmentError(ValueError):
    pass


SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION = 2
SOURCE_BUILD_ENVIRONMENT_MANIFEST = "molt-source-build-environment.json"

# Project-owned stable schema. Recording only the standard build-marker inputs
# avoids coupling seal identity to incidental keys added by packaging releases.
SOURCE_MARKER_ENVIRONMENT_FIELDS = (
    "implementation_name",
    "implementation_version",
    "os_name",
    "platform_machine",
    "platform_python_implementation",
    "platform_release",
    "platform_system",
    "platform_version",
    "python_full_version",
    "python_version",
    "sys_platform",
)


class _SourceBuildAddress(TypedDict):
    schema_version: int
    dependency_group: str
    dependency_group_requirements: list[str]
    uv_lock_sha256: str
    python: dict[str, str]
    uv: dict[str, str]


class _SourceBuildCustody(_SourceBuildAddress):
    environment_id: str


@dataclass(frozen=True)
class LockedSourceBuildEnvironment:
    root: Path
    python_executable: Path
    manifest_path: Path
    custody: Mapping[str, object]
    active: bool


def canonical_source_marker_environment(
    environment: Mapping[str, str] | None = None,
) -> dict[str, str]:
    source = default_environment() if environment is None else environment
    missing = [field for field in SOURCE_MARKER_ENVIRONMENT_FIELDS if field not in source]
    if missing:
        raise SourceBuildEnvironmentError(
            "source build marker environment is missing: " + ", ".join(missing)
        )
    return {field: str(source[field]) for field in SOURCE_MARKER_ENVIRONMENT_FIELDS}


def active_source_build_requirements(
    requirements: Sequence[str], marker_environment: Mapping[str, str]
) -> tuple[tuple[str, Requirement], ...]:
    environment = canonical_source_marker_environment(marker_environment)
    active: list[tuple[str, Requirement]] = []
    for raw in requirements:
        try:
            requirement = Requirement(raw)
        except InvalidRequirement as exc:
            raise SourceBuildEnvironmentError(
                f"invalid source build requirement {raw!r}: {exc}"
            ) from exc
        if requirement.url is not None:
            raise SourceBuildEnvironmentError(
                "source build requirements with direct URLs cannot be revalidated "
                f"from installed distribution metadata: {raw!r}"
            )
        if requirement.marker is None or requirement.marker.evaluate(
            environment=environment
        ):
            active.append((raw, requirement))
    return tuple(active)


def _python_identity() -> dict[str, str]:
    raw_base = getattr(sys, "_base_executable", None) or sys.executable
    base_executable = Path(raw_base).resolve()
    if not base_executable.is_file():
        raise SourceBuildEnvironmentError(
            f"cannot attest source-build base Python executable: {base_executable}"
        )
    return {
        "implementation": sys.implementation.name,
        "version": (
            f"{sys.version_info.major}.{sys.version_info.minor}."
            f"{sys.version_info.micro}"
        ),
        "platform": sysconfig.get_platform(),
        "base_executable": base_executable.name,
        "base_executable_sha256": _sha256_file(base_executable),
    }


def _uv_identity() -> tuple[Path, dict[str, str]]:
    raw_uv = shutil.which("uv")
    if raw_uv is None:
        raise SourceBuildEnvironmentError(
            "locked source-build environment provisioning requires uv on PATH"
        )
    uv = Path(raw_uv).resolve()
    # This is a bounded bootstrap identity probe, before the guarded build
    # environment exists. It never launches package build work.
    result = subprocess.run(
        [str(uv), "--version"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    version = result.stdout.strip()
    if result.returncode != 0 or not version:
        detail = (result.stderr or result.stdout).strip()
        raise SourceBuildEnvironmentError(
            "cannot attest uv for locked source-build provisioning: "
            f"{detail or f'returncode={result.returncode}'}"
        )
    return uv, {
        "executable": uv.name,
        "version": version,
        "sha256": _sha256_file(uv),
    }


def _declared_dependency_group(
    repo_root: Path, dependency_group: str
) -> tuple[str, ...]:
    try:
        payload = tomllib.loads(
            (repo_root / "pyproject.toml").read_text(encoding="utf-8")
        )
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise SourceBuildEnvironmentError(
            f"cannot read source-build dependency-group authority: {exc}"
        ) from exc
    groups = payload.get("dependency-groups")
    requirements = groups.get(dependency_group) if isinstance(groups, Mapping) else None
    if (
        not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item.strip() for item in requirements)
    ):
        raise SourceBuildEnvironmentError(
            f"source-build dependency group {dependency_group!r} is not declared"
        )
    normalized = tuple(item.strip() for item in requirements)
    for raw in normalized:
        try:
            requirement = Requirement(raw)
        except InvalidRequirement as exc:
            raise SourceBuildEnvironmentError(
                f"invalid requirement in source-build group {dependency_group!r}: "
                f"{raw!r}: {exc}"
            ) from exc
        if requirement.url is not None:
            raise SourceBuildEnvironmentError(
                f"source-build dependency group {dependency_group!r} contains "
                f"an unverifiable direct URL: {raw!r}"
            )
    return normalized


def _environment_spec(
    repo_root: Path, dependency_group: str
) -> tuple[Path, Path, Path, _SourceBuildCustody, Path]:
    repo_root = repo_root.resolve()
    if not dependency_group or any(
        character not in "abcdefghijklmnopqrstuvwxyz0123456789-_"
        for character in dependency_group
    ):
        raise SourceBuildEnvironmentError(
            f"invalid source-build dependency group {dependency_group!r}"
        )
    group_requirements = _declared_dependency_group(repo_root, dependency_group)
    lock_path = repo_root / "uv.lock"
    if not lock_path.is_file():
        raise SourceBuildEnvironmentError(f"locked source-build input is absent: {lock_path}")
    lock_digest = _sha256_file(lock_path)
    python = _python_identity()
    uv, uv_payload = _uv_identity()
    address_payload: _SourceBuildAddress = {
        "schema_version": SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_group": dependency_group,
        "dependency_group_requirements": list(group_requirements),
        "uv_lock_sha256": lock_digest,
        "python": python,
        "uv": uv_payload,
    }
    environment_id = hashlib.sha256(
        json.dumps(address_payload, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    custody: _SourceBuildCustody = {
        "environment_id": environment_id,
        **address_payload,
    }
    custody_root = canonical_molt_root(repo_root) / "build-environments" / "source-extension"
    root = custody_root / environment_id
    python_executable = root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    return root, python_executable, root / SOURCE_BUILD_ENVIRONMENT_MANIFEST, custody, uv


def _canonical_distribution_roots() -> tuple[str, ...]:
    roots: list[str] = []
    seen: set[str] = set()
    for scheme in ("purelib", "platlib"):
        raw_root = sysconfig.get_path(scheme)
        if not isinstance(raw_root, str) or not raw_root.strip():
            raise SourceBuildEnvironmentError(
                f"source-build environment has no {scheme} sysconfig path"
            )
        root = str(Path(raw_root).resolve())
        identity = os.path.normcase(root)
        if identity not in seen:
            seen.add(identity)
            roots.append(root)
    return tuple(roots)


def _installed_distributions() -> list[dict[str, str]]:
    rows: dict[str, dict[str, str]] = {}
    for distribution in importlib_metadata.distributions(
        path=list(_canonical_distribution_roots())
    ):
        raw_name = distribution.metadata.get("Name")
        if not isinstance(raw_name, str) or not raw_name.strip():
            raise SourceBuildEnvironmentError(
                "source-build environment contains a distribution without Name metadata"
            )
        name = canonicalize_name(raw_name)
        row = {"name": name, "version": distribution.version}
        previous = rows.get(name)
        if previous is not None and previous != row:
            raise SourceBuildEnvironmentError(
                f"source-build environment contains duplicate distribution {name!r}"
            )
        rows[name] = row
    return [rows[name] for name in sorted(rows)]


_DISTRIBUTION_PROBE = """
import importlib.metadata as m, json, os, sysconfig
from pathlib import Path
from packaging.utils import canonicalize_name
roots = []
seen = set()
for scheme in ('purelib', 'platlib'):
    raw_root = sysconfig.get_path(scheme)
    if not isinstance(raw_root, str) or not raw_root.strip():
        raise SystemExit('missing sysconfig path: ' + scheme)
    root = str(Path(raw_root).resolve())
    identity = os.path.normcase(root)
    if identity not in seen:
        seen.add(identity)
        roots.append(root)
rows = {}
for distribution in m.distributions(path=roots):
    raw_name = distribution.metadata.get('Name')
    if not isinstance(raw_name, str) or not raw_name.strip():
        raise SystemExit('distribution without Name metadata')
    name = canonicalize_name(raw_name)
    row = {'name': name, 'version': distribution.version}
    if name in rows and rows[name] != row:
        raise SystemExit('duplicate distribution: ' + name)
    rows[name] = row
print(json.dumps([rows[name] for name in sorted(rows)], separators=(',', ':')))
"""


def _probe_environment_distributions(python_executable: Path) -> list[dict[str, str]]:
    # This bounded, read-only bootstrap probe is the trust boundary used to
    # decide whether an environment may launch guarded package build work.
    probe_environment = os.environ.copy()
    probe_environment.pop("PYTHONHOME", None)
    probe_environment.pop("PYTHONPATH", None)
    probe_environment["PYTHONNOUSERSITE"] = "1"
    result = subprocess.run(
        [str(python_executable), "-P", "-c", _DISTRIBUTION_PROBE],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        env=probe_environment,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SourceBuildEnvironmentError(
            "cannot attest provisioned source-build distributions: "
            f"{detail or f'returncode={result.returncode}'}"
        )
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise SourceBuildEnvironmentError(
            "source-build distribution probe returned invalid JSON"
        ) from exc
    if not isinstance(payload, list) or not all(
        isinstance(item, dict)
        and set(item) == {"name", "version"}
        and all(isinstance(value, str) and value for value in item.values())
        for item in payload
    ):
        raise SourceBuildEnvironmentError(
            "source-build distribution probe returned an invalid payload"
        )
    return payload


def _validate_declared_group_resolutions(
    requirements: Sequence[str], distributions: Sequence[Mapping[str, str]]
) -> None:
    installed = {item["name"]: item["version"] for item in distributions}
    marker_environment = canonical_source_marker_environment()
    missing: list[str] = []
    for raw in requirements:
        requirement = Requirement(raw)
        if requirement.marker is not None and not requirement.marker.evaluate(
            environment=marker_environment
        ):
            continue
        version = installed.get(canonicalize_name(requirement.name))
        if version is None or (
            requirement.specifier
            and not requirement.specifier.contains(version, prereleases=True)
        ):
            missing.append(raw)
    if missing:
        raise SourceBuildEnvironmentError(
            "provisioned source-build environment does not satisfy its declared "
            "dependency group: " + ", ".join(missing)
        )


def _read_attestation(path: Path) -> Mapping[str, object] | None:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    return payload if isinstance(payload, Mapping) else None


def _provisioning_record(custody: Mapping[str, object]) -> dict[str, object]:
    return {"state": "provisioning", "custody": dict(custody)}


def _provisioning_record_path(root: Path) -> Path:
    return root.parent / ".provisioning" / f"{root.name}.json"


def _validated_active_attestation(
    *, root: Path, manifest_path: Path, custody: Mapping[str, object]
) -> Mapping[str, object]:
    try:
        active_root = Path(sys.prefix).resolve(strict=True)
    except OSError as exc:
        raise SourceBuildEnvironmentError(
            f"cannot resolve active source-build environment: {exc}"
        ) from exc
    if active_root != root.resolve():
        raise SourceBuildEnvironmentError(
            f"active interpreter is not the locked source-build environment {root}"
        )
    manifest = _read_attestation(manifest_path)
    expected = {**custody, "installed_distributions": _installed_distributions()}
    if manifest != expected:
        raise SourceBuildEnvironmentError(
            f"locked source-build environment attestation is stale or invalid: {manifest_path}"
        )
    return custody


def source_build_environment(
    repo_root: Path, dependency_group: str
) -> LockedSourceBuildEnvironment:
    root, python_executable, manifest_path, custody, _uv = _environment_spec(
        repo_root, dependency_group
    )
    active = Path(sys.prefix).resolve() == root.resolve()
    if active:
        _validated_active_attestation(
            root=root, manifest_path=manifest_path, custody=custody
        )
    return LockedSourceBuildEnvironment(
        root=root,
        python_executable=python_executable,
        manifest_path=manifest_path,
        custody=custody,
        active=active,
    )


def provision_source_build_environment(
    repo_root: Path, dependency_group: str
) -> LockedSourceBuildEnvironment:
    root, python_executable, manifest_path, custody, uv = _environment_spec(
        repo_root, dependency_group
    )
    lock_path = root.parent / ".locks" / f"{root.name}.lock"
    provisioning_path = _provisioning_record_path(root)
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=900.0,
        timeout_message=(
            "timed out waiting for locked source-build environment provisioning "
            f"lock {lock_path}"
        ),
    )
    try:
        existing = _read_attestation(manifest_path)
        provisioning = _read_attestation(provisioning_path)
        expected_provisioning = _provisioning_record(custody)
        if provisioning_path.exists() and provisioning is None:
            raise SourceBuildEnvironmentError(
                f"malformed source-build provisioning record: {provisioning_path}"
            )
        complete_fields = {*custody, "installed_distributions"}
        if (
            python_executable.is_file()
            and isinstance(existing, Mapping)
            and set(existing) == complete_fields
        ):
            expected_core = dict(custody)
            actual_core = {key: existing.get(key) for key in expected_core}
            if actual_core == expected_core:
                installed = _probe_environment_distributions(python_executable)
                if existing.get("installed_distributions") == installed:
                    if provisioning is not None:
                        if provisioning != expected_provisioning:
                            raise SourceBuildEnvironmentError(
                                "complete source-build environment has a foreign "
                                f"provisioning record: {provisioning_path}"
                            )
                        provisioning_path.unlink()
                    return LockedSourceBuildEnvironment(
                        root=root,
                        python_executable=python_executable,
                        manifest_path=manifest_path,
                        custody=custody,
                        active=False,
                    )
        if root.exists():
            if provisioning != expected_provisioning:
                raise SourceBuildEnvironmentError(
                    "immutable source-build environment address exists without "
                    f"its exact attestation or sibling provisioning record: {root}"
                )
            _remove_file_or_tree(root)
        elif provisioning is not None and provisioning != expected_provisioning:
            raise SourceBuildEnvironmentError(
                f"foreign source-build provisioning record: {provisioning_path}"
            )
        root.parent.mkdir(parents=True, exist_ok=True)
        if provisioning is None:
            _atomic_write_json(
                provisioning_path,
                expected_provisioning,
                sort_keys=True,
            )

        environment = os.environ.copy()
        environment["UV_PROJECT_ENVIRONMENT"] = str(root)
        raw_base = getattr(sys, "_base_executable", None) or sys.executable
        # uv is the provisioner for the environment that will subsequently
        # launch guarded build work. The canonical file lock and provisional
        # record make this direct-final mutation recoverable but inadmissible;
        # the complete attestation is the logical publication point.
        result = subprocess.run(
            [
                str(uv),
                "sync",
                "--project",
                str(repo_root.resolve()),
                "--python",
                str(Path(raw_base).resolve()),
                "--frozen",
                "--no-default-groups",
                "--group",
                dependency_group,
                "--no-install-project",
            ],
            cwd=repo_root,
            env=environment,
            check=False,
        )
        if result.returncode != 0:
            raise SourceBuildEnvironmentError(
                "locked source-build environment provisioning failed: "
                f"uv sync returned {result.returncode}"
            )
        if not python_executable.is_file():
            raise SourceBuildEnvironmentError(
                f"uv sync did not create source-build Python: {python_executable}"
            )
        installed = _probe_environment_distributions(python_executable)
        _validate_declared_group_resolutions(
            custody["dependency_group_requirements"], installed
        )
        manifest = {**custody, "installed_distributions": installed}
        _atomic_write_json(
            manifest_path,
            manifest,
            sort_keys=True,
        )
        provisioning_path.unlink()
        return LockedSourceBuildEnvironment(
            root=root,
            python_executable=python_executable,
            manifest_path=manifest_path,
            custody=custody,
            active=False,
        )
    finally:
        _release_file_lock(handle)


def source_build_environment_problems(payload: object) -> list[str]:
    expected_fields = {
        "python",
        "requirements",
        "marker_environment",
        "active_requirements",
        "resolved",
        "custody",
    }
    if not isinstance(payload, Mapping) or set(payload) != expected_fields:
        return ["extension-set manifest build_environment shape is invalid"]
    payload = cast(Mapping[str, object], payload)

    problems: list[str] = []
    python = payload.get("python")
    requirements = payload.get("requirements")
    raw_environment = payload.get("marker_environment")
    recorded_active = payload.get("active_requirements")
    resolved = payload.get("resolved")
    custody = payload.get("custody")
    if not isinstance(custody, Mapping) or set(custody) != {
        "schema_version",
        "environment_id",
        "dependency_group",
        "dependency_group_requirements",
        "uv_lock_sha256",
        "python",
        "uv",
    }:
        problems.append("extension-set manifest build-environment custody is invalid")
    else:
        custody = cast(Mapping[str, object], custody)

        def valid_sha256(value: object) -> bool:
            return (
                isinstance(value, str)
                and len(value) == 64
                and all(character in "0123456789abcdef" for character in value)
            )

        digest_fields = ("environment_id", "uv_lock_sha256")
        if custody.get("schema_version") != SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION or any(
            not valid_sha256(custody.get(field))
            for field in digest_fields
        ):
            problems.append("extension-set manifest build-environment custody is invalid")
        if not isinstance(custody.get("dependency_group"), str) or not custody.get(
            "dependency_group"
        ):
            problems.append("extension-set manifest build-environment custody is invalid")
        group_requirements = custody.get("dependency_group_requirements")
        candidate_group_requirements = (
            [item for item in group_requirements if isinstance(item, str) and item]
            if isinstance(group_requirements, list)
            else []
        )
        group_requirements_are_valid = not (
            not isinstance(group_requirements, list)
            or not group_requirements
            or len(candidate_group_requirements) != len(group_requirements)
        )
        normalized_group_requirements = (
            candidate_group_requirements if group_requirements_are_valid else None
        )
        if normalized_group_requirements is None:
            problems.append("extension-set manifest build dependency group is invalid")
        else:
            try:
                parsed_group = [Requirement(item) for item in normalized_group_requirements]
            except InvalidRequirement:
                problems.append(
                    "extension-set manifest build dependency group is invalid"
                )
            else:
                if any(item.url is not None for item in parsed_group):
                    problems.append(
                        "extension-set manifest build dependency group is invalid"
                    )
        custody_python = custody.get("python")
        custody_uv = custody.get("uv")
        if not isinstance(custody_python, Mapping) or set(custody_python) != {
            "implementation",
            "version",
            "platform",
            "base_executable",
            "base_executable_sha256",
        }:
            problems.append("extension-set manifest build Python custody is invalid")
        elif (
            not all(
                isinstance(custody_python.get(field), str)
                and custody_python.get(field)
                for field in (
                    "implementation",
                    "version",
                    "platform",
                    "base_executable",
                )
            )
            or any(
                separator in str(custody_python.get("base_executable"))
                for separator in ("/", "\\")
            )
            or not valid_sha256(custody_python.get("base_executable_sha256"))
        ):
            problems.append("extension-set manifest build Python custody is invalid")
        if not isinstance(custody_uv, Mapping) or set(custody_uv) != {
            "executable",
            "version",
            "sha256",
        }:
            problems.append("extension-set manifest uv custody is invalid")
        elif (
            not all(
                isinstance(custody_uv.get(field), str) and custody_uv.get(field)
                for field in ("executable", "version")
            )
            or any(
                separator in str(custody_uv.get("executable"))
                for separator in ("/", "\\")
            )
            or not valid_sha256(custody_uv.get("sha256"))
        ):
            problems.append("extension-set manifest uv custody is invalid")
        if (
            isinstance(custody.get("dependency_group"), str)
            and valid_sha256(custody.get("uv_lock_sha256"))
            and isinstance(custody_python, Mapping)
            and normalized_group_requirements is not None
            and isinstance(custody_uv, Mapping)
        ):
            address_payload = {
                "schema_version": custody["schema_version"],
                "dependency_group": custody["dependency_group"],
                "dependency_group_requirements": normalized_group_requirements,
                "uv_lock_sha256": custody["uv_lock_sha256"],
                "python": dict(custody_python),
                "uv": dict(custody_uv) if isinstance(custody_uv, Mapping) else custody_uv,
            }
            expected_environment_id = hashlib.sha256(
                json.dumps(
                    address_payload, sort_keys=True, separators=(",", ":")
                ).encode()
            ).hexdigest()
            if custody.get("environment_id") != expected_environment_id:
                problems.append(
                    "extension-set manifest build-environment address digest is invalid"
                )

    if not isinstance(python, Mapping) or set(python) != {
        "implementation",
        "version",
        "executable",
    } or not all(isinstance(value, str) and value for value in python.values()):
        problems.append("extension-set manifest build Python identity is invalid")
    if (
        not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item for item in requirements)
    ):
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    requirements = [item for item in requirements if isinstance(item, str) and item]
    if not isinstance(raw_environment, Mapping) or set(raw_environment) != set(
        SOURCE_MARKER_ENVIRONMENT_FIELDS
    ) or not all(isinstance(value, str) for value in raw_environment.values()):
        problems.append("extension-set manifest marker environment is invalid")
        return problems
    raw_environment = {
        str(key): value
        for key, value in raw_environment.items()
        if isinstance(key, str) and isinstance(value, str)
    }
    try:
        active = active_source_build_requirements(requirements, raw_environment)
    except SourceBuildEnvironmentError:
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    if isinstance(python, Mapping):
        executable = str(python.get("executable", ""))
        if (
            any(separator in executable for separator in ("/", "\\"))
            or str(python.get("implementation"))
            != str(raw_environment.get("implementation_name"))
            or str(python.get("version"))
            != str(raw_environment.get("python_full_version"))
        ):
            problems.append("extension-set manifest build Python identity is invalid")
        custody_python_value = custody.get("python") if isinstance(custody, Mapping) else None
        if isinstance(custody_python_value, Mapping):
            custody_python = cast(Mapping[str, object], custody_python_value)
            if (
                python.get("implementation") != custody_python.get("implementation")
                or python.get("version") != custody_python.get("version")
            ):
                problems.append(
                    "extension-set manifest build Python identity differs from custody"
                )
    expected_active = [raw for raw, _requirement in active]
    if recorded_active != expected_active:
        problems.append(
            "extension-set manifest active requirements do not match the recorded "
            "marker environment"
        )
    if not isinstance(resolved, list) or not resolved:
        problems.append("extension-set manifest resolved requirements are empty")
        return problems

    resolved_requirements: list[str] = []
    for index, item in enumerate(resolved):
        if not isinstance(item, Mapping) or set(item) != {
            "requirement",
            "distribution",
            "version",
        }:
            problems.append("extension-set manifest resolved requirement shape is invalid")
            continue
        if not all(
            isinstance(item.get(field), str) and item.get(field)
            for field in ("requirement", "distribution", "version")
        ):
            problems.append("extension-set manifest resolved requirement values are invalid")
            continue
        item = cast(Mapping[str, object], item)
        raw = cast(str, item["requirement"])
        distribution = cast(str, item["distribution"])
        raw_version = cast(str, item["version"])
        resolved_requirements.append(raw)
        if index >= len(active) or raw != active[index][0]:
            continue
        requirement = active[index][1]
        if canonicalize_name(distribution) != canonicalize_name(requirement.name):
            problems.append(
                f"extension-set manifest resolved distribution does not satisfy {raw!r}"
            )
            continue
        try:
            version = Version(raw_version)
        except InvalidVersion:
            problems.append(
                f"extension-set manifest resolved version is invalid for {raw!r}"
            )
            continue
        if requirement.specifier and not requirement.specifier.contains(
            version, prereleases=True
        ):
            problems.append(
                f"extension-set manifest resolved version does not satisfy {raw!r}"
            )

    if resolved_requirements != expected_active:
        if (
            len(resolved_requirements) == len(expected_active)
            and sorted(resolved_requirements) == sorted(expected_active)
        ):
            problems.append(
                "extension-set manifest resolved requirements are out of source order"
            )
        else:
            problems.append(
                "extension-set manifest resolved requirements do not exactly cover "
                "the source requirement authority"
            )
    return problems
