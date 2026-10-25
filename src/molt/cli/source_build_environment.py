"""Exact-address publication for development and source-build Python images."""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

import json
import hashlib
import os
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Mapping, Sequence
from contextlib import nullcontext
from pathlib import Path
from typing import cast

from packaging.requirements import InvalidRequirement, Requirement

from molt.cli import source_build_environment_schema as _schema
from molt.cli.atomic_io import _atomic_write_json, _remove_file_or_tree
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.toolchain_identity import stable_executable_probe
from molt.python_file_node_custody import PythonFileCaptureContext
from molt.dx import checkout_custody, artifact_root, checkout_component, DxProject
from molt.exact_json import ExactJsonError, canonical_json_sha256, loads_exact
from molt import process_guard
from molt import python_environment_identity
from molt.python_uv_lock_identity import (
    packaging_tag_source,
    validate_python_wheel_target,
)
from molt.python_environment_location import (
    environment_python_path,
    environment_bytecode_prefix,
    environment_provisioning_record_path,
)
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
    return _probe_source_build_python(
        Path(base).resolve(strict=True), capture_wheel_target=True
    )


def _uv_identity(
    environ: Mapping[str, str] | None = None,
) -> tuple[Path, dict[str, str]]:
    raw_uv = shutil.which(
        "uv", path=None if environ is None else environ.get("PATH", "")
    )
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
    if not isinstance(requirements, list) or not all(
        isinstance(item, str) and item.strip() for item in requirements
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


def _editable_binding(repo_root: Path) -> dict[str, object]:
    from packaging.utils import canonicalize_name

    project_path = repo_root / "pyproject.toml"
    project_bytes = project_path.read_bytes()
    document = tomllib.loads(project_bytes.decode("utf-8"))
    project = document.get("project", {})
    if (
        project.get("dynamic")
        or not isinstance(project.get("name"), str)
        or not isinstance(project.get("version"), str)
    ):
        raise _schema.SourceBuildEnvironmentError(
            "development image requires declared static project metadata"
        )
    inputs = {"pyproject.toml": hashlib.sha256(project_bytes).hexdigest()}
    for field in ("readme", "license"):
        declaration = project.get(field)
        filename = (
            declaration
            if isinstance(declaration, str) and field == "readme"
            else declaration.get("file")
            if isinstance(declaration, dict)
            else None
        )
        if filename is not None:
            path = (repo_root / filename).resolve(strict=True)
            if not path.is_relative_to(repo_root):
                raise _schema.SourceBuildEnvironmentError(
                    "editable metadata input escapes source root"
                )
            inputs[path.relative_to(repo_root).as_posix()] = hashlib.sha256(
                path.read_bytes()
            ).hexdigest()
    return {
        "source_root": str(repo_root),
        "project": canonicalize_name(project["name"]),
        "version": project["version"],
        "metadata_inputs": inputs,
    }


def _selected_python_recipe(
    repo_root: Path, selector: str | None, environ: Mapping[str, str]
) -> tuple[Path, dict[str, object], dict[str, str], dict[str, object]]:
    from molt.python_interpreter import resolve_python_selector, probe_python_command

    if selector is None:
        selector = (repo_root / ".python-version").read_text(encoding="utf-8").strip()
    bootstrap_env = ProcessEnvironment(environ).updated(
        {"UV_PYTHON_DOWNLOADS": "never"}
    )
    # This selected-interpreter observation occurs at provisioning, never in a
    # pure DX path accessor. Standard-library APIs define PEP 508 platform facts.
    script = """import json,os,platform,sys
v=sys.implementation.version
iv='.'.join(map(str,(v.major,v.minor,v.micro)))
if v.releaselevel != 'final': iv += v.releaselevel[0] + str(v.serial)
print(json.dumps(dict(implementation_name=sys.implementation.name,implementation_version=iv,os_name=os.name,platform_machine=platform.machine(),platform_python_implementation=platform.python_implementation(),platform_release=platform.release(),platform_system=platform.system(),platform_version=platform.version(),python_full_version=platform.python_version(),python_version='.'.join(platform.python_version_tuple()[:2]),sys_platform=sys.platform)))
"""
    with (
        python_environment_identity.python_identity_verifier_prefix() as verifier_prefix
    ):
        command = resolve_python_selector(
            selector,
            env=bootstrap_env,
            cwd=repo_root,
            verifier_bytecode_prefix=verifier_prefix,
        )
        selected = probe_python_command(
            command,
            env=bootstrap_env,
            cwd=repo_root,
            verifier_bytecode_prefix=verifier_prefix,
        )
        python = Path(selected.executable).absolute()
        capture = _probe_source_build_python(
            python, verifier_bytecode_prefix=verifier_prefix, capture_wheel_target=True
        )
        runtime = cast(dict[str, object], capture["runtime"])
        wheel_target = cast(dict[str, object], capture["wheel_target"])
        result = process_guard.run_completed_command(
            [
                str(python),
                *python_environment_identity.python_identity_verifier_arguments(
                    verifier_prefix
                ),
                "-S",
                "-c",
                script,
            ],
            env=bootstrap_env,
            capture_output=True,
            text=True,
            check=False,
            timeout=120,
        )
        if result.returncode != 0:
            raise _schema.SourceBuildEnvironmentError(
                "cannot capture selected interpreter dependency markers"
            )
        markers = _schema.canonical_source_marker_environment(
            loads_exact(result.stdout)
        )
    return python, runtime, markers, wheel_target


def _environment_spec(
    repo_root: Path,
    dependency_group: str | None,
    *,
    python_runtime: dict[str, object] | None = None,
    wheel_target: Mapping[str, object] | None = None,
    marker_environment: Mapping[str, str] | None = None,
    groups: tuple[str, ...] | None = None,
    editable: bool = False,
    environ: Mapping[str, str] | None = None,
) -> tuple[Path, Path, Path, _schema._SourceBuildCustody, Path]:
    repo_root = repo_root.resolve()
    if dependency_group is not None and (
        not dependency_group
        or any(
            character not in "abcdefghijklmnopqrstuvwxyz0123456789-_"
            for character in dependency_group
        )
    ):
        raise _schema.SourceBuildEnvironmentError(
            f"invalid source-build dependency group {dependency_group!r}"
        )
    selected_groups = tuple(
        sorted(
            set(
                (() if dependency_group is None else (dependency_group,))
                if groups is None
                else groups
            )
        )
    )
    if dependency_group is not None and dependency_group not in selected_groups:
        raise _schema.SourceBuildEnvironmentError(
            "primary dependency group is absent from selection"
        )
    group_requirements = tuple(
        requirement
        for group in selected_groups
        for requirement in _declared_dependency_group(repo_root, group)
    )
    if python_runtime is None:
        capture = _python_identity()
        python_runtime = cast(dict[str, object], capture["runtime"])
        wheel_target = cast(dict[str, object], capture["wheel_target"])
    if (
        wheel_target is None
        or wheel_target.get("runtime_closure_sha256")
        != python_runtime["runtime_closure_sha256"]
    ):
        raise _schema.SourceBuildEnvironmentError(
            "wheel target differs from selected Python runtime"
        )
    markers = _schema.canonical_source_marker_environment(marker_environment)
    if not _schema._marker_environment_matches_runtime(markers, python_runtime):
        raise _schema.SourceBuildEnvironmentError(
            "dependency markers differ from selected Python runtime"
        )
    try:
        lock_closure = selected_uv_lock_group_closure(
            repo_root,
            dependency_group,
            group_requirements,
            wheel_target=wheel_target,
            marker_environment=markers,
            dependency_groups=selected_groups,
        )
    except PythonEnvironmentIdentityError as exc:
        raise _schema.SourceBuildEnvironmentError(str(exc)) from exc
    uv, uv_payload = _uv_identity() if environ is None else _uv_identity(environ)
    address_payload: _schema._SourceBuildAddress = {
        "schema_version": _schema.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_groups": list(selected_groups),
        "editable_binding": _editable_binding(repo_root) if editable else None,
        "bytecode_policy": _schema.ORDINARY_BYTECODE_POLICY,
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
    custody_root = (
        artifact_root(repo_root, os.environ if environ is None else environ)
        / "build-environments"
        / "development"
        / checkout_component(repo_root)
        if editable
        else _source_build_custody_root(repo_root)
    )
    root = custody_root / environment_id
    python_executable = environment_python_path(root)
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
    python_executable: Path,
    root: Path,
    *,
    external_root: Path | None = None,
    source_root: Path | None = None,
) -> dict[str, object]:
    """Capture the selected environment through the shared content authority."""

    return _probe_source_build_python(
        python_executable,
        root=root,
        external_root=external_root,
        source_root=source_root,
    )


def _probe_source_build_python(
    python_executable: Path,
    *,
    root: Path | None = None,
    external_root: Path | None = None,
    source_root: Path | None = None,
    publish_bytecode: bool = False,
    verifier_bytecode_prefix: Path | None = None,
    capture_wheel_target: bool = False,
) -> dict[str, object]:
    """One isolated bootstrap for recipe and realized-environment identities."""

    probe_environment = ProcessEnvironmentBuilder(os.environ)
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
    if external_root is not None:
        probe_arguments.extend(("--admit-external-root", str(external_root)))
    if publish_bytecode:
        probe_arguments.append("--publish-bytecode")
    if capture_wheel_target:
        if root is not None:
            raise _schema.SourceBuildEnvironmentError(
                "wheel target requires runtime capture"
            )
        probe_arguments.extend(("--wheel-tag-source", str(packaging_tag_source())))
    kind = "runtime" if root is None else "environment"
    prefix_owner = (
        python_environment_identity.python_identity_verifier_prefix()
        if verifier_bytecode_prefix is None
        else nullcontext(verifier_bytecode_prefix)
    )
    with prefix_owner as verifier_prefix:
        probe_argv = [
            str(python_executable),
            *python_environment_identity.python_identity_probe_arguments(
                probe_arguments,
                no_site=root is None,
                verifier_bytecode_prefix=verifier_prefix,
                source_root=source_root,
            ),
        ]
        result = process_guard.run_completed_command(
            probe_argv,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            env=ProcessEnvironment(probe_environment),
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
    if publish_bytecode:
        return payload
    try:
        if capture_wheel_target:
            if not isinstance(payload, dict) or set(payload) != {
                "runtime",
                "wheel_target",
            }:
                raise PythonEnvironmentIdentityError(
                    "runtime wheel capture shape is invalid"
                )
            runtime = validate_python_runtime_identity(payload["runtime"])
            target = validate_python_wheel_target(payload["wheel_target"])
            if target["runtime_closure_sha256"] != runtime["runtime_closure_sha256"]:
                raise PythonEnvironmentIdentityError(
                    "runtime wheel capture binding is invalid"
                )
            return {"runtime": runtime, "wheel_target": target}
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
    binding = custody.get("editable_binding")
    compared = dict(realized)
    if binding is not None:
        assert isinstance(binding, Mapping)
        distributions = cast(list[Mapping[str, object]], realized["distributions"])
        project = [row for row in distributions if row["name"] == binding["project"]]
        if (
            len(project) != 1
            or project[0]["version"] != binding["version"]
            or project[0]["external_source"] is None
        ):
            raise _schema.SourceBuildEnvironmentError(
                "development image has no exact editable project binding"
            )
        source = cast(Mapping[str, object], project[0]["external_source"])
        roots = {
            row["id"]: row["path"]
            for row in cast(list[Mapping[str, object]], realized["external_roots"])
        }
        if source.get("root") not in roots or (
            Path(str(roots[source["root"]])) / str(source["path"])
        ).resolve() != Path(str(binding["source_root"])):
            raise _schema.SourceBuildEnvironmentError(
                "editable installation points to a different physical source root"
            )
        compared["distributions"] = [
            row for row in distributions if row["name"] != binding["project"]
        ]
    if (
        not environment_matches_lock_closure(compared, typed_lock_closure)
        or realized.get("runtime") != python_runtime
    ):
        raise _schema.SourceBuildEnvironmentError(
            "provisioned source-build environment differs from its selected lock "
            "or CPython runtime closure"
        )
    if binding is not None:
        from molt.python_environment_custody import immutable_environment_image

        return {**custody, "realized_image": immutable_environment_image(realized)}
    return {**custody, "realized_environment": dict(realized)}


def _read_attestation(path: Path) -> Mapping[str, object] | None:
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError):
        return None
    return payload if isinstance(payload, Mapping) else None


def _provisioning_record(custody: Mapping[str, object]) -> dict[str, object]:
    return {"state": "provisioning", "custody": dict(custody)}


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
        from molt.python_uv_lock_identity import validate_uv_lock_group_closure

        active_closure = validate_uv_lock_group_closure(
            active_manifest.get("lock_closure")
        )
        spec = _environment_spec(
            repo_root,
            dependency_group,
            python_runtime=cast(dict[str, object], realized["runtime"]),
            wheel_target=cast(Mapping[str, object], active_closure["wheel_target"]),
        )
    else:
        spec = _environment_spec(repo_root, dependency_group)
    root, python_executable, manifest_path, custody, _uv = spec
    active = Path(sys.prefix).resolve() == root.resolve()
    if not active and active_manifest is not None:
        if active_manifest.get("dependency_groups") == [dependency_group]:
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
        return _provision_source_build_environment(repo_root, spec)
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
    environment: ProcessEnvironment,
) -> subprocess.CompletedProcess[bytes]:
    """Run the single provisioning mutation behind its module-owned seam."""

    return process_guard.run_completed_command(
        list(argv),
        cwd=cwd,
        env=environment,
        check=False,
    )


def _validate_recipe_inputs(repo_root: Path, custody: Mapping[str, object]) -> None:
    groups = tuple(cast(list[str], custody["dependency_groups"]))
    requirements = tuple(
        requirement
        for group in groups
        for requirement in _declared_dependency_group(repo_root, group)
    )
    closure = cast(Mapping[str, object], custody["lock_closure"])
    expected = selected_uv_lock_group_closure(
        repo_root,
        None,
        requirements,
        wheel_target=cast(Mapping[str, object], closure["wheel_target"]),
        marker_environment=cast(Mapping[str, str], closure["marker_environment"]),
        dependency_groups=groups,
    )
    if (
        expected != closure
        or list(requirements) != custody["dependency_group_requirements"]
    ):
        raise _schema.SourceBuildEnvironmentError(
            "environment recipe differs from current declared lock inputs"
        )
    binding = custody.get("editable_binding")
    if binding is not None and _editable_binding(repo_root) != binding:
        raise _schema.SourceBuildEnvironmentError(
            "editable installation binding/metadata differs from this checkout"
        )


def _publish_environment_bytecode(
    python_executable: Path,
    root: Path,
    *,
    external_root: Path | None = None,
    source_root: Path | None = None,
) -> None:
    _probe_source_build_python(
        python_executable,
        root=root,
        external_root=external_root,
        source_root=source_root,
        publish_bytecode=True,
    )


def _provision_source_build_environment(
    repo_root: Path,
    spec: tuple[Path, Path, Path, _schema._SourceBuildCustody, Path],
    *,
    python_executable_selection: Path | None = None,
    environ: Mapping[str, str] | None = None,
) -> _schema.LockedSourceBuildEnvironment:
    root, python_executable, manifest_path, custody, uv = spec
    binding = custody.get("editable_binding")
    external_root = Path(str(binding["source_root"])) if binding is not None else None
    # Publication's reader belongs to the selected editable checkout. The
    # operational controller can be another immutable source generation.
    # External-root admission alone never selects executable probe code.
    source_root = external_root if binding is not None else None
    reader_inputs = (
        python_environment_identity.python_capture_authority_paths(
            source_root=source_root
        )
        if source_root is not None
        else ()
    )

    def capture() -> dict[str, object]:
        verify_inputs()
        if external_root is None:
            realized = _probe_environment_identity(python_executable, root)
        else:
            realized = _probe_environment_identity(
                python_executable,
                root,
                external_root=external_root,
                source_root=source_root,
            )
        verify_inputs()
        return realized

    lock_path = root.parent / ".locks" / f"{root.name}.lock"
    provisioning_path = environment_provisioning_record_path(root)
    input_paths = (
        repo_root / "pyproject.toml",
        repo_root / "uv.lock",
        repo_root / ".python-version",
    )
    if binding is not None:
        input_paths = tuple(
            dict.fromkeys(
                (
                    *input_paths,
                    *reader_inputs,
                    *(
                        repo_root / name
                        for name in cast(Mapping[str, str], binding["metadata_inputs"])
                    ),
                )
            )
        )
    absent_inputs = {path for path in input_paths if not path.exists()}
    missing_authority = absent_inputs.intersection(reader_inputs)
    if missing_authority:
        raise _schema.SourceBuildEnvironmentError(
            "selected editable Python capture authority is missing: "
            + ", ".join(str(path) for path in sorted(missing_authority))
        )
    input_context = PythonFileCaptureContext()
    for path in input_paths:
        if path not in absent_inputs:
            input_context.bind(
                path, path.lstat(), label="environment publication input"
            )

    def verify_inputs() -> None:
        if any(path.exists() or path.is_symlink() for path in absent_inputs):
            raise _schema.SourceBuildEnvironmentError(
                "environment recipe inputs changed during publication"
            )
        try:
            input_context.verify()
        except (OSError, ValueError) as exc:
            raise _schema.SourceBuildEnvironmentError(
                "environment recipe inputs changed during publication"
            ) from exc

    handle = _acquire_file_lock(
        lock_path,
        timeout_s=900.0,
        timeout_message=(
            "timed out waiting for locked source-build environment provisioning "
            f"lock {lock_path}"
        ),
    )
    try:
        _validate_recipe_inputs(repo_root, custody)
        verify_inputs()
        existing = _read_attestation(manifest_path)
        provisioning = _read_attestation(provisioning_path)
        expected_provisioning = _provisioning_record(custody)
        if provisioning_path.exists() and provisioning is None:
            raise _schema.SourceBuildEnvironmentError(
                f"malformed source-build provisioning record: {provisioning_path}"
            )
        complete_fields = {
            *custody,
            "realized_image" if binding is not None else "realized_environment",
        }
        if (
            python_executable.is_file()
            and isinstance(existing, Mapping)
            and set(existing) == complete_fields
        ):
            expected_core = dict(custody)
            actual_core = {key: existing.get(key) for key in expected_core}
            if actual_core == expected_core:
                realized = capture()
                expected_attestation = _attested_environment(custody, realized)
                if existing == expected_attestation:
                    _validate_recipe_inputs(repo_root, custody)
                    verify_inputs()
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
        if manifest_path.exists() or manifest_path.is_symlink():
            raise _schema.SourceBuildEnvironmentError(
                f"published immutable environment failed admission and cannot be reprovisioned: {root}"
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

        environment = ProcessEnvironmentBuilder(
            os.environ if environ is None else environ
        )
        # This sync has an exact destination; the launcher's active venv is not
        # a second environment selector (nor a useful uv mismatch warning).
        environment.pop("VIRTUAL_ENV", None)
        environment["UV_PROJECT_ENVIRONMENT"] = str(root)
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        environment["PYTHONPYCACHEPREFIX"] = str(environment_bytecode_prefix(root))
        child_environment = ProcessEnvironment(environment)
        raw_base = (
            python_executable_selection
            or getattr(sys, "_base_executable", None)
            or sys.executable
        )
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
            # Defaults are disabled above; --no-dev would also exclude an
            # explicitly selected dev group, since uv exclusions always win.
            *(
                argument
                for group in custody["dependency_groups"]
                for argument in ("--group", group)
            ),
            *(["--no-install-project"] if binding is None else []),
        )
        result = _run_uv_sync(
            sync_argv,
            cwd=repo_root,
            environment=child_environment,
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
        verify_inputs()
        _publish_environment_bytecode(
            python_executable,
            root,
            external_root=external_root,
            source_root=source_root,
        )
        realized = capture()
        manifest = _attested_environment(custody, realized)
        _validate_recipe_inputs(repo_root, custody)
        verify_inputs()
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
            provisioned=True,
        )
    finally:
        input_context.close()
        _release_file_lock(handle)


def _published_recipe(root: Path) -> tuple[dict[str, object], Mapping[str, object]]:
    manifest = _read_attestation(root / _schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST)
    if manifest is None:
        raise _schema.SourceBuildEnvironmentError(
            f"selected environment has no published image attestation: {root}"
        )
    recipe_keys = set(_schema._SourceBuildAddress.__annotations__)
    field = (
        "realized_image"
        if manifest.get("editable_binding") is not None
        else "realized_environment"
    )
    if set(manifest) != recipe_keys | {"environment_id", field}:
        raise _schema.SourceBuildEnvironmentError(
            "published environment manifest shape is invalid"
        )
    recipe = {key: manifest[key] for key in recipe_keys}
    if (
        type(recipe["schema_version"]) is not int
        or recipe["schema_version"] != _schema.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION
        or recipe["bytecode_policy"] != _schema.ORDINARY_BYTECODE_POLICY
        or manifest["environment_id"] != root.name
        or canonical_json_sha256(recipe) != root.name
    ):
        raise _schema.SourceBuildEnvironmentError(
            "published environment address/recipe is invalid"
        )
    from molt.python_uv_lock_identity import validate_uv_lock_group_closure

    closure = validate_uv_lock_group_closure(recipe["lock_closure"])
    runtime = validate_python_runtime_identity(recipe["python_runtime"])
    wheel_target = cast(Mapping[str, object], closure["wheel_target"])
    if wheel_target["runtime_closure_sha256"] != runtime["runtime_closure_sha256"]:
        raise _schema.SourceBuildEnvironmentError(
            "published wheel target differs from runtime recipe"
        )
    if (
        closure["dependency_groups"] != recipe["dependency_groups"]
        or closure["requirements"] != recipe["dependency_group_requirements"]
    ):
        raise _schema.SourceBuildEnvironmentError(
            "published environment selection differs from its closure"
        )
    if not _schema._marker_environment_matches_runtime(
        cast(Mapping[str, str], closure["marker_environment"]),
        runtime,
    ):
        raise _schema.SourceBuildEnvironmentError(
            "published dependency markers differ from the selected runtime"
        )
    custody = {"environment_id": manifest["environment_id"], **recipe}
    if field == "realized_image":
        projection = manifest[field]
        if (
            not isinstance(projection, Mapping)
            or set(projection) != {"image", "image_sha256"}
            or not isinstance(projection["image"], Mapping)
        ):
            raise _schema.SourceBuildEnvironmentError(
                "published development image projection is invalid"
            )
        try:
            expected = _attested_environment(custody, projection["image"])
        except (
            KeyError,
            TypeError,
            AssertionError,
            PythonEnvironmentIdentityError,
        ) as exc:
            raise _schema.SourceBuildEnvironmentError(
                "published development image projection is malformed"
            ) from exc
        if expected != manifest:
            raise _schema.SourceBuildEnvironmentError(
                "published development image projection differs from its recipe"
            )
    return custody, manifest


def admit_published_environment(
    root: Path, realized: Mapping[str, object]
) -> Mapping[str, object]:
    """Use the existing admission capture; never spawn or rescan for this check."""
    custody, manifest = _published_recipe(root)
    expected = _attested_environment(custody, realized)
    if expected != manifest:
        raise _schema.SourceBuildEnvironmentError(
            "published environment image differs from captured live bytes"
        )
    return manifest


def load_development_environment(
    root: Path,
    *,
    repo_root: Path,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
) -> _schema.LockedSourceBuildEnvironment:
    """Read an exact selection at a boundary, without claiming live admission."""
    # Normalize ordinary relative/dot coordinates without following aliases.
    # The physical comparison still refuses symlink indirection at admission.
    physical = root.resolve(strict=True)
    lexical = Path(os.path.abspath(root))
    repo_root = repo_root.resolve(strict=True)
    if root.is_symlink() or physical != lexical:
        raise _schema.SourceBuildEnvironmentError(
            "development image selection must be its physical root"
        )
    root = lexical
    if (
        root.parent.name != checkout_component(repo_root)
        or root.parent.parent.name != "development"
        or root.parent.parent.parent.name != "build-environments"
    ):
        raise _schema.SourceBuildEnvironmentError(
            "development image is outside its checkout generation namespace"
        )
    custody, manifest = _published_recipe(root)
    if groups is not None and custody["dependency_groups"] != sorted(set(groups)):
        raise _schema.SourceBuildEnvironmentError(
            "selected development image has different requested groups"
        )
    if python is not None:
        version = str(cast(Mapping[str, object], custody["python_runtime"])["version"])
        if not python or not all(part.isdigit() for part in python.split(".")):
            raise _schema.SourceBuildEnvironmentError(
                "an interpreter command requires explicit runtime selection before image reuse"
            )
        if not (version == python or version.startswith(python + ".")):
            raise _schema.SourceBuildEnvironmentError(
                "selected development image has a different requested Python"
            )
    prefix = environment_bytecode_prefix(root)
    if not prefix.is_dir() or prefix.is_symlink() or prefix.resolve() != prefix:
        raise _schema.SourceBuildEnvironmentError(
            "development image has no physical sealed bytecode prefix"
        )
    if custody.get("editable_binding") is None:
        raise _schema.SourceBuildEnvironmentError(
            "selected image is not a development environment"
        )
    _validate_recipe_inputs(repo_root, custody)
    return _schema.LockedSourceBuildEnvironment(
        root,
        environment_python_path(root),
        root / _schema.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
        manifest,
        active=Path(sys.prefix).resolve() == root,
    )


def development_environment(
    repo_root: Path,
    *,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
    provision: bool = False,
    environ: Mapping[str, str] | None = None,
) -> _schema.LockedSourceBuildEnvironment:
    """Select/publish development through the one exact-address image owner."""
    repo_root = repo_root.resolve(strict=True)
    environment = ProcessEnvironment(os.environ if environ is None else environ)
    selected_groups = tuple(
        sorted(
            set(DxProject(repo_root).dependency_groups() if groups is None else groups)
        )
    )
    explicit = environment.get("UV_PROJECT_ENVIRONMENT", "").strip()
    if explicit:
        selected_root = Path(explicit).expanduser()
        if not selected_root.is_absolute():
            selected_root = repo_root / selected_root
        image = load_development_environment(selected_root, repo_root=repo_root)
        if image.custody["dependency_groups"] != list(selected_groups):
            raise _schema.SourceBuildEnvironmentError(
                "explicit development image has different requested groups"
            )
        selector = (
            python
            or (repo_root / ".python-version").read_text(encoding="utf-8").strip()
        )
        version = str(
            cast(Mapping[str, object], image.custody["python_runtime"])["version"]
        )
        if selector and all(part.isdigit() for part in selector.split(".")):
            if not (version == selector or version.startswith(selector + ".")):
                raise _schema.SourceBuildEnvironmentError(
                    "explicit development image has a different requested Python"
                )
        else:
            _python, requested_runtime, _markers, _target = _selected_python_recipe(
                repo_root, selector, environment
            )
            if requested_runtime != image.custody["python_runtime"]:
                raise _schema.SourceBuildEnvironmentError(
                    "explicit development image has a different requested interpreter"
                )
        return image
    selected_python, runtime, markers, wheel_target = _selected_python_recipe(
        repo_root, python, environment
    )
    spec = _environment_spec(
        repo_root,
        None,
        python_runtime=runtime,
        wheel_target=wheel_target,
        marker_environment=markers,
        groups=selected_groups,
        editable=True,
        environ=environment,
    )
    if provision:
        return _provision_source_build_environment(
            repo_root,
            spec,
            python_executable_selection=selected_python,
            environ=environment,
        )
    root, executable, manifest, custody, _uv = spec
    return _schema.LockedSourceBuildEnvironment(
        root, executable, manifest, custody, active=Path(sys.prefix).resolve() == root
    )
