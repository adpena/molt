#!/usr/bin/env python3
"""Generate host-capability profiles and runtime tiers from one authority."""

from __future__ import annotations

import argparse
import re
import shutil
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from collections.abc import Iterable
from typing import cast

from generator_io import generated_file_matches, write_generated_text
from molt.cli.build_output_layout import _DEPLOY_PROFILE_CHOICES
from molt.release_matrix import RELEASE_TARGETS, SUPPORTED_CPYTHON_VERSIONS
from molt.wasi_sysroot import WASI_TARGET_INCLUDE_DIRS

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "runtime" / "host_capabilities.toml"
OUT_PYTHON = ROOT / "src" / "molt" / "_host_capabilities_generated.py"
OUT_RUST = (
    ROOT / "runtime" / "molt-runtime-core" / "src" / "host_capabilities_generated.rs"
)
OUT_JAVASCRIPT = ROOT / "wasm" / "host_capabilities_generated.js"
OUT_MARKDOWN = (
    ROOT / "docs" / "spec" / "areas" / "security" / "host_capabilities.generated.md"
)
OUTPUTS = (OUT_PYTHON, OUT_RUST, OUT_JAVASCRIPT, OUT_MARKDOWN)

EXECUTION_TARGETS = frozenset(("native", *_DEPLOY_PROFILE_CHOICES))
WASI_RUST_TARGET = WASI_TARGET_INCLUDE_DIRS[0]
TARGET_PLATFORMS = frozenset(
    {str(target["platform"]) for target in RELEASE_TARGETS} | {"wasi"}
)
TARGET_ARCHITECTURES = frozenset(
    {
        *(str(target["rust_target"]).split("-", 1)[0] for target in RELEASE_TARGETS),
        WASI_RUST_TARGET.split("-", 1)[0],
    }
)
TARGET_PYTHON_VERSIONS = frozenset(SUPPORTED_CPYTHON_VERSIONS)

_NAME_RE = re.compile(r"^[a-z][a-z0-9_-]*$")
_TOKEN_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")


class SchemaError(ValueError):
    """The checked-in host-capability authority is inconsistent."""


@dataclass(frozen=True)
class GrantFamily:
    name: str
    description: str
    grants: tuple[str, ...]


@dataclass(frozen=True)
class Tier(GrantFamily):
    inherits: tuple[str, ...]
    effective_grants: tuple[str, ...]


@dataclass(frozen=True)
class Operation:
    name: str
    capabilities: tuple[str, ...]
    targets: tuple[str, ...]
    platforms: tuple[str, ...]
    architectures: tuple[str, ...]
    python_versions: tuple[str, ...]


@dataclass(frozen=True)
class Schema:
    default_tier: str
    explicit_policy_tier: str
    maximum_builtin_tier: str
    profiles: tuple[GrantFamily, ...]
    tiers: tuple[Tier, ...]
    capabilities: tuple[str, ...]
    operations: tuple[Operation, ...]


def _rows(value: object, context: str) -> list[dict[str, object]]:
    if not isinstance(value, list) or not value:
        raise SchemaError(f"{context} must be a non-empty array of tables")
    if not all(isinstance(row, dict) for row in value):
        raise SchemaError(f"{context} rows must be tables")
    return cast(list[dict[str, object]], value)


def _name(value: object, context: str) -> str:
    if not isinstance(value, str) or _NAME_RE.fullmatch(value) is None:
        raise SchemaError(f"{context} must be a lowercase identifier")
    return value


def _token(value: object, context: str) -> str:
    if not isinstance(value, str) or _TOKEN_RE.fullmatch(value) is None:
        raise SchemaError(f"{context} must be a lowercase dotted identifier")
    return value


def _projection_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", value).strip("_").upper()


def _rust_variant(value: str) -> str:
    return "".join(part.capitalize() for part in re.split(r"[^A-Za-z0-9]+", value))


def _validate_projection_names(values: Iterable[str], context: str) -> None:
    source = tuple(values)
    constant_names = [_projection_name(value) for value in source]
    rust_variants = [_rust_variant(value) for value in source]
    if len(constant_names) != len(set(constant_names)):
        raise SchemaError(f"{context} identifiers collide after constant projection")
    if len(rust_variants) != len(set(rust_variants)):
        raise SchemaError(f"{context} identifiers collide after Rust projection")


def _description(value: object, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise SchemaError(f"{context} must be a non-empty string")
    return value


def _strings(value: object, context: str, *, tokens: bool) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise SchemaError(f"{context} must be an array of strings")
    result = tuple(cast(str, item) for item in value)
    if len(result) != len(set(result)):
        raise SchemaError(f"{context} entries must be unique")
    if tokens:
        invalid = [item for item in result if _TOKEN_RE.fullmatch(item) is None]
        if invalid:
            raise SchemaError(
                f"{context} contains invalid capability tokens: {invalid!r}"
            )
    return result


def _validate_vocabulary(
    values: tuple[str, ...], vocabulary: frozenset[str], context: str
) -> None:
    unknown = sorted(set(values) - vocabulary)
    if unknown:
        raise SchemaError(
            f"{context} contains unsupported values {unknown!r}; "
            f"expected members of {sorted(vocabulary)!r}"
        )


def _family(row: dict[str, object], context: str) -> GrantFamily:
    if set(row) != {"name", "description", "grants"}:
        raise SchemaError(f"{context} keys must be exactly name, description, grants")
    name = _name(row["name"], f"{context}.name")
    return GrantFamily(
        name=name,
        description=_description(row["description"], f"{context}.{name}.description"),
        grants=_strings(row["grants"], f"{context}.{name}.grants", tokens=True),
    )


def load_schema(path: Path = SOURCE) -> Schema:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    expected_keys = {
        "schema_version",
        "default_tier",
        "explicit_policy_tier",
        "maximum_builtin_tier",
        "profile",
        "tier",
        "operation",
    }
    if set(data) != expected_keys:
        raise SchemaError(f"top-level keys must be exactly {sorted(expected_keys)!r}")
    if data["schema_version"] != 1:
        raise SchemaError("host-capability authority schema_version must be 1")

    profiles = tuple(
        _family(row, f"profile[{index}]")
        for index, row in enumerate(_rows(data["profile"], "profile"))
    )
    profile_names = [profile.name for profile in profiles]
    if len(profile_names) != len(set(profile_names)):
        raise SchemaError("profile names must be unique")

    raw_tiers = _rows(data["tier"], "tier")
    tier_names: list[str] = []
    tier_rows: list[tuple[GrantFamily, tuple[str, ...]]] = []
    for index, row in enumerate(raw_tiers):
        if set(row) != {"name", "description", "inherits", "grants"}:
            raise SchemaError(
                f"tier[{index}] keys must be exactly name, description, inherits, grants"
            )
        family = GrantFamily(
            name=_name(row["name"], f"tier[{index}].name"),
            description=_description(row["description"], f"tier[{index}].description"),
            grants=_strings(row["grants"], f"tier[{index}].grants", tokens=True),
        )
        inherits = _strings(row["inherits"], f"tier[{index}].inherits", tokens=False)
        if family.name in tier_names:
            raise SchemaError(f"duplicate tier name {family.name!r}")
        unknown_or_forward = [name for name in inherits if name not in tier_names]
        if unknown_or_forward:
            raise SchemaError(
                f"tier {family.name!r} must inherit only earlier tiers: "
                f"{unknown_or_forward!r}"
            )
        tier_names.append(family.name)
        tier_rows.append((family, inherits))

    effective_by_name: dict[str, tuple[str, ...]] = {}
    tiers: list[Tier] = []
    for family, inherits in tier_rows:
        effective = tuple(
            dict.fromkeys(
                grant
                for inherited in inherits
                for grant in effective_by_name[inherited]
            )
        )
        effective = tuple(dict.fromkeys([*effective, *family.grants]))
        effective_by_name[family.name] = effective
        tiers.append(
            Tier(
                name=family.name,
                description=family.description,
                grants=family.grants,
                inherits=inherits,
                effective_grants=effective,
            )
        )

    default_tier = _name(data["default_tier"], "default_tier")
    explicit_policy_tier = _name(data["explicit_policy_tier"], "explicit_policy_tier")
    maximum_builtin_tier = _name(data["maximum_builtin_tier"], "maximum_builtin_tier")
    for context, tier in (
        ("default_tier", default_tier),
        ("explicit_policy_tier", explicit_policy_tier),
        ("maximum_builtin_tier", maximum_builtin_tier),
    ):
        if tier not in effective_by_name:
            raise SchemaError(f"{context} references unknown tier {tier!r}")
    if effective_by_name[explicit_policy_tier]:
        raise SchemaError("explicit_policy_tier must grant no ambient capabilities")

    operations: list[Operation] = []
    operation_names: set[str] = set()
    for index, row in enumerate(_rows(data["operation"], "operation")):
        required_keys = {"name", "capabilities"}
        optional_keys = {"targets", "platforms", "architectures", "python_versions"}
        if not required_keys <= set(row) or set(row) - required_keys - optional_keys:
            raise SchemaError(
                f"operation[{index}] keys must be name, capabilities, and optional "
                "targets/platforms/architectures/python_versions"
            )
        name = _token(row["name"], f"operation[{index}].name")
        capabilities = _strings(
            row["capabilities"], f"operation[{index}].capabilities", tokens=True
        )
        if not capabilities:
            raise SchemaError(f"operation[{index}].capabilities must not be empty")
        targets = _strings(
            row.get("targets", []), f"operation[{index}].targets", tokens=True
        )
        platforms = _strings(
            row.get("platforms", []), f"operation[{index}].platforms", tokens=True
        )
        architectures = _strings(
            row.get("architectures", []),
            f"operation[{index}].architectures",
            tokens=True,
        )
        python_versions = _strings(
            row.get("python_versions", []),
            f"operation[{index}].python_versions",
            tokens=True,
        )
        _validate_vocabulary(targets, EXECUTION_TARGETS, f"operation[{index}].targets")
        _validate_vocabulary(
            platforms, TARGET_PLATFORMS, f"operation[{index}].platforms"
        )
        _validate_vocabulary(
            architectures,
            TARGET_ARCHITECTURES,
            f"operation[{index}].architectures",
        )
        _validate_vocabulary(
            python_versions,
            TARGET_PYTHON_VERSIONS,
            f"operation[{index}].python_versions",
        )
        if name in operation_names:
            raise SchemaError(f"duplicate operation name {name!r}")
        operation_names.add(name)
        operations.append(
            Operation(
                name,
                capabilities,
                targets,
                platforms,
                architectures,
                python_versions,
            )
        )

    capabilities = tuple(
        sorted(
            {
                *(grant for profile in profiles for grant in profile.grants),
                *(grant for tier in tiers for grant in tier.effective_grants),
                *(
                    capability
                    for operation in operations
                    for capability in operation.capabilities
                ),
            }
        )
    )
    _validate_projection_names(capabilities, "capability")
    _validate_projection_names(operation_names, "operation")
    missing_from_maximum = sorted(
        set(capabilities) - set(effective_by_name[maximum_builtin_tier])
    )
    if missing_from_maximum:
        raise SchemaError(
            f"maximum_builtin_tier omits built-in capabilities: {missing_from_maximum!r}"
        )

    return Schema(
        default_tier,
        explicit_policy_tier,
        maximum_builtin_tier,
        profiles,
        tuple(tiers),
        capabilities,
        tuple(operations),
    )


def render_python(schema: Schema) -> str:
    lines = [
        "# @generated by tools/gen_host_capabilities.py from\n",
        "# runtime/host_capabilities.toml. DO NOT EDIT.\n",
        '"""Built-in host-capability profiles and runtime tiers."""\n\n',
        "from __future__ import annotations\n\n",
        "from enum import StrEnum\n",
        "from types import MappingProxyType\n",
        "from typing import Final, Mapping\n\n",
        "class CapabilityId(StrEnum):\n",
    ]
    lines.extend(
        f"    {_projection_name(capability)} = {capability!r}\n"
        for capability in schema.capabilities
    )
    lines.extend(["\n", "class OperationId(StrEnum):\n"])
    lines.extend(
        f"    {_projection_name(operation.name)} = {operation.name!r}\n"
        for operation in schema.operations
    )
    lines.extend(
        [
            "\n",
            "OPERATION_CAPABILITIES: Final[Mapping[OperationId, tuple[CapabilityId, ...]]] = MappingProxyType({\n",
        ]
    )
    for operation in schema.operations:
        capabilities = ", ".join(
            f"CapabilityId.{_projection_name(capability)}"
            for capability in operation.capabilities
        )
        if len(operation.capabilities) == 1:
            capabilities += ","
        lines.append(
            f"    OperationId.{_projection_name(operation.name)}: ({capabilities}),\n"
        )
    lines.extend(
        [
            "})\n",
            "OPERATION_TARGETS: Final[Mapping[OperationId, tuple[str, ...]]] = MappingProxyType({\n",
        ]
    )
    for operation in schema.operations:
        if operation.targets:
            lines.append(
                f"    OperationId.{_projection_name(operation.name)}: "
                f"{operation.targets!r},\n"
            )
    lines.extend(
        [
            "})\n",
            "OPERATION_PLATFORMS: Final[Mapping[OperationId, tuple[str, ...]]] = MappingProxyType({\n",
        ]
    )
    for operation in schema.operations:
        if operation.platforms:
            lines.append(
                f"    OperationId.{_projection_name(operation.name)}: "
                f"{operation.platforms!r},\n"
            )
    lines.extend(
        [
            "})\n",
            "OPERATION_ARCHITECTURES: Final[Mapping[OperationId, tuple[str, ...]]] = MappingProxyType({\n",
        ]
    )
    for operation in schema.operations:
        if operation.architectures:
            lines.append(
                f"    OperationId.{_projection_name(operation.name)}: "
                f"{operation.architectures!r},\n"
            )
    lines.extend(
        [
            "})\n",
            "OPERATION_PYTHON_VERSIONS: Final[Mapping[OperationId, tuple[str, ...]]] = MappingProxyType({\n",
        ]
    )
    for operation in schema.operations:
        if operation.python_versions:
            lines.append(
                f"    OperationId.{_projection_name(operation.name)}: "
                f"{operation.python_versions!r},\n"
            )
    lines.extend(
        [
            "})\n\n",
            f"SUPPORTED_EXECUTION_TARGETS: Final = {tuple(sorted(EXECUTION_TARGETS))!r}\n",
            f"SUPPORTED_TARGET_PLATFORMS: Final = {tuple(sorted(TARGET_PLATFORMS))!r}\n",
            f"SUPPORTED_TARGET_ARCHITECTURES: Final = {tuple(sorted(TARGET_ARCHITECTURES))!r}\n",
            f"SUPPORTED_TARGET_PYTHON_VERSIONS: Final = {tuple(sorted(TARGET_PYTHON_VERSIONS))!r}\n",
            f"DEFAULT_CAPABILITY_TIER: Final = {schema.default_tier!r}\n",
            f"EXPLICIT_CAPABILITY_TIER: Final = {schema.explicit_policy_tier!r}\n",
            f"MAXIMUM_BUILTIN_CAPABILITY_TIER: Final = {schema.maximum_builtin_tier!r}\n",
            "CAPABILITY_PROFILES: Final[Mapping[str, tuple[str, ...]]] = MappingProxyType({\n",
        ]
    )
    for profile in schema.profiles:
        lines.append(f"    {profile.name!r}: {profile.grants!r},\n")
    lines.append("})\n\n")
    lines.append(
        "CAPABILITY_TIERS: Final[Mapping[str, tuple[str, ...]]] = MappingProxyType({\n"
    )
    for tier in schema.tiers:
        lines.append(f"    {tier.name!r}: {tier.effective_grants!r},\n")
    lines.extend(
        [
            "})\n\n",
            "def capabilities_for_operation(operation: OperationId) -> tuple[CapabilityId, ...]:\n",
            '    """Return the exact built-in capabilities required by an operation."""\n',
            "    return OPERATION_CAPABILITIES[operation]\n\n",
            "def operation_supports_target(\n",
            "    operation: OperationId, *, target: str, platform: str, architecture: str, python_version: str\n",
            ") -> bool:\n",
            '    """Return whether an operation is declared for one target cell."""\n',
            "    if target not in SUPPORTED_EXECUTION_TARGETS:\n",
            "        return False\n",
            "    if platform not in SUPPORTED_TARGET_PLATFORMS:\n",
            "        return False\n",
            "    if architecture not in SUPPORTED_TARGET_ARCHITECTURES:\n",
            "        return False\n",
            "    if python_version not in SUPPORTED_TARGET_PYTHON_VERSIONS:\n",
            "        return False\n",
            "    targets = OPERATION_TARGETS.get(operation, ())\n",
            "    platforms = OPERATION_PLATFORMS.get(operation, ())\n",
            "    architectures = OPERATION_ARCHITECTURES.get(operation, ())\n",
            "    python_versions = OPERATION_PYTHON_VERSIONS.get(operation, ())\n",
            "    return (not targets or target in targets) and (\n",
            "        not platforms or platform in platforms\n",
            "    ) and (not architectures or architecture in architectures) and (\n",
            "        not python_versions or python_version in python_versions\n",
            "    )\n\n",
            "def capabilities_for_tier(name: str) -> tuple[str, ...] | None:\n",
            '    """Return flattened grants, or ``None`` for an unknown tier."""\n',
            "    return CAPABILITY_TIERS.get(name.strip().casefold())\n",
        ]
    )
    return "".join(lines)


def _rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _rust_constant(name: str) -> str:
    return name.upper().replace("-", "_")


def render_rust(schema: Schema) -> str:
    lines = [
        "// @generated by tools/gen_host_capabilities.py from\n",
        "// runtime/host_capabilities.toml. DO NOT EDIT.\n\n",
        "//! Built-in host-capability runtime tiers.\n\n",
        "#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n",
        "pub enum CapabilityId {\n",
    ]
    lines.extend(
        f"    {_rust_variant(capability)},\n" for capability in schema.capabilities
    )
    lines.extend(
        [
            "}\n\n",
            "impl CapabilityId {\n",
            "    pub const fn as_str(self) -> &'static str {\n",
            "        match self {\n",
        ]
    )
    lines.extend(
        f"            Self::{_rust_variant(capability)} => {_rust_string(capability)},\n"
        for capability in schema.capabilities
    )
    lines.extend(
        [
            "        }\n",
            "    }\n",
            "}\n\n",
            f"pub const SUPPORTED_EXECUTION_TARGETS: [&str; {len(EXECUTION_TARGETS)}] = [{', '.join(_rust_string(value) for value in sorted(EXECUTION_TARGETS))}];\n",
            f"pub const SUPPORTED_TARGET_PLATFORMS: [&str; {len(TARGET_PLATFORMS)}] = [{', '.join(_rust_string(value) for value in sorted(TARGET_PLATFORMS))}];\n",
            f"pub const SUPPORTED_TARGET_ARCHITECTURES: [&str; {len(TARGET_ARCHITECTURES)}] = [{', '.join(_rust_string(value) for value in sorted(TARGET_ARCHITECTURES))}];\n",
            f"pub const SUPPORTED_TARGET_PYTHON_VERSIONS: [&str; {len(TARGET_PYTHON_VERSIONS)}] = [{', '.join(_rust_string(value) for value in sorted(TARGET_PYTHON_VERSIONS))}];\n\n",
            "#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n",
            "pub enum OperationId {\n",
        ]
    )
    lines.extend(
        f"    {_rust_variant(operation.name)},\n" for operation in schema.operations
    )
    lines.extend(
        [
            "}\n\n",
            "impl OperationId {\n",
            "    pub const fn as_str(self) -> &'static str {\n",
            "        match self {\n",
        ]
    )
    lines.extend(
        f"            Self::{_rust_variant(operation.name)} => {_rust_string(operation.name)},\n"
        for operation in schema.operations
    )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn required_capabilities(self) -> &'static [CapabilityId] {\n",
            "        match self {\n",
        ]
    )
    for operation in schema.operations:
        capabilities = ", ".join(
            f"CapabilityId::{_rust_variant(capability)}"
            for capability in operation.capabilities
        )
        lines.append(
            f"            Self::{_rust_variant(operation.name)} => &[{capabilities}],\n"
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn targets(self) -> &'static [&'static str] {\n",
            "        match self {\n",
        ]
    )
    for operation in schema.operations:
        targets = ", ".join(_rust_string(value) for value in operation.targets)
        lines.append(
            f"            Self::{_rust_variant(operation.name)} => &[{targets}],\n"
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn platforms(self) -> &'static [&'static str] {\n",
            "        match self {\n",
        ]
    )
    for operation in schema.operations:
        platforms = ", ".join(_rust_string(value) for value in operation.platforms)
        lines.append(
            f"            Self::{_rust_variant(operation.name)} => &[{platforms}],\n"
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn architectures(self) -> &'static [&'static str] {\n",
            "        match self {\n",
        ]
    )
    for operation in schema.operations:
        architectures = ", ".join(
            _rust_string(value) for value in operation.architectures
        )
        lines.append(
            f"            Self::{_rust_variant(operation.name)} => &[{architectures}],\n"
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub const fn python_versions(self) -> &'static [&'static str] {\n",
            "        match self {\n",
        ]
    )
    for operation in schema.operations:
        python_versions = ", ".join(
            _rust_string(value) for value in operation.python_versions
        )
        lines.append(
            f"            Self::{_rust_variant(operation.name)} => &[{python_versions}],\n"
        )
    lines.extend(
        [
            "        }\n",
            "    }\n\n",
            "    pub fn supports_target(\n",
            "        self,\n",
            "        target: &str,\n",
            "        platform: &str,\n",
            "        architecture: &str,\n",
            "        python_major: i64,\n",
            "        python_minor: i64,\n",
            "    ) -> bool {\n",
            "        let python_version = match (python_major, python_minor) {\n",
        ]
    )
    for version in sorted(TARGET_PYTHON_VERSIONS):
        major, minor = version.split(".")
        lines.append(f"            ({major}, {minor}) => {_rust_string(version)},\n")
    lines.extend(
        [
            "            _ => return false,\n",
            "        };\n",
            "        SUPPORTED_EXECUTION_TARGETS.contains(&target)\n",
            "            && SUPPORTED_TARGET_PLATFORMS.contains(&platform)\n",
            "            && SUPPORTED_TARGET_ARCHITECTURES.contains(&architecture)\n",
            "            && (self.targets().is_empty() || self.targets().contains(&target))\n",
            "            && (self.platforms().is_empty() || self.platforms().contains(&platform))\n",
            "            && (self.architectures().is_empty() || self.architectures().contains(&architecture))\n",
            "            && (self.python_versions().is_empty() || self.python_versions().contains(&python_version))\n",
            "    }\n",
            "}\n\n",
            f"pub const DEFAULT_CAPABILITY_TIER: &str = {_rust_string(schema.default_tier)};\n",
            f"pub const MAXIMUM_BUILTIN_CAPABILITY_TIER: &str = {_rust_string(schema.maximum_builtin_tier)};\n",
            "\n",
        ]
    )
    for tier in schema.tiers:
        constant = _rust_constant(tier.name)
        lines.append(
            f"pub const TIER_{constant}: [&str; {len(tier.effective_grants)}] = [\n"
        )
        lines.extend(f"    {_rust_string(grant)},\n" for grant in tier.effective_grants)
        lines.append("];\n\n")
    lines.extend(
        [
            "pub fn grants_for_tier(name: &str) -> Option<&'static [&'static str]> {\n",
            "    match name {\n",
        ]
    )
    for tier in schema.tiers:
        lines.append(
            f"        {_rust_string(tier.name)} => Some(&TIER_{_rust_constant(tier.name)}),\n"
        )
    lines.extend(["        _ => None,\n", "    }\n", "}\n\n"])
    lines.extend(
        [
            "pub fn minimum_tier_for(capability: &str) -> Option<&'static str> {\n",
        ]
    )
    for tier in schema.tiers:
        if not tier.effective_grants:
            continue
        lines.append(
            f"    if TIER_{_rust_constant(tier.name)}.contains(&capability) {{\n"
        )
        lines.append(f"        return Some({_rust_string(tier.name)});\n")
        lines.append("    }\n")
    lines.extend(["    None\n", "}\n"])
    lines.extend(
        [
            "\n#[cfg(test)]\n",
            "mod tests {\n",
            "    use super::*;\n\n",
            "    #[test]\n",
            "    fn explicit_policy_tier_has_no_ambient_grants() {\n",
            f"        assert_eq!(grants_for_tier({_rust_string(schema.explicit_policy_tier)}), Some(&[][..]));\n",
            '        assert!(grants_for_tier("unknown").is_none());\n',
            "    }\n\n",
            "    #[test]\n",
            "    fn tiers_are_monotone() {\n",
        ]
    )
    for parent, child in zip(schema.tiers, schema.tiers[1:]):
        lines.append(
            f"        for capability in TIER_{_rust_constant(parent.name)} {{\n"
        )
        lines.append(
            f"            assert!(TIER_{_rust_constant(child.name)}.contains(&capability));\n"
        )
        lines.append("        }\n")
    lines.extend(["    }\n", "}\n"])
    return "".join(lines)


def render_javascript(schema: Schema) -> str:
    lines = [
        "// @generated by tools/gen_host_capabilities.py from\n",
        "// runtime/host_capabilities.toml. DO NOT EDIT.\n\n",
        "export const CapabilityId = Object.freeze({\n",
    ]
    lines.extend(
        f"  {_projection_name(capability)}: {capability!r},\n"
        for capability in schema.capabilities
    )
    lines.extend(["});\n", "export const OperationId = Object.freeze({\n"])
    lines.extend(
        f"  {_projection_name(operation.name)}: {operation.name!r},\n"
        for operation in schema.operations
    )
    lines.extend(["});\n", "export const OPERATION_CAPABILITIES = Object.freeze({\n"])
    for operation in schema.operations:
        capabilities = ", ".join(repr(item) for item in operation.capabilities)
        lines.append(f"  [{operation.name!r}]: Object.freeze([{capabilities}]),\n")
    lines.extend(
        [
            "});\n",
            "export const OPERATION_TARGETS = Object.freeze({\n",
        ]
    )
    for operation in schema.operations:
        if operation.targets:
            values = ", ".join(repr(item) for item in operation.targets)
            lines.append(f"  [{operation.name!r}]: Object.freeze([{values}]),\n")
    lines.extend(
        [
            "});\n",
            "export const OPERATION_PLATFORMS = Object.freeze({\n",
        ]
    )
    for operation in schema.operations:
        if operation.platforms:
            values = ", ".join(repr(item) for item in operation.platforms)
            lines.append(f"  [{operation.name!r}]: Object.freeze([{values}]),\n")
    lines.extend(
        [
            "});\n",
            "export const OPERATION_ARCHITECTURES = Object.freeze({\n",
        ]
    )
    for operation in schema.operations:
        if operation.architectures:
            values = ", ".join(repr(item) for item in operation.architectures)
            lines.append(f"  [{operation.name!r}]: Object.freeze([{values}]),\n")
    lines.extend(
        [
            "});\n",
            "export const OPERATION_PYTHON_VERSIONS = Object.freeze({\n",
        ]
    )
    for operation in schema.operations:
        if operation.python_versions:
            values = ", ".join(repr(item) for item in operation.python_versions)
            lines.append(f"  [{operation.name!r}]: Object.freeze([{values}]),\n")
    lines.extend(
        [
            "});\n",
            f"export const SUPPORTED_EXECUTION_TARGETS = Object.freeze({list(sorted(EXECUTION_TARGETS))!r});\n",
            f"export const SUPPORTED_TARGET_PLATFORMS = Object.freeze({list(sorted(TARGET_PLATFORMS))!r});\n",
            f"export const SUPPORTED_TARGET_ARCHITECTURES = Object.freeze({list(sorted(TARGET_ARCHITECTURES))!r});\n",
            f"export const SUPPORTED_TARGET_PYTHON_VERSIONS = Object.freeze({list(sorted(TARGET_PYTHON_VERSIONS))!r});\n",
            "export function operationSupportsTarget(operation, { target, platform, architecture, pythonVersion }) {\n",
            "  if (!SUPPORTED_EXECUTION_TARGETS.includes(target)\n",
            "      || !SUPPORTED_TARGET_PLATFORMS.includes(platform)\n",
            "      || !SUPPORTED_TARGET_ARCHITECTURES.includes(architecture)\n",
            "      || !SUPPORTED_TARGET_PYTHON_VERSIONS.includes(pythonVersion)) return false;\n",
            "  const targets = OPERATION_TARGETS[operation] || [];\n",
            "  const platforms = OPERATION_PLATFORMS[operation] || [];\n",
            "  const architectures = OPERATION_ARCHITECTURES[operation] || [];\n",
            "  const pythonVersions = OPERATION_PYTHON_VERSIONS[operation] || [];\n",
            "  return (!targets.length || targets.includes(target))\n",
            "    && (!platforms.length || platforms.includes(platform))\n",
            "    && (!architectures.length || architectures.includes(architecture))\n",
            "    && (!pythonVersions.length || pythonVersions.includes(pythonVersion));\n",
            "}\n",
            f"export const DEFAULT_CAPABILITY_TIER = {schema.default_tier!r};\n",
            f"export const EXPLICIT_CAPABILITY_TIER = {schema.explicit_policy_tier!r};\n",
            f"export const MAXIMUM_BUILTIN_CAPABILITY_TIER = {schema.maximum_builtin_tier!r};\n",
            "export const CAPABILITY_TIERS = Object.freeze({\n",
        ]
    )
    for tier in schema.tiers:
        grants = ", ".join(repr(grant) for grant in tier.effective_grants)
        lines.append(f"  {tier.name!r}: Object.freeze([{grants}]),\n")
    lines.append("});\n")
    return "".join(lines)


def render_markdown(schema: Schema) -> str:
    minimum_tier: dict[str, str] = {}
    for tier in schema.tiers:
        for capability in tier.effective_grants:
            minimum_tier.setdefault(capability, tier.name)
    lines = [
        "<!-- @generated by tools/gen_host_capabilities.py from "
        "runtime/host_capabilities.toml. DO NOT EDIT. -->\n\n",
        "# Host capability registry\n\n",
        "This is the generated human projection of the canonical built-in host "
        "permission schema. Empty target, platform, architecture, or Python-version cells mean "
        "the operation is not restricted on that axis.\n\n",
        "## Profiles\n\n",
        "| Profile | Exact grants |\n",
        "| --- | --- |\n",
    ]
    for profile in schema.profiles:
        grants = ", ".join(f"`{grant}`" for grant in profile.grants) or "none"
        lines.append(f"| `{profile.name}` | {grants} |\n")
    lines.extend(
        [
            "\n## Built-in capabilities\n\n",
            "| Capability | Minimum tier |\n",
            "| --- | --- |\n",
        ]
    )
    for capability in schema.capabilities:
        lines.append(
            f"| `{capability}` | `{minimum_tier.get(capability, schema.maximum_builtin_tier)}` |\n"
        )
    lines.extend(
        [
            "\n## Audited operations\n\n",
            "| Operation | Required capabilities | Targets | Platforms | Architectures | CPython versions |\n",
            "| --- | --- | --- | --- | --- | --- |\n",
        ]
    )
    for operation in schema.operations:
        capabilities = ", ".join(
            f"`{capability}`" for capability in operation.capabilities
        )
        targets = ", ".join(f"`{value}`" for value in operation.targets) or "all"
        platforms = ", ".join(f"`{value}`" for value in operation.platforms) or "all"
        architectures = (
            ", ".join(f"`{value}`" for value in operation.architectures) or "all"
        )
        python_versions = (
            ", ".join(f"`{value}`" for value in operation.python_versions) or "all"
        )
        lines.append(
            f"| `{operation.name}` | {capabilities} | {targets} | {platforms} | {architectures} | {python_versions} |\n"
        )
    return "".join(lines)


def _format_python(source: str) -> str:
    completed = _COMMANDS.run(
        [
            sys.executable,
            "-m",
            "ruff",
            "format",
            "-",
            "--stdin-filename",
            str(OUT_PYTHON),
        ],
        cwd=ROOT,
        input=source,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"ruff format failed:\n{completed.stderr}")
    return completed.stdout


def _format_rust(source: str) -> str:
    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        raise RuntimeError("rustfmt is required to generate host capabilities")
    completed = _COMMANDS.run(
        [rustfmt, "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"rustfmt failed:\n{completed.stderr}")
    return completed.stdout


def render_all(schema: Schema) -> dict[Path, str]:
    return {
        OUT_PYTHON: _format_python(render_python(schema)),
        OUT_RUST: _format_rust(render_rust(schema)),
        OUT_JAVASCRIPT: render_javascript(schema),
        OUT_MARKDOWN: render_markdown(schema),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="fail if generated outputs are stale"
    )
    args = parser.parse_args(argv)
    stale = False
    for path, source in render_all(load_schema()).items():
        if args.check:
            if not generated_file_matches(path, source):
                print(
                    f"STALE generated file: {path.relative_to(ROOT)}", file=sys.stderr
                )
                stale = True
        else:
            write_generated_text(path, source)
    return int(stale)


if __name__ == "__main__":
    raise SystemExit(main())
