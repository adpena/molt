"""Exact-address publication for development and source-build Python images."""

from __future__ import annotations

from molt import python_environment_contract as _contract
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

import json
import os
import shutil
import subprocess
import sys
import tomllib
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import cast

from molt import source_build_environment_schema as _schema
from molt.source_build_environment_selection import (
    capture_source_build_inputs,
    development_coordinates,
    development_selection_record,
    valid_declared_input_digests,
)
from molt.file_deletion import delete_path
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.toolchain_identity import stable_executable_probe
from molt.python_file_node_custody import PythonFileCaptureContext
from molt.dx import checkout_custody, artifact_root, checkout_component, DxProject
from molt.exact_json import (
    ExactJsonError,
    canonical_json_sha256,
    loads_exact,
    write_exact,
)
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
    attestation_records_writable_installation,
    installed_access_entry,
    seal_environment_installation,
)
from molt.python_environment_identity import (
    PythonEnvironmentIdentityError,
    PythonIdentityVerifier,
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
    with PythonIdentityVerifier() as verifier:
        return _probe_source_build_python(
            Path(base).resolve(strict=True),
            verifier=verifier,
            capture_wheel_target=True,
        )


def _uv_identity(
    environ: Mapping[str, str] | None = None,
) -> tuple[Path, dict[str, str]]:
    raw_uv = shutil.which(
        "uv", path=None if environ is None else environ.get("PATH", "")
    )
    if raw_uv is None:
        raise _contract.SourceBuildEnvironmentError(
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
        raise _contract.SourceBuildEnvironmentError(
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
    from packaging.requirements import InvalidRequirement, Requirement

    try:
        payload = tomllib.loads(
            (repo_root / "pyproject.toml").read_text(encoding="utf-8")
        )
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise _contract.SourceBuildEnvironmentError(
            f"cannot read source-build dependency-group authority: {exc}"
        ) from exc
    groups = payload.get("dependency-groups")
    requirements = groups.get(dependency_group) if isinstance(groups, Mapping) else None
    if not isinstance(requirements, list) or not all(
        isinstance(item, str) and item.strip() for item in requirements
    ):
        raise _contract.SourceBuildEnvironmentError(
            f"source-build dependency group {dependency_group!r} is not declared"
        )
    normalized = tuple(item.strip() for item in requirements)
    for raw in normalized:
        try:
            requirement = Requirement(raw)
        except InvalidRequirement as exc:
            raise _contract.SourceBuildEnvironmentError(
                f"invalid requirement in source-build group {dependency_group!r}: "
                f"{raw!r}: {exc}"
            ) from exc
        if requirement.url is not None:
            raise _contract.SourceBuildEnvironmentError(
                f"source-build dependency group {dependency_group!r} contains "
                f"an unverifiable direct URL: {raw!r}"
            )
    return normalized


def _selected_python_recipe(
    repo_root: Path, selector: str | None, environ: Mapping[str, str]
) -> tuple[Path, dict[str, object], dict[str, str], dict[str, object]]:
    from molt.python_interpreter import (
        is_python_version_selector,
        parse_python_interpreter_request,
        probe_python_command,
        resolve_python_selector,
    )

    if selector is None:
        selector = (repo_root / ".python-version").read_text(encoding="utf-8").strip()
    request = (
        parse_python_interpreter_request(selector)
        if is_python_version_selector(selector)
        else None
    )
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
    with PythonIdentityVerifier() as verifier:
        command = resolve_python_selector(
            selector,
            env=bootstrap_env,
            cwd=repo_root,
            verifier=verifier,
        )
        selected = probe_python_command(
            command,
            env=bootstrap_env,
            cwd=repo_root,
            verifier=verifier,
        )
        python = Path(selected.executable).absolute()
        capture = _probe_source_build_python(
            python, verifier=verifier, capture_wheel_target=True
        )
        runtime = cast(dict[str, object], capture["runtime"])
        wheel_target = cast(dict[str, object], capture["wheel_target"])
        result = verifier.run(
            [
                str(python),
                *python_environment_identity.python_identity_verifier_arguments(
                    verifier.prefix
                ),
                "-S",
                "-c",
                script,
            ],
            memory_guard_prefix=process_guard.CLI_MEMORY_GUARD_PREFIX,
            env=bootstrap_env,
            capture_output=True,
            text=True,
            check=False,
            timeout=120,
            encoding="utf-8",
        )
        if result.returncode != 0:
            raise _contract.SourceBuildEnvironmentError(
                "cannot capture selected interpreter dependency markers"
            )
        markers = _schema.canonical_source_marker_environment(
            loads_exact(result.stdout)
        )
        # Discovery, runtime capture and marker capture are separate process
        # observations. Bind all of them before any recipe can be constructed.
        runtime_version = str(runtime["version"])
        if request is not None and not request.matches_version(runtime_version):
            raise _contract.SourceBuildEnvironmentError(
                f"requested CPython {request.selector} differs from captured runtime {runtime_version}"
            )
        if selected.version != runtime_version:
            raise _contract.SourceBuildEnvironmentError(
                "selected interpreter identity differs from captured runtime"
            )
        if not _schema._marker_environment_matches_runtime(markers, runtime):
            raise _contract.SourceBuildEnvironmentError(
                "dependency markers differ from selected Python runtime"
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
        raise _contract.SourceBuildEnvironmentError(
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
        raise _contract.SourceBuildEnvironmentError(
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
        raise _contract.SourceBuildEnvironmentError(
            "wheel target differs from selected Python runtime"
        )
    markers = _schema.canonical_source_marker_environment(marker_environment)
    if not _schema._marker_environment_matches_runtime(markers, python_runtime):
        raise _contract.SourceBuildEnvironmentError(
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
        raise _contract.SourceBuildEnvironmentError(str(exc)) from exc
    uv, uv_payload = _uv_identity() if environ is None else _uv_identity(environ)
    inputs = capture_source_build_inputs(repo_root, editable=editable)
    address_payload: _schema._SourceBuildAddress = {
        "producer_contract": _contract.source_build_environment_contract(),
        "declared_inputs": inputs.digests,
        "schema_version": _contract.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "dependency_groups": list(selected_groups),
        "editable_binding": inputs.editable_binding,
        "bytecode_policy": _contract.IMAGE_BYTECODE_POLICY,
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
        root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
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
    attestation: Mapping[str, object] | None = None,
) -> dict[str, object]:
    """Capture the selected environment through the shared content authority.

    ``attestation`` is the parsed published manifest that the caller admits
    against this capture; publication passes none.
    """

    sealed = attestation is not None and attestation_records_writable_installation(
        attestation
    )
    if sealed:
        # Sealing is an intentional one-way permission transition. Seal before
        # the capture so the caller's admission compares the sealed live
        # identity against the original attestation. The child stays a plain
        # read-only capture and imports nothing from the image.
        try:
            seal_environment_installation(root)
        except (OSError, PythonEnvironmentIdentityError) as exc:
            raise _contract.SourceBuildEnvironmentError(
                f"published image could not be sealed before admission: {root}; {exc}"
            ) from exc
    try:
        with PythonIdentityVerifier() as verifier:
            return _probe_source_build_python(
                python_executable,
                verifier=verifier,
                root=root,
                external_root=external_root,
                source_root=source_root,
            )
    except _contract.SourceBuildEnvironmentError as exc:
        if sealed:
            raise _contract.SourceBuildEnvironmentError(
                f"published image was sealed before admission: {root}; {exc}"
            ) from exc
        raise


def _probe_source_build_python(
    python_executable: Path,
    *,
    root: Path | None = None,
    external_root: Path | None = None,
    source_root: Path | None = None,
    verifier: PythonIdentityVerifier,
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
    if capture_wheel_target:
        if root is not None:
            raise _contract.SourceBuildEnvironmentError(
                "wheel target requires runtime capture"
            )
        probe_arguments.extend(("--wheel-tag-source", str(packaging_tag_source())))
    kind = "runtime" if root is None else "environment"
    probe_argv = [
        str(python_executable),
        *python_environment_identity.python_identity_probe_arguments(
            probe_arguments,
            no_site=root is None,
            verifier_bytecode_prefix=verifier.prefix,
            source_root=source_root,
        ),
    ]
    result = verifier.run(
        probe_argv,
        memory_guard_prefix=process_guard.CLI_MEMORY_GUARD_PREFIX,
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
        raise _contract.SourceBuildEnvironmentError(
            f"cannot attest source-build {kind} content: {detail or f'returncode={result.returncode}'}"
        )
    try:
        payload = loads_exact(result.stdout)
    except (json.JSONDecodeError, ExactJsonError) as exc:
        raise _contract.SourceBuildEnvironmentError(
            f"source-build {kind} probe returned invalid JSON"
        ) from exc
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
        raise _contract.SourceBuildEnvironmentError(
            f"source-build {kind} probe returned an invalid payload: {exc}"
        ) from exc


def _attested_environment(
    custody: Mapping[str, object], realized: Mapping[str, object]
) -> dict[str, object]:
    lock_closure = custody.get("lock_closure")
    python_runtime = custody.get("python_runtime")
    if not isinstance(lock_closure, Mapping) or not isinstance(python_runtime, Mapping):
        raise _contract.SourceBuildEnvironmentError(
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
            raise _contract.SourceBuildEnvironmentError(
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
            raise _contract.SourceBuildEnvironmentError(
                "editable installation points to a different physical source root"
            )
        compared["distributions"] = [
            row for row in distributions if row["name"] != binding["project"]
        ]
    if (
        not environment_matches_lock_closure(compared, typed_lock_closure)
        or realized.get("runtime") != python_runtime
    ):
        raise _contract.SourceBuildEnvironmentError(
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


def _admitted_permission_transition(
    manifest: Mapping[str, object], captured: Mapping[str, object]
) -> dict[str, object]:
    """Project only legacy installed write-bit removals into comparison.

    Neither stored attestations nor live identities change. Read/execute bits,
    bytes, aliases, membership and every field outside the installed tree stay
    exact. A previously sealed entry can never become writable on this path.
    Derived digests are rebuilt only for the permitted permission transition.
    """
    field = "realized_image" if "realized_image" in manifest else "realized_environment"
    if not isinstance(manifest.get(field), Mapping) or not isinstance(
        captured.get(field), Mapping
    ):
        return dict(manifest)
    previous = cast(Mapping[str, object], manifest[field])
    current = cast(Mapping[str, object], captured[field])
    if field == "realized_image" and (
        not isinstance(previous.get("image"), Mapping)
        or not isinstance(current.get("image"), Mapping)
    ):
        return dict(manifest)
    image = (
        cast(Mapping[str, object], previous["image"])
        if field == "realized_image"
        else previous
    )
    live = (
        cast(Mapping[str, object], current["image"])
        if field == "realized_image"
        else current
    )
    if not isinstance(image.get("tree"), Mapping) or not isinstance(
        live.get("tree"), Mapping
    ):
        return dict(manifest)
    tree = cast(Mapping[str, object], image["tree"])
    live_tree = cast(Mapping[str, object], live["tree"])
    if not isinstance(tree.get("entries"), list) or not isinstance(
        live_tree.get("entries"), list
    ):
        return dict(manifest)
    rows = cast(list[Mapping[str, object]], tree["entries"])
    if not all(
        isinstance(row, Mapping)
        and isinstance(row.get("path"), str)
        and "kind" in row
        and isinstance(row.get("access", {}), Mapping)
        for row in (*rows, *cast(list[Mapping[str, object]], live_tree["entries"]))
    ):
        return dict(manifest)
    if canonical_json_sha256(rows) != tree.get("manifest_sha256"):
        return dict(manifest)
    if field == "realized_image":
        if canonical_json_sha256(image) != previous.get("image_sha256"):
            return dict(manifest)
    elif canonical_json_sha256(
        {
            key: value
            for key, value in image.items()
            if key != "environment_closure_sha256"
        }
    ) != image.get("environment_closure_sha256"):
        return dict(manifest)
    live_rows = {
        row["path"]: row
        for row in cast(list[Mapping[str, object]], live_tree["entries"])
    }
    transitioned = []
    changed = False
    for row in rows:
        observed = live_rows.get(row["path"], {})
        access = cast(Mapping[str, object], row.get("access", {}))
        observed_access = cast(Mapping[str, object], observed.get("access", {}))
        if (
            installed_access_entry(row)
            and access.get("writable") is True
            and observed_access.get("writable") is False
        ):
            row = {**row, "access": {**access, "writable": False}}
            changed = True
        transitioned.append(row)
    if not changed:
        return dict(manifest)
    sealed_image = {
        **image,
        "tree": {
            **tree,
            "entries": transitioned,
            "manifest_sha256": canonical_json_sha256(transitioned),
        },
    }
    if field == "realized_image":
        projection = {
            "image": sealed_image,
            "image_sha256": canonical_json_sha256(sealed_image),
        }
    else:
        material = {
            key: value
            for key, value in sealed_image.items()
            if key != "environment_closure_sha256"
        }
        projection = {
            **material,
            "environment_closure_sha256": canonical_json_sha256(material),
        }
    return {**manifest, field: projection}


def _attestation_differences(previous: object, current: object) -> str:
    """Bounded structural diagnostics, keyed by file paths rather than indices."""
    differences: list[str] = []
    derived = {
        "manifest_sha256",
        "image_sha256",
        "environment_closure_sha256",
        "distribution_inventory_sha256",
    }

    def report(path: str, detail: str) -> None:
        differences.append(f"{path[:240]}: {detail[:160]}")

    def compare(before: object, after: object, path: str) -> None:
        if len(differences) >= 6 or before == after:
            return
        if isinstance(before, Mapping) and isinstance(after, Mapping):
            for key in sorted(
                before.keys() | after.keys(), key=lambda key: (key in derived, str(key))
            ):
                name = f"{path}.{key}" if path else str(key)
                if key not in before:
                    report(name, "added")
                elif key not in after:
                    report(name, "removed")
                else:
                    compare(before[key], after[key], name)
                if len(differences) >= 6:
                    return
        elif isinstance(before, list) and isinstance(after, list):
            identity = next(
                (
                    key
                    for key in ("path", "id", "name")
                    if all(
                        isinstance(row, Mapping) and key in row
                        for row in (*before, *after)
                    )
                ),
                None,
            )
            if identity is not None:
                compare(
                    {str(row[identity]): row for row in before},
                    {str(row[identity]): row for row in after},
                    path,
                )
            else:
                for index in range(max(len(before), len(after))):
                    if index >= len(before):
                        report(f"{path}[{index}]", "added")
                    elif index >= len(after):
                        report(f"{path}[{index}]", "removed")
                    else:
                        compare(before[index], after[index], f"{path}[{index}]")
                    if len(differences) >= 6:
                        return
        else:

            def preview(value: object) -> str:
                if isinstance(value, (Mapping, list)):
                    return type(value).__name__
                return str(value)[:64]

            report(path, f"{preview(before)} -> {preview(after)}")

    # Name content mutations by their installed paths before node-table hashes.
    for field in ("realized_image", "realized_environment"):
        if (
            isinstance(previous, Mapping)
            and isinstance(current, Mapping)
            and field in previous
            and field in current
        ):
            if not isinstance(previous[field], Mapping) or not isinstance(
                current[field], Mapping
            ):
                continue
            before = cast(Mapping[str, object], previous[field])
            after = cast(Mapping[str, object], current[field])
            if field == "realized_image":
                if not isinstance(before.get("image"), Mapping) or not isinstance(
                    after.get("image"), Mapping
                ):
                    continue
                before = cast(Mapping[str, object], before["image"])
                after = cast(Mapping[str, object], after["image"])
            if not isinstance(before.get("tree"), Mapping) or not isinstance(
                after.get("tree"), Mapping
            ):
                continue
            before_tree = cast(Mapping[str, object], before["tree"])
            after_tree = cast(Mapping[str, object], after["tree"])

            def content_rows(tree: Mapping[str, object]) -> dict[str, object] | None:
                if not isinstance(tree.get("file_nodes"), list) or not isinstance(
                    tree.get("entries"), list
                ):
                    return None
                if not all(
                    isinstance(row, Mapping) and "id" in row
                    for row in cast(list[object], tree["file_nodes"])
                ) or not all(
                    isinstance(row, Mapping) and "path" in row
                    for row in cast(list[object], tree["entries"])
                ):
                    return None
                nodes = {
                    row["id"]: row
                    for row in cast(list[Mapping[str, object]], tree["file_nodes"])
                }
                return {
                    str(row["path"]): {**row, "content": nodes.get(row.get("node"))}
                    for row in cast(list[Mapping[str, object]], tree["entries"])
                }

            old_rows, new_rows = content_rows(before_tree), content_rows(after_tree)
            if old_rows is not None and new_rows is not None:
                compare(old_rows, new_rows, "tree")
    compare(previous, current, "")
    return "; ".join(differences)[:2048] or "attestation differs"


def _admit_attestation(
    manifest: Mapping[str, object],
    custody: Mapping[str, object],
    realized: Mapping[str, object],
) -> dict[str, object]:
    try:
        expected = _attested_environment(custody, realized)
    except _contract.SourceBuildEnvironmentError as exc:
        field = (
            "realized_image"
            if custody.get("editable_binding") is not None
            else "realized_environment"
        )
        from molt.python_environment_custody import immutable_environment_image

        captured = {
            **custody,
            field: immutable_environment_image(realized)
            if field == "realized_image"
            else dict(realized),
        }
        raise _contract.SourceBuildEnvironmentError(
            f"{exc}; " + _attestation_differences(manifest, captured)
        ) from exc
    if manifest != expected:
        compared = _admitted_permission_transition(manifest, expected)
    else:
        return expected
    if compared != expected:
        raise _contract.SourceBuildEnvironmentError(
            "published environment image differs from captured live bytes: "
            + _attestation_differences(compared, expected)
        )
    return expected


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
        raise _contract.SourceBuildEnvironmentError(
            f"cannot resolve active source-build environment: {exc}"
        ) from exc
    if active_root != root.resolve():
        raise _contract.SourceBuildEnvironmentError(
            f"active interpreter is not the locked source-build environment {root}"
        )
    manifest = _read_attestation(manifest_path)
    if manifest is None:
        raise _contract.SourceBuildEnvironmentError(
            f"locked source-build environment attestation is stale or invalid: {manifest_path}"
        )
    if realized is None:
        realized = _probe_environment_identity(
            Path(sys.executable), root, attestation=manifest
        )
    try:
        return _admit_attestation(manifest, custody, realized)
    except _contract.SourceBuildEnvironmentError as exc:
        raise _contract.SourceBuildEnvironmentError(
            "locked source-build environment attestation is stale or invalid: "
            f"{manifest_path}; {exc}"
        ) from exc


def source_build_environment(
    repo_root: Path, dependency_group: str, *, provision: bool = False
) -> _schema.LockedSourceBuildEnvironment:
    active_root = Path(sys.prefix).resolve()
    realized = None
    active_manifest = None
    if active_root.parent == _source_build_custody_root(repo_root).resolve():
        active_manifest = _read_attestation(
            active_root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST
        )
        if active_manifest is None:
            raise _contract.SourceBuildEnvironmentError(
                "locked source-build environment attestation is stale or invalid"
            )
        realized = _probe_environment_identity(
            Path(sys.executable), active_root, attestation=active_manifest
        )
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
            raise _contract.SourceBuildEnvironmentError(
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
        ):
            raise _contract.SourceBuildEnvironmentError(
                "active source-build environment attestation is stale or invalid"
            )
        try:
            _admit_attestation(active_manifest, prior_custody, realized)
        except _contract.SourceBuildEnvironmentError as exc:
            raise _contract.SourceBuildEnvironmentError(
                f"active source-build environment attestation is stale or invalid; {exc}"
            ) from exc
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
        raise _contract.SourceBuildEnvironmentError(
            "environment recipe differs from current declared lock inputs"
        )
    binding = custody.get("editable_binding")
    inputs = capture_source_build_inputs(repo_root, editable=binding is not None)
    if inputs.digests != custody.get("declared_inputs"):
        raise _contract.SourceBuildEnvironmentError(
            "environment recipe differs from current declared input bytes"
        )
    if inputs.editable_binding != binding:
        raise _contract.SourceBuildEnvironmentError(
            "editable installation binding/metadata differs from this checkout"
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

    def capture(attestation: Mapping[str, object] | None = None) -> dict[str, object]:
        verify_inputs()
        if external_root is None:
            realized = _probe_environment_identity(
                python_executable, root, attestation=attestation
            )
        else:
            realized = _probe_environment_identity(
                python_executable,
                root,
                external_root=external_root,
                source_root=source_root,
                attestation=attestation,
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
        raise _contract.SourceBuildEnvironmentError(
            "selected editable Python capture authority is missing: "
            + ", ".join(str(path) for path in sorted(missing_authority))
        )
    source_inputs = capture_source_build_inputs(repo_root, editable=binding is not None)
    input_context = PythonFileCaptureContext()
    for path in input_paths:
        if path not in absent_inputs:
            input_context.bind(
                path, path.lstat(), label="environment publication input"
            )

    def verify_inputs() -> None:
        if any(path.exists() or path.is_symlink() for path in absent_inputs):
            raise _contract.SourceBuildEnvironmentError(
                "environment recipe inputs changed during publication"
            )
        try:
            source_inputs.verify()
            input_context.verify()
        except (OSError, ValueError) as exc:
            raise _contract.SourceBuildEnvironmentError(
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
            raise _contract.SourceBuildEnvironmentError(
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
                realized = capture(existing)
                try:
                    expected_attestation = _admit_attestation(
                        existing, custody, realized
                    )
                except _contract.SourceBuildEnvironmentError as exc:
                    raise _contract.SourceBuildEnvironmentError(
                        "published immutable environment failed admission and cannot be "
                        f"reprovisioned: {root}; {exc}"
                    ) from exc
                else:
                    _validate_recipe_inputs(repo_root, custody)
                    verify_inputs()
                    if provisioning is not None:
                        if provisioning != expected_provisioning:
                            raise _contract.SourceBuildEnvironmentError(
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
            raise _contract.SourceBuildEnvironmentError(
                "published immutable environment failed admission and cannot be "
                f"reprovisioned: {root}; " + _attestation_differences(existing, custody)
            )
        if root.exists():
            if provisioning != expected_provisioning:
                raise _contract.SourceBuildEnvironmentError(
                    "immutable source-build environment address exists without "
                    f"its exact attestation or sibling provisioning record: {root}"
                )
            removed, error = delete_path(root, owning_root=root.parent)
            if not removed:
                raise _contract.SourceBuildEnvironmentError(
                    f"cannot recover unpublished source-build environment {root}: {error}"
                )
        elif provisioning is not None and provisioning != expected_provisioning:
            raise _contract.SourceBuildEnvironmentError(
                f"foreign source-build provisioning record: {provisioning_path}"
            )
        root.parent.mkdir(parents=True, exist_ok=True)
        if provisioning is None:
            write_exact(provisioning_path, expected_provisioning)

        environment = ProcessEnvironmentBuilder(
            os.environ if environ is None else environ
        )
        # This sync has an exact destination; the launcher's active venv is not
        # a second environment selector (nor a useful uv mismatch warning).
        environment.pop("VIRTUAL_ENV", None)
        environment["UV_PROJECT_ENVIRONMENT"] = str(root)
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        environment["UV_COMPILE_BYTECODE"] = "0"
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
            raise _contract.SourceBuildEnvironmentError(
                "locked source-build environment provisioning failed: "
                f"uv sync returned {result.returncode}"
            )
        if not python_executable.is_file():
            raise _contract.SourceBuildEnvironmentError(
                f"uv sync did not create source-build Python: {python_executable}"
            )
        verify_inputs()
        # The published read namespace is empty. CPython compiles only modules
        # actually imported by a command; -B keeps this image immutable.
        environment_bytecode_prefix(root).mkdir(exist_ok=True)
        if binding is not None:
            write_exact(
                root / _contract.DEVELOPMENT_ENVIRONMENT_SELECTION,
                development_selection_record(root, custody),
            )
        seal_environment_installation(root)
        realized = capture()
        verify_inputs()
        manifest = _attested_environment(custody, realized)
        _validate_recipe_inputs(repo_root, custody)
        verify_inputs()
        write_exact(manifest_path, manifest)
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
    manifest = _read_attestation(root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST)
    if manifest is None:
        raise _contract.SourceBuildEnvironmentError(
            f"selected environment has no published image attestation: {root}"
        )
    recipe_keys = set(_schema._SourceBuildAddress.__annotations__)
    field = (
        "realized_image"
        if manifest.get("editable_binding") is not None
        else "realized_environment"
    )
    if set(manifest) != recipe_keys | {"environment_id", field}:
        raise _contract.SourceBuildEnvironmentError(
            "published environment manifest shape is invalid"
        )
    recipe = {key: manifest[key] for key in recipe_keys}
    if (
        type(recipe["schema_version"]) is not int
        or recipe["schema_version"] != _contract.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION
        or recipe["bytecode_policy"] != _contract.IMAGE_BYTECODE_POLICY
        or not _contract.source_build_environment_contract_matches(
            recipe["producer_contract"]
        )
        or not valid_declared_input_digests(recipe["declared_inputs"])
        or manifest["environment_id"] != root.name
        or canonical_json_sha256(recipe) != root.name
    ):
        raise _contract.SourceBuildEnvironmentError(
            "published environment address/recipe is invalid"
        )
    from molt.python_uv_lock_identity import validate_uv_lock_group_closure

    closure = validate_uv_lock_group_closure(recipe["lock_closure"])
    runtime = validate_python_runtime_identity(recipe["python_runtime"])
    wheel_target = cast(Mapping[str, object], closure["wheel_target"])
    if wheel_target["runtime_closure_sha256"] != runtime["runtime_closure_sha256"]:
        raise _contract.SourceBuildEnvironmentError(
            "published wheel target differs from runtime recipe"
        )
    if (
        closure["dependency_groups"] != recipe["dependency_groups"]
        or closure["requirements"] != recipe["dependency_group_requirements"]
    ):
        raise _contract.SourceBuildEnvironmentError(
            "published environment selection differs from its closure"
        )
    if not _schema._marker_environment_matches_runtime(
        cast(Mapping[str, str], closure["marker_environment"]),
        runtime,
    ):
        raise _contract.SourceBuildEnvironmentError(
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
            raise _contract.SourceBuildEnvironmentError(
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
            raise _contract.SourceBuildEnvironmentError(
                "published development image projection is malformed"
            ) from exc
        if expected != manifest:
            raise _contract.SourceBuildEnvironmentError(
                "published development image projection differs from its recipe"
            )
    return custody, manifest


def admit_published_environment(
    root: Path,
    realized: Mapping[str, object],
    *,
    development_admission: _contract.DevelopmentEnvironmentRequest | None = None,
) -> Mapping[str, object]:
    """Compare inside the existing capture fence; never spawn or rescan here.

    The capture caller seals before opening its fence. Legacy attestations stay
    intact; only installed write-bit removals can differ in that live capture.
    """
    repo_root: Path | None = None
    if development_admission is not None:
        root, repo_root = development_coordinates(root, development_admission.repo_root)
    custody, manifest = _published_recipe(root)
    if development_admission is not None:
        assert repo_root is not None
        _validate_development_selection(
            root,
            repo_root=repo_root,
            python=development_admission.python,
            groups=development_admission.groups,
            custody=custody,
        )
    _admit_attestation(manifest, custody, realized)
    return manifest


def _validate_development_selection(
    root: Path,
    *,
    repo_root: Path,
    python: str | None,
    groups: tuple[str, ...] | None,
    custody: Mapping[str, object],
) -> None:
    """Validate one already-read recipe without opening its manifest again."""
    if groups is not None and custody["dependency_groups"] != sorted(set(groups)):
        raise _contract.SourceBuildEnvironmentError(
            "selected development image has different requested groups"
        )
    if python is not None:
        from molt.python_interpreter import (
            is_python_version_selector,
            parse_python_interpreter_request,
        )

        version = str(cast(Mapping[str, object], custody["python_runtime"])["version"])
        if not is_python_version_selector(python):
            raise _contract.SourceBuildEnvironmentError(
                "an interpreter command requires explicit runtime selection before image reuse"
            )
        try:
            request = parse_python_interpreter_request(python)
        except ValueError as exc:
            raise _contract.SourceBuildEnvironmentError(
                f"invalid Python interpreter request: {exc}"
            ) from exc
        if not request.matches_version(version):
            raise _contract.SourceBuildEnvironmentError(
                "selected development image has a different requested Python"
            )
    from molt.python_environment_location import validate_managed_bytecode_prefix

    try:
        validate_managed_bytecode_prefix(root)
    except PythonEnvironmentIdentityError as exc:
        raise _contract.SourceBuildEnvironmentError(str(exc)) from exc
    if custody.get("editable_binding") is None:
        raise _contract.SourceBuildEnvironmentError(
            "selected image is not a development environment"
        )
    _validate_recipe_inputs(repo_root, custody)


def load_development_environment(
    root: Path,
    *,
    repo_root: Path,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
) -> _schema.LockedSourceBuildEnvironment:
    """Read an exact selection at a boundary, without claiming live admission."""
    root, repo_root = development_coordinates(root, repo_root)
    custody, manifest = _published_recipe(root)
    _validate_development_selection(
        root, repo_root=repo_root, python=python, groups=groups, custody=custody
    )
    return _schema.LockedSourceBuildEnvironment(
        root,
        environment_python_path(root),
        root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST,
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
            raise _contract.SourceBuildEnvironmentError(
                "explicit development image has different requested groups"
            )
        selector = (
            python
            or (repo_root / ".python-version").read_text(encoding="utf-8").strip()
        )
        version = str(
            cast(Mapping[str, object], image.custody["python_runtime"])["version"]
        )
        from molt.python_interpreter import (
            is_python_version_selector,
            parse_python_interpreter_request,
        )

        if is_python_version_selector(selector):
            try:
                request = parse_python_interpreter_request(selector)
            except ValueError as exc:
                raise _contract.SourceBuildEnvironmentError(
                    f"invalid Python interpreter request: {exc}"
                ) from exc
            if not request.matches_version(version):
                raise _contract.SourceBuildEnvironmentError(
                    "explicit development image has a different requested Python"
                )
        else:
            _python, requested_runtime, _markers, _target = _selected_python_recipe(
                repo_root, selector, environment
            )
            if requested_runtime != image.custody["python_runtime"]:
                raise _contract.SourceBuildEnvironmentError(
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
