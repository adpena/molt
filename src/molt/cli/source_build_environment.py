"""Locked, addressable build-environment custody for source extensions."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
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
from molt.toolchain_identity import stable_executable_probe
from molt.dx import checkout_custody
from molt.exact_json import ExactJsonError, canonical_json_sha256, loads_exact
from molt import process_guard
from molt import python_environment_identity
from molt.python_environment_identity import (
    PYTHON_MARKER_ENVIRONMENT_FIELDS,
    PythonEnvironmentIdentityError,
    capture_current_python_runtime,
    environment_matches_lock_closure,
    selected_uv_lock_group_closure,
    validate_python_environment_identity,
    validate_python_runtime_identity,
    validate_uv_lock_group_closure,
)


class SourceBuildEnvironmentError(ValueError):
    pass


SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION = 5
SOURCE_BUILD_ENVIRONMENT_MANIFEST = "molt-source-build-environment.json"


class _SourceBuildAddress(TypedDict):
    schema_version: int
    dependency_group: str
    dependency_group_requirements: list[str]
    lock_closure: dict[str, object]
    python_runtime: dict[str, object]
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
    missing = [
        field for field in PYTHON_MARKER_ENVIRONMENT_FIELDS if field not in source
    ]
    if missing:
        raise SourceBuildEnvironmentError(
            "source build marker environment is missing: " + ", ".join(missing)
        )
    return {field: str(source[field]) for field in PYTHON_MARKER_ENVIRONMENT_FIELDS}


def _marker_environment_matches_runtime(
    marker: Mapping[str, str], runtime: Mapping[str, object]
) -> bool:
    version = str(runtime.get("version", ""))
    version_parts = version.split(".")
    architecture = {
        "amd64": "x86_64",
        "x86_64": "x86_64",
        "arm64": "arm64",
        "aarch64": "arm64",
    }.get(marker.get("platform_machine", "").casefold())
    os_identity = {
        "windows": ("nt", "win32", "Windows"),
        "macos": ("posix", "darwin", "Darwin"),
        "linux": ("posix", "linux", "Linux"),
    }.get(str(runtime.get("operating_system", "")))
    return (
        len(version_parts) == 3
        and os_identity is not None
        and marker.get("implementation_name") == "cpython"
        and marker.get("platform_python_implementation") == "CPython"
        and marker.get("implementation_version") == version
        and marker.get("python_full_version") == version
        and marker.get("python_version") == ".".join(version_parts[:2])
        and architecture == runtime.get("architecture")
        and (
            marker.get("os_name"),
            marker.get("sys_platform"),
            marker.get("platform_system"),
        )
        == os_identity
    )


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


def _python_identity() -> dict[str, object]:
    try:
        return capture_current_python_runtime()
    except PythonEnvironmentIdentityError as exc:
        raise SourceBuildEnvironmentError(str(exc)) from exc


def _uv_identity() -> tuple[Path, dict[str, str]]:
    raw_uv = shutil.which("uv")
    if raw_uv is None:
        raise SourceBuildEnvironmentError(
            "locked source-build environment provisioning requires uv on PATH"
        )
    uv = Path(raw_uv).resolve()
    # This is a bounded bootstrap identity probe, before the guarded build
    # environment exists. It never launches package build work.
    with stable_executable_probe(uv, label="source-build uv") as (
        entrypoint,
        executable,
    ):
        result = process_guard.run_completed_command(
            [str(entrypoint), "--version"],
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
        "sha256": executable.sha256,
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
    repo_root: Path,
    dependency_group: str,
    *,
    python_runtime: dict[str, object] | None = None,
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
    try:
        lock_closure = selected_uv_lock_group_closure(
            repo_root,
            dependency_group,
            group_requirements,
            marker_environment=canonical_source_marker_environment(),
        )
    except PythonEnvironmentIdentityError as exc:
        raise SourceBuildEnvironmentError(str(exc)) from exc
    if python_runtime is None:
        python_runtime = _python_identity()
    uv, uv_payload = _uv_identity()
    address_payload: _SourceBuildAddress = {
        "schema_version": SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_group": dependency_group,
        "dependency_group_requirements": list(group_requirements),
        "lock_closure": lock_closure,
        "python_runtime": python_runtime,
        "uv": uv_payload,
    }
    environment_id = canonical_json_sha256(address_payload)
    custody: _SourceBuildCustody = {
        "environment_id": environment_id,
        **address_payload,
    }
    custody_root = _source_build_custody_root(repo_root)
    root = custody_root / environment_id
    python_executable = root / (
        "Scripts/python.exe" if os.name == "nt" else "bin/python"
    )
    return (
        root,
        python_executable,
        root / SOURCE_BUILD_ENVIRONMENT_MANIFEST,
        custody,
        uv,
    )


def _source_build_custody_root(repo_root: Path) -> Path:
    return (
        checkout_custody(repo_root, os.environ).custody_root
        / "build-environments"
        / "source-extension"
    )


def _probe_environment_identity(
    python_executable: Path, root: Path
) -> dict[str, object]:
    """Capture the selected environment through the shared content authority."""

    probe_environment = os.environ.copy()
    probe_environment.pop("PYTHONHOME", None)
    probe_environment.pop("PYTHONPATH", None)
    probe_environment["PYTHONNOUSERSITE"] = "1"
    probe_environment["PYTHONDONTWRITEBYTECODE"] = "1"
    probe_source = Path(python_environment_identity.__file__).resolve(strict=True)
    probe_argv = [
        str(python_executable),
        "-I",
        str(probe_source),
        "--capture-environment",
        str(root.resolve()),
        "--admit-virtualenv-bootstrap",
    ]
    result = process_guard.run_completed_command(
        probe_argv,
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
            "cannot attest provisioned source-build environment content: "
            f"{detail or f'returncode={result.returncode}'}"
        )
    try:
        payload = loads_exact(result.stdout)
    except (json.JSONDecodeError, ExactJsonError) as exc:
        raise SourceBuildEnvironmentError(
            "source-build environment probe returned invalid JSON"
        ) from exc
    try:
        return validate_python_environment_identity(payload)
    except PythonEnvironmentIdentityError as exc:
        raise SourceBuildEnvironmentError(
            f"source-build environment probe returned an invalid payload: {exc}"
        ) from exc


def _attested_environment(
    custody: Mapping[str, object], realized: Mapping[str, object]
) -> dict[str, object]:
    lock_closure = custody.get("lock_closure")
    python_runtime = custody.get("python_runtime")
    if not isinstance(lock_closure, Mapping) or not isinstance(python_runtime, Mapping):
        raise SourceBuildEnvironmentError(
            "provisioned source-build environment has an incomplete recipe"
        )
    typed_lock_closure = cast(Mapping[str, object], lock_closure)
    if (
        not environment_matches_lock_closure(realized, typed_lock_closure)
        or realized.get("runtime") != python_runtime
    ):
        raise SourceBuildEnvironmentError(
            "provisioned source-build environment differs from its selected lock "
            "or CPython runtime closure"
        )
    return {**custody, "realized_environment": dict(realized)}


def _read_attestation(path: Path) -> Mapping[str, object] | None:
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError):
        return None
    return payload if isinstance(payload, Mapping) else None


def _provisioning_record(custody: Mapping[str, object]) -> dict[str, object]:
    return {"state": "provisioning", "custody": dict(custody)}


def _provisioning_record_path(root: Path) -> Path:
    return root.parent / ".provisioning" / f"{root.name}.json"


def _validated_active_attestation(
    *,
    root: Path,
    manifest_path: Path,
    custody: Mapping[str, object],
    realized: dict[str, object] | None = None,
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
    if realized is None:
        realized = _probe_environment_identity(Path(sys.executable), root)
    expected = _attested_environment(custody, realized)
    if manifest != expected:
        raise SourceBuildEnvironmentError(
            f"locked source-build environment attestation is stale or invalid: {manifest_path}"
        )
    return expected


def source_build_environment(
    repo_root: Path, dependency_group: str, *, provision: bool = False
) -> LockedSourceBuildEnvironment:
    active_root = Path(sys.prefix).resolve()
    realized = None
    active_manifest = None
    if active_root.parent == _source_build_custody_root(repo_root).resolve():
        active_manifest = _read_attestation(
            active_root / SOURCE_BUILD_ENVIRONMENT_MANIFEST
        )
        if active_manifest is None:
            raise SourceBuildEnvironmentError(
                "locked source-build environment attestation is stale or invalid"
            )
        realized = _probe_environment_identity(Path(sys.executable), active_root)
        spec = _environment_spec(
            repo_root,
            dependency_group,
            python_runtime=cast(dict[str, object], realized["runtime"]),
        )
    else:
        spec = _environment_spec(repo_root, dependency_group)
    root, python_executable, manifest_path, custody, _uv = spec
    active = Path(sys.prefix).resolve() == root.resolve()
    if not active and active_manifest is not None:
        if active_manifest.get("dependency_group") == dependency_group:
            raise SourceBuildEnvironmentError(
                "active source-build environment recipe/path differs from the requested dependency group"
            )
        # A request for another dependency group may re-exec into its own
        # environment, but the current namespace must still be authentic.
        prior_custody = {
            key: value
            for key, value in active_manifest.items()
            if key != "realized_environment"
        }
        prior_address = {
            key: value
            for key, value in prior_custody.items()
            if key != "environment_id"
        }
        if (
            prior_custody.get("environment_id") != active_root.name
            or canonical_json_sha256(prior_address) != active_root.name
            or realized is None
            or _attested_environment(prior_custody, realized) != active_manifest
        ):
            raise SourceBuildEnvironmentError(
                "active source-build environment attestation is stale or invalid"
            )
    if not active and provision:
        return _provision_source_build_environment(repo_root, dependency_group, spec)
    if active:
        custody = cast(
            _SourceBuildCustody,
            _validated_active_attestation(
                root=root,
                manifest_path=manifest_path,
                custody=custody,
                realized=realized,
            ),
        )
    return LockedSourceBuildEnvironment(
        root=root,
        python_executable=python_executable,
        manifest_path=manifest_path,
        custody=custody,
        active=active,
    )


def _run_uv_sync(
    argv: Sequence[str],
    *,
    cwd: Path,
    environment: Mapping[str, str],
) -> subprocess.CompletedProcess[bytes]:
    """Run the single provisioning mutation behind its module-owned seam."""

    return process_guard.run_completed_command(
        list(argv),
        cwd=cwd,
        env=dict(environment),
        check=False,
    )


def _provision_source_build_environment(
    repo_root: Path,
    dependency_group: str,
    spec: tuple[Path, Path, Path, _SourceBuildCustody, Path],
) -> LockedSourceBuildEnvironment:
    root, python_executable, manifest_path, custody, uv = spec
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
        complete_fields = {*custody, "realized_environment"}
        if (
            python_executable.is_file()
            and isinstance(existing, Mapping)
            and set(existing) == complete_fields
        ):
            expected_core = dict(custody)
            actual_core = {key: existing.get(key) for key in expected_core}
            if actual_core == expected_core:
                realized = _probe_environment_identity(python_executable, root)
                expected_attestation = _attested_environment(custody, realized)
                if existing == expected_attestation:
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
                        custody=expected_attestation,
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
        sync_argv = (
            str(uv),
            "sync",
            "--project",
            str(repo_root.resolve()),
            "--python",
            str(Path(raw_base).resolve()),
            "--frozen",
            "--no-default-groups",
            "--no-dev",
            "--group",
            dependency_group,
            "--no-install-project",
            "--compile-bytecode",
        )
        result = _run_uv_sync(
            sync_argv,
            cwd=repo_root,
            environment=environment,
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
        realized = _probe_environment_identity(python_executable, root)
        manifest = _attested_environment(custody, realized)
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
            custody=manifest,
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
    validated_lock: Mapping[str, object] | None = None
    validated_environment: Mapping[str, object] | None = None

    custody = payload.get("custody")
    recipe_keys = {
        "schema_version",
        "environment_id",
        "dependency_group",
        "dependency_group_requirements",
        "lock_closure",
        "python_runtime",
        "uv",
    }
    if not isinstance(custody, Mapping) or set(custody) != {
        *recipe_keys,
        "realized_environment",
    }:
        problems.append("extension-set manifest build-environment custody is invalid")
        custody = {}
    else:
        custody = cast(Mapping[str, object], custody)
        recipe = {key: custody[key] for key in recipe_keys if key != "environment_id"}
        environment_id = custody.get("environment_id")
        if (
            type(custody.get("schema_version")) is not int
            or custody.get("schema_version") != SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION
            or not isinstance(environment_id, str)
            or len(environment_id) != 64
            or any(character not in "0123456789abcdef" for character in environment_id)
            or environment_id != canonical_json_sha256(recipe)
        ):
            problems.append(
                "extension-set manifest build-environment address digest is invalid"
            )
        lock_closure = custody.get("lock_closure")
        python_runtime = custody.get("python_runtime")
        realized = custody.get("realized_environment")
        try:
            validated_lock = validate_uv_lock_group_closure(lock_closure)
            validated_runtime = validate_python_runtime_identity(python_runtime)
            validated_environment = validate_python_environment_identity(realized)
        except PythonEnvironmentIdentityError:
            problems.append(
                "extension-set manifest build-environment content custody is invalid"
            )
        else:
            if (
                validated_environment.get("runtime") != validated_runtime
                or not environment_matches_lock_closure(
                    validated_environment, validated_lock
                )
                or validated_lock.get("dependency_group")
                != custody.get("dependency_group")
                or validated_lock.get("requirements")
                != custody.get("dependency_group_requirements")
            ):
                problems.append(
                    "extension-set manifest build-environment content differs from recipe"
                )
        uv = custody.get("uv")
        if (
            not isinstance(uv, Mapping)
            or set(uv) != {"executable", "version", "sha256"}
            or not all(
                isinstance(uv.get(field), str) and uv.get(field)
                for field in ("executable", "version")
            )
            or any(separator in str(uv.get("executable")) for separator in ("/", "\\"))
            or not isinstance(uv.get("sha256"), str)
            or len(str(uv.get("sha256"))) != 64
            or any(
                character not in "0123456789abcdef"
                for character in str(uv.get("sha256"))
            )
        ):
            problems.append("extension-set manifest uv custody is invalid")

    python = payload.get("python")
    requirements = payload.get("requirements")
    raw_environment = payload.get("marker_environment")
    recorded_active = payload.get("active_requirements")
    resolved = payload.get("resolved")
    if (
        not isinstance(python, Mapping)
        or set(python) != {"implementation", "version", "executable"}
        or not all(isinstance(value, str) and value for value in python.values())
    ):
        problems.append("extension-set manifest build Python identity is invalid")
    if (
        not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item for item in requirements)
    ):
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    requirements = [item for item in requirements if isinstance(item, str) and item]
    if (
        not isinstance(raw_environment, Mapping)
        or set(raw_environment) != set(PYTHON_MARKER_ENVIRONMENT_FIELDS)
        or not all(isinstance(value, str) for value in raw_environment.values())
    ):
        problems.append("extension-set manifest marker environment is invalid")
        return problems
    marker_environment = {
        str(key): value
        for key, value in raw_environment.items()
        if isinstance(key, str) and isinstance(value, str)
    }
    if (
        validated_lock is not None
        and validated_lock.get("marker_environment") != marker_environment
    ):
        problems.append(
            "extension-set manifest build-environment lock marker environment "
            "differs from the recorded marker environment"
        )
    if validated_environment is not None and not _marker_environment_matches_runtime(
        marker_environment, validated_environment
    ):
        problems.append(
            "extension-set manifest marker environment differs from the realized "
            "Python runtime"
        )
    try:
        active = active_source_build_requirements(requirements, marker_environment)
    except SourceBuildEnvironmentError:
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    if isinstance(python, Mapping):
        executable = str(python.get("executable", ""))
        realized = custody.get("realized_environment")
        selected = (
            realized.get("selected_executable")
            if isinstance(realized, Mapping)
            else None
        )
        selected_path = (
            str(selected.get("path", "")) if isinstance(selected, Mapping) else ""
        )
        if (
            any(separator in executable for separator in ("/", "\\"))
            or str(python.get("implementation"))
            != str(marker_environment.get("implementation_name"))
            or str(python.get("version"))
            != str(marker_environment.get("python_full_version"))
            or Path(selected_path).name != executable
        ):
            problems.append("extension-set manifest build Python identity is invalid")
    expected_active = [raw for raw, _requirement in active]
    if recorded_active != expected_active:
        problems.append(
            "extension-set manifest active requirements do not match the recorded "
            "marker environment"
        )
    if not isinstance(resolved, list) or not resolved:
        problems.append("extension-set manifest resolved requirements are empty")
        return problems

    raw_realized_distributions = (
        validated_environment.get("distributions")
        if validated_environment is not None
        else None
    )
    realized_distributions = (
        raw_realized_distributions
        if isinstance(raw_realized_distributions, list)
        else []
    )
    realized_versions: dict[str, str] = {}
    for row in realized_distributions:
        if not isinstance(row, Mapping):
            continue
        typed_row = cast(Mapping[str, object], row)
        name = typed_row.get("name")
        version = typed_row.get("version")
        if isinstance(name, str) and isinstance(version, str):
            realized_versions[canonicalize_name(name)] = version
    resolved_requirements: list[str] = []
    for index, item in enumerate(resolved):
        if not isinstance(item, Mapping) or set(item) != {
            "requirement",
            "distribution",
            "version",
        }:
            problems.append(
                "extension-set manifest resolved requirement shape is invalid"
            )
            continue
        item = cast(Mapping[str, object], item)
        if not all(
            isinstance(item.get(field), str) and item.get(field)
            for field in ("requirement", "distribution", "version")
        ):
            problems.append(
                "extension-set manifest resolved requirement values are invalid"
            )
            continue
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
        if realized_versions.get(canonicalize_name(distribution)) != raw_version:
            problems.append(
                "extension-set manifest resolved distribution differs from the "
                f"realized environment for {raw!r}"
            )

    if resolved_requirements != expected_active:
        if len(resolved_requirements) == len(expected_active) and sorted(
            resolved_requirements
        ) == sorted(expected_active):
            problems.append(
                "extension-set manifest resolved requirements are out of source order"
            )
        else:
            problems.append(
                "extension-set manifest resolved requirements do not exactly cover "
                "the source requirement authority"
            )
    return problems
