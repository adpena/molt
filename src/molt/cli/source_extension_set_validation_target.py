"""Host-independent target facts and canonical compiler-command custody."""

from __future__ import annotations
import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping
from molt.cli.compiler_target import (
    compiler_target_triple,
    validate_compiler_target,
    validate_source_extension_compiler_dialect,
)
from molt.cli.source_extension_set_registry import SourceExtensionVariant
from molt.cli.source_extension_set_validation_schema import (
    SourceExtensionSetValidationError,
)
from molt.cli.source_extension_target import (
    SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION,
    SourceExtensionTargetPlan,
    source_extension_recorded_target_plan,
    source_extension_target_is_wasm,
)
from molt.cli.source_extension_toolchain import (
    _meson_cross_text,
    _meson_native_text,
    _source_extension_meson_cross_properties,
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
    required_commands = {"ar", "c", "cpp", "nm"}
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
        "build_toolchain",
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
        SOURCE_EXTENSION_TARGET_METADATA_SCHEMA_VERSION,
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
        "meson_native_sha256": (
            publish_root / "provenance/metadata/target/meson.native"
        ),
    }
    if set(target_digests) != set(target_files):
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata digest family differs from schema"
        )
    for digest_name, target_file in target_files.items():
        if not target_file.is_file() or target_digests.get(digest_name) != _sha256_file(
            target_file
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set target_metadata {digest_name} is false"
            )
    commands = _validated_toolchain_commands(
        target_metadata.get("toolchain"), target_triple=variant.target_triple
    )
    build_toolchain = target_metadata.get("build_toolchain")
    if not isinstance(build_toolchain, Mapping) or set(build_toolchain) != {
        "target_triple",
        "compiler_kind",
        "tools",
        "commands",
    }:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has an invalid build-machine toolchain"
        )
    build_triple = build_toolchain.get("target_triple")
    compiler_kind = build_toolchain.get("compiler_kind")
    if (
        not isinstance(build_triple, str)
        or not isinstance(compiler_kind, str)
        or not compiler_kind
    ):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has invalid build-machine coordinates"
        )
    try:
        source_extension_recorded_target_plan("native", target_triple=build_triple)
        build_commands = _validated_toolchain_commands(
            build_toolchain, target_triple=build_triple
        )
    except ValueError as exc:
        raise SourceExtensionSetValidationError(
            f"extension-set build-machine toolchain is invalid: {exc}"
        ) from exc
    if target_plan.requested == "native" and (
        build_triple != target_plan.target_triple
        or build_toolchain["tools"] != target_metadata["toolchain"]["tools"]
        or build_toolchain["commands"] != target_metadata["toolchain"]["commands"]
    ):
        raise SourceExtensionSetValidationError(
            "extension-set native target and build-machine toolchains differ"
        )
    _validate_meson_machine_files(
        target_metadata=target_metadata,
        target_plan=target_plan,
        commands=dict(commands),
        build_commands=dict(build_commands),
        cross_file=target_files["meson_cross_sha256"],
        native_file=target_files["meson_native_sha256"],
    )
    return ValidatedSourceExtensionTarget(plan=target_plan, commands=commands)


def _validate_meson_machine_files(
    *,
    target_metadata: Mapping[str, Any],
    target_plan: SourceExtensionTargetPlan,
    commands: Mapping[str, tuple[str, ...]],
    build_commands: Mapping[str, tuple[str, ...]],
    cross_file: Path,
    native_file: Path,
) -> None:
    if target_metadata.get("meson_cross_properties") != (
        _source_extension_meson_cross_properties(target_plan)
    ):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata Meson cross properties differ from target plan"
        )
    paths = target_metadata.get("paths")
    pkg_config_dir = paths.get("pkg_config_dir") if isinstance(paths, Mapping) else None
    abi = target_metadata.get("abi")
    include_dirs = abi.get("include_dirs") if isinstance(abi, Mapping) else None
    if not isinstance(pkg_config_dir, str) or not pkg_config_dir:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has no Meson pkg-config path"
        )
    if (
        not isinstance(include_dirs, list)
        or not include_dirs
        or any(not isinstance(path, str) or not path for path in include_dirs)
    ):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has invalid Meson include paths"
        )
    compiler_builtins: str | None = None
    if target_plan.target_triple == "wasm32-wasip1":
        toolchain = target_metadata.get("toolchain")
        archives = (
            toolchain.get("link_probe_archives")
            if isinstance(toolchain, Mapping)
            else None
        )
        builtins = (
            archives.get("compiler_builtins") if isinstance(archives, Mapping) else None
        )
        compiler_builtins = (
            builtins.get("path") if isinstance(builtins, Mapping) else None
        )
        if not isinstance(compiler_builtins, str) or not compiler_builtins:
            raise SourceExtensionSetValidationError(
                "extension-set target metadata has no Meson compiler-builtins path"
            )
    expected_cross = _meson_cross_text(
        target_plan=target_plan,
        pkg_config_dir=pkg_config_dir,
        commands=commands,
        compiler_builtins=compiler_builtins,
        include_dirs=tuple(include_dirs),
    )
    expected_native = _meson_native_text(commands=build_commands)
    if cross_file.read_bytes() != expected_cross.encode("utf-8"):
        raise SourceExtensionSetValidationError(
            "extension-set Meson cross file differs from bound target commands and paths"
        )
    if native_file.read_bytes() != expected_native.encode("utf-8"):
        raise SourceExtensionSetValidationError(
            "extension-set Meson native file differs from bound build commands"
        )


def _validated_toolchain_commands(
    toolchain: object, *, target_triple: str
) -> tuple[tuple[str, tuple[str, ...]], ...]:
    tools = toolchain.get("tools") if isinstance(toolchain, Mapping) else None
    target_commands = (
        toolchain.get("commands") if isinstance(toolchain, Mapping) else None
    )
    tool_roles, required_command_roles = _source_extension_tool_role_contract(
        target_triple
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
        if command_role in {"c", "cpp"}:
            try:
                validate_source_extension_compiler_dialect(arguments, target_triple)
                validate_compiler_target(
                    arguments, compiler_target_triple(arguments, target_triple)
                )
            except ValueError as exc:
                raise SourceExtensionSetValidationError(
                    f"extension-set target metadata {command_role} command is invalid: {exc}"
                ) from exc
        validated_commands.append((command_role, arguments))

    return tuple(sorted(validated_commands))
