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
    development_selection_record,
)
from molt.file_deletion import delete_path
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.toolchain_identity import stable_executable_probe
from molt.python_file_node_custody import PythonFileCaptureContext
from molt.dx import checkout_custody, artifact_root, checkout_component
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
        timeout=python_environment_identity.PYTHON_IDENTITY_PROBE_TIMEOUT_S,
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
    if not any(
        str(row.get("path", "")).endswith(f"/{_contract.IMAGE_NO_BYTECODE_HOOK}")
        for row in cast(list[Mapping[str, object]], realized["site_bootstrap_files"])
    ):
        raise _contract.SourceBuildEnvironmentError(
            "provisioned source-build environment has no admitted no-bytecode hook"
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


def _project_legacy_attestation(
    manifest: Mapping[str, object], captured: Mapping[str, object]
) -> dict[str, object]:
    """Project the former uv lock and one-way installation sealing out of comparison.

    The stored attestation and live capture stay intact. Every retained file,
    alias and permission still compares exactly after these two projections.
    """
    field = "realized_image" if "realized_image" in manifest else "realized_environment"
    if not isinstance(manifest.get(field), Mapping) or not isinstance(
        captured.get(field), Mapping
    ):
        return dict(manifest)
    previous = cast(Mapping[str, object], manifest[field])
    current = cast(Mapping[str, object], captured[field])
    if field == "realized_image" and (
        set(previous) != {"image", "image_sha256"}
        or not isinstance(previous.get("image"), Mapping)
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
    from molt.python_environment_custody import without_legacy_uv_environment_lock

    projected = without_legacy_uv_environment_lock(image, tree, rows)
    changed = projected is not image
    if changed:
        tree = cast(Mapping[str, object], projected["tree"])
        rows = cast(list[Mapping[str, object]], tree["entries"])
    live_rows = {
        row["path"]: row
        for row in cast(list[Mapping[str, object]], live_tree["entries"])
    }
    transitioned = []
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
    projected_image = {
        **projected,
        "tree": {
            **tree,
            "entries": transitioned,
            "manifest_sha256": canonical_json_sha256(transitioned),
        },
    }
    if field == "realized_image":
        projection = {
            "image": projected_image,
            "image_sha256": canonical_json_sha256(projected_image),
        }
    else:
        material = {
            key: value
            for key, value in projected_image.items()
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
            f"{exc}; "
            + _attestation_differences(
                _project_legacy_attestation(manifest, captured), captured
            )
        ) from exc
    if manifest != expected:
        compared = _project_legacy_attestation(manifest, expected)
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


def _install_no_bytecode_hook(
    python_executable: Path, environment: ProcessEnvironment, root: Path
) -> None:
    """Write the image's no-bytecode hook into the site root its own CPython reads.

    The image interpreter names its purelib scheme; this process may run another
    CPython minor. Under -S, sys.prefix is the base install, so the scheme takes
    the image root explicitly, as capture's site roots do. Capture admits only
    the exact contract bytes there.
    """
    image = str(root.resolve(strict=True))
    script = (
        "import sysconfig\n"
        "from pathlib import Path\n"
        f"root = Path({image!r})\n"
        "site = Path(sysconfig.get_path('purelib', vars=dict.fromkeys("
        "('base', 'platbase', 'installed_base', 'installed_platbase'), str(root))))\n"
        "site.resolve(strict=True).relative_to(root)\n"
        f"(site / {_contract.IMAGE_NO_BYTECODE_HOOK!r})"
        f".write_bytes({_contract.IMAGE_NO_BYTECODE_HOOK_SOURCE!r})\n"
    )
    result = process_guard.run_completed_command(
        [str(python_executable), "-I", "-B", "-S", "-c", script],
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        timeout=60.0,
    )
    if result.returncode != 0:
        raise _contract.SourceBuildEnvironmentError(
            "cannot install the image no-bytecode hook: "
            + (
                result.stderr or result.stdout or f"returncode={result.returncode}"
            ).strip()
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
        from molt.python_interpreter import observe_python_interpreter

        selected_interpreter = observe_python_interpreter(
            Path(raw_base).resolve(strict=True), child_environment, cwd=repo_root
        )
        version = str(custody["python_runtime"]["version"])
        if selected_interpreter.version != version:
            raise _contract.SourceBuildEnvironmentError(
                f"cannot bind CPython {version}: found {selected_interpreter.executable} "
                f"(CPython {selected_interpreter.version}); run `uv python install {version}`"
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
            # uv derives UpgradePolicy::Fixed from a patch request. A file
            # request permits the managed minor alias even after realpath().
            version,
            "--no-python-downloads",
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
                f"uv sync returned {result.returncode}; selected "
                f"{selected_interpreter.executable} (CPython {selected_interpreter.version}); "
                f"run `uv python install {version}`"
            )
        if not python_executable.is_file():
            raise _contract.SourceBuildEnvironmentError(
                f"uv sync did not create source-build Python: {python_executable}"
            )
        verify_inputs()
        realized_interpreter = observe_python_interpreter(
            python_executable, child_environment, cwd=repo_root
        )
        if realized_interpreter != selected_interpreter:
            raise _contract.SourceBuildEnvironmentError(
                f"uv did not bind CPython {version} at {selected_interpreter.executable}: "
                f"found {realized_interpreter.executable} (CPython {realized_interpreter.version}); "
                f"run `uv python install {version}`"
            )
        realized_interpreter.publish(root)
        # The published read namespace is empty. CPython compiles only modules
        # actually imported by a command; -B and the image's own no-bytecode
        # site hook keep this image immutable.
        environment_bytecode_prefix(root).mkdir(exist_ok=True)
        _install_no_bytecode_hook(python_executable, child_environment, root)
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
