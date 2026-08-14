"""Canonical target/content identity for source-extension package seals."""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Mapping
from pathlib import Path
from typing import Any, cast

from molt.cli.source_extension_manifest_codec import (
    _manifest_dependencies,
    _manifest_sequence,
    _validate_compact_source_extension_manifest,
)
from molt.cli.source_extension_reproducibility import _require_location_neutral
from molt.cli.source_extension_set_registry import (
    validate_source_extension_module_target,
)
from molt.cli.source_extension_target import (
    source_extension_artifact_kind,
    source_extension_artifact_suffix,
)
from molt.target_python import _parse_target_python_version

_OBJECT_SEQUENCE_FIELDS = (
    "defined_symbols",
    "undefined_symbols",
    "required_c_api_symbols",
    "required_capsules",
    "project_generated_c_api_symbols",
)
SOURCE_EXTENSION_SET_SCHEMA_VERSION = 4


def _digest_payload(payload: Any) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def _sha256_file(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _extension_content_projection(manifest: Mapping[str, Any]) -> dict[str, Any]:
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if not isinstance(objects, list) or not objects:
        raise ValueError("extension identity requires a non-empty object closure")
    projected_objects: list[dict[str, Any]] = []
    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise ValueError(f"extension identity object[{index}] is invalid")
        item = cast(Mapping[str, Any], item)
        projected = {
            key: item.get(key)
            for key in ("source", "object", "source_sha256", "object_sha256")
            if key in item
        }
        projected["compile_command"] = _manifest_sequence(
            manifest, item, "compile_command"
        )
        projected["symbol_command"] = _manifest_sequence(
            manifest, item, "symbol_command"
        )
        projected["dependencies"] = _manifest_dependencies(manifest, item)
        for field in _OBJECT_SEQUENCE_FIELDS:
            if field in item or f"{field}_ref" in item:
                projected[field] = _manifest_sequence(manifest, item, field)
        projected_objects.append(projected)
    source_plan = manifest.get("source_plan")
    closure = cast(Mapping[str, Any], closure)
    projection = {
        key: manifest.get(key)
        for key in (
            "schema_version",
            "module",
            "name",
            "version",
            "python_tag",
            "target_python",
            "abi_tag",
            "abi_tier",
            "molt_c_api_version",
            "target_triple",
            "artifact_kind",
            "loader_kind",
            "runtime_linkage",
            "deterministic",
            "extension_sha256",
            "wheel_sha256",
            "init_symbol",
            "capabilities",
            "capability_profiles",
            "python_exports",
            "provided_capsules",
            "runtime_python_import_modules",
            "effects",
            "link_requirements",
        )
        if key in manifest
    }
    projection["source_plan"] = {
        key: source_plan.get(key)
        for key in (
            "kind",
            "target_id",
            "target_name",
            "target_selector",
            "target_type",
        )
        if isinstance(source_plan, Mapping) and key in source_plan
    }
    projection["object_closure"] = {
        "schema_version": closure.get("schema_version"),
        "root_symbol": closure.get("root_symbol"),
        "init_symbol_owner": closure.get("init_symbol_owner"),
        "runtime_symbols": closure.get("runtime_symbols"),
        "defined_symbols": closure.get("defined_symbols"),
        "undefined_symbols": closure.get("undefined_symbols"),
        "required_c_api_symbols": closure.get("required_c_api_symbols"),
        "required_capsules": closure.get("required_capsules"),
        "project_generated_c_api_symbols": closure.get(
            "project_generated_c_api_symbols"
        ),
        "project_generated_c_api_prefixes": closure.get(
            "project_generated_c_api_prefixes"
        ),
        "objects": projected_objects,
    }
    return projection


def _target_semantic_projection(set_manifest: Mapping[str, Any]) -> dict[str, Any]:
    if set_manifest.get("schema_version") != SOURCE_EXTENSION_SET_SCHEMA_VERSION:
        raise ValueError(
            "extension-set identity requires schema_version "
            f"{SOURCE_EXTENSION_SET_SCHEMA_VERSION}"
        )
    if set_manifest.get("kind") != "molt-source-extension-set":
        raise ValueError(
            "extension-set identity requires kind 'molt-source-extension-set'"
        )
    cpython = set_manifest.get("cpython")
    if not isinstance(cpython, str) or not cpython:
        raise ValueError("extension-set identity requires CPython version custody")
    try:
        _parse_target_python_version(cpython)
    except ValueError as exc:
        raise ValueError(
            f"extension-set identity has invalid CPython version {cpython!r}: {exc}"
        ) from exc
    extensions = set_manifest.get("extensions")
    if not isinstance(extensions, list) or not extensions:
        raise ValueError("extension-set identity requires typed extensions")
    target_metadata = set_manifest.get("target_metadata")
    abi = target_metadata.get("abi") if isinstance(target_metadata, Mapping) else None
    abi_tier = set_manifest.get("abi_tier")
    if not isinstance(abi_tier, str) or not abi_tier or not isinstance(abi, Mapping):
        raise ValueError("extension-set identity requires ABI target metadata")
    if abi.get("tier") != abi_tier:
        raise ValueError("extension-set identity ABI tier differs from target metadata")
    projection = {
        key: set_manifest.get(key)
        for key in (
            "schema_version",
            "kind",
            "package",
            "package_version",
            "name",
            "seal_name",
            "cpython",
            "source_head",
            "submodules",
            "target_triple",
            "abi_tier",
        )
    }
    projection["extensions"] = [
        {
            key: item.get(key)
            for key in (
                "module",
                "target",
                "python_exports",
                "capabilities",
                "provided_capsules",
                "exclude_linked_static_libraries",
            )
        }
        for item in extensions
        if isinstance(item, Mapping)
    ]
    projection["abi"] = {
        "tier": abi.get("tier") if isinstance(abi, Mapping) else None,
        "python_header_sha256": (
            abi.get("python_header_sha256") if isinstance(abi, Mapping) else None
        ),
        "include_surface": (
            abi.get("include_surface") if isinstance(abi, Mapping) else None
        ),
    }
    return projection


def _source_extension_set_identity(
    payload_root: Path,
    *,
    inventory_sha256: Mapping[str, str],
    set_manifest: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Project verified inventory into semantic identity and full attestation."""

    root = payload_root.resolve()
    if set_manifest is None:
        path = root / "extension_set_manifest.json"
        try:
            loaded = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"cannot read extension-set manifest {path}: {exc}"
            ) from exc
        if not isinstance(loaded, Mapping):
            raise ValueError(f"extension-set manifest is not an object: {path}")
        set_manifest = loaded
    target_semantics = _target_semantic_projection(set_manifest)
    extensions = set_manifest.get("extensions")
    if not isinstance(extensions, list) or not extensions:
        raise ValueError("extension-set identity requires extension sidecars")
    if not all(isinstance(item, Mapping) for item in extensions):
        raise ValueError("extension-set identity has invalid extension entries")
    extension_keys = [
        validate_source_extension_module_target(item.get("module"), item.get("target"))
        for item in extensions
    ]
    if len(set(extension_keys)) != len(extension_keys):
        raise ValueError("extension-set identity has duplicate extension keys")
    target_triple = set_manifest.get("target_triple")
    if not isinstance(target_triple, str) or not target_triple:
        raise ValueError("extension-set identity requires a target triple")
    artifact_suffix = source_extension_artifact_suffix(target_triple)
    abi_tier = set_manifest.get("abi_tier")
    cpython = set_manifest.get("cpython")
    package_version = set_manifest.get("package_version")
    assert isinstance(abi_tier, str)
    assert isinstance(cpython, str)
    if not isinstance(package_version, str) or not package_version:
        raise ValueError("extension-set identity requires package version custody")
    target_python = _parse_target_python_version(cpython)
    artifact_paths = [
        root.joinpath(
            *str(module).split(".")[:-1],
            f"{target}{artifact_suffix}",
        )
        for module, target in extension_keys
    ]
    sidecar_paths = [
        artifact.with_name(f"{artifact.name}.extension_manifest.json")
        for artifact in artifact_paths
    ]
    expected_artifacts = {path.relative_to(root).as_posix() for path in artifact_paths}
    inventoried_artifacts = {
        path for path in inventory_sha256 if path.endswith((".molt.wasm", ".molt.a"))
    }
    if inventoried_artifacts != expected_artifacts:
        raise ValueError(
            "extension-set identity artifact inventory differs from typed set"
        )
    expected_sidecars = {path.relative_to(root).as_posix() for path in sidecar_paths}
    inventoried_sidecars = {
        path
        for path in inventory_sha256
        if path.endswith(
            (
                ".molt.wasm.extension_manifest.json",
                ".molt.a.extension_manifest.json",
            )
        )
    }
    if inventoried_sidecars != expected_sidecars:
        raise ValueError(
            "extension-set identity sidecar inventory differs from typed set"
        )
    extension_content: list[dict[str, Any]] = []
    producer_sidecars: list[dict[str, Any]] = []
    for set_entry, (module, target), artifact_path, path in zip(
        extensions,
        extension_keys,
        artifact_paths,
        sidecar_paths,
        strict=True,
    ):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise ValueError(f"cannot read extension sidecar {path}: {exc}") from exc
        if not isinstance(payload, Mapping):
            raise ValueError(f"extension sidecar is not an object: {path}")
        try:
            _validate_compact_source_extension_manifest(payload)
        except ValueError as exc:
            raise ValueError(f"extension sidecar is invalid: {path}: {exc}") from exc
        source_plan = payload.get("source_plan")
        expected_contract = {
            "module": module,
            "version": package_version,
            "target_triple": target_triple,
            "abi_tier": abi_tier,
            "target_python": target_python.tag,
            "artifact_kind": source_extension_artifact_kind(target_triple),
            "python_exports": set_entry.get("python_exports"),
            "capabilities": set_entry.get("capabilities"),
            "provided_capsules": set_entry.get("provided_capsules"),
        }
        mismatches = [
            f"{field}: expected {expected!r}, got {payload.get(field)!r}"
            for field, expected in expected_contract.items()
            if payload.get(field) != expected
        ]
        actual_target = (
            source_plan.get("target_selector")
            if isinstance(source_plan, Mapping)
            else None
        )
        if actual_target != target:
            mismatches.append(
                "source_plan.target_selector: "
                f"expected {target!r}, got {actual_target!r}"
            )
        if mismatches:
            raise ValueError(
                "extension sidecar differs from set variant contract: "
                + "; ".join(mismatches)
            )
        artifact_relative = artifact_path.relative_to(root).as_posix()
        artifact_sha256 = inventory_sha256.get(artifact_relative)
        if (
            not artifact_path.is_file()
            or not isinstance(artifact_sha256, str)
            or _sha256_file(artifact_path) != artifact_sha256
            or payload.get("extension_sha256") != artifact_sha256
        ):
            raise ValueError(
                "extension-set artifact bytes differ from sidecar and inventory: "
                f"{artifact_relative}"
            )
        relative = path.relative_to(root).as_posix()
        extension_content.append(
            {"path": relative, "identity": _extension_content_projection(payload)}
        )
        producer_sidecars.append({"path": relative, "manifest": dict(payload)})
    installed = set_manifest.get("installed_package_files")
    if not isinstance(installed, list) or not all(
        isinstance(value, str) and value for value in installed
    ):
        raise ValueError("extension-set installed package inventory is invalid")
    if installed != sorted(set(installed)):
        raise ValueError("extension-set installed package inventory is not canonical")
    installed_content = []
    for relative in installed:
        path = root / relative
        sha256 = inventory_sha256.get(relative)
        if (
            not path.is_file()
            or not path.resolve().is_relative_to(root)
            or not isinstance(sha256, str)
        ):
            raise ValueError(f"extension-set installed content is missing: {relative}")
        installed_content.append({"path": relative, "sha256": sha256})
    content = {"installed": installed_content, "extensions": extension_content}
    _require_location_neutral(target_semantics, authority="target semantic identity")
    _require_location_neutral(content, authority="extension content identity")
    target_semantic_sha256 = _digest_payload(target_semantics)
    content_sha256 = _digest_payload(content)
    producer_inventory = [
        {"path": path, "sha256": sha256}
        for path, sha256 in sorted(inventory_sha256.items())
    ]
    producer_attestation = {
        "set_manifest": dict(set_manifest),
        "sidecars": producer_sidecars,
        "inventory": producer_inventory,
    }
    identity_payload = {
        "schema_version": 1,
        "target_semantic_sha256": target_semantic_sha256,
        "content_sha256": content_sha256,
    }
    return identity_payload | {
        "canonical_sha256": _digest_payload(identity_payload),
        "producer_attestation_sha256": _digest_payload(producer_attestation),
    }


def _require_expected_source_extension_set_identity(
    payload_root: Path,
    expected_sha256: str,
    *,
    inventory_sha256: Mapping[str, str],
) -> dict[str, Any]:
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise ValueError("expected source-extension identity must be lowercase SHA-256")
    identity = _source_extension_set_identity(
        payload_root,
        inventory_sha256=inventory_sha256,
    )
    if identity["canonical_sha256"] != expected_sha256:
        raise ValueError(
            "source-extension canonical identity mismatch: "
            f"expected {expected_sha256}, got {identity['canonical_sha256']}"
        )
    return identity


def _source_extension_reproduction_comparison(
    *,
    expected_incumbent_sha256: str,
    expected_candidate_sha256: str,
    incumbent_seal_sha256: str,
    incumbent_identity: Mapping[str, Any],
    candidate_seal_sha256: str,
    candidate_identity: Mapping[str, Any],
) -> dict[str, Any]:
    if incumbent_identity.get("canonical_sha256") != expected_incumbent_sha256:
        raise ValueError(
            "incumbent changed after expected-identity verification; publication "
            "is not authorized"
        )
    return {
        "schema_version": 1,
        "kind": "source-extension-identity-reproduction",
        "expected_incumbent_identity_sha256": expected_incumbent_sha256,
        "expected_candidate_identity_sha256": expected_candidate_sha256,
        "incumbent_seal_sha256": incumbent_seal_sha256,
        "incumbent_identity": dict(incumbent_identity),
        "candidate_seal_sha256": candidate_seal_sha256,
        "candidate_identity": dict(candidate_identity),
        "reproduced": (
            candidate_identity.get("canonical_sha256") == expected_candidate_sha256
        ),
    }
