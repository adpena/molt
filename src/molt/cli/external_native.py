from __future__ import annotations

import contextlib
import json
import os
import re
from collections.abc import Collection, Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_copy_file, _remove_file_or_tree
from molt.cli.extension_manifest import _validate_extension_manifest
from molt.cli.file_hashing import _sha256_file
from molt.cli.models import (
    _BuildOutputLayout,
    _ExternalPackageNativeArtifact,
    _ExternalPackageNativeArtifactPlan,
    _ImportAdmissionPolicy,
    _StagedExternalPackageNativeArtifact,
)
from molt.cli.module_graph import _case_exact_file
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail


def _parse_external_static_packages(raw: str) -> tuple[frozenset[str], str | None]:
    packages: set[str] = set()
    for part in re.split(r"[\s,]+", raw.strip()):
        if not part:
            continue
        if not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*",
            part,
        ):
            return frozenset(), (
                "MOLT_EXTERNAL_STATIC_PACKAGES must contain comma/space-separated "
                f"Python package names; invalid entry: {part!r}"
            )
        packages.add(part)
    return frozenset(packages), None


_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES = (".so", ".pyd")


_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_EXCLUDED_DIRS = {
    ".git",
    ".hg",
    ".mypy_cache",
    ".nox",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "__pycache__",
    "build",
    "dist",
    "node_modules",
    "site-packages",
}


def _external_package_dir(root: Path, package: str) -> Path | None:
    package_dir = root.joinpath(*package.split("."))
    init_file = package_dir / "__init__.py"
    if package_dir.is_dir() and _case_exact_file(init_file):
        return package_dir.resolve()
    return None


def _is_external_package_native_artifact(path: Path) -> bool:
    name = path.name.lower()
    return any(
        name.endswith(suffix) for suffix in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES
    )


def _iter_external_package_native_artifacts(package_dir: Path) -> list[Path]:
    artifacts: list[Path] = []
    for current_root, dirnames, filenames in os.walk(package_dir):
        dirnames[:] = sorted(
            dirname
            for dirname in dirnames
            if dirname not in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_EXCLUDED_DIRS
            and not (Path(current_root) / dirname).is_symlink()
        )
        current = Path(current_root)
        for filename in sorted(filenames):
            path = current / filename
            if path.is_symlink() or not _is_external_package_native_artifact(path):
                continue
            artifacts.append(path.resolve())
    return artifacts


def _external_extension_module_name(
    *,
    package: str,
    package_dir: Path,
    artifact_path: Path,
) -> str:
    rel = artifact_path.resolve().relative_to(package_dir.resolve())
    parent_parts = rel.parent.parts
    basename = rel.name
    for suffix in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES:
        if basename.lower().endswith(suffix):
            basename = basename[: -len(suffix)]
            break
    basename = basename.split(".", 1)[0]
    return ".".join(part for part in (package, *parent_parts, basename) if part)


def _extension_path_matches_manifest(
    *,
    path: Path,
    manifest_extension: str,
    manifest_dir: Path,
    package_dir: Path,
) -> bool:
    expected_norm = manifest_extension.replace("\\", "/").strip()
    if not expected_norm:
        return False
    artifact_path = path.resolve()
    manifest_path = Path(expected_norm)
    if manifest_path.is_absolute():
        return manifest_path.resolve() == artifact_path
    return (manifest_dir / manifest_path).resolve() == artifact_path or (
        package_dir / manifest_path
    ).resolve() == artifact_path


def _find_external_extension_manifest(
    *,
    artifact_path: Path,
    package_dir: Path,
) -> Path | None:
    package_root = package_dir.resolve()
    current = artifact_path.resolve().parent
    for _ in range(6):
        if not (current == package_root or current.is_relative_to(package_root)):
            return None
        candidate = current / "extension_manifest.json"
        if _case_exact_file(candidate):
            return candidate.resolve()
        if current == package_root:
            return None
        current = current.parent
    return None


def _required_manifest_str(
    manifest: Mapping[str, Any],
    field_name: str,
    errors: list[str],
) -> str:
    value = manifest.get(field_name)
    if isinstance(value, str) and value.strip():
        return value.strip()
    errors.append(f"extension_manifest.json missing non-empty {field_name!r}")
    return ""


def _validate_external_package_native_artifact(
    *,
    package: str,
    package_dir: Path,
    artifact_path: Path,
) -> tuple[_ExternalPackageNativeArtifact | None, list[str]]:
    errors: list[str] = []
    module_name = _external_extension_module_name(
        package=package,
        package_dir=package_dir,
        artifact_path=artifact_path,
    )
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return None, [
            f"{package}: native artifact {artifact_path} is missing "
            "extension_manifest.json sidecar"
        ]
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return None, [f"{package}: invalid extension manifest {manifest_path}: {exc}"]
    if not isinstance(manifest, dict):
        return None, [
            f"{package}: extension manifest {manifest_path} must be an object"
        ]
    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=manifest_path.parent,
        wheel_path=None,
        require_capabilities=True,
        required_abi=None,
        require_checksum=False,
        warn_missing_checksum=False,
    )
    errors.extend(f"{package}: {error}" for error in validation.errors)
    manifest_module = _required_manifest_str(manifest, "module", errors)
    if manifest_module and manifest_module != module_name:
        errors.append(
            f"{package}: manifest module {manifest_module!r} does not match "
            f"native artifact module {module_name!r}"
        )
    manifest_extension = _required_manifest_str(manifest, "extension", errors)
    if manifest_extension and not _extension_path_matches_manifest(
        path=artifact_path,
        manifest_extension=manifest_extension,
        manifest_dir=manifest_path.parent,
        package_dir=package_dir,
    ):
        errors.append(
            f"{package}: manifest extension {manifest_extension!r} does not "
            f"match native artifact {artifact_path}"
        )
    expected_extension_sha = _required_manifest_str(
        manifest,
        "extension_sha256",
        errors,
    ).lower()
    actual_extension_sha = _sha256_file(artifact_path).lower()
    if expected_extension_sha and expected_extension_sha != actual_extension_sha:
        errors.append(
            f"{package}: extension_sha256 mismatch for {artifact_path.name}: "
            f"expected {expected_extension_sha}, got {actual_extension_sha}"
        )
    target_triple = _required_manifest_str(manifest, "target_triple", errors)
    platform_tag = _required_manifest_str(manifest, "platform_tag", errors)
    abi_tag = _required_manifest_str(manifest, "abi_tag", errors)
    if errors:
        return None, errors
    return (
        _ExternalPackageNativeArtifact(
            package=package,
            module=module_name,
            package_dir=package_dir.resolve(),
            path=artifact_path.resolve(),
            manifest_path=manifest_path.resolve(),
            extension_sha256=actual_extension_sha,
            manifest_sha256=_sha256_file(manifest_path),
            capabilities=tuple(validation.capabilities),
            abi_tag=abi_tag,
            target_triple=target_triple,
            platform_tag=platform_tag,
        ),
        [],
    )


def _resolve_external_package_native_artifact_plan(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
) -> tuple[_ExternalPackageNativeArtifactPlan | None, list[str]]:
    artifacts: list[_ExternalPackageNativeArtifact] = []
    errors: list[str] = []
    for package in sorted(admitted_packages):
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            for artifact_path in _iter_external_package_native_artifacts(package_dir):
                artifact, artifact_errors = _validate_external_package_native_artifact(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                errors.extend(artifact_errors)
                if artifact is not None:
                    artifacts.append(artifact)
    if errors:
        return None, errors
    return (
        _ExternalPackageNativeArtifactPlan(
            artifacts=tuple(
                sorted(artifacts, key=lambda item: (item.module, str(item.path)))
            )
        ),
        [],
    )


def _resolve_import_admission_policy(
    *,
    external_module_roots: Sequence[Path],
    json_output: bool,
) -> tuple[_ImportAdmissionPolicy | None, _CliFailure | None]:
    packages, error = _parse_external_static_packages(
        os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES", "")
    )
    if error is not None:
        return None, _fail(error, json_output, command="build")
    native_plan, native_plan_errors = _resolve_external_package_native_artifact_plan(
        external_module_roots=external_module_roots,
        admitted_packages=packages,
    )
    if native_plan_errors:
        return None, _fail(
            "External static package native-artifact custody errors: "
            + "; ".join(native_plan_errors),
            json_output,
            command="build",
        )
    assert native_plan is not None
    return _ImportAdmissionPolicy(
        external_roots=tuple(external_module_roots),
        admitted_external_packages=packages,
        native_artifact_plan=native_plan,
    ), None


def _external_package_source_root(package_dir: Path, package: str) -> Path:
    resolved = package_dir.resolve()
    package_parts = tuple(part for part in package.split(".") if part)
    if (
        package_parts
        and len(resolved.parts) >= len(package_parts)
        and tuple(resolved.parts[-len(package_parts) :]) == package_parts
    ):
        return resolved.parents[len(package_parts) - 1]
    return resolved.parent


def _external_package_init_source_paths(
    *,
    package_dir: Path,
    package: str,
) -> tuple[Path, ...]:
    package_root = _external_package_source_root(package_dir, package)
    package_parts = tuple(part for part in package.split(".") if part)
    return tuple(
        package_root.joinpath(*package_parts[:index], "__init__.py")
        for index in range(1, len(package_parts) + 1)
    )


def _external_native_support_source_paths(
    artifact: _ExternalPackageNativeArtifact,
) -> tuple[Path, ...]:
    out: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            out.append(path)

    for init_path in _external_package_init_source_paths(
        package_dir=artifact.package_dir,
        package=artifact.package,
    ):
        add(init_path)

    artifact_path = artifact.path
    add(artifact_path.with_name(f"{artifact_path.name}.molt.py"))
    add(artifact_path.with_name(f"{artifact_path.name}.py"))
    stripped = artifact_path.with_suffix("")
    if stripped != artifact_path:
        add(stripped.with_name(f"{stripped.name}.molt.py"))
        add(stripped.with_name(f"{stripped.name}.py"))
        for marker in (".cpython-", ".abi", ".cp"):
            marker_index = stripped.name.rfind(marker)
            if marker_index > 0:
                prefix_name = stripped.name[:marker_index]
                add(stripped.with_name(f"{prefix_name}.molt.py"))
                add(stripped.with_name(f"{prefix_name}.py"))
    if artifact_path.parent.name == "__pycache__":
        parent = artifact_path.parent.parent
        stem = artifact_path.name.rsplit(".", 1)[0]
        module_stem = stem.split(".", 1)[0]
        if module_stem:
            add(parent / f"{module_stem}.molt.py")
            add(parent / f"{module_stem}.py")
    local_name = artifact.module.rsplit(".", 1)[-1]
    if local_name:
        add(artifact_path.parent / f"{local_name}.molt.py")
        add(artifact_path.parent / f"{local_name}.py")
        add(artifact_path.parent / local_name / "__init__.molt.py")
        add(artifact_path.parent / local_name / "__init__.py")
    if artifact_path.name.startswith("__init__."):
        add(artifact_path.parent / "__init__.molt.py")
        add(artifact_path.parent / "__init__.py")
    return tuple(out)


def _external_staged_path_for_source(
    *,
    runtime_root: Path,
    package_source_root: Path,
    source_path: Path,
) -> Path:
    resolved_source = source_path.resolve()
    try:
        relative = resolved_source.relative_to(package_source_root)
    except ValueError as exc:
        raise OSError(
            f"external native support path escapes admitted package root: {source_path}"
        ) from exc
    return runtime_root / relative


def _remove_staged_external_candidate(path: Path) -> None:
    with contextlib.suppress(OSError):
        if path.exists() or path.is_symlink():
            _remove_file_or_tree(path)


def _stage_external_native_required_file(
    *,
    source_path: Path,
    staged_path: Path,
    expected_sha256: str,
    label: str,
) -> None:
    expected = expected_sha256.lower()
    actual = _sha256_file(source_path).lower()
    if actual != expected:
        raise OSError(
            f"External native artifact {label} checksum changed before staging: "
            f"{source_path} expected {expected}, got {actual}"
        )
    _atomic_copy_file(source_path, staged_path)
    staged = _sha256_file(staged_path).lower()
    if staged != expected:
        _remove_staged_external_candidate(staged_path)
        raise OSError(
            f"External native artifact {label} changed during staging: "
            f"{source_path} expected {expected}, staged {staged}"
        )


def _stage_external_native_support_files(
    artifact: _ExternalPackageNativeArtifact,
    *,
    runtime_root: Path,
    package_source_root: Path,
) -> tuple[Path, ...]:
    staged_paths: list[Path] = []
    for source_path in _external_native_support_source_paths(artifact):
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=source_path,
        )
        if source_path.is_file():
            _atomic_copy_file(source_path, staged_path)
            staged_paths.append(staged_path)
        else:
            _remove_staged_external_candidate(staged_path)
    return tuple(staged_paths)


def _stage_external_package_native_artifacts_for_build(
    native_artifact_plan: _ExternalPackageNativeArtifactPlan,
    *,
    artifacts_root: Path,
) -> tuple[_StagedExternalPackageNativeArtifact, ...]:
    if not native_artifact_plan.artifacts:
        return ()
    runtime_root = (
        artifacts_root / "external_static_packages" / native_artifact_plan.digest()
    )
    staged_artifacts: list[_StagedExternalPackageNativeArtifact] = []
    for artifact in native_artifact_plan.artifacts:
        package_source_root = _external_package_source_root(
            artifact.package_dir,
            artifact.package,
        )
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=artifact.path,
        )
        staged_manifest_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=artifact.manifest_path,
        )
        _stage_external_native_required_file(
            source_path=artifact.path,
            staged_path=staged_path,
            expected_sha256=artifact.extension_sha256,
            label="extension",
        )
        _stage_external_native_required_file(
            source_path=artifact.manifest_path,
            staged_path=staged_manifest_path,
            expected_sha256=artifact.manifest_sha256,
            label="manifest",
        )
        staged_support_paths = _stage_external_native_support_files(
            artifact,
            runtime_root=runtime_root,
            package_source_root=package_source_root,
        )
        staged_artifacts.append(
            _StagedExternalPackageNativeArtifact(
                package=artifact.package,
                module=artifact.module,
                runtime_root=runtime_root,
                source_path=artifact.path,
                source_manifest_path=artifact.manifest_path,
                staged_path=staged_path,
                staged_manifest_path=staged_manifest_path,
                staged_support_paths=staged_support_paths,
                extension_sha256=artifact.extension_sha256,
                manifest_sha256=artifact.manifest_sha256,
                capabilities=artifact.capabilities,
                abi_tag=artifact.abi_tag,
                target_triple=artifact.target_triple,
                platform_tag=artifact.platform_tag,
            )
        )
    return tuple(staged_artifacts)


def _external_native_artifact_output_custody_error(
    *,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan,
    output_layout: _BuildOutputLayout,
    target: str,
) -> str | None:
    if not native_artifact_plan.artifacts:
        return None
    if (
        not output_layout.is_wasm
        and not output_layout.is_rust_transpile
        and not output_layout.is_luau_transpile
        and not output_layout.is_mlir_emit
        and output_layout.emit_mode == "bin"
    ):
        return None
    packages = ", ".join(
        sorted({artifact.package for artifact in native_artifact_plan.artifacts})
    )
    return (
        "External static native packages require native binary output so Molt can "
        "stage validated package bytes and inject the staged root into runtime "
        "import custody before startup. "
        f"Unsupported target/emit combination: target={target}, "
        f"emit={output_layout.emit_mode}, packages={packages}."
    )
