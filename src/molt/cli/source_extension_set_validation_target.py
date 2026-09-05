"""Host-independent target facts and canonical compiler-command custody."""

from __future__ import annotations
import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping
from molt.cli.source_extension_set_registry import SourceExtensionVariant
from molt.cli.source_extension_set_validation_schema import (
    SourceExtensionSetValidationError,
)
from molt.cli.source_extension_target import (
    SourceExtensionTargetPlan,
    source_extension_recorded_target_plan,
    source_extension_target_is_wasm,
)
from molt.exact_json import loads_exact
from molt.file_hashing import _sha256_file


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionTarget:
    plan: SourceExtensionTargetPlan
    commands: tuple[tuple[str, tuple[str, ...]], ...]


def _source_extension_tool_role_contract(
    target_triple: str,
) -> tuple[dict[str, str], frozenset[str]]:
    identity_role_by_command = {
        "ar": "ar",
        "c": "cc",
        "cpp": "cxx",
        "ld": "wasm_ld",
        "nm": "nm",
        "ranlib": "ranlib",
        "strip": "strip",
    }
    required_commands = {"ar", "c", "nm"}
    if source_extension_target_is_wasm(target_triple):
        required_commands.add("ld")
    return identity_role_by_command, frozenset(required_commands)


def _validated_command(value: object, *, role: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise SourceExtensionSetValidationError(
            f"extension-set target metadata {role} command must be a non-empty list"
        )
    arguments: list[str] = []
    for argument in value:
        if not isinstance(argument, str):
            raise SourceExtensionSetValidationError(
                f"extension-set target metadata {role} command arguments must be strings"
            )
        arguments.append(argument)
    if not arguments[0]:
        raise SourceExtensionSetValidationError(
            f"extension-set target metadata {role} command requires an executable"
        )
    return tuple(arguments)


def validate_source_extension_target_metadata(
    *,
    publish_root: Path,
    set_manifest: Mapping[str, Any],
    variant: SourceExtensionVariant,
) -> ValidatedSourceExtensionTarget:
    target_metadata = set_manifest.get("target_metadata")
    if not isinstance(target_metadata, Mapping):
        raise SourceExtensionSetValidationError(
            "extension-set manifest target_metadata is missing"
        )
    expected_target_metadata_keys = {
        "schema_version",
        "kind",
        "target_triple",
        "target",
        "python",
        "abi",
        "toolchain",
        "meson_cross_properties",
        "paths",
        "env",
        "digests",
        "digest",
    }
    if set(target_metadata) != expected_target_metadata_keys:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata keys differ from schema"
        )
    metadata_contract = (
        target_metadata.get("schema_version"),
        target_metadata.get("kind"),
        target_metadata.get("target_triple"),
    )
    expected_metadata_contract = (
        3,
        "molt-source-extension-target-metadata",
        variant.target_triple,
    )
    if (
        type(target_metadata.get("schema_version")) is not int
        or metadata_contract != expected_metadata_contract
    ):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata contract differs from selected variant: "
            f"expected {expected_metadata_contract!r}, got {metadata_contract!r}"
        )
    expected_python = {
        "implementation": "cpython",
        "version": variant.cpython,
    }
    if target_metadata.get("python") != expected_python:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata Python authority differs from selected "
            f"variant: expected {expected_python!r}, got "
            f"{target_metadata.get('python')!r}"
        )
    target_facts = target_metadata.get("target")
    if not isinstance(target_facts, Mapping) or set(target_facts) != {
        "requested",
        "compiler_target_triple",
        "artifact_kind",
    }:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has an invalid target fact set"
        )
    requested_target = target_facts.get("requested")
    if not isinstance(requested_target, str) or not requested_target:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has no requested-target authority"
        )
    try:
        target_plan = source_extension_recorded_target_plan(
            requested_target,
            target_triple=variant.target_triple,
        )
    except ValueError as exc:
        raise SourceExtensionSetValidationError(
            f"extension-set target metadata requested target is invalid: {exc}"
        ) from exc
    expected_target_facts = {
        "requested": target_plan.requested,
        "compiler_target_triple": target_plan.compiler_target_triple,
        "artifact_kind": target_plan.artifact_kind,
    }
    if (
        target_plan.target_triple != variant.target_triple
        or dict(target_facts) != expected_target_facts
    ):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata facts differ from the canonical target "
            f"plan: expected target {variant.target_triple!r} and facts "
            f"{expected_target_facts!r}, got target {target_plan.target_triple!r} "
            f"and facts {dict(target_facts)!r}"
        )
    target_identity = dict(target_metadata)
    target_digest = target_identity.pop("digest", None)
    computed_target_digest = hashlib.sha256(
        json.dumps(
            target_identity,
            sort_keys=True,
            allow_nan=False,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    if target_digest != computed_target_digest:
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata identity checksum is false"
        )
    target_sidecar_path = (
        publish_root
        / "provenance"
        / "metadata"
        / "target"
        / "source-extension-target-metadata.json"
    )
    try:
        target_sidecar = loads_exact(target_sidecar_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as exc:
        raise SourceExtensionSetValidationError(
            f"cannot read canonical target metadata sidecar: {exc}"
        ) from exc
    if target_sidecar != target_metadata:
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata differs from its canonical sidecar"
        )
    target_digests = target_metadata.get("digests")
    if not isinstance(target_digests, Mapping):
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata digests are missing"
        )
    target_files = {
        "python_pc_sha256": (
            publish_root / "provenance/metadata/target/pkgconfig/python3.pc"
        ),
        "meson_cross_sha256": (publish_root / "provenance/metadata/target/meson.cross"),
    }
    for digest_name, target_file in target_files.items():
        if not target_file.is_file() or target_digests.get(digest_name) != _sha256_file(
            target_file
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set target_metadata {digest_name} is false"
            )
    toolchain = target_metadata.get("toolchain")
    tools = toolchain.get("tools") if isinstance(toolchain, Mapping) else None
    target_commands = (
        toolchain.get("commands") if isinstance(toolchain, Mapping) else None
    )
    tool_roles, required_command_roles = _source_extension_tool_role_contract(
        variant.target_triple
    )
    if not isinstance(tools, Mapping) or set(tools) != set(tool_roles.values()):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has an incomplete tool identity family"
        )
    if not isinstance(target_commands, Mapping):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has no command family"
        )
    unknown_command_roles = set(target_commands) - set(tool_roles)
    missing_command_roles = required_command_roles - set(target_commands)
    if unknown_command_roles or missing_command_roles:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata command family differs from target "
            f"contract: missing={sorted(missing_command_roles)!r}, "
            f"unknown={sorted(unknown_command_roles)!r}"
        )
    validated_commands: list[tuple[str, tuple[str, ...]]] = []
    for command_role, command in target_commands.items():
        if not isinstance(command_role, str):
            raise SourceExtensionSetValidationError(
                "extension-set target metadata command role must be a string"
            )
        tool_role = tool_roles[command_role]
        identity = tools.get(tool_role)
        if not (
            isinstance(identity, Mapping)
            and isinstance(identity.get("path"), str)
            and isinstance(identity.get("sha256"), str)
            and len(identity["sha256"]) == 64
            and all(character in "0123456789abcdef" for character in identity["sha256"])
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set target metadata {command_role} identity is invalid"
            )
        arguments = _validated_command(command, role=command_role)
        identity_arguments = _validated_command(identity.get("command"), role=tool_role)
        if arguments[0] != identity_arguments[0]:
            raise SourceExtensionSetValidationError(
                f"extension-set target metadata {command_role} identity is invalid"
            )
        validated_commands.append((command_role, arguments))

    return ValidatedSourceExtensionTarget(
        plan=target_plan,
        commands=tuple(sorted(validated_commands)),
    )
