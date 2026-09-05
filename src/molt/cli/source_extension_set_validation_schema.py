"""Exact recorded-set structure and registered-admission comparison."""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Any, Mapping, cast

from molt.cli.source_extension_reproducibility import _require_location_neutral
from molt.cli.source_extension_set_identity import SOURCE_EXTENSION_SET_SCHEMA_VERSION
from molt.cli.source_extension_set_registry import (
    SourceExtensionSet,
    SourceExtensionSource,
    SourceExtensionSpec,
    SourceExtensionVariant,
    validate_source_extension_module_target,
)
from molt.target_python import _parse_target_python_version


class SourceExtensionSetValidationError(ValueError):
    """A package set violates its structural or registered admission contract."""


@dataclass(frozen=True, slots=True)
class RecordedSourceExtensionSet:
    """Recorded build contract, never a current-registry admission capability."""

    package: str
    package_version: str
    name: str
    seal_name: str
    source: SourceExtensionSource
    meson_setup_args: tuple[str, ...]
    use_pkg_config: bool
    required_config_tools: tuple[str, ...]
    extensions: tuple[SourceExtensionSpec, ...]


def validate_source_extension_set_manifest_schema(
    set_manifest: Mapping[str, Any],
) -> tuple[RecordedSourceExtensionSet, SourceExtensionVariant]:
    expected_manifest_keys = {
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
        "build_environment",
        "meson",
        "target_metadata",
        "installed_package_files",
        "extensions",
    }
    if set(set_manifest) != expected_manifest_keys:
        raise SourceExtensionSetValidationError(
            "extension-set manifest keys differ from schema: "
            f"missing={sorted(expected_manifest_keys - set(set_manifest))!r}, "
            f"unknown={sorted(set(set_manifest) - expected_manifest_keys)!r}"
        )

    if (
        type(set_manifest.get("schema_version")) is not int
        or set_manifest["schema_version"] != SOURCE_EXTENSION_SET_SCHEMA_VERSION
        or set_manifest.get("kind") != "molt-source-extension-set"
    ):
        raise SourceExtensionSetValidationError(
            "extension-set manifest schema is invalid"
        )
    for field in (
        "package",
        "package_version",
        "name",
        "seal_name",
        "cpython",
        "source_head",
        "target_triple",
        "abi_tier",
    ):
        value = set_manifest.get(field)
        if not isinstance(value, str) or not value or value != value.strip():
            raise SourceExtensionSetValidationError(
                f"extension-set manifest {field} must be a canonical string"
            )
    validate_source_extension_module_target(
        set_manifest["package"], set_manifest["package"]
    )
    if re.fullmatch(r"[0-9a-f]{40}", set_manifest["source_head"]) is None:
        raise SourceExtensionSetValidationError(
            "extension-set source_head must be a full Git commit"
        )
    for field in ("build_environment", "meson", "target_metadata"):
        if not isinstance(set_manifest.get(field), Mapping):
            raise SourceExtensionSetValidationError(
                f"extension-set manifest {field} must be an object"
            )
    if not isinstance(set_manifest.get("submodules"), list):
        raise SourceExtensionSetValidationError(
            "extension-set manifest submodules must be a list"
        )
    meson = cast(Mapping[str, Any], set_manifest["meson"])
    setup_args = meson.get("setup_args")
    config_tools = meson.get("config_tools")
    if not isinstance(setup_args, list) or not all(
        isinstance(arg, str) for arg in setup_args
    ):
        raise SourceExtensionSetValidationError(
            "extension-set Meson setup_args must be strings"
        )
    if not isinstance(config_tools, list) or not all(
        isinstance(tool, Mapping) and isinstance(tool.get("name"), str) and tool["name"]
        for tool in config_tools
    ):
        raise SourceExtensionSetValidationError(
            "extension-set Meson config_tools is invalid"
        )
    config_names = tuple(str(tool["name"]) for tool in config_tools)
    if len(set(config_names)) != len(config_names):
        raise SourceExtensionSetValidationError(
            "extension-set Meson config tool names are duplicated"
        )
    raw_extensions = set_manifest.get("extensions")
    extension_entry_keys = {
        "module",
        "target",
        "python_exports",
        "capabilities",
        "provided_capsules",
        "exclude_linked_static_libraries",
        "artifact_sha256",
        "wheel_sha256",
        "object_closure_sha256",
    }
    if not isinstance(raw_extensions, list) or not all(
        isinstance(item, Mapping)
        and set(item) == extension_entry_keys
        and isinstance(item.get("module"), str)
        and isinstance(item.get("target"), str)
        and isinstance(item.get("python_exports"), list)
        and isinstance(item.get("capabilities"), list)
        and isinstance(item.get("provided_capsules"), list)
        and isinstance(item.get("exclude_linked_static_libraries"), list)
        and all(isinstance(value, str) for value in item["python_exports"])
        and all(isinstance(value, str) for value in item["capabilities"])
        and all(isinstance(value, str) for value in item["provided_capsules"])
        and all(
            isinstance(value, str) for value in item["exclude_linked_static_libraries"]
        )
        and all(
            isinstance(item.get(field), str)
            and len(item[field]) == 64
            and all(character in "0123456789abcdef" for character in item[field])
            for field in (
                "artifact_sha256",
                "wheel_sha256",
                "object_closure_sha256",
            )
        )
        for item in raw_extensions
    ):
        raise SourceExtensionSetValidationError(
            "extension-set manifest extensions must be module objects"
        )

    specs = []
    for entry in raw_extensions:
        module, target = validate_source_extension_module_target(
            entry["module"], entry["target"]
        )
        specs.append(
            SourceExtensionSpec(
                module=module,
                target=target,
                python_exports=tuple(entry["python_exports"]),
                capabilities=tuple(entry["capabilities"]),
                provided_capsules=tuple(entry["provided_capsules"]),
                exclude_linked_static_libraries=tuple(
                    entry["exclude_linked_static_libraries"]
                ),
            )
        )
    if not specs or len({spec.module for spec in specs}) != len(specs):
        raise SourceExtensionSetValidationError(
            "extension-set typed extension modules are empty or duplicated"
        )
    variant = SourceExtensionVariant(
        target_python=_parse_target_python_version(set_manifest["cpython"]),
        abi_tier=set_manifest["abi_tier"],
        target_triple=set_manifest["target_triple"],
    )
    if variant.cpython != set_manifest["cpython"]:
        raise SourceExtensionSetValidationError(
            "extension-set CPython version is not canonical"
        )
    _require_location_neutral(set_manifest, authority="source-extension set manifest")
    return RecordedSourceExtensionSet(
        package=set_manifest["package"],
        package_version=set_manifest["package_version"],
        name=set_manifest["name"],
        seal_name=set_manifest["seal_name"],
        source=SourceExtensionSource("git", set_manifest["source_head"]),
        meson_setup_args=tuple(setup_args),
        use_pkg_config=meson.get("pkg_config_requirement") is not None,
        required_config_tools=config_names,
        extensions=tuple(specs),
    ), variant


def require_source_extension_set_registered_contract(
    recorded: RecordedSourceExtensionSet,
    recorded_variant: SourceExtensionVariant,
    installed_package_files: tuple[str, ...],
    extension_set: SourceExtensionSet,
    variant: SourceExtensionVariant,
) -> None:
    """Apply current registry policy to facts already structurally validated."""

    fields = (
        "package",
        "package_version",
        "name",
        "seal_name",
        "meson_setup_args",
        "use_pkg_config",
        "required_config_tools",
        "extensions",
    )
    mismatches = [
        field
        for field in fields
        if getattr(recorded, field) != getattr(extension_set, field)
    ]
    if recorded.source.commit != extension_set.source.commit:
        mismatches.append("source_head")
    if recorded_variant != variant:
        mismatches.append("variant")
    if mismatches:
        raise SourceExtensionSetValidationError(
            "extension-set manifest differs from registered package-set authority: "
            + ", ".join(mismatches)
        )
    missing = sorted(
        set(extension_set.required_installed_files) - set(installed_package_files)
    )
    if missing:
        raise SourceExtensionSetValidationError(
            "extension-set installed package inventory is missing configured files: "
            + ", ".join(missing)
        )
