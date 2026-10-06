"""Locked, addressable build-environment custody for source extensions."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import cast

from packaging.requirements import InvalidRequirement, Requirement

from molt.cli import source_build_environment_schema as _schema
from molt.cli.atomic_io import _atomic_write_json, _remove_file_or_tree
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.toolchain_identity import stable_executable_probe
from molt.dx import checkout_custody
from molt.exact_json import ExactJsonError, canonical_json_sha256, loads_exact
from molt import process_guard
from molt import python_environment_identity
from molt.python_environment_identity import (
    PythonEnvironmentIdentityError,
    environment_matches_lock_closure,
    selected_uv_lock_group_closure,
    validate_python_environment_identity,
    validate_python_runtime_identity,
)


def _python_identity() -> dict[str, object]:
    # The caller's imported extensions (for example proof_queue's sqlite3)
    # are not the interpreter recipe. Capture through the same isolated probe
    # used to attest provisioned environments, before arbitrary client imports.
    base = getattr(sys, "_base_executable", None) or sys.executable
    return _probe_source_build_python(Path(base).resolve(strict=True))


def _uv_identity() -> tuple[Path, dict[str, str]]:
    raw_uv = shutil.which("uv")
    if raw_uv is None:
        raise _schema.SourceBuildEnvironmentError(
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
        raise _schema.SourceBuildEnvironmentError(
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
        raise _schema.SourceBuildEnvironmentError(
            f"cannot read source-build dependency-group authority: {exc}"
        ) from exc
    groups = payload.get("dependency-groups")
    requirements = groups.get(dependency_group) if isinstance(groups, Mapping) else None
    if (
        not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item.strip() for item in requirements)
    ):
        raise _schema.SourceBuildEnvironmentError(
            f"source-build dependency group {dependency_group!r} is not declared"
        )
    normalized = tuple(item.strip() for item in requirements)
    for raw in normalized:
        try:
            requirement = Requirement(raw)
        except InvalidRequirement as exc:
            raise _schema.SourceBuildEnvironmentError(
                f"invalid requirement in source-build group {dependency_group!r}: "
                f"{raw!r}: {exc}"
            ) from exc
        if requirement.url is not None:
            raise _schema.SourceBuildEnvironmentError(
                f"source-build dependency group {dependency_group!r} contains "
                f"an unverifiable direct URL: {raw!r}"
            )
    return normalized


def _environment_spec(
    repo_root: Path,
    dependency_group: str,
    *,
    python_runtime: dict[str, object] | None = None,
) -> tuple[Path, Path, Path, _schema._SourceBuildCustody, Path]:
    repo_root = repo_root.resolve()
    if not dependency_group or any(
        character not in "abcdefghijklmnopqrstuvwxyz0123456789-_"
        for character in dependency_group
    ):
        raise _schema.SourceBuildEnvironmentError(
            f"invalid source-build dependency group {dependency_group!r}"
        )
    group_requirements = _declared_dependency_group(repo_root, dependency_group)
    try:
        lock_closure = selected_uv_lock_group_closure(
            repo_root,
            dependency_group,
            group_requirements,
            marker_environment=_schema.canonical_source_marker_environment(),
        )
    except PythonEnvironmentIdentityError as exc:
        raise _schema.SourceBuildEnvironmentError(str(exc)) from exc
    if python_runtime is None:
        python_runtime = _python_identity()
    uv, uv_payload = _uv_identity()
    address_payload: _schema._SourceBuildAddress = {
        "schema_version": _schema.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_group": dependency_group,
        "dependency_group_requirements": list(group_requirements),
        "lock_closure": lock_closure,
        "python_runtime": python_runtime,
        "uv": uv_payload,
    }
    environment_id = canonical_json_sha256(address_payload)
    custody: _schema._SourceBuildCustody = {
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
        root / _schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
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

    return _probe_source_build_python(python_executable, root=root)


def _probe_source_build_python(
    python_executable: Path, *, root: Path | None = None
) -> dict[str, object]:
    """One isolated bootstrap for recipe and realized-environment identities."""

    probe_environment = os.environ.copy()
    probe_environment.pop("PYTHONHOME", None)
    probe_environment.pop("PYTHONPATH", None)
    probe_environment["PYTHONNOUSERSITE"] = "1"
    if root is None:
        # A recipe addresses the base interpreter, not installed site startup
        # hooks. Realized environment bootstrap is admitted separately below.
        probe_arguments = ["--capture-runtime"]
    else:
        probe_arguments = [
            "--capture-environment",
            str(root.resolve()),
            "--admit-virtualenv-bootstrap",
        ]
    probe_argv = [
        str(python_executable),
        *python_environment_identity.python_identity_probe_arguments(
            probe_arguments, no_site=root is None
        ),
    ]
    kind = "runtime" if root is None else "environment"
    result = process_guard.run_completed_command(
        probe_argv,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        env=probe_environment,
        timeout=120,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise _schema.SourceBuildEnvironmentError(
            f"cannot attest source-build {kind} content: "
            f"{detail or f'returncode={result.returncode}'}"
        )
    try:
        payload = loads_exact(result.stdout)
    except (json.JSONDecodeError, ExactJsonError) as exc:
        raise _schema.SourceBuildEnvironmentError(
            f"source-build {kind} probe returned invalid JSON"
        ) from exc
    try:
        return (
            validate_python_runtime_identity(payload)
            if root is None
            else validate_python_environment_identity(payload)
        )
    except PythonEnvironmentIdentityError as exc:
        raise _schema.SourceBuildEnvironmentError(
            f"source-build {kind} probe returned an invalid payload: {exc}"
        ) from exc


def _attested_environment(
    custody: Mapping[str, object], realized: Mapping[str, object]
) -> dict[str, object]:
    lock_closure = custody.get("lock_closure")
    python_runtime = custody.get("python_runtime")
    if not isinstance(lock_closure, Mapping) or not isinstance(python_runtime, Mapping):
        raise _schema.SourceBuildEnvironmentError(
            "provisioned source-build environment has an incomplete recipe"
        )
    typed_lock_closure = cast(Mapping[str, object], lock_closure)
    if (
        not environment_matches_lock_closure(realized, typed_lock_closure)
        or realized.get("runtime") != python_runtime
    ):
        raise _schema.SourceBuildEnvironmentError(
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
        raise _schema.SourceBuildEnvironmentError(
            f"cannot resolve active source-build environment: {exc}"
        ) from exc
    if active_root != root.resolve():
        raise _schema.SourceBuildEnvironmentError(
            f"active interpreter is not the locked source-build environment {root}"
        )
    manifest = _read_attestation(manifest_path)
    if realized is None:
        realized = _probe_environment_identity(Path(sys.executable), root)
    expected = _attested_environment(custody, realized)
    if manifest != expected:
        raise _schema.SourceBuildEnvironmentError(
            f"locked source-build environment attestation is stale or invalid: {manifest_path}"
        )
    return expected


def source_build_environment(
    repo_root: Path, dependency_group: str, *, provision: bool = False
) -> _schema.LockedSourceBuildEnvironment:
    active_root = Path(sys.prefix).resolve()
    realized = None
    active_manifest = None
    if active_root.parent == _source_build_custody_root(repo_root).resolve():
        active_manifest = _read_attestation(
            active_root / _schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST
        )
        if active_manifest is None:
            raise _schema.SourceBuildEnvironmentError(
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
            raise _schema.SourceBuildEnvironmentError(
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
            raise _schema.SourceBuildEnvironmentError(
                "active source-build environment attestation is stale or invalid"
            )
    if not active and provision:
        return _provision_source_build_environment(repo_root, dependency_group, spec)
    if active:
        custody = cast(
            _schema._SourceBuildCustody,
            _validated_active_attestation(
                root=root,
                manifest_path=manifest_path,
                custody=custody,
                realized=realized,
            ),
        )
    return _schema.LockedSourceBuildEnvironment(
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
    spec: tuple[Path, Path, Path, _schema._SourceBuildCustody, Path],
) -> _schema.LockedSourceBuildEnvironment:
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
            raise _schema.SourceBuildEnvironmentError(
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
                            raise _schema.SourceBuildEnvironmentError(
                                "complete source-build environment has a foreign "
                                f"provisioning record: {provisioning_path}"
                            )
                        provisioning_path.unlink()
                    return _schema.LockedSourceBuildEnvironment(
                        root=root,
                        python_executable=python_executable,
                        manifest_path=manifest_path,
                        custody=expected_attestation,
                        active=False,
                    )
        if root.exists():
            if provisioning != expected_provisioning:
                raise _schema.SourceBuildEnvironmentError(
                    "immutable source-build environment address exists without "
                    f"its exact attestation or sibling provisioning record: {root}"
                )
            _remove_file_or_tree(root)
        elif provisioning is not None and provisioning != expected_provisioning:
            raise _schema.SourceBuildEnvironmentError(
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
        # This sync has an exact destination; the launcher's active venv is not
        # a second environment selector (nor a useful uv mismatch warning).
        environment.pop("VIRTUAL_ENV", None)
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
            raise _schema.SourceBuildEnvironmentError(
                "locked source-build environment provisioning failed: "
                f"uv sync returned {result.returncode}"
            )
        if not python_executable.is_file():
            raise _schema.SourceBuildEnvironmentError(
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
        return _schema.LockedSourceBuildEnvironment(
            root=root,
            python_executable=python_executable,
            manifest_path=manifest_path,
            custody=manifest,
            active=False,
        )
    finally:
        _release_file_lock(handle)
