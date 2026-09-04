"""Portable, target-parametric Pact witness acceptance receipt authority."""

from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path, PurePosixPath
from typing import Any

from molt.exact_json import ExactJsonError, loads_exact
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.toolchain_identity import stable_file_sha256
from tools.git_identity import is_git_object_id

SCHEMA_VERSION = 2
KIND = "molt-pact-witness-acceptance"
STATUS_PASS = "PASS"
TARGETS = frozenset({"native", "wasm"})
PACKAGE_NAMES = frozenset({"numpy", "scipy"})
_HEX = frozenset("0123456789abcdef")
_COMMON_ARTIFACT_ROLES = frozenset(
    {"candidate_outputs", "reference_oracle", "target_artifact"}
)


def _portable_path(value: object) -> PurePosixPath | None:
    try:
        return portable_relative_path(value)
    except ValueError:
        return None


def _register_closure_path(
    paths: dict[str, tuple[PurePosixPath, str]],
    path: PurePosixPath,
    *,
    field: str,
    allow_exact_alias: bool = False,
) -> str | None:
    identity = portable_path_identity(path.as_posix())
    prior = paths.get(identity)
    if prior is None:
        paths[identity] = (path, field)
        return None
    prior_path, prior_field = prior
    if allow_exact_alias and prior_path == path:
        return None
    return (
        f"{field}.path collides with {prior_field}.path under the portable "
        f"filesystem identity: {path.as_posix()!r} versus {prior_path.as_posix()!r}"
    )


def _receipt_root(receipt_path: Path) -> Path:
    return receipt_path.resolve().parent


def _resolve_receipt_artifact(
    value: object,
    *,
    receipt_root: Path | None,
) -> Path | None:
    relative = _portable_path(value)
    if relative is None or receipt_root is None:
        return None
    candidate = receipt_root.joinpath(*relative.parts)
    try:
        resolved = candidate.resolve()
    except (OSError, ValueError):
        return None
    return resolved if resolved.is_relative_to(receipt_root) else None


def artifact_receipt(
    role: str,
    path: Path,
    *,
    receipt_path: Path,
) -> dict[str, str | int]:
    """Bind one file beneath ``receipt_path.parent`` with a portable path."""

    resolved = path.resolve(strict=True)
    if not resolved.is_file():
        raise ValueError(f"Pact acceptance artifact is not a file: {resolved}")
    receipt_root = _receipt_root(receipt_path)
    try:
        relative = resolved.relative_to(receipt_root)
    except ValueError as exc:
        raise ValueError(
            "Pact acceptance artifact must remain beneath the receipt directory: "
            f"artifact={resolved} receipt={receipt_path.resolve()}"
        ) from exc
    portable = PurePosixPath(*relative.parts).as_posix()
    if _portable_path(portable) is None:
        raise ValueError(f"Pact acceptance artifact path is not portable: {portable!r}")
    return {
        "role": role,
        "path": portable,
        "sha256": stable_file_sha256(resolved, label="Pact witness receipt input"),
        "size": resolved.stat().st_size,
    }


def _sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in _HEX for character in value)
    )


def _validate_hashed_file(
    value: object,
    *,
    field: str,
    receipt_root: Path | None,
    require_artifacts: bool,
) -> list[str]:
    expected_keys = {"path", "sha256", "size"}
    if not isinstance(value, Mapping) or set(value) != expected_keys:
        return [f"{field} must contain exactly path, sha256, and size"]
    raw_path = value.get("path")
    digest = value.get("sha256")
    size = value.get("size")
    path = _resolve_receipt_artifact(
        raw_path,
        receipt_root=receipt_root,
    )
    problems: list[str] = []
    if _portable_path(raw_path) is None:
        problems.append(f"{field}.path must be a portable relative POSIX path")
    elif receipt_root is not None and path is None:
        problems.append(f"{field}.path escapes the receipt directory")
    if not _sha256(digest):
        problems.append(f"{field}.sha256 must be lowercase SHA-256")
    if not isinstance(size, int) or isinstance(size, bool) or size < 0:
        problems.append(f"{field}.size must be a non-negative integer")
    if require_artifacts and path is not None:
        if not path.is_file():
            problems.append(f"{field} artifact does not exist: {path}")
        else:
            if isinstance(size, int) and not isinstance(size, bool):
                if path.stat().st_size != size:
                    problems.append(f"{field} artifact size mismatch: {path}")
            if (
                _sha256(digest)
                and stable_file_sha256(
                    path,
                    label="Pact witness receipt artifact",
                )
                != digest
            ):
                problems.append(f"{field} artifact checksum mismatch: {path}")
    return problems


def _validate_wasm_execution_manifest(
    manifest_item: Mapping[str, Any],
    target_item: Mapping[str, Any],
    *,
    receipt_root: Path,
    closure_paths: dict[str, tuple[PurePosixPath, str]],
) -> list[str]:
    manifest_path = _resolve_receipt_artifact(
        manifest_item.get("path"),
        receipt_root=receipt_root,
    )
    target_path = _resolve_receipt_artifact(
        target_item.get("path"),
        receipt_root=receipt_root,
    )
    if manifest_path is None or target_path is None or not manifest_path.is_file():
        return []
    try:
        payload = loads_exact(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError) as exc:
        return [f"acceptance receipt execution manifest is invalid: {exc}"]
    if not isinstance(payload, Mapping):
        return ["acceptance receipt execution manifest root must be an object"]
    mode = payload.get("mode")
    labels = (
        {"linked"}
        if mode == "linked"
        else {"app", "runtime"}
        if mode == "split-runtime"
        else set()
    )
    problems: list[str] = []
    if not labels:
        problems.append(
            f"acceptance receipt execution manifest mode is invalid: {mode!r}"
        )
        return problems
    modules = payload.get("modules")
    if not isinstance(modules, Mapping) or set(modules) != labels:
        problems.append(
            "acceptance receipt execution manifest modules differ from mode: "
            f"expected={sorted(labels)!r}"
        )
        return problems
    entry = payload.get("entry")
    entry_label = entry.get("module") if isinstance(entry, Mapping) else None
    expected_entry = "linked" if mode == "linked" else "app"
    if entry_label != expected_entry:
        problems.append(
            "acceptance receipt execution manifest entry module is invalid: "
            f"expected={expected_entry!r}, got={entry_label!r}"
        )
    manifest_root = manifest_path.parent.resolve()
    manifest_relative = _portable_path(manifest_item.get("path"))
    if manifest_relative is None:
        return problems
    resolved_modules: dict[str, Path] = {}
    module_paths: list[Path] = []
    portable_module_paths: list[str] = []
    for label in sorted(labels):
        descriptor = modules.get(label)
        field = f"acceptance receipt execution manifest modules.{label}"
        if not isinstance(descriptor, Mapping):
            problems.append(f"{field} must be an object")
            continue
        raw_path = descriptor.get("path")
        size = descriptor.get("size")
        digest = descriptor.get("sha256")
        relative = _portable_path(raw_path)
        if relative is None:
            problems.append(f"{field}.path must be a portable relative POSIX path")
            continue
        portable_module_paths.append(portable_path_identity(relative.as_posix()))
        destination = manifest_relative.parent / relative
        collision = _register_closure_path(
            closure_paths,
            destination,
            field=field,
            allow_exact_alias=True,
        )
        if collision is not None:
            problems.append(collision)
        try:
            module_path = manifest_root.joinpath(*relative.parts).resolve(strict=True)
        except (OSError, ValueError):
            problems.append(f"{field} artifact does not exist")
            continue
        if not module_path.is_relative_to(manifest_root):
            problems.append(f"{field}.path escapes the execution manifest directory")
            continue
        resolved_modules[label] = module_path
        module_paths.append(module_path)
        if not module_path.is_file():
            problems.append(f"{field} artifact does not exist: {module_path}")
            continue
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            problems.append(f"{field}.size must be a non-negative integer")
        elif module_path.stat().st_size != size:
            problems.append(f"{field} artifact size mismatch: {module_path}")
        if not _sha256(digest):
            problems.append(f"{field}.sha256 must be lowercase SHA-256")
        elif (
            stable_file_sha256(
                module_path,
                label="Pact witness module artifact",
            )
            != digest
        ):
            problems.append(f"{field} artifact checksum mismatch: {module_path}")
    if len(module_paths) != len(set(module_paths)) or len(portable_module_paths) != len(
        set(portable_module_paths)
    ):
        problems.append("acceptance receipt execution manifest module paths duplicate")
    entry_path = resolved_modules.get(expected_entry)
    if entry_path is not None and target_path != entry_path:
        problems.append(
            "acceptance receipt target_artifact differs from execution manifest entry"
        )
    return problems


def acceptance_coordinate(payload: Mapping[str, Any]) -> tuple[str, str, str] | None:
    variant = payload.get("variant")
    if not isinstance(variant, Mapping):
        return None
    values = tuple(
        variant.get(field) for field in ("cpython", "abi_tier", "target_triple")
    )
    return values if all(isinstance(value, str) and value for value in values) else None


def validate_acceptance_receipt(
    payload: object,
    *,
    receipt_path: Path | None = None,
    expected: Mapping[str, Any] | None = None,
    require_artifacts: bool = True,
) -> tuple[str, ...]:
    """Return every violation of the exact portable acceptance contract."""

    expected_root_keys = {
        "schema_version",
        "kind",
        "status",
        "target",
        "variant",
        "packages",
        "git",
        "artifacts",
        "parity_gate",
        "iteration_mode",
    }
    if not isinstance(payload, Mapping):
        return ("acceptance receipt root must be an object",)
    problems: list[str] = []
    if set(payload) != expected_root_keys:
        problems.append(
            "acceptance receipt keys differ from schema: "
            f"missing={sorted(expected_root_keys - set(payload))!r}, "
            f"unknown={sorted(set(payload) - expected_root_keys)!r}"
        )
    if payload.get("schema_version") != SCHEMA_VERSION:
        problems.append(f"acceptance receipt schema_version must be {SCHEMA_VERSION}")
    if payload.get("kind") != KIND:
        problems.append(f"acceptance receipt kind must be {KIND!r}")
    if payload.get("status") != STATUS_PASS:
        problems.append("acceptance receipt status must be PASS")
    target = payload.get("target")
    if target not in TARGETS:
        problems.append("acceptance receipt target must be native or wasm")
    if payload.get("iteration_mode") is not False:
        problems.append("acceptance receipt iteration_mode must be false")

    receipt_root = _receipt_root(receipt_path) if receipt_path is not None else None
    closure_paths: dict[str, tuple[PurePosixPath, str]] = {}
    reserved_collision = _register_closure_path(
        closure_paths,
        PurePosixPath("acceptance-receipt.json"),
        field="acceptance receipt",
    )
    assert reserved_collision is None
    if require_artifacts and receipt_root is None:
        problems.append(
            "acceptance receipt_path is required to validate portable artifacts"
        )

    variant = payload.get("variant")
    variant_keys = {"cpython", "abi_tier", "target_triple"}
    if not isinstance(variant, Mapping) or set(variant) != variant_keys:
        problems.append("acceptance receipt variant schema is invalid")
    elif not all(
        isinstance(variant.get(field), str) and variant.get(field)
        for field in variant_keys
    ):
        problems.append("acceptance receipt variant values must be non-empty strings")

    packages = payload.get("packages")
    package_keys = {
        "version",
        "module_set",
        "seal_sha256",
        "identity_sha256",
    }
    if not isinstance(packages, Mapping) or set(packages) != PACKAGE_NAMES:
        problems.append(
            "acceptance receipt packages must contain exactly numpy and scipy"
        )
    else:
        for package in sorted(PACKAGE_NAMES):
            item = packages.get(package)
            field = f"acceptance receipt packages.{package}"
            if not isinstance(item, Mapping) or set(item) != package_keys:
                problems.append(f"{field} schema is invalid")
                continue
            if not all(
                isinstance(item.get(name), str) and item.get(name)
                for name in ("version", "module_set")
            ):
                problems.append(f"{field} version/module_set is invalid")
            for digest_name in ("seal_sha256", "identity_sha256"):
                if not _sha256(item.get(digest_name)):
                    problems.append(f"{field}.{digest_name} must be lowercase SHA-256")

    git = payload.get("git")
    if not isinstance(git, Mapping) or set(git) != {"source_sha"}:
        problems.append("acceptance receipt git must contain exactly source_sha")
    elif not is_git_object_id(git.get("source_sha")):
        problems.append(
            "acceptance receipt git.source_sha must be lowercase 40- or 64-hex"
        )

    artifacts = payload.get("artifacts")
    expected_roles = set(_COMMON_ARTIFACT_ROLES)
    if target == "wasm":
        expected_roles.add("execution_manifest")
    if not isinstance(artifacts, list):
        problems.append("acceptance receipt artifacts must be a list")
    else:
        roles: list[object] = []
        paths: list[object] = []
        artifacts_by_role: dict[str, Mapping[str, Any]] = {}
        for index, item in enumerate(artifacts):
            field = f"acceptance receipt artifacts[{index}]"
            if not isinstance(item, Mapping) or set(item) != {
                "role",
                "path",
                "sha256",
                "size",
            }:
                problems.append(f"{field} schema is invalid")
                continue
            role = item.get("role")
            roles.append(role)
            raw_path = item.get("path")
            paths.append(raw_path)
            relative = _portable_path(raw_path)
            if relative is not None:
                collision = _register_closure_path(
                    closure_paths,
                    relative,
                    field=field,
                )
                if collision is not None:
                    problems.append(collision)
            if isinstance(role, str) and role not in artifacts_by_role:
                artifacts_by_role[role] = item
            problems.extend(
                _validate_hashed_file(
                    {
                        "path": item.get("path"),
                        "sha256": item.get("sha256"),
                        "size": item.get("size"),
                    },
                    field=field,
                    receipt_root=receipt_root,
                    require_artifacts=require_artifacts,
                )
            )
        string_roles = [role for role in roles if isinstance(role, str)]
        if len(string_roles) != len(set(string_roles)):
            problems.append("acceptance receipt artifact roles must not duplicate")
        string_paths = [path for path in paths if isinstance(path, str)]
        portable_paths = [
            portable_path_identity(path)
            for path in string_paths
            if _portable_path(path) is not None
        ]
        if len(portable_paths) != len(set(portable_paths)):
            problems.append("acceptance receipt artifact paths must not duplicate")
        if roles != sorted(expected_roles):
            problems.append(
                "acceptance receipt artifact roles must be sorted and exact: "
                f"expected={sorted(expected_roles)!r}, got={roles!r}"
            )
        if target == "wasm" and require_artifacts and receipt_root is not None:
            manifest_item = artifacts_by_role.get("execution_manifest")
            target_item = artifacts_by_role.get("target_artifact")
            if manifest_item is not None and target_item is not None:
                problems.extend(
                    _validate_wasm_execution_manifest(
                        manifest_item,
                        target_item,
                        receipt_root=receipt_root,
                        closure_paths=closure_paths,
                    )
                )

    parity_gate = payload.get("parity_gate")
    problems.extend(
        _validate_hashed_file(
            parity_gate,
            field="acceptance receipt parity_gate",
            receipt_root=receipt_root,
            require_artifacts=require_artifacts,
        )
    )
    if isinstance(parity_gate, Mapping):
        parity_relative = _portable_path(parity_gate.get("path"))
        if parity_relative is not None:
            collision = _register_closure_path(
                closure_paths,
                parity_relative,
                field="acceptance receipt parity_gate",
            )
            if collision is not None:
                problems.append(collision)

    if expected is not None:
        if target != expected.get("target"):
            problems.append(
                "acceptance receipt target differs from shared registry coordinate"
            )
        expected_variant = expected.get("variant")
        if variant != expected_variant:
            problems.append(
                "acceptance receipt variant differs from shared registry coordinate"
            )
        expected_packages = expected.get("packages")
        if isinstance(packages, Mapping) and isinstance(expected_packages, Mapping):
            for package in sorted(PACKAGE_NAMES):
                actual_item = packages.get(package)
                expected_item = expected_packages.get(package)
                if not isinstance(actual_item, Mapping) or not isinstance(
                    expected_item, Mapping
                ):
                    problems.append(
                        f"acceptance receipt packages.{package} differs from registry"
                    )
                    continue
                for field in ("version", "module_set", "identity_sha256"):
                    if actual_item.get(field) != expected_item.get(field):
                        problems.append(
                            f"acceptance receipt packages.{package}.{field} "
                            "differs from shared registry coordinate"
                        )
    return tuple(problems)
