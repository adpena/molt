"""Canonical capability-policy algebra shared by manifests and CLI adapters.

This module owns capability profiles, token validation, package scoping, and
allow/deny resolution.  It is deliberately independent of file formats,
environment access, runtime intrinsics, and artifact-link facts so every
consumer applies the same policy without acquiring ambient authority.
"""

from __future__ import annotations

import hashlib
import json
import re
import tempfile
import tomllib
from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, TypeAlias, cast

from molt._host_capabilities_generated import CAPABILITY_PROFILES

CAPABILITY_TOKEN_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
CapabilityInput: TypeAlias = str | list[str] | dict[str, Any]


@dataclass(frozen=True)
class CapabilityInputResolution:
    """One immutable parse-and-resolution result for a CLI policy input."""

    capabilities: tuple[str, ...] | None
    profiles: tuple[str, ...]
    source: str | None
    errors: tuple[str, ...]
    policy: CapabilityPolicy | None
    resolution: CapabilityResolution | None


@dataclass
class PackageCapabilities:
    """Capability restrictions for one package.

    ``None`` means the field was omitted and therefore inherits the global
    policy.  An explicit empty list means the package receives none of that
    field.  Keeping those states distinct is required for default-deny policy.
    """

    name: str = ""
    allow: list[str] | None = None
    deny: list[str] = field(default_factory=list)
    effects: list[str] | None = None

    def merged(self, other: PackageCapabilities) -> PackageCapabilities:
        if self.name and other.name and self.name != other.name:
            raise ValueError(
                "cannot merge package capability policies for "
                f"{self.name!r} and {other.name!r}"
            )
        return PackageCapabilities(
            name=self.name or other.name,
            allow=merge_optional_capability_lists(self.allow, other.allow),
            deny=dedupe_preserve_order([*self.deny, *other.deny]),
            effects=merge_optional_capability_lists(self.effects, other.effects),
        )


@dataclass
class CapabilityPolicy:
    """Format-independent capability grants and package restrictions."""

    allow: list[str] | None = None
    deny: list[str] = field(default_factory=list)
    effects: list[str] | None = None
    packages: dict[str, PackageCapabilities] = field(default_factory=dict)


@dataclass(frozen=True)
class ResolvedPackagePolicy:
    """Immutable effective policy for one package scope."""

    name: str
    capabilities: tuple[str, ...]
    effects: tuple[str, ...] | None


@dataclass(frozen=True)
class CapabilityResolution:
    """Immutable effective grants and package attenuation."""

    capabilities: tuple[str, ...]
    profiles: tuple[str, ...]
    effects: tuple[str, ...] | None
    packages: tuple[ResolvedPackagePolicy, ...]
    errors: tuple[str, ...]

    def canonical_payload(self) -> dict[str, object]:
        """Return the order-independent semantic policy projection."""

        if self.errors:
            raise ValueError("cannot serialize invalid capability policy")
        return {
            "schema": "molt.resolved-capability-grants.v1",
            "capabilities": sorted(self.capabilities),
            "effects": None if self.effects is None else sorted(self.effects),
            "packages": {
                package.name: {
                    "capabilities": sorted(package.capabilities),
                    "effects": (
                        None if package.effects is None else sorted(package.effects)
                    ),
                }
                for package in sorted(self.packages, key=lambda item: item.name)
            },
        }

    def digest(self) -> str:
        canonical = json.dumps(
            self.canonical_payload(),
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        return f"sha256:{hashlib.sha256(canonical).hexdigest()}"


def dedupe_preserve_order(items: Iterable[str]) -> list[str]:
    """Return the first occurrence of each string in deterministic order."""

    return list(dict.fromkeys(items))


def split_capability_tokens(value: str) -> list[str]:
    """Split the CLI's comma-or-whitespace capability syntax."""

    return [token for token in re.split(r"[,\s]+", value) if token]


def merge_optional_capability_lists(
    left: list[str] | None,
    right: list[str] | None,
) -> list[str] | None:
    if left is None:
        return right
    if right is None:
        return left
    return dedupe_preserve_order([*left, *right])


def _coerce_token_list(
    value: Any,
    field_name: str,
    errors: list[str],
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if isinstance(value, list):
        tokens: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    tokens.append(stripped)
            else:
                errors.append(f"{field_name} entries must be strings")
        return tokens, True
    if isinstance(value, str):
        return split_capability_tokens(value), True
    errors.append(f"{field_name} must be a list or string")
    return [], True


def _fs_entry_enabled(
    value: Any,
    field_name: str,
    errors: list[str],
) -> tuple[bool, bool]:
    if value is None:
        return False, False
    if isinstance(value, bool):
        return True, value
    if isinstance(value, str):
        return True, bool(value.strip())
    if isinstance(value, list):
        for entry in value:
            if not isinstance(entry, str):
                errors.append(f"{field_name} entries must be strings")
        return True, bool(value)
    errors.append(f"{field_name} must be a list, string, or bool")
    return True, False


def _parse_fs_block(
    value: Any,
    field_name: str,
    errors: list[str],
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if not isinstance(value, dict):
        errors.append(f"{field_name} must be a table")
        return [], True
    allow: list[str] = []
    for key, capability in (("read", "fs.read"), ("write", "fs.write")):
        present, enabled = _fs_entry_enabled(
            value.get(key), f"{field_name}.{key}", errors
        )
        if present and enabled:
            allow.append(capability)
    return allow, True


def _parse_package_capabilities(
    value: Any,
    field_name: str,
    errors: list[str],
    *,
    package_name: str,
) -> PackageCapabilities:
    if value is None:
        return PackageCapabilities(name=package_name)
    if isinstance(value, (list, str)):
        allow, _present = _coerce_token_list(value, f"{field_name}.allow", errors)
        return PackageCapabilities(
            name=package_name,
            allow=dedupe_preserve_order(allow),
        )
    if not isinstance(value, dict):
        errors.append(f"{field_name} must be a list, string, or table")
        return PackageCapabilities(name=package_name)

    allow_tokens, allow_present = _coerce_token_list(
        value.get("allow"), f"{field_name}.allow", errors
    )
    caps_value = value.get("capabilities")
    caps_tokens: list[str] = []
    caps_present = False
    if isinstance(caps_value, dict):
        nested = _parse_package_capabilities(
            caps_value,
            f"{field_name}.capabilities",
            errors,
            package_name=package_name,
        )
        allow_tokens = dedupe_preserve_order(allow_tokens + (nested.allow or []))
        allow_present = True
        if nested.deny:
            errors.append(f"{field_name}.capabilities must not include deny entries")
        if nested.effects is not None:
            errors.append(f"{field_name}.capabilities must not include effects entries")
    else:
        caps_tokens, caps_present = _coerce_token_list(
            caps_value, f"{field_name}.capabilities", errors
        )
    deny_tokens, _deny_present = _coerce_token_list(
        value.get("deny"), f"{field_name}.deny", errors
    )
    effects_tokens, effects_present = _coerce_token_list(
        value.get("effects"), f"{field_name}.effects", errors
    )
    fs_tokens, fs_present = _parse_fs_block(value.get("fs"), f"{field_name}.fs", errors)

    combined_allow: list[str] = []
    if allow_present:
        combined_allow.extend(allow_tokens)
    if caps_present:
        combined_allow.extend(caps_tokens)
    if fs_present:
        combined_allow.extend(fs_tokens)
    allow = (
        dedupe_preserve_order(combined_allow)
        if allow_present or caps_present or fs_present
        else None
    )
    effects = dedupe_preserve_order(effects_tokens) if effects_present else None
    return PackageCapabilities(
        name=package_name,
        allow=allow,
        deny=dedupe_preserve_order(deny_tokens),
        effects=effects,
    )


def _parse_package_map(
    value: Any,
    field_name: str,
    errors: list[str],
) -> dict[str, PackageCapabilities]:
    packages: dict[str, PackageCapabilities] = {}
    if value is None:
        return packages
    if isinstance(value, dict):
        for name, entry in value.items():
            if not isinstance(name, str) or not name:
                errors.append(f"{field_name} entries must be keyed by package name")
                continue
            grant = _parse_package_capabilities(
                entry,
                f"{field_name}.{name}",
                errors,
                package_name=name,
            )
            packages[name] = packages[name].merged(grant) if name in packages else grant
        return packages
    if isinstance(value, list):
        for index, entry in enumerate(value):
            if not isinstance(entry, dict):
                errors.append(f"{field_name}[{index}] must be a table")
                continue
            entry_map = cast(Mapping[str, Any], entry)
            name = entry_map.get("name") or entry_map.get("package")
            if not isinstance(name, str) or not name:
                errors.append(f"{field_name}[{index}].name must be a non-empty string")
                continue
            grant = _parse_package_capabilities(
                entry,
                f"{field_name}.{name}",
                errors,
                package_name=name,
            )
            packages[name] = packages[name].merged(grant) if name in packages else grant
        return packages
    errors.append(f"{field_name} must be a table or list")
    return packages


def parse_capability_policy(
    data: Any,
    field_name: str = "capabilities",
) -> tuple[CapabilityPolicy | None, list[str]]:
    """Parse all supported config layouts into one policy representation."""

    errors: list[str] = []
    if not isinstance(data, dict):
        return None, [f"{field_name} must be a table"]

    allow: list[str] | None = None
    deny: list[str] = []
    effects: list[str] | None = None
    packages: dict[str, PackageCapabilities] = {}

    def apply_section(section: Any, context: str) -> None:
        nonlocal allow, deny, effects, packages
        if not isinstance(section, dict):
            errors.append(f"{context} must be a table")
            return
        caps_value = section.get("capabilities")
        if isinstance(caps_value, dict):
            apply_section(caps_value, f"{context}.capabilities")
            caps_value = None

        allow_tokens, allow_present = _coerce_token_list(
            section.get("allow"), f"{context}.allow", errors
        )
        caps_tokens: list[str] = []
        caps_present = False
        if caps_value is not None:
            caps_tokens, caps_present = _coerce_token_list(
                caps_value, f"{context}.capabilities", errors
            )
        fs_tokens, fs_present = _parse_fs_block(
            section.get("fs"), f"{context}.fs", errors
        )
        combined_allow = [*allow_tokens, *caps_tokens, *fs_tokens]
        if allow_present or caps_present or fs_present:
            allow = merge_optional_capability_lists(allow, combined_allow)

        deny_tokens, deny_present = _coerce_token_list(
            section.get("deny"), f"{context}.deny", errors
        )
        if deny_present:
            deny = dedupe_preserve_order([*deny, *deny_tokens])

        effect_tokens, effects_present = _coerce_token_list(
            section.get("effects"), f"{context}.effects", errors
        )
        if effects_present:
            effects = merge_optional_capability_lists(effects, effect_tokens)

        package_entries = _parse_package_map(
            section.get("packages"), f"{context}.packages", errors
        )
        for name, grant in package_entries.items():
            packages[name] = packages[name].merged(grant) if name in packages else grant

    apply_section(data, field_name)
    molt_section = data.get("molt")
    if isinstance(molt_section, dict):
        apply_section(molt_section, f"{field_name}.molt")
    tool_section = data.get("tool")
    if isinstance(tool_section, dict):
        tool_molt = tool_section.get("molt")
        if isinstance(tool_molt, dict):
            apply_section(tool_molt, f"{field_name}.tool.molt")

    return (
        CapabilityPolicy(
            allow=allow,
            deny=deny,
            effects=effects,
            packages=packages,
        ),
        errors,
    )


def expand_capabilities(items: Iterable[str]) -> tuple[list[str], list[str]]:
    """Expand named profiles while preserving deterministic first-use order."""

    expanded: list[str] = []
    profiles: list[str] = []
    for item in items:
        key = item.strip()
        if not key:
            continue
        profile = CAPABILITY_PROFILES.get(key)
        if profile is None:
            expanded.append(key)
        else:
            profiles.append(key)
            expanded.extend(profile)
    return dedupe_preserve_order(expanded), dedupe_preserve_order(profiles)


def _validate_tokens(
    tokens: Iterable[str],
    field_name: str,
    errors: list[str],
) -> None:
    for token in tokens:
        if not CAPABILITY_TOKEN_RE.fullmatch(token):
            errors.append(f"invalid capability token in {field_name}: {token}")


def resolve_capability_policy(policy: CapabilityPolicy) -> CapabilityResolution:
    """Validate and resolve a policy without consulting runtime state."""

    errors: list[str] = []
    allow_expanded, allow_profiles = expand_capabilities(policy.allow or ())
    deny_expanded, deny_profiles = expand_capabilities(policy.deny)
    profiles = dedupe_preserve_order([*allow_profiles, *deny_profiles])
    _validate_tokens(allow_expanded, "allow", errors)
    _validate_tokens(deny_expanded, "deny", errors)
    denied = set(deny_expanded)
    resolved = [capability for capability in allow_expanded if capability not in denied]

    if policy.effects is not None:
        _validate_tokens(policy.effects, "effects", errors)
    allowed_globally = set(resolved)
    effective_effects = (
        tuple(dedupe_preserve_order(policy.effects))
        if policy.effects is not None
        else None
    )
    allowed_effects = set(effective_effects) if effective_effects is not None else None
    resolved_packages: list[ResolvedPackagePolicy] = []
    for name, grant in policy.packages.items():
        package_allow, package_allow_profiles = expand_capabilities(grant.allow or ())
        package_deny, package_deny_profiles = expand_capabilities(grant.deny)
        profiles = dedupe_preserve_order(
            [*profiles, *package_allow_profiles, *package_deny_profiles]
        )
        _validate_tokens(package_allow, f"packages.{name}.allow", errors)
        _validate_tokens(package_deny, f"packages.{name}.deny", errors)
        if grant.allow is not None:
            extras = [cap for cap in package_allow if cap not in allowed_globally]
            if extras:
                errors.append(
                    f"packages.{name}.allow includes capabilities not in global "
                    f"allowlist: {', '.join(extras)}"
                )
        if grant.effects is not None:
            _validate_tokens(grant.effects, f"packages.{name}.effects", errors)
            if allowed_effects is not None:
                extras = [
                    effect for effect in grant.effects if effect not in allowed_effects
                ]
                if extras:
                    errors.append(
                        f"packages.{name}.effects includes effects not in global "
                        f"effects allowlist: {', '.join(dedupe_preserve_order(extras))}"
                    )

        package_denied = set(package_deny)
        if grant.allow is None:
            package_capabilities = [
                capability
                for capability in resolved
                if capability not in package_denied
            ]
        else:
            package_allowed = set(package_allow)
            package_capabilities = [
                capability
                for capability in resolved
                if capability in package_allowed and capability not in package_denied
            ]
        if grant.effects is None:
            package_effects = effective_effects
        else:
            package_effect_set = set(grant.effects)
            package_effects = tuple(
                effect
                for effect in (effective_effects or tuple(grant.effects))
                if effect in package_effect_set
            )
        resolved_packages.append(
            ResolvedPackagePolicy(
                name=name,
                capabilities=tuple(package_capabilities),
                effects=package_effects,
            )
        )

    return CapabilityResolution(
        capabilities=tuple(resolved),
        profiles=tuple(profiles),
        effects=effective_effects,
        packages=tuple(resolved_packages),
        errors=tuple(errors),
    )


def merge_capability_policies(
    *policies: CapabilityPolicy | None,
) -> CapabilityPolicy | None:
    """Compose explicit grant sources without flattening policy structure.

    Allow/effect declarations are positive grants and therefore accumulate;
    deny declarations always accumulate and are applied after profile
    expansion. Package policies retain their own allow/deny/effect scopes.
    """

    merged: CapabilityPolicy | None = None
    for policy in policies:
        if policy is None:
            continue
        if merged is None:
            merged = CapabilityPolicy(
                allow=None if policy.allow is None else list(policy.allow),
                deny=list(policy.deny),
                effects=None if policy.effects is None else list(policy.effects),
                packages={
                    name: PackageCapabilities(
                        name=grant.name,
                        allow=None if grant.allow is None else list(grant.allow),
                        deny=list(grant.deny),
                        effects=(
                            None if grant.effects is None else list(grant.effects)
                        ),
                    )
                    for name, grant in policy.packages.items()
                },
            )
            continue
        merged.allow = merge_optional_capability_lists(merged.allow, policy.allow)
        merged.deny = dedupe_preserve_order([*merged.deny, *policy.deny])
        merged.effects = merge_optional_capability_lists(merged.effects, policy.effects)
        for name, grant in policy.packages.items():
            merged.packages[name] = (
                merged.packages[name].merged(grant)
                if name in merged.packages
                else PackageCapabilities(
                    name=grant.name,
                    allow=None if grant.allow is None else list(grant.allow),
                    deny=list(grant.deny),
                    effects=None if grant.effects is None else list(grant.effects),
                )
            )
    return merged


def allowed_capabilities_for_package(
    global_allow: Iterable[str],
    policy: CapabilityPolicy | None,
    package_name: str | None,
) -> set[str]:
    """Project global grants through an optional package restriction."""

    allowed = set(global_allow)
    if policy is None or not package_name:
        return allowed
    grant = policy.packages.get(package_name)
    if grant is None:
        return allowed
    if grant.allow is not None:
        grant_allow, _profiles = expand_capabilities(grant.allow)
        allowed &= set(grant_allow)
    if grant.deny:
        grant_deny, _profiles = expand_capabilities(grant.deny)
        allowed -= set(grant_deny)
    return allowed


def allowed_effects_for_package(
    policy: CapabilityPolicy | None,
    package_name: str | None,
) -> set[str] | None:
    """Return the package's effective effect allowlist, if constrained."""

    if policy is None:
        return None
    allowed = set(policy.effects) if policy.effects is not None else None
    grant = policy.packages.get(package_name) if package_name else None
    if grant is None or grant.effects is None:
        return allowed
    grant_effects = set(grant.effects)
    return grant_effects if allowed is None else allowed & grant_effects


def parse_capability_input(
    value: CapabilityInput | None,
) -> CapabilityInputResolution:
    """Load and resolve one CLI capability input through this authority."""

    if value is None:
        return CapabilityInputResolution(None, (), None, (), None, None)

    source: str
    policy_data: object
    if isinstance(value, dict):
        source = "config"
        policy_data = value
    elif isinstance(value, list):
        source = "config"
        policy_data = {"allow": value}
    elif not value.strip():
        source = "inline"
        policy_data = {"allow": []}
    else:
        path = Path(value)
        if path.exists():
            source = str(path)
            try:
                text = path.read_text(encoding="utf-8")
                policy_data = (
                    json.loads(text)
                    if path.suffix.casefold() == ".json"
                    else tomllib.loads(text)
                )
            except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError):
                return CapabilityInputResolution(
                    None,
                    (),
                    source,
                    ("failed to load capabilities file",),
                    None,
                    None,
                )
        else:
            source = "inline"
            policy_data = {"allow": split_capability_tokens(value)}

    policy, parse_errors = parse_capability_policy(policy_data)
    if policy is None:
        return CapabilityInputResolution(
            None, (), source, tuple(parse_errors), None, None
        )

    resolution = resolve_capability_policy(policy)
    errors = (*parse_errors, *resolution.errors)
    return CapabilityInputResolution(
        None if errors else resolution.capabilities,
        resolution.profiles,
        source,
        errors,
        policy,
        resolution,
    )


def format_capability_input(value: CapabilityInput | None) -> str:
    """Render a capability input deterministically for diagnostics."""

    if value is None:
        return "none"
    if isinstance(value, list):
        return ", ".join(value) if value else "(empty)"
    if isinstance(value, str):
        return value if value else "(empty)"
    return json.dumps(value, sort_keys=True)


def materialize_capability_input(
    value: CapabilityInput,
) -> tuple[str, Path | None]:
    """Materialize structured policy input for a child CLI invocation."""

    if isinstance(value, list):
        return ",".join(value), None
    if isinstance(value, str):
        return value, None
    handle = tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        suffix=".json",
        prefix="molt_capabilities_",
        delete=False,
    )
    try:
        json.dump(value, handle, sort_keys=True, indent=2)
        handle.write("\n")
        path = Path(handle.name)
    finally:
        handle.close()
    return str(path), path
