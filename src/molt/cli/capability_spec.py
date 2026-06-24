from __future__ import annotations

import json
import re
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, cast


CAPABILITY_PROFILES: dict[str, list[str]] = {
    "core": [],
    "fs": ["fs.read", "fs.write"],
    "env": ["env.read", "env.write"],
    "net": ["net", "websocket.connect", "websocket.listen"],
    "db": ["db.read", "db.write"],
    "time": ["time"],
    "random": ["random"],
}
CAPABILITY_TOKEN_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
CapabilityInput = str | list[str] | dict[str, Any]


@dataclass(frozen=True)
class CapabilityGrant:
    allow: list[str] | None
    deny: list[str]
    effects: list[str] | None

    def merged(self, other: "CapabilityGrant") -> "CapabilityGrant":
        allow = _merge_optional_list(self.allow, other.allow)
        deny = _dedupe_preserve_order([*self.deny, *other.deny])
        effects = _merge_optional_list(self.effects, other.effects)
        return CapabilityGrant(allow=allow, deny=deny, effects=effects)


@dataclass(frozen=True)
class CapabilityManifest:
    allow: list[str] | None
    deny: list[str]
    effects: list[str] | None
    packages: dict[str, CapabilityGrant]


@dataclass(frozen=True)
class CapabilitySpec:
    capabilities: list[str] | None
    profiles: list[str]
    source: str | None
    errors: list[str]
    manifest: CapabilityManifest | None


def _dedupe_preserve_order(items: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    deduped: list[str] = []
    for item in items:
        if item in seen:
            continue
        seen.add(item)
        deduped.append(item)
    return deduped


def _split_tokens(value: str) -> list[str]:
    return [token for token in re.split(r"[,\s]+", value) if token]


def _merge_optional_list(
    left: list[str] | None, right: list[str] | None
) -> list[str] | None:
    if left is None:
        return right
    if right is None:
        return left
    return _dedupe_preserve_order([*left, *right])


def _coerce_token_list(
    value: Any, field: str, errors: list[str]
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
                errors.append(f"{field} entries must be strings")
        return tokens, True
    if isinstance(value, str):
        return _split_tokens(value), True
    errors.append(f"{field} must be a list or string")
    return [], True


def _coerce_effects_list(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if isinstance(value, list):
        effects: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    effects.append(stripped)
            else:
                errors.append(f"{field} entries must be strings")
        return effects, True
    if isinstance(value, str):
        return _split_tokens(value), True
    errors.append(f"{field} must be a list or string")
    return [], True


def _fs_entry_enabled(value: Any, field: str, errors: list[str]) -> tuple[bool, bool]:
    if value is None:
        return False, False
    if isinstance(value, bool):
        return True, value
    if isinstance(value, str):
        return True, bool(value.strip())
    if isinstance(value, list):
        for entry in value:
            if not isinstance(entry, str):
                errors.append(f"{field} entries must be strings")
        return True, bool(value)
    errors.append(f"{field} must be a list, string, or bool")
    return True, False


def _parse_fs_block(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if not isinstance(value, dict):
        errors.append(f"{field} must be a table")
        return [], True
    allow: list[str] = []
    for key, capability in (("read", "fs.read"), ("write", "fs.write")):
        present, enabled = _fs_entry_enabled(value.get(key), f"{field}.{key}", errors)
        if present and enabled:
            allow.append(capability)
    return allow, True


def _parse_package_grant(value: Any, field: str, errors: list[str]) -> CapabilityGrant:
    if value is None:
        return CapabilityGrant(allow=None, deny=[], effects=None)
    if isinstance(value, (list, str)):
        allow, _present = _coerce_token_list(value, f"{field}.allow", errors)
        return CapabilityGrant(
            allow=_dedupe_preserve_order(allow), deny=[], effects=None
        )
    if not isinstance(value, dict):
        errors.append(f"{field} must be a list, string, or table")
        return CapabilityGrant(allow=None, deny=[], effects=None)
    allow_tokens, allow_present = _coerce_token_list(
        value.get("allow"), f"{field}.allow", errors
    )
    caps_value = value.get("capabilities")
    caps_tokens: list[str] = []
    caps_present = False
    if isinstance(caps_value, dict):
        nested = _parse_package_grant(caps_value, f"{field}.capabilities", errors)
        allow_tokens = _dedupe_preserve_order(allow_tokens + (nested.allow or []))
        allow_present = True
        if nested.deny:
            errors.append(f"{field}.capabilities must not include deny entries")
        if nested.effects is not None:
            errors.append(f"{field}.capabilities must not include effects entries")
    else:
        caps_tokens, caps_present = _coerce_token_list(
            caps_value, f"{field}.capabilities", errors
        )
    deny_tokens, _deny_present = _coerce_token_list(
        value.get("deny"), f"{field}.deny", errors
    )
    effects_tokens, effects_present = _coerce_effects_list(
        value.get("effects"), f"{field}.effects", errors
    )
    fs_tokens, fs_present = _parse_fs_block(value.get("fs"), f"{field}.fs", errors)
    combined_allow: list[str] = []
    if allow_present:
        combined_allow.extend(allow_tokens)
    if caps_present:
        combined_allow.extend(caps_tokens)
    if fs_present:
        combined_allow.extend(fs_tokens)
    allow = (
        _dedupe_preserve_order(combined_allow)
        if allow_present or caps_present or fs_present
        else None
    )
    effects = _dedupe_preserve_order(effects_tokens) if effects_present else None
    return CapabilityGrant(
        allow=allow,
        deny=_dedupe_preserve_order(deny_tokens),
        effects=effects,
    )


def _parse_package_grants(
    value: Any, field: str, errors: list[str]
) -> dict[str, CapabilityGrant]:
    packages: dict[str, CapabilityGrant] = {}
    if value is None:
        return packages
    if isinstance(value, dict):
        for name, entry in value.items():
            if not isinstance(name, str) or not name:
                errors.append(f"{field} entries must be keyed by package name")
                continue
            grant = _parse_package_grant(entry, f"{field}.{name}", errors)
            if name in packages:
                packages[name] = packages[name].merged(grant)
            else:
                packages[name] = grant
        return packages
    if isinstance(value, list):
        for idx, entry in enumerate(value):
            if not isinstance(entry, dict):
                errors.append(f"{field}[{idx}] must be a table")
                continue
            entry_map = cast(Mapping[str, Any], entry)
            name = entry_map.get("name") or entry_map.get("package")
            if not isinstance(name, str) or not name:
                errors.append(f"{field}[{idx}].name must be a non-empty string")
                continue
            grant = _parse_package_grant(entry, f"{field}.{name}", errors)
            if name in packages:
                packages[name] = packages[name].merged(grant)
            else:
                packages[name] = grant
        return packages
    errors.append(f"{field} must be a table or list")
    return packages


def _parse_capability_manifest_dict(
    data: Any, field: str, errors: list[str]
) -> CapabilityManifest | None:
    if not isinstance(data, dict):
        errors.append(f"{field} must be a table")
        return None
    allow: list[str] | None = None
    deny: list[str] = []
    effects: list[str] | None = None
    packages: dict[str, CapabilityGrant] = {}

    def apply_section(section: Any, ctx: str) -> None:
        nonlocal allow, deny, effects, packages
        if not isinstance(section, dict):
            errors.append(f"{ctx} must be a table")
            return
        caps_value = section.get("capabilities")
        if isinstance(caps_value, dict):
            apply_section(caps_value, f"{ctx}.capabilities")
            caps_value = None
        allow_tokens, allow_present = _coerce_token_list(
            section.get("allow"), f"{ctx}.allow", errors
        )
        caps_tokens: list[str] = []
        caps_present = False
        if caps_value is not None:
            caps_tokens, caps_present = _coerce_token_list(
                caps_value, f"{ctx}.capabilities", errors
            )
        fs_tokens, fs_present = _parse_fs_block(section.get("fs"), f"{ctx}.fs", errors)
        combined_allow: list[str] = []
        if allow_present:
            combined_allow.extend(allow_tokens)
        if caps_present:
            combined_allow.extend(caps_tokens)
        if fs_present:
            combined_allow.extend(fs_tokens)
        if allow_present or caps_present or fs_present:
            if allow is None:
                allow = _dedupe_preserve_order(combined_allow)
            else:
                allow = _dedupe_preserve_order([*allow, *combined_allow])
        deny_tokens, deny_present = _coerce_token_list(
            section.get("deny"), f"{ctx}.deny", errors
        )
        if deny_present:
            deny = _dedupe_preserve_order([*deny, *deny_tokens])
        effects_tokens, effects_present = _coerce_effects_list(
            section.get("effects"), f"{ctx}.effects", errors
        )
        if effects_present:
            if effects is None:
                effects = _dedupe_preserve_order(effects_tokens)
            else:
                effects = _dedupe_preserve_order([*effects, *effects_tokens])
        pkg_entries = _parse_package_grants(
            section.get("packages"), f"{ctx}.packages", errors
        )
        if pkg_entries:
            for name, grant in pkg_entries.items():
                if name in packages:
                    packages[name] = packages[name].merged(grant)
                else:
                    packages[name] = grant

    apply_section(data, field)
    molt_section = data.get("molt")
    if isinstance(molt_section, dict):
        apply_section(molt_section, f"{field}.molt")
    tool_section = data.get("tool")
    if isinstance(tool_section, dict):
        tool_molt = tool_section.get("molt")
        if isinstance(tool_molt, dict):
            apply_section(tool_molt, f"{field}.tool.molt")

    return CapabilityManifest(
        allow=allow,
        deny=deny,
        effects=effects,
        packages=packages,
    )


def _validate_capability_tokens(
    tokens: Iterable[str], field: str, errors: list[str]
) -> None:
    for cap in tokens:
        if not CAPABILITY_TOKEN_RE.match(cap):
            errors.append(f"invalid capability token in {field}: {cap}")


def _validate_effect_tokens(
    tokens: Iterable[str], field: str, errors: list[str]
) -> None:
    for effect in tokens:
        if not CAPABILITY_TOKEN_RE.match(effect):
            errors.append(f"invalid effect token in {field}: {effect}")


def _resolve_capability_manifest(
    manifest: CapabilityManifest,
) -> tuple[list[str], list[str], list[str]]:
    errors: list[str] = []
    allow_tokens = manifest.allow or []
    allow_expanded, allow_profiles = _expand_capabilities(allow_tokens)
    deny_expanded, deny_profiles = _expand_capabilities(manifest.deny)
    profiles = _dedupe_preserve_order([*allow_profiles, *deny_profiles])
    _validate_capability_tokens(allow_expanded, "allow", errors)
    _validate_capability_tokens(deny_expanded, "deny", errors)
    deny_set = set(deny_expanded)
    resolved = _dedupe_preserve_order(
        cap for cap in allow_expanded if cap not in deny_set
    )
    manifest_effects_set: set[str] | None = None
    if manifest.effects is not None:
        _validate_effect_tokens(manifest.effects, "effects", errors)
        manifest_effects_set = set(manifest.effects)
    global_allow = set(resolved)
    for name, grant in manifest.packages.items():
        pkg_allow_tokens = grant.allow or []
        pkg_allow_expanded, pkg_allow_profiles = _expand_capabilities(pkg_allow_tokens)
        pkg_deny_expanded, pkg_deny_profiles = _expand_capabilities(grant.deny)
        profiles = _dedupe_preserve_order(
            [*profiles, *pkg_allow_profiles, *pkg_deny_profiles]
        )
        _validate_capability_tokens(
            pkg_allow_expanded, f"packages.{name}.allow", errors
        )
        _validate_capability_tokens(pkg_deny_expanded, f"packages.{name}.deny", errors)
        if grant.allow is not None:
            extras = [
                cap
                for cap in _dedupe_preserve_order(pkg_allow_expanded)
                if cap not in global_allow
            ]
            if extras:
                errors.append(
                    "packages."
                    + name
                    + ".allow includes capabilities not in global allowlist: "
                    + ", ".join(extras)
                )
        if grant.effects is not None:
            _validate_effect_tokens(grant.effects, f"packages.{name}.effects", errors)
            if manifest_effects_set is not None:
                effect_extras = [
                    effect
                    for effect in _dedupe_preserve_order(grant.effects)
                    if effect not in manifest_effects_set
                ]
                if effect_extras:
                    errors.append(
                        "packages."
                        + name
                        + ".effects includes effects not in global effects allowlist: "
                        + ", ".join(effect_extras)
                    )
    return resolved, profiles, errors


def _parse_capabilities_spec(
    capabilities: CapabilityInput | None,
) -> CapabilitySpec:
    if capabilities is None:
        return CapabilitySpec(
            capabilities=None,
            profiles=[],
            source=None,
            errors=[],
            manifest=None,
        )
    errors: list[str] = []
    profiles: list[str] = []
    source: str | None = None
    manifest: CapabilityManifest | None = None
    if isinstance(capabilities, dict):
        source = "config"
        manifest = _parse_capability_manifest_dict(capabilities, "capabilities", errors)
    elif isinstance(capabilities, list):
        source = "config"
        tokens, _present = _coerce_token_list(capabilities, "capabilities", errors)
        manifest = CapabilityManifest(
            allow=_dedupe_preserve_order(tokens),
            deny=[],
            effects=None,
            packages={},
        )
    else:
        if isinstance(capabilities, str) and not capabilities.strip():
            source = "inline"
            manifest = CapabilityManifest(
                allow=[],
                deny=[],
                effects=None,
                packages={},
            )
            resolved, profiles, resolve_errors = _resolve_capability_manifest(manifest)
            if resolve_errors:
                return CapabilitySpec(
                    capabilities=None,
                    profiles=profiles,
                    source=None,
                    errors=resolve_errors,
                    manifest=manifest,
                )
            return CapabilitySpec(
                capabilities=resolved,
                profiles=profiles,
                source=source,
                errors=[],
                manifest=manifest,
            )
        path = Path(capabilities)
        if path.exists():
            source = str(path)
            try:
                if path.suffix == ".json":
                    data = json.loads(path.read_text())
                else:
                    data = tomllib.loads(path.read_text())
            except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError):
                return CapabilitySpec(
                    capabilities=None,
                    profiles=[],
                    source=source,
                    errors=["failed to load capabilities file"],
                    manifest=None,
                )
            manifest = _parse_capability_manifest_dict(data, "capabilities", errors)
        else:
            source = "inline"
            tokens = _split_tokens(capabilities)
            manifest = CapabilityManifest(
                allow=_dedupe_preserve_order(tokens),
                deny=[],
                effects=None,
                packages={},
            )
    if manifest is None:
        return CapabilitySpec(
            capabilities=None,
            profiles=profiles,
            source=source,
            errors=errors,
            manifest=None,
        )
    resolved, profiles, resolve_errors = _resolve_capability_manifest(manifest)
    errors.extend(resolve_errors)
    if errors:
        return CapabilitySpec(
            capabilities=None,
            profiles=profiles,
            source=source,
            errors=errors,
            manifest=manifest,
        )
    return CapabilitySpec(
        capabilities=resolved,
        profiles=profiles,
        source=source,
        errors=[],
        manifest=manifest,
    )


def _parse_capabilities(
    capabilities: CapabilityInput | None,
) -> tuple[list[str] | None, list[str], str | None, list[str]]:
    spec = _parse_capabilities_spec(capabilities)
    return spec.capabilities, spec.profiles, spec.source, spec.errors


def _format_capabilities_input(value: CapabilityInput | None) -> str:
    if value is None:
        return "none"
    if isinstance(value, list):
        return ", ".join(value) if value else "(empty)"
    if isinstance(value, str):
        return value if value else "(empty)"
    return json.dumps(value, sort_keys=True)


def _allowed_capabilities_for_package(
    global_allow: list[str],
    manifest: CapabilityManifest | None,
    package_name: str | None,
) -> set[str]:
    allowed = set(global_allow)
    if manifest is None or not package_name:
        return allowed
    grant = manifest.packages.get(package_name)
    if grant is None:
        return allowed
    if grant.allow is not None:
        grant_allow, _profiles = _expand_capabilities(grant.allow)
        allowed &= set(grant_allow)
    if grant.deny:
        grant_deny, _profiles = _expand_capabilities(grant.deny)
        allowed -= set(grant_deny)
    return allowed


def _allowed_effects_for_package(
    manifest: CapabilityManifest | None,
    package_name: str | None,
) -> set[str] | None:
    if manifest is None:
        return None
    allowed: set[str] | None = None
    if manifest.effects is not None:
        allowed = set(manifest.effects)
    grant = manifest.packages.get(package_name) if package_name else None
    if grant is None or grant.effects is None:
        return allowed
    grant_effects = set(grant.effects)
    if allowed is None:
        return grant_effects
    return allowed & grant_effects


def _materialize_capabilities_arg(
    capabilities: CapabilityInput,
) -> tuple[str, Path | None]:
    if isinstance(capabilities, list):
        return ",".join(capabilities), None
    if isinstance(capabilities, str):
        return capabilities, None
    handle = tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        suffix=".json",
        prefix="molt_capabilities_",
        delete=False,
    )
    try:
        json.dump(capabilities, handle, sort_keys=True, indent=2)
        handle.write("\n")
        path = Path(handle.name)
    finally:
        handle.close()
    return str(path), path


def _expand_capabilities(items: list[str]) -> tuple[list[str], list[str]]:
    expanded: list[str] = []
    profiles: list[str] = []
    for item in items:
        key = item.strip()
        if not key:
            continue
        profile = CAPABILITY_PROFILES.get(key)
        if profile is not None:
            profiles.append(key)
            expanded.extend(profile)
        else:
            expanded.append(key)
    # Preserve order while de-duplicating.
    seen: set[str] = set()
    deduped: list[str] = []
    for cap in expanded:
        if cap in seen:
            continue
        seen.add(cap)
        deduped.append(cap)
    return deduped, profiles
