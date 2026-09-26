"""Canonical target/content identity for source-extension package seals."""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from molt.cli.extension_manifest import (
    _EXTENSION_SUPPORT_FILE_SUFFIXES,
    _manifest_callable_exports,
)
from molt.cli.models import _ExternalNativeCallableExport
from molt.cli.source_extension_reproducibility import _require_location_neutral
from molt.target_python import _parse_target_python_version
from molt.cli.source_package_seal import (
    SealFileInventoryEntry,
    validate_source_package_relative_path,
)
from molt.exact_json import loads_exact

# Version 6 binds the locked Ninja distribution to the backend identity and
# records the binary's self-report separately. Version-5 seals remain evidence,
# but are not interpreted under this changed identity projection.
SOURCE_EXTENSION_SET_SCHEMA_VERSION = 6


def _digest_payload(payload: Any) -> str:
    return hashlib.sha256(
        json.dumps(
            payload, sort_keys=True, separators=(",", ":"), allow_nan=False
        ).encode("utf-8")
    ).hexdigest()


@dataclass(frozen=True, slots=True)
class SealedSourceExtensionSupportFile:
    """One exact inventory member, never a producer-side source remapping."""

    rel_path: str
    sha256: str

    def digest_payload(self) -> dict[str, str]:
        return {"path": self.rel_path, "sha256": self.sha256}


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionExecutionMetadata:
    callable_exports: tuple[_ExternalNativeCallableExport, ...]
    support_files: tuple[SealedSourceExtensionSupportFile, ...]

    @property
    def direct_function_exports(self) -> tuple[str, ...]:
        return tuple(
            sorted(
                {
                    export.symbol
                    for export in self.callable_exports
                    if export.binding == "direct_symbol" and export.symbol is not None
                }
            )
        )


def validate_source_extension_execution_metadata(
    manifest: Mapping[str, Any],
    *,
    inventory_sha256: Mapping[str, str],
) -> ValidatedSourceExtensionExecutionMetadata:
    """Bind execution metadata to exact verified payload-inventory facts.

    Producer support APIs may map a source into a different destination. A
    sealed manifest instead contains only the emitted destination and digest;
    both its source and destination are that same inventoried file. No external
    source path is consulted, and no independently hashed bytes can substitute
    for the bytes owned by the seal.
    """

    module = manifest.get("module")
    if not isinstance(module, str) or not module:
        raise ValueError("extension identity requires a module name")
    errors: list[str] = []
    callable_exports = _manifest_callable_exports(
        manifest,
        package=module.split(".", 1)[0],
        errors=errors,
    )
    if errors:
        raise ValueError(
            "extension identity has invalid execution metadata: " + "; ".join(errors)
        )
    raw_support_files = manifest.get("support_files", [])
    if not isinstance(raw_support_files, list):
        raise ValueError("sealed support_files must be a list of path/sha256 objects")
    support_files: list[SealedSourceExtensionSupportFile] = []
    seen: set[str] = set()
    for index, item in enumerate(raw_support_files):
        label = f"sealed support_files[{index}]"
        if not isinstance(item, Mapping) or set(item) != {"path", "sha256"}:
            raise ValueError(
                f"{label} must contain exactly path and sha256; "
                "producer source remapping is not sealed support authority"
            )
        relative = validate_source_package_relative_path(item.get("path"), field=label)
        if not relative.endswith(_EXTENSION_SUPPORT_FILE_SUFFIXES):
            raise ValueError(f"{label} must name a supported artifact or Python source")
        sha256 = item.get("sha256")
        if not isinstance(sha256, str) or re.fullmatch(r"[0-9a-f]{64}", sha256) is None:
            raise ValueError(f"{label}.sha256 must be a canonical lowercase SHA-256")
        if relative in seen:
            raise ValueError(f"{label} duplicates support file {relative!r}")
        if inventory_sha256.get(relative) != sha256:
            raise ValueError(
                f"{label} source/destination bytes differ from the sealed inventory: {relative}"
            )
        seen.add(relative)
        support_files.append(SealedSourceExtensionSupportFile(relative, sha256))
    return ValidatedSourceExtensionExecutionMetadata(
        callable_exports=callable_exports,
        support_files=tuple(sorted(support_files, key=lambda entry: entry.rel_path)),
    )


def _extension_content_projection(
    manifest: Mapping[str, Any],
    *,
    validated_closure: tuple[dict[str, Any], str],
    execution_metadata: ValidatedSourceExtensionExecutionMetadata,
) -> dict[str, Any]:
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, Mapping) else None
    if not isinstance(objects, list) or not objects:
        raise ValueError("extension identity requires a non-empty object closure")

    source_plan = manifest.get("source_plan")
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
            "platform_tag",
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
            "runtime_python_imports",
            "runtime_python_import_modules",
            "effects",
            "link_requirements",
        )
        if key in manifest
    }
    projection["callable_exports"] = [
        export.digest_payload() for export in execution_metadata.callable_exports
    ]
    projection["support_files"] = [
        support_file.digest_payload()
        for support_file in execution_metadata.support_files
    ]
    projection["source_plan"] = {
        key: source_plan.get(key)
        for key in (
            "kind",
            "target_id",
            "target_name",
            "target_selector",
            "target_type",
            "producer_link_args",
        )
        if isinstance(source_plan, Mapping) and key in source_plan
    }
    projection["object_closure"] = validated_closure[0]
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


@dataclass(frozen=True, slots=True)
class SourceExtensionSetIdentity:
    """Location-neutral identity; all receipt fields are immutable scalars."""

    target_semantic_sha256: str
    content_sha256: str
    canonical_sha256: str
    producer_attestation_sha256: str

    def digest_payload(self) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "target_semantic_sha256": self.target_semantic_sha256,
            "content_sha256": self.content_sha256,
            "canonical_sha256": self.canonical_sha256,
            "producer_attestation_sha256": self.producer_attestation_sha256,
        }


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionIdentitySidecar:
    """One validated sidecar snapshot, independent of its filesystem location."""

    module: str
    target: str
    artifact_relative_path: str
    sidecar_relative_path: str
    artifact_sha256: str
    sidecar_sha256: str
    manifest_json: bytes
    content_json: bytes


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionSetIdentityInputs:
    """Only the structural validator produces these immutable identity inputs."""

    set_manifest_json: bytes
    target_semantics_json: bytes
    installed_package_files: tuple[str, ...]
    sidecars: tuple[ValidatedSourceExtensionIdentitySidecar, ...]
    inventory: tuple[SealFileInventoryEntry, ...]


def _source_extension_set_identity(
    inputs: ValidatedSourceExtensionSetIdentityInputs,
) -> SourceExtensionSetIdentity:
    """Pure projection of validated facts: no filesystem reads or trust switch."""

    inventory_sha256 = {entry.relative_path: entry.sha256 for entry in inputs.inventory}
    extension_content = [
        {
            "path": sidecar.sidecar_relative_path,
            "identity": loads_exact(sidecar.content_json.decode("utf-8")),
        }
        for sidecar in inputs.sidecars
    ]
    installed_content = [
        {"path": relative, "sha256": inventory_sha256[relative]}
        for relative in inputs.installed_package_files
    ]
    content = {"installed": installed_content, "extensions": extension_content}
    target_semantics = loads_exact(inputs.target_semantics_json.decode("utf-8"))
    _require_location_neutral(target_semantics, authority="target semantic identity")
    _require_location_neutral(content, authority="extension content identity")
    target_semantic_sha256 = _digest_payload(target_semantics)
    content_sha256 = _digest_payload(content)
    producer_attestation = {
        "set_manifest": loads_exact(inputs.set_manifest_json.decode("utf-8")),
        "sidecars": [
            {
                "path": sidecar.sidecar_relative_path,
                "manifest": loads_exact(sidecar.manifest_json.decode("utf-8")),
            }
            for sidecar in inputs.sidecars
        ],
        "inventory": [
            {"path": path, "sha256": sha256}
            for path, sha256 in sorted(inventory_sha256.items())
        ],
    }
    identity_payload = {
        "schema_version": 1,
        "target_semantic_sha256": target_semantic_sha256,
        "content_sha256": content_sha256,
    }
    return SourceExtensionSetIdentity(
        target_semantic_sha256=target_semantic_sha256,
        content_sha256=content_sha256,
        canonical_sha256=_digest_payload(identity_payload),
        producer_attestation_sha256=_digest_payload(producer_attestation),
    )


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
