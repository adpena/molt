"""Canonical target/content identity for source-extension package seals."""

from __future__ import annotations

import hashlib
import json
import keyword
import re
from collections.abc import Mapping
from pathlib import Path
from typing import Any, cast

from molt.cli.source_extension_manifest_codec import (
    _manifest_dependencies,
    _manifest_sequence,
)
from molt.cli.source_extension_reproducibility import _require_location_neutral

_OBJECT_SEQUENCE_FIELDS = (
    "defined_symbols",
    "undefined_symbols",
    "required_c_api_symbols",
    "required_capsules",
    "project_generated_c_api_symbols",
)
_WINDOWS_RESERVED_FILENAME = re.compile(
    r"(?i)\A(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?\Z"
)


def _require_safe_extension_key(module: object, target: object) -> tuple[str, str]:
    if (
        not isinstance(module, str)
        or not module
        or not all(
            part.isidentifier() and not keyword.iskeyword(part)
            for part in module.split(".")
        )
    ):
        raise ValueError(f"extension-set module is not import syntax: {module!r}")
    if (
        not isinstance(target, str)
        or not target
        or target in {".", ".."}
        or any(ord(character) < 32 for character in target)
        or any(character in target for character in '<>:"/\\|?*')
        or target.startswith(("~", "$", "%"))
        or target.endswith((".", " "))
        or _WINDOWS_RESERVED_FILENAME.fullmatch(target) is not None
    ):
        raise ValueError(f"extension-set target is not a safe filename: {target!r}")
    return module, target


def _digest_payload(payload: Any) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


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
            "abi_tag",
            "abi_tier",
            "molt_c_api_version",
            "target_triple",
            "artifact_kind",
            "loader_kind",
            "runtime_linkage",
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
    extensions = set_manifest.get("extensions")
    if not isinstance(extensions, list) or not extensions:
        raise ValueError("extension-set identity requires typed extensions")
    target_metadata = set_manifest.get("target_metadata")
    abi = target_metadata.get("abi") if isinstance(target_metadata, Mapping) else None
    projection = {
        key: set_manifest.get(key)
        for key in (
            "kind",
            "package",
            "name",
            "seal_name",
            "source_head",
            "submodules",
            "target",
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
        _require_safe_extension_key(item.get("module"), item.get("target"))
        for item in extensions
    ]
    if len(set(extension_keys)) != len(extension_keys):
        raise ValueError("extension-set identity has duplicate extension keys")
    target_triple = set_manifest.get("target_triple")
    if not isinstance(target_triple, str) or not target_triple:
        raise ValueError("extension-set identity requires a target triple")
    artifact_suffix = (
        ".molt.wasm" if target_triple.lower().startswith("wasm32") else ".molt.a"
    )
    sidecar_paths = [
        root.joinpath(
            *str(module).split(".")[:-1],
            f"{target}{artifact_suffix}.extension_manifest.json",
        )
        for module, target in extension_keys
    ]
    expected_sidecars = {path.relative_to(root).as_posix() for path in sidecar_paths}
    inventoried_sidecars = {
        path
        for path in inventory_sha256
        if path.endswith(f"{artifact_suffix}.extension_manifest.json")
    }
    if inventoried_sidecars != expected_sidecars:
        raise ValueError(
            "extension-set identity sidecar inventory differs from typed set"
        )
    extension_content: list[dict[str, Any]] = []
    producer_sidecars: list[dict[str, Any]] = []
    for path in sidecar_paths:
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise ValueError(f"cannot read extension sidecar {path}: {exc}") from exc
        if not isinstance(payload, Mapping):
            raise ValueError(f"extension sidecar is not an object: {path}")
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
