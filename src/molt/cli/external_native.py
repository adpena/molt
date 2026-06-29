from __future__ import annotations

import contextlib
import json
import os
import re
from collections.abc import Collection, Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.cli.atomic_io import _atomic_copy_file, _remove_file_or_tree
from molt.cli.extension_manifest import (
    _host_target_triple,
    _validate_extension_manifest,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.models import (
    _BuildOutputLayout,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ExternalPackageNativeArtifact,
    _ExternalPackageNativeArtifactPlan,
    _ImportAdmissionPolicy,
    _StagedExternalPackageNativeArtifact,
)
from molt.cli.module_resolution import _case_exact_file
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
_EXTERNAL_PACKAGE_STATIC_LINK_ARTIFACT_SUFFIXES = (".molt.wasm", ".o", ".a")
_EXTERNAL_PACKAGE_ARTIFACT_SUFFIXES = (
    *_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES,
    *_EXTERNAL_PACKAGE_STATIC_LINK_ARTIFACT_SUFFIXES,
)
_SOURCE_RECOMPILED_EXTERNAL_PACKAGE_ROOTS = frozenset({"numpy", "scipy"})
_EXTERNAL_PACKAGE_EXTENSION_MANIFEST = "extension_manifest.json"
_EXTERNAL_PACKAGE_ARTIFACT_MANIFEST_SUFFIX = ".extension_manifest.json"


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


def _source_recompiled_external_package_root(package: str) -> str | None:
    normalized = package.strip()
    for root in sorted(_SOURCE_RECOMPILED_EXTERNAL_PACKAGE_ROOTS):
        if normalized == root or normalized.startswith(root + "."):
            return root
    return None


def _native_artifact_source_packages_for_admission(
    admitted_packages: Collection[str],
) -> frozenset[str]:
    return frozenset(
        root
        for package in admitted_packages
        if (root := _source_recompiled_external_package_root(package)) is not None
    )


def _is_external_package_native_artifact(path: Path) -> bool:
    name = path.name.lower()
    return any(
        name.endswith(suffix) for suffix in _EXTERNAL_PACKAGE_NATIVE_ARTIFACT_SUFFIXES
    )


def _is_external_package_static_link_artifact(path: Path) -> bool:
    name = path.name.lower()
    return any(
        name.endswith(suffix)
        for suffix in _EXTERNAL_PACKAGE_STATIC_LINK_ARTIFACT_SUFFIXES
    )


def _external_artifact_manifest_path(artifact_path: Path) -> Path:
    return artifact_path.with_name(
        artifact_path.name + _EXTERNAL_PACKAGE_ARTIFACT_MANIFEST_SUFFIX
    )


def _is_external_extension_manifest_filename(filename: str) -> bool:
    lowered = filename.lower()
    return lowered == _EXTERNAL_PACKAGE_EXTENSION_MANIFEST or lowered.endswith(
        _EXTERNAL_PACKAGE_ARTIFACT_MANIFEST_SUFFIX
    )


def _manifest_declared_static_link_artifact(
    *,
    manifest_path: Path,
    package_dir: Path,
) -> Path | None:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(manifest, dict):
        return None
    if manifest.get("loader_kind") != "libmolt_source":
        return None
    if manifest.get("runtime_linkage") != "static_link":
        return None
    extension = manifest.get("extension")
    if not isinstance(extension, str) or not extension.strip():
        return None
    extension_path = Path(extension.strip())
    candidates = (
        (extension_path,)
        if extension_path.is_absolute()
        else (manifest_path.parent / extension_path, package_dir / extension_path)
    )
    package_root = package_dir.resolve()
    for candidate in candidates:
        resolved = candidate.resolve()
        if not (resolved == package_root or resolved.is_relative_to(package_root)):
            continue
        if _case_exact_file(resolved) and _is_external_package_static_link_artifact(
            resolved
        ):
            return resolved
    return None


def _iter_external_package_native_artifacts(package_dir: Path) -> list[Path]:
    artifacts: dict[Path, None] = {}
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
            if path.is_symlink():
                continue
            if _is_external_package_native_artifact(path):
                artifacts[path.resolve()] = None
                continue
            if _is_external_extension_manifest_filename(filename):
                declared = _manifest_declared_static_link_artifact(
                    manifest_path=path,
                    package_dir=package_dir,
                )
                if declared is not None:
                    artifacts[declared] = None
    return sorted(artifacts)


def _external_package_native_artifact_candidate_errors(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
) -> list[str]:
    errors: list[str] = []
    for package in sorted(admitted_packages):
        native_root = _source_recompiled_external_package_root(package)
        if native_root is None:
            continue
        package_dirs: list[Path] = []
        artifact_candidate_count = 0
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            package_dirs.append(package_dir)
            artifact_candidate_count += len(
                _iter_external_package_native_artifacts(package_dir)
            )
        if not package_dirs or artifact_candidate_count:
            continue
        roots = ", ".join(str(path) for path in package_dirs)
        errors.append(
            f"{package}: source-recompiled external static package admission "
            f"for native package root {native_root!r} requires at least one "
            "package-local Molt native artifact candidate before graph admission; "
            f"found none under {roots}. Build or stage libmolt source-recompiled "
            f"{native_root} artifacts, or remove {package!r} from "
            "MOLT_EXTERNAL_STATIC_PACKAGES."
        )
    return errors


def _external_extension_module_name(
    *,
    package: str,
    package_dir: Path,
    artifact_path: Path,
) -> str:
    rel = artifact_path.resolve().relative_to(package_dir.resolve())
    parent_parts = rel.parent.parts
    basename = rel.name
    for suffix in _EXTERNAL_PACKAGE_ARTIFACT_SUFFIXES:
        if basename.lower().endswith(suffix):
            basename = basename[: -len(suffix)]
            break
    basename = basename.split(".", 1)[0]
    return ".".join(part for part in (package, *parent_parts, basename) if part)


def _external_native_artifact_module_required(
    module_name: str,
    required_modules: frozenset[str] | None,
) -> bool:
    if required_modules is None:
        return True
    for required_module in required_modules:
        if (
            module_name == required_module
            or module_name.startswith(required_module + ".")
            or required_module.startswith(module_name + ".")
        ):
            return True
    return False


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
    artifact_specific = _external_artifact_manifest_path(artifact_path.resolve())
    if artifact_specific.parent == artifact_path.resolve().parent and _case_exact_file(
        artifact_specific
    ):
        return artifact_specific.resolve()
    current = artifact_path.resolve().parent
    for _ in range(6):
        if not (current == package_root or current.is_relative_to(package_root)):
            return None
        candidate = current / _EXTERNAL_PACKAGE_EXTENSION_MANIFEST
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


def _manifest_str_tuple(
    manifest: Mapping[str, Any],
    field_name: str,
) -> tuple[str, ...]:
    value = manifest.get(field_name)
    if not isinstance(value, list):
        return ()
    return tuple(
        sorted(
            {item.strip() for item in value if isinstance(item, str) and item.strip()}
        )
    )


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
    loader_kind = _required_manifest_str(manifest, "loader_kind", errors)
    if loader_kind and loader_kind != "libmolt_source":
        errors.append(
            f"{package}: external static package native artifacts must use "
            f"loader_kind 'libmolt_source', found {loader_kind!r}"
        )
    runtime_linkage = _required_manifest_str(manifest, "runtime_linkage", errors)
    artifact_kind = _required_manifest_str(manifest, "artifact_kind", errors)
    init_symbol = _required_manifest_str(manifest, "init_symbol", errors)
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
    provided_capsules = _manifest_str_tuple(manifest, "provided_capsules")
    object_closure = manifest.get("object_closure")
    required_capsules: tuple[str, ...] = ()
    if isinstance(object_closure, Mapping):
        required_capsules = _manifest_str_tuple(object_closure, "required_capsules")
    if errors:
        return None, errors
    support_file_sha256 = _external_native_support_file_sha256(
        package=package,
        package_dir=package_dir,
        module=module_name,
    )
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
            init_symbol=init_symbol,
            runtime_linkage=runtime_linkage,
            artifact_kind=artifact_kind,
            support_file_sha256=support_file_sha256,
            provided_capsules=provided_capsules,
            required_capsules=required_capsules,
        ),
        [],
    )


def _peek_external_artifact_provided_capsules(
    *,
    artifact_path: Path,
    package_dir: Path,
) -> tuple[str, ...]:
    manifest_path = _find_external_extension_manifest(
        artifact_path=artifact_path,
        package_dir=package_dir,
    )
    if manifest_path is None:
        return ()
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(manifest, dict):
        return ()
    return _manifest_str_tuple(manifest, "provided_capsules")


def _capsule_provider_errors(
    artifacts: Sequence[_ExternalPackageNativeArtifact],
) -> list[str]:
    providers: dict[str, str] = {}
    errors: list[str] = []
    for artifact in artifacts:
        for capsule in artifact.provided_capsules:
            existing = providers.get(capsule)
            if existing is not None and existing != artifact.module:
                errors.append(
                    f"{artifact.package}: capsule {capsule!r} is provided by "
                    f"multiple native artifacts: {existing}, {artifact.module}"
                )
                continue
            providers[capsule] = artifact.module
    return errors


def _missing_capsule_requirements(
    artifacts: Sequence[_ExternalPackageNativeArtifact],
) -> dict[str, tuple[_ExternalPackageNativeArtifact, ...]]:
    provided = {
        capsule for artifact in artifacts for capsule in artifact.provided_capsules
    }
    missing: dict[str, list[_ExternalPackageNativeArtifact]] = {}
    for artifact in artifacts:
        for capsule in artifact.required_capsules:
            if capsule not in provided:
                missing.setdefault(capsule, []).append(artifact)
    return {
        capsule: tuple(sorted(consumers, key=lambda artifact: artifact.module))
        for capsule, consumers in sorted(missing.items())
    }


def _close_external_native_capsule_provider_artifacts(
    artifacts: Sequence[_ExternalPackageNativeArtifact],
    provider_candidates: Sequence[tuple[str, Path, Path, str]],
) -> tuple[tuple[_ExternalPackageNativeArtifact, ...] | None, list[str]]:
    selected = list(artifacts)
    selected_keys = {(artifact.package, artifact.path) for artifact in selected}
    remaining = {
        (package, artifact_path): (
            package,
            package_dir,
            artifact_path,
            module_name,
        )
        for package, package_dir, artifact_path, module_name in provider_candidates
        if (package, artifact_path) not in selected_keys
    }
    while True:
        provider_errors = _capsule_provider_errors(selected)
        if provider_errors:
            return None, provider_errors
        missing = _missing_capsule_requirements(selected)
        if not missing:
            return tuple(selected), []
        to_add: dict[
            tuple[str, Path],
            tuple[str, Path, Path, str, _ExternalPackageNativeArtifact],
        ] = {}
        errors: list[str] = []
        for capsule, consumers in missing.items():
            consumer_modules = ", ".join(consumer.module for consumer in consumers)
            consumer_keys = {
                (consumer.runtime_linkage, consumer.target_triple.lower())
                for consumer in consumers
            }
            providers: list[
                tuple[
                    str,
                    Path,
                    Path,
                    str,
                    _ExternalPackageNativeArtifact,
                ]
            ] = []
            incompatible_providers: list[str] = []
            for candidate in remaining.values():
                package, package_dir, artifact_path, _module_name = candidate
                if capsule not in _peek_external_artifact_provided_capsules(
                    artifact_path=artifact_path,
                    package_dir=package_dir,
                ):
                    continue
                provider, provider_errors = _validate_external_package_native_artifact(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                if provider_errors:
                    return None, provider_errors
                assert provider is not None
                provider_key = (
                    provider.runtime_linkage,
                    provider.target_triple.lower(),
                )
                if provider_key in consumer_keys:
                    providers.append((*candidate, provider))
                else:
                    incompatible_providers.append(
                        f"{provider.module}={provider.runtime_linkage}/"
                        f"{provider.target_triple}"
                    )
            if not providers:
                suffix = ""
                if incompatible_providers:
                    suffix = "; incompatible provider artifact(s): " + ", ".join(
                        sorted(incompatible_providers)
                    )
                consumer_key_text = ", ".join(
                    f"{linkage}/{target}" for linkage, target in sorted(consumer_keys)
                )
                errors.append(
                    "native capsule "
                    f"{capsule!r} required by {consumer_modules} has no "
                    "target-compatible validated provider artifact in the admitted "
                    f"package plan for {consumer_key_text}{suffix}"
                )
                continue
            if len(providers) > 1:
                provider_names = ", ".join(sorted(item[3] for item in providers))
                errors.append(
                    "native capsule "
                    f"{capsule!r} required by {consumer_modules} has multiple "
                    f"candidate provider artifacts: {provider_names}"
                )
                continue
            provider = providers[0]
            to_add[(provider[0], provider[2])] = provider
        if errors:
            return None, errors
        if not to_add:
            return None, [
                "native capsule provider closure made no progress for "
                + ", ".join(missing)
            ]
        for package, _package_dir, artifact_path, _module_name, artifact in sorted(
            to_add.values(),
            key=lambda item: (item[0], item[3], str(item[2])),
        ):
            selected.append(artifact)
            key = (artifact.package, artifact.path)
            selected_keys.add(key)
            remaining.pop(key, None)


def _resolve_external_package_native_artifact_plan(
    *,
    external_module_roots: Sequence[Path],
    admitted_packages: Collection[str],
    required_modules: Collection[str] | None = None,
) -> tuple[_ExternalPackageNativeArtifactPlan | None, list[str]]:
    artifacts: list[_ExternalPackageNativeArtifact] = []
    errors: list[str] = []
    errors.extend(
        _external_package_native_artifact_candidate_errors(
            external_module_roots=external_module_roots,
            admitted_packages=admitted_packages,
        )
    )
    if errors:
        return None, errors
    required = frozenset(required_modules) if required_modules is not None else None
    provider_candidates: list[tuple[str, Path, Path, str]] = []
    for package in sorted(admitted_packages):
        for root in external_module_roots:
            package_dir = _external_package_dir(root.resolve(), package)
            if package_dir is None:
                continue
            for artifact_path in _iter_external_package_native_artifacts(package_dir):
                module_name = _external_extension_module_name(
                    package=package,
                    package_dir=package_dir,
                    artifact_path=artifact_path,
                )
                provider_candidates.append(
                    (package, package_dir, artifact_path, module_name)
                )
                if not _external_native_artifact_module_required(
                    module_name,
                    required,
                ):
                    continue
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
    closed_artifacts, capsule_errors = _close_external_native_capsule_provider_artifacts(
        artifacts,
        provider_candidates,
    )
    if capsule_errors:
        return None, capsule_errors
    assert closed_artifacts is not None
    return (
        _ExternalPackageNativeArtifactPlan(
            artifacts=tuple(
                sorted(closed_artifacts, key=lambda item: (item.module, str(item.path)))
            )
        ),
        [],
    )


def _external_native_artifact_error_summary(
    errors: Sequence[str],
    *,
    limit: int = 12,
) -> str:
    if len(errors) <= limit:
        return "; ".join(errors)
    shown = "; ".join(errors[:limit])
    remaining = len(errors) - limit
    return (
        f"{shown}; ... and {remaining} more external native artifact custody error(s)."
    )


def _resolve_import_admission_policy(
    *,
    external_module_roots: Sequence[Path],
    json_output: bool,
    defer_native_artifacts: bool = False,
) -> tuple[_ImportAdmissionPolicy | None, _CliFailure | None]:
    packages, error = _parse_external_static_packages(
        os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES", "")
    )
    if error is not None:
        return None, _fail(error, json_output, command="build")
    candidate_errors = _external_package_native_artifact_candidate_errors(
        external_module_roots=external_module_roots,
        admitted_packages=packages,
    )
    if candidate_errors:
        return None, _fail(
            "External static package native-artifact custody errors: "
            + _external_native_artifact_error_summary(candidate_errors),
            json_output,
            command="build",
        )
    if defer_native_artifacts:
        native_plan = _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    else:
        native_plan, native_plan_errors = _resolve_external_package_native_artifact_plan(
            external_module_roots=external_module_roots,
            admitted_packages=packages,
        )
        if native_plan_errors:
            return None, _fail(
                "External static package native-artifact custody errors: "
                + _external_native_artifact_error_summary(native_plan_errors),
                json_output,
                command="build",
            )
        assert native_plan is not None
    return _ImportAdmissionPolicy(
        external_roots=tuple(external_module_roots),
        admitted_external_packages=packages,
        native_artifact_source_packages=(
            _native_artifact_source_packages_for_admission(packages)
        ),
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


def _external_native_support_source_paths_for(
    *,
    package: str,
    package_dir: Path,
    module: str,
) -> tuple[Path, ...]:
    out: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            out.append(path)

    package_source_root = _external_package_source_root(package_dir, package)
    module_parent_parts = tuple(part for part in module.split(".")[:-1] if part)
    for index in range(1, len(module_parent_parts) + 1):
        init_path = package_source_root.joinpath(
            *module_parent_parts[:index],
            "__init__.py",
        )
        add(init_path)

    return tuple(out)


def _external_native_support_source_paths(
    artifact: _ExternalPackageNativeArtifact,
) -> tuple[Path, ...]:
    return _external_native_support_source_paths_for(
        package=artifact.package,
        package_dir=artifact.package_dir,
        module=artifact.module,
    )


def _external_native_support_file_sha256(
    *,
    package: str,
    package_dir: Path,
    module: str,
) -> tuple[tuple[str, str], ...]:
    package_source_root = _external_package_source_root(package_dir, package)
    support: list[tuple[str, str]] = []
    for source_path in _external_native_support_source_paths_for(
        package=package,
        package_dir=package_dir,
        module=module,
    ):
        if not source_path.is_file():
            continue
        rel_path = (
            source_path.resolve().relative_to(package_source_root.resolve()).as_posix()
        )
        support.append((rel_path, _sha256_file(source_path).lower()))
    return tuple(sorted(support))


def _external_native_disallowed_shim_source_paths(
    artifact: _ExternalPackageNativeArtifact,
) -> tuple[Path, ...]:
    out: list[Path] = []
    seen: set[Path] = set()

    def add(path: Path) -> None:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            out.append(path)

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
    expected_support = dict(artifact.support_file_sha256)
    for source_path in _external_native_support_source_paths(artifact):
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=source_path,
        )
        rel_path = (
            source_path.resolve().relative_to(package_source_root.resolve()).as_posix()
        )
        if source_path.is_file():
            expected_sha256 = expected_support.get(rel_path)
            if expected_sha256 is None:
                raise OSError(
                    "External native support file appeared after plan resolution: "
                    f"{source_path}"
                )
            _stage_external_native_required_file(
                source_path=source_path,
                staged_path=staged_path,
                expected_sha256=expected_sha256,
                label=f"support file {rel_path}",
            )
            staged_paths.append(staged_path)
        elif rel_path in expected_support:
            raise OSError(
                "External native support file disappeared after plan resolution: "
                f"{source_path}"
            )
        else:
            _remove_staged_external_candidate(staged_path)
    for source_path in _external_native_disallowed_shim_source_paths(artifact):
        staged_path = _external_staged_path_for_source(
            runtime_root=runtime_root,
            package_source_root=package_source_root,
            source_path=source_path,
        )
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
                init_symbol=artifact.init_symbol,
                runtime_linkage=artifact.runtime_linkage,
                artifact_kind=artifact.artifact_kind,
                support_file_sha256=artifact.support_file_sha256,
                provided_capsules=artifact.provided_capsules,
                required_capsules=artifact.required_capsules,
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
        expected_target = (output_layout.target_triple or _host_target_triple()).lower()
        mismatches = [
            f"{artifact.module}={artifact.target_triple}"
            for artifact in native_artifact_plan.artifacts
            if artifact.target_triple.lower() != expected_target
        ]
        if mismatches:
            return (
                "External static package native-artifact target mismatch: "
                f"expected {expected_target}; " + ", ".join(mismatches)
            )
        linkage_mismatches = [
            f"{artifact.module}={artifact.runtime_linkage}/{artifact.artifact_kind}"
            for artifact in native_artifact_plan.artifacts
            if artifact.runtime_linkage != "host_resolved"
            or artifact.artifact_kind != "shared_library"
        ]
        if linkage_mismatches:
            return (
                "External static package native binary output requires "
                "host_resolved shared_library artifacts: "
                + ", ".join(linkage_mismatches)
            )
        return None
    if output_layout.is_wasm and output_layout.linked:
        mismatches = [
            f"{artifact.module}={artifact.runtime_linkage}/"
            f"{artifact.artifact_kind}/{artifact.target_triple}"
            for artifact in native_artifact_plan.artifacts
            if artifact.runtime_linkage != "static_link"
            or artifact.artifact_kind
            not in {
                "wasm_relocatable_object",
                "static_archive",
            }
            or not artifact.target_triple.lower().startswith("wasm32")
        ]
        if mismatches:
            return (
                "Linked WASM external static packages require wasm32 static_link "
                "libmolt_source artifacts: " + ", ".join(mismatches)
            )
        return None
    packages = ", ".join(
        sorted({artifact.package for artifact in native_artifact_plan.artifacts})
    )
    return (
        "External static packages require native binary output with host_resolved "
        "shared artifacts, or linked WASM output with wasm32 static_link artifacts. "
        f"Unsupported target/emit combination: target={target}, "
        f"emit={output_layout.emit_mode}, packages={packages}."
    )
