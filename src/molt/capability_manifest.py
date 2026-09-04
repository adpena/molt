"""Molt Capability Manifest v2.0 -- TOML-based unified manifest parser.

Parses and validates capability manifests that unify:
  - Capability grants (allow/deny/effects, per-package scoping)
  - Resource limits (memory, duration, allocations, operation guards)
  - IO mode (real, virtual with VFS mounts, callback)
  - Audit configuration (sink type, output destination)
  - Deterministic host-policy propagation and integrity

Usage::

    from molt.capability_manifest import load_manifest
    manifest = load_manifest("molt.capabilities.toml")
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
import tomllib
import warnings
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, Mapping, Optional, Union, cast

from molt.capability_policy import (
    CapabilityPolicy,
    CapabilityResolution,
    PackageCapabilities,
    expand_capabilities,
    merge_capability_policies,
    parse_capability_policy,
    resolve_capability_policy,
)
from molt._host_capabilities_generated import (
    DEFAULT_CAPABILITY_TIER,
    EXPLICIT_CAPABILITY_TIER,
    capabilities_for_tier,
)

VALID_IO_MODES: frozenset[str] = frozenset({"real", "virtual", "callback"})
VALID_AUDIT_SINKS: frozenset[str] = frozenset({"null", "stderr", "jsonl"})
VALID_AUDIT_OUTPUTS: frozenset[str] = frozenset({"stderr", "stdout", "null"})
VALID_MOUNT_TYPES: frozenset[str] = frozenset({"memory", "readonly", "readwrite"})
AuditSink = Literal["null", "stderr", "jsonl"]
IoMode = Literal["real", "virtual", "callback"]


# ---------------------------------------------------------------------------
# Size / duration parsing
# ---------------------------------------------------------------------------

_SIZE_RE = re.compile(r"^\s*(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)\s*$", re.IGNORECASE)
_SIZE_MULTIPLIERS: dict[str, int] = {
    "B": 1,
    "KB": 1_024,
    "MB": 1_024 * 1_024,
    "GB": 1_024 * 1_024 * 1_024,
    "TB": 1_024 * 1_024 * 1_024 * 1_024,
}

_DURATION_RE = re.compile(r"^\s*(\d+(?:\.\d+)?)\s*(ms|s|m|h)\s*$", re.IGNORECASE)
_DURATION_MULTIPLIERS: dict[str, float] = {
    "ms": 0.001,
    "s": 1.0,
    "m": 60.0,
    "h": 3600.0,
}


def parse_size(s: str | int | float) -> int:
    """Parse a human-readable size string to bytes.

    Accepted formats: ``"64MB"``, ``"10KB"``, ``"1GB"``, ``"512B"``.
    Raises :class:`ValueError` with a clear message on invalid input.
    """
    if isinstance(s, (int, float)):
        return int(s)
    m = _SIZE_RE.match(s)
    if m is None:
        raise ValueError(
            f"invalid size string {s!r} -- expected format like '64MB', '10KB', '1GB'"
        )
    value = float(m.group(1))
    unit = m.group(2).upper()
    result = int(value * _SIZE_MULTIPLIERS[unit])
    if result < 0:
        raise ValueError(f"size must be non-negative, got {s!r}")
    return result


def parse_duration(s: str | int | float) -> float:
    """Parse a human-readable duration string to seconds.

    Accepted formats: ``"30s"``, ``"500ms"``, ``"2m"``, ``"1h"``.
    Raises :class:`ValueError` with a clear message on invalid input.
    """
    if isinstance(s, (int, float)):
        return float(s)
    m = _DURATION_RE.match(s)
    if m is None:
        raise ValueError(
            f"invalid duration string {s!r} -- expected format like '30s', '500ms', '2m'"
        )
    value = float(m.group(1))
    unit = m.group(2).lower()
    result = value * _DURATION_MULTIPLIERS[unit]
    if result < 0:
        raise ValueError(f"duration must be non-negative, got {s!r}")
    return result


# ---------------------------------------------------------------------------
# Dataclasses
# ---------------------------------------------------------------------------


@dataclass
class ResourceLimits:
    """Resource constraints enforced at the WASM host boundary."""

    max_memory: Optional[int] = None  # bytes
    max_duration: Optional[float] = None  # seconds
    max_allocations: Optional[int] = None
    max_recursion_depth: Optional[int] = None
    # Pre-emptive operation guards (bytes)
    max_pow_result: Optional[int] = None
    max_repeat_result: Optional[int] = None
    max_shift_result: Optional[int] = None
    max_string_result: Optional[int] = None


@dataclass
class VirtualMount:
    """A single virtual filesystem mount point."""

    path: str
    type: str  # "memory" | "readonly" | "readwrite"
    max_size: Optional[int] = None  # bytes, for memory mounts
    source: Optional[str] = None  # host path, for readonly/readwrite mounts


@dataclass
class AuditConfig:
    """Audit trail configuration."""

    enabled: bool = False
    sink: AuditSink = "null"
    output: str = "stderr"


@dataclass
class IoConfig:
    """IO mode and virtual filesystem mounts."""

    mode: IoMode = "real"
    virtual_mounts: list[VirtualMount] = field(default_factory=list)


@dataclass(frozen=True)
class ResolvedVirtualMount:
    """Immutable mount authority carried by a resolved runtime policy."""

    path: str
    type: str
    max_size: int | None
    source: str | None


@dataclass(frozen=True)
class ResolvedRuntimePolicy:
    """Immutable execution policy shared by caches, artifacts, and runtimes."""

    grants: CapabilityResolution
    tier: str
    max_memory: int | None
    max_duration_ms: int | None
    max_allocations: int | None
    max_recursion_depth: int | None
    operation_limits: tuple[tuple[str, int | None], ...]
    audit_enabled: bool
    audit_sink: str
    audit_output: str
    io_mode: str
    mounts: tuple[ResolvedVirtualMount, ...]

    def canonical_payload(self) -> dict[str, object]:
        return {
            "schema": "molt.resolved-runtime-policy.v1",
            "tier": self.tier,
            "grants": self.grants.canonical_payload(),
            "resources": {
                "max_memory": self.max_memory,
                "max_duration_ms": self.max_duration_ms,
                "max_allocations": self.max_allocations,
                "max_recursion_depth": self.max_recursion_depth,
                "operation_limits": dict(self.operation_limits),
            },
            "audit": {
                "enabled": self.audit_enabled,
                "sink": self.audit_sink,
                "output": self.audit_output,
            },
            "io": {
                "mode": self.io_mode,
                "mounts": [
                    {
                        "path": mount.path,
                        "type": mount.type,
                        "max_size": mount.max_size,
                        "source": mount.source,
                    }
                    for mount in self.mounts
                ],
            },
        }

    def digest(self) -> str:
        payload = json.dumps(
            self.canonical_payload(),
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        return f"sha256:{hashlib.sha256(payload).hexdigest()}"

    def to_env_vars(self) -> dict[str, str]:
        env = {
            "MOLT_CAPABILITIES": ",".join(sorted(self.grants.capabilities)),
            "MOLT_CAPABILITY_TIER": self.tier,
            "MOLT_CAPABILITY_POLICY_DIGEST": self.digest(),
        }
        for value, env_name in (
            (self.max_memory, "MOLT_RESOURCE_MAX_MEMORY"),
            (self.max_duration_ms, "MOLT_RESOURCE_MAX_DURATION_MS"),
            (self.max_allocations, "MOLT_RESOURCE_MAX_ALLOCATIONS"),
            (self.max_recursion_depth, "MOLT_RESOURCE_MAX_RECURSION_DEPTH"),
        ):
            if value is not None:
                env[env_name] = str(value)
        for field_name, value in self.operation_limits:
            if value is not None:
                env[f"MOLT_RESOURCE_{field_name.upper()}"] = str(value)
        if self.audit_enabled:
            env["MOLT_AUDIT_ENABLED"] = "1"
            env["MOLT_AUDIT_SINK"] = self.audit_sink
            env["MOLT_AUDIT_OUTPUT"] = self.audit_output
        if self.io_mode != "real":
            env["MOLT_IO_MODE"] = self.io_mode
        return env


@dataclass
class CapabilityManifest(CapabilityPolicy):
    """Complete parsed and validated capability manifest."""

    version: str = "2.0"
    description: str = ""
    # Sub-configs
    resources: ResourceLimits = field(default_factory=ResourceLimits)
    audit: AuditConfig = field(default_factory=AuditConfig)
    io: IoConfig = field(default_factory=IoConfig)
    signature: Optional[str] = None

    def expanded_allow(self) -> set[str]:
        """Return the full set of allowed capabilities after profile expansion."""
        expanded, _profiles = expand_capabilities(self.allow or ())
        return set(expanded)

    def effective_capabilities(self) -> set[str]:
        """Return allowed minus denied capabilities."""
        return set(resolve_capability_policy(self).capabilities)

    def resolve(
        self,
        policy: CapabilityPolicy | None = None,
        *,
        tier: str = EXPLICIT_CAPABILITY_TIER,
    ) -> ResolvedRuntimePolicy:
        """Resolve the complete manifest envelope into immutable runtime state."""

        validate_manifest(self)
        tier_grants = capabilities_for_tier(tier)
        if tier_grants is None:
            raise ManifestError(f"unknown capability tier {tier!r}")
        selected_policy = self if policy is None else policy
        combined_policy = merge_capability_policies(
            CapabilityPolicy(allow=list(tier_grants)), selected_policy
        )
        assert combined_policy is not None
        grants = resolve_capability_policy(combined_policy)
        if grants.errors:
            raise ManifestError("; ".join(grants.errors))
        mounts = tuple(
            ResolvedVirtualMount(
                path=mount.path,
                type=mount.type,
                max_size=mount.max_size,
                source=mount.source,
            )
            for mount in sorted(
                self.io.virtual_mounts,
                key=lambda item: (
                    item.path,
                    item.type,
                    item.source or "",
                    item.max_size if item.max_size is not None else -1,
                ),
            )
        )
        return ResolvedRuntimePolicy(
            grants=grants,
            tier=tier,
            max_memory=self.resources.max_memory,
            max_duration_ms=(
                None
                if self.resources.max_duration is None
                else int(self.resources.max_duration * 1000)
            ),
            max_allocations=self.resources.max_allocations,
            max_recursion_depth=self.resources.max_recursion_depth,
            operation_limits=(
                ("max_pow_result", self.resources.max_pow_result),
                ("max_repeat_result", self.resources.max_repeat_result),
                ("max_shift_result", self.resources.max_shift_result),
                ("max_string_result", self.resources.max_string_result),
            ),
            audit_enabled=self.audit.enabled,
            audit_sink=self.audit.sink,
            audit_output=self.audit.output,
            io_mode=self.io.mode,
            mounts=mounts,
        )

    def resolved_policy_payload(
        self,
        policy: CapabilityPolicy | None = None,
        *,
        tier: str = EXPLICIT_CAPABILITY_TIER,
    ) -> dict[str, object]:
        """Freeze every execution-relevant policy field into one projection."""

        return self.resolve(policy, tier=tier).canonical_payload()

    def resolved_policy_digest(
        self,
        policy: CapabilityPolicy | None = None,
        *,
        tier: str = EXPLICIT_CAPABILITY_TIER,
    ) -> str:
        return self.resolve(policy, tier=tier).digest()

    def to_env_vars(
        self,
        policy: CapabilityPolicy | None = None,
        *,
        tier: str = EXPLICIT_CAPABILITY_TIER,
    ) -> dict[str, str]:
        """Convert manifest to environment variables for runtime propagation."""
        return self.resolve(policy, tier=tier).to_env_vars()


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------


class ManifestError(Exception):
    """Raised when a manifest is structurally invalid."""


def resolve_runtime_policy_from_env(
    env: Mapping[str, str],
) -> ResolvedRuntimePolicy:
    """Canonicalize the embedding environment into one runtime policy."""

    tier = env.get("MOLT_CAPABILITY_TIER", DEFAULT_CAPABILITY_TIER).strip().casefold()
    explicit = [
        token.strip()
        for token in env.get("MOLT_CAPABILITIES", "").split(",")
        if token.strip()
    ]

    def optional_int(name: str) -> int | None:
        raw = env.get(name)
        if raw is None or not raw.strip():
            return None
        try:
            value = int(raw)
        except ValueError as exc:
            raise ManifestError(f"{name} must be an integer") from exc
        if value <= 0:
            raise ManifestError(f"{name} must be positive")
        return value

    raw_audit_enabled = env.get("MOLT_AUDIT_ENABLED", "").strip().casefold()
    if raw_audit_enabled in {"", "0", "false", "no", "off"}:
        audit_enabled = False
    elif raw_audit_enabled in {"1", "true", "yes", "on"}:
        audit_enabled = True
    else:
        raise ManifestError(
            "MOLT_AUDIT_ENABLED must be one of 0, 1, false, true, no, yes, off, on"
        )
    audit_sink = env.get("MOLT_AUDIT_SINK", "null")
    if audit_sink not in VALID_AUDIT_SINKS:
        raise ManifestError(f"invalid audit sink {audit_sink!r}")
    io_mode = env.get("MOLT_IO_MODE", "real")
    if io_mode not in VALID_IO_MODES:
        raise ManifestError(f"invalid I/O mode {io_mode!r}")
    duration_ms = optional_int("MOLT_RESOURCE_MAX_DURATION_MS")
    manifest = CapabilityManifest(
        allow=list(dict.fromkeys(explicit)),
        resources=ResourceLimits(
            max_memory=optional_int("MOLT_RESOURCE_MAX_MEMORY"),
            max_duration=None if duration_ms is None else duration_ms / 1000.0,
            max_allocations=optional_int("MOLT_RESOURCE_MAX_ALLOCATIONS"),
            max_recursion_depth=optional_int("MOLT_RESOURCE_MAX_RECURSION_DEPTH"),
            max_pow_result=optional_int("MOLT_RESOURCE_MAX_POW_RESULT"),
            max_repeat_result=optional_int("MOLT_RESOURCE_MAX_REPEAT_RESULT"),
            max_shift_result=optional_int("MOLT_RESOURCE_MAX_SHIFT_RESULT"),
            max_string_result=optional_int("MOLT_RESOURCE_MAX_STRING_RESULT"),
        ),
        audit=AuditConfig(
            enabled=audit_enabled,
            sink=cast(AuditSink, audit_sink),
            output=env.get("MOLT_AUDIT_OUTPUT", "stderr"),
        ),
        io=IoConfig(mode=cast(IoMode, io_mode)),
    )
    validate_manifest(manifest)
    return manifest.resolve(tier=tier)


def _require_exact_keys(
    data: dict[str, Any],
    allowed: frozenset[str],
    context: str,
) -> None:
    unknown = sorted(set(data) - allowed)
    if unknown:
        raise ManifestError(
            f"{context} contains unsupported field(s): {', '.join(unknown)}"
        )


def validate_manifest(manifest: CapabilityManifest) -> list[str]:
    """Validate a parsed manifest, returning a list of warnings.

    Raises :class:`ManifestError` for fatal structural issues.
    Returns a (possibly empty) list of non-fatal warnings.
    """
    warnings: list[str] = []

    # Version check
    if manifest.version not in ("1.0", "2.0"):
        raise ManifestError(
            f"unrecognized manifest version {manifest.version!r}; "
            f"this parser supports versions 1.0 and 2.0"
        )

    resolution = resolve_capability_policy(manifest)
    if resolution.errors:
        raise ManifestError("; ".join(resolution.errors))

    # Deny items that are not in allow are not actionable
    expanded = manifest.expanded_allow()
    denied, _profiles = expand_capabilities(manifest.deny)
    denied_not_allowed = set(denied) - expanded
    for cap in sorted(denied_not_allowed):
        warnings.append(
            f"capability {cap!r} is in deny but not in allow -- has no effect"
        )

    # Resource limits sanity
    rl = manifest.resources
    if rl.max_memory is not None and rl.max_memory <= 0:
        raise ManifestError(f"max_memory must be positive, got {rl.max_memory}")
    if rl.max_duration is not None and rl.max_duration <= 0:
        raise ManifestError(f"max_duration must be positive, got {rl.max_duration}")
    if rl.max_allocations is not None and rl.max_allocations <= 0:
        raise ManifestError(
            f"max_allocations must be positive, got {rl.max_allocations}"
        )
    if rl.max_recursion_depth is not None and rl.max_recursion_depth <= 0:
        raise ManifestError(
            f"max_recursion_depth must be positive, got {rl.max_recursion_depth}"
        )
    for op_field in (
        "max_pow_result",
        "max_repeat_result",
        "max_shift_result",
        "max_string_result",
    ):
        value = getattr(rl, op_field)
        if value is not None and value <= 0:
            raise ManifestError(f"{op_field} must be positive, got {value}")

    # IO mode
    if manifest.io.mode not in VALID_IO_MODES:
        raise ManifestError(
            f"invalid io.mode {manifest.io.mode!r}; "
            f"valid modes: {', '.join(sorted(VALID_IO_MODES))}"
        )
    if manifest.io.mode != "virtual" and manifest.io.virtual_mounts:
        warnings.append(
            "virtual_mounts are configured but io.mode is not 'virtual' "
            "-- mounts will be ignored"
        )

    # Audit
    if manifest.audit.sink not in VALID_AUDIT_SINKS:
        raise ManifestError(
            f"invalid audit.sink {manifest.audit.sink!r}; "
            f"valid sinks: {', '.join(sorted(VALID_AUDIT_SINKS))}"
        )
    if manifest.audit.output not in VALID_AUDIT_OUTPUTS:
        # If not a well-known output, treat as a file path and validate it.
        out_path = Path(manifest.audit.output)
        if ".." in out_path.parts:
            raise ManifestError(
                f"audit.output path contains '..' traversal: {manifest.audit.output!r}"
            )
        resolved_out = out_path.resolve()
        cwd = Path.cwd().resolve()
        if not resolved_out.is_relative_to(cwd):
            raise ManifestError(
                f"audit.output resolves to {str(resolved_out)!r} which is outside "
                f"the project directory {str(cwd)!r}"
            )
    if manifest.audit.enabled:
        if manifest.audit.sink == "stderr" and manifest.audit.output != "stderr":
            raise ManifestError(
                "audit.output must be 'stderr' when audit.sink is 'stderr'"
            )
        if manifest.audit.sink == "jsonl" and manifest.audit.output == "null":
            raise ManifestError(
                "audit.output cannot be 'null' when audit.sink is 'jsonl'"
            )

    return warnings


# ---------------------------------------------------------------------------
# TOML loading
# ---------------------------------------------------------------------------


def _parse_resources(data: dict[str, Any]) -> ResourceLimits:
    """Parse the [resources] table into a ResourceLimits dataclass."""
    _require_exact_keys(
        data,
        frozenset(
            {
                "max_memory",
                "max_duration",
                "max_allocations",
                "max_recursion_depth",
                "operation_limits",
            }
        ),
        "resources",
    )
    rl = ResourceLimits()

    if "max_memory" in data:
        rl.max_memory = parse_size(data["max_memory"])
    if "max_duration" in data:
        rl.max_duration = parse_duration(data["max_duration"])
    if "max_allocations" in data:
        v = data["max_allocations"]
        if not isinstance(v, int):
            raise ManifestError(
                f"max_allocations must be an integer, got {type(v).__name__}"
            )
        rl.max_allocations = v
    if "max_recursion_depth" in data:
        v = data["max_recursion_depth"]
        if not isinstance(v, int):
            raise ManifestError(
                f"max_recursion_depth must be an integer, got {type(v).__name__}"
            )
        rl.max_recursion_depth = v

    # Operation limits sub-table
    op = data.get("operation_limits", {})
    if not isinstance(op, dict):
        raise ManifestError("resources.operation_limits must be a table")
    _require_exact_keys(
        op,
        frozenset(
            {
                "max_pow_result",
                "max_repeat_result",
                "max_shift_result",
                "max_string_result",
            }
        ),
        "resources.operation_limits",
    )
    for op_field in (
        "max_pow_result",
        "max_repeat_result",
        "max_shift_result",
        "max_string_result",
    ):
        if op_field in op:
            setattr(rl, op_field, parse_size(op[op_field]))

    return rl


def _parse_virtual_mounts(data: dict[str, Any]) -> list[VirtualMount]:
    """Parse [io.virtual_mounts] into a list of VirtualMount objects."""
    mounts: list[VirtualMount] = []
    for mount_path, mount_cfg in data.items():
        if not isinstance(mount_cfg, dict):
            raise ManifestError(
                f"virtual mount {mount_path!r} must be a table, got {type(mount_cfg).__name__}"
            )
        _require_exact_keys(
            mount_cfg,
            frozenset({"type", "max_size", "source"}),
            f"io.virtual_mounts.{mount_path}",
        )
        mount_type = mount_cfg.get("type")
        if mount_type is None:
            raise ManifestError(
                f"virtual mount {mount_path!r} is missing required 'type' field"
            )
        if mount_type not in VALID_MOUNT_TYPES:
            raise ManifestError(
                f"virtual mount {mount_path!r} has invalid type {mount_type!r}; "
                f"valid types: {', '.join(sorted(VALID_MOUNT_TYPES))}"
            )
        vm = VirtualMount(path=mount_path, type=mount_type)
        if "max_size" in mount_cfg:
            vm.max_size = parse_size(mount_cfg["max_size"])
        if "source" in mount_cfg:
            raw_source = mount_cfg["source"]
            if ".." in Path(raw_source).parts:
                raise ManifestError(
                    f"virtual mount {mount_path!r} source contains '..' traversal"
                )
            # VFS-internal references (e.g. "/bundle/data") are allowed as-is.
            _VFS_PREFIXES = ("/bundle", "/tmp", "/state", "/dev")
            is_vfs_ref = any(
                raw_source == p or raw_source.startswith(p + "/") for p in _VFS_PREFIXES
            )
            if is_vfs_ref:
                vm.source = raw_source
            else:
                resolved = Path(raw_source).resolve()
                # Reject host paths outside the project tree.
                cwd = Path.cwd().resolve()
                if not resolved.is_relative_to(cwd):
                    raise ManifestError(
                        f"virtual mount {mount_path!r} source resolves to "
                        f"{str(resolved)!r} which is outside the project "
                        f"directory {str(cwd)!r}; use a relative path within "
                        f"the project"
                    )
                vm.source = str(resolved)
        # Validate: readonly/readwrite need source
        if mount_type in ("readonly", "readwrite") and vm.source is None:
            raise ManifestError(
                f"virtual mount {mount_path!r} of type {mount_type!r} requires a 'source' path"
            )
        mounts.append(vm)
    return mounts


def _parse_io(data: dict[str, Any]) -> IoConfig:
    """Parse the [io] table into an IoConfig dataclass."""
    _require_exact_keys(data, frozenset({"mode", "virtual_mounts"}), "io")
    io = IoConfig()
    if "mode" in data:
        io.mode = data["mode"]
    if "virtual_mounts" in data:
        io.virtual_mounts = _parse_virtual_mounts(data["virtual_mounts"])
    return io


def _parse_audit(data: dict[str, Any]) -> AuditConfig:
    """Parse the [audit] table into an AuditConfig dataclass."""
    _require_exact_keys(data, frozenset({"enabled", "sink", "output"}), "audit")
    return AuditConfig(
        enabled=data.get("enabled", False),
        sink=data.get("sink", "null"),
        output=data.get("output", "stderr"),
    )


def _parse_capabilities(
    data: dict[str, Any],
) -> tuple[
    list[str] | None,
    list[str],
    list[str] | None,
    dict[str, PackageCapabilities],
]:
    """Parse the manifest's policy section through the shared authority."""

    policy, errors = parse_capability_policy(data)
    if policy is None or errors:
        raise ManifestError("; ".join(errors or ["capabilities must be a table"]))
    return (
        policy.allow,
        list(policy.deny),
        policy.effects,
        policy.packages,
    )


def _parse_v2_dict(data: dict[str, Any]) -> CapabilityManifest:
    """Parse a v2.0 manifest from an already-loaded dict (TOML or YAML)."""
    _require_exact_keys(
        data,
        frozenset(
            {"manifest", "capabilities", "resources", "io", "audit", "signature"}
        ),
        "manifest root",
    )
    manifest_meta = data.get("manifest", {})
    if not isinstance(manifest_meta, dict):
        raise ManifestError("manifest must be a table")
    _require_exact_keys(
        manifest_meta,
        frozenset({"version", "description"}),
        "manifest",
    )
    version = manifest_meta.get("version", "2.0")
    description = manifest_meta.get("description", "")
    if version != "2.0":
        raise ManifestError(
            f"TOML/YAML capability manifests require version '2.0', got {version!r}"
        )
    if not isinstance(description, str):
        raise ManifestError("manifest.description must be a string")

    caps_data = data.get("capabilities", {})
    if not isinstance(caps_data, dict):
        raise ManifestError("capabilities must be a table")
    allow, deny, effects, packages = _parse_capabilities(caps_data)

    resources_data = data.get("resources", {})
    io_data = data.get("io", {})
    audit_data = data.get("audit", {})
    if not isinstance(resources_data, dict):
        raise ManifestError("resources must be a table")
    if not isinstance(io_data, dict):
        raise ManifestError("io must be a table")
    if not isinstance(audit_data, dict):
        raise ManifestError("audit must be a table")
    resources = _parse_resources(resources_data)
    io = _parse_io(io_data)
    audit = _parse_audit(audit_data)
    signature_raw = data.get("signature")
    signature: Optional[str] = None
    if isinstance(signature_raw, dict):
        _require_exact_keys(signature_raw, frozenset({"value"}), "signature")
        # TOML [signature] table -- extract the value key.
        signature = signature_raw.get("value")
    elif isinstance(signature_raw, str):
        # JSON/YAML scalar key.
        signature = signature_raw
    elif signature_raw is not None:
        raise ManifestError("signature must be a string or table")
    if signature is not None and not isinstance(signature, str):
        raise ManifestError("signature.value must be a string")

    return CapabilityManifest(
        version=version,
        description=description,
        allow=allow,
        deny=deny,
        effects=effects,
        packages=packages,
        resources=resources,
        audit=audit,
        io=io,
        signature=signature,
    )


def _parse_json_manifest(data: dict[str, Any]) -> CapabilityManifest:
    """Parse the v1 JSON envelope from an already-decoded snapshot."""
    version = data.get("version", "1.0")
    if version != "1.0":
        raise ManifestError(
            f"unrecognized manifest version {version!r}; JSON capability manifests require version '1.0'"
        )
    policy, errors = parse_capability_policy(data)
    if policy is None or errors:
        raise ManifestError("; ".join(errors or ["JSON manifest must be an object"]))

    signature_raw = data.get("signature")
    signature: Optional[str] = None
    if isinstance(signature_raw, str):
        signature = signature_raw

    return CapabilityManifest(
        version=version,
        allow=policy.allow,
        deny=list(policy.deny),
        effects=policy.effects,
        packages=policy.packages,
        signature=signature,
    )


# ---------------------------------------------------------------------------
# Signature verification
# ---------------------------------------------------------------------------


def _parse_manifest_snapshot(text: str, suffix: str) -> dict[str, Any]:
    """Parse one immutable manifest text snapshot structurally."""

    if suffix == ".toml":
        data = tomllib.loads(text)
    elif suffix == ".json":
        data = json.loads(text)
    elif suffix in (".yaml", ".yml"):
        try:
            import yaml
        except ImportError as exc:
            raise ImportError(
                "YAML manifests require the 'pyyaml' package: pip install pyyaml"
            ) from exc
        data = yaml.safe_load(text) or {}
    else:
        raise ManifestError(f"unsupported manifest format: {suffix!r}")
    if not isinstance(data, dict):
        raise ManifestError("manifest root must be a mapping")
    return cast(dict[str, Any], data)


def _read_manifest_snapshot(manifest_path: Path) -> dict[str, Any]:
    try:
        text = manifest_path.read_bytes().decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ManifestError("manifest must be valid UTF-8") from exc
    return _parse_manifest_snapshot(text, manifest_path.suffix.casefold())


def _manifest_hash_from_snapshot(data: dict[str, Any]) -> str:
    unsigned = dict(data)
    unsigned.pop("signature", None)
    canonical = json.dumps(
        unsigned, sort_keys=True, ensure_ascii=False, separators=(",", ":")
    )
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def compute_manifest_hash(manifest_path: Path) -> str:
    """Compute a SHA-256 hash of a manifest file with the signature stripped.

    The file is parsed structurally, the ``signature`` key is removed,
    and the remaining data is serialized to sorted JSON for deterministic
    hashing.  This avoids regex-based stripping which is fragile against
    crafted input (e.g. ``[signature]`` inside TOML multi-line strings).

    Parameters
    ----------
    manifest_path : Path
        Path to the manifest file (.toml, .json, .yaml, .yml).

    Returns
    -------
    str
        Hex-encoded SHA-256 digest of the canonical JSON representation.
    """
    return _manifest_hash_from_snapshot(_read_manifest_snapshot(manifest_path))


class SignatureStatus:
    """Result of manifest signature verification."""

    VERIFIED = "verified"
    UNSIGNED = "unsigned"

    def __init__(self, status: str) -> None:
        self.status = status

    @property
    def is_verified(self) -> bool:
        return self.status == self.VERIFIED

    @property
    def is_unsigned(self) -> bool:
        return self.status == self.UNSIGNED

    def __bool__(self) -> bool:
        return self.is_verified

    def __repr__(self) -> str:
        return f"SignatureStatus({self.status!r})"


def _verify_manifest_signature_snapshot(
    manifest_path: Path,
    manifest: CapabilityManifest,
    data: dict[str, Any],
) -> SignatureStatus:
    """Verify the cryptographic signature embedded in a manifest.

    Returns a :class:`SignatureStatus` that is truthy only when the
    signature is present **and** matches.  Callers that require signed
    manifests should check ``result.is_verified`` or use ``bool(result)``.

    Parameters
    ----------
    manifest_path : Path
        Path to the on-disk manifest file.
    manifest : CapabilityManifest
        The already-parsed manifest object.

    Returns
    -------
    SignatureStatus
        ``VERIFIED`` if the signature matches, ``UNSIGNED`` if absent.

    Raises
    ------
    ManifestError
        If the signature format is invalid or the digest does not match.
    """
    if manifest.signature is None:
        warnings.warn(
            f"manifest {str(manifest_path)!r} is unsigned; "
            "consider signing with `molt sign-manifest`",
            stacklevel=2,
        )
        return SignatureStatus(SignatureStatus.UNSIGNED)

    sig = manifest.signature
    if not sig.startswith("sha256:"):
        raise ManifestError(
            f"invalid signature format {sig!r}; expected 'sha256:<hex_digest>'"
        )

    stored_digest = sig[len("sha256:") :]
    if len(stored_digest) != 64 or not all(
        c in "0123456789abcdef" for c in stored_digest
    ):
        raise ManifestError(
            f"invalid signature digest: expected 64 lowercase hex characters, "
            f"got {stored_digest!r}"
        )
    expected_digest = _manifest_hash_from_snapshot(data)

    if stored_digest != expected_digest:
        raise ManifestError(
            f"manifest signature mismatch: stored digest {stored_digest!r} "
            f"does not match computed digest {expected_digest!r}; "
            f"the manifest file may have been tampered with"
        )
    return SignatureStatus(SignatureStatus.VERIFIED)


def verify_manifest_signature(
    manifest_path: Path, manifest: CapabilityManifest
) -> SignatureStatus:
    """Verify a manifest against one fresh on-disk snapshot.

    ``load_manifest`` uses the private snapshot form so parsing and integrity
    verification are bound to the same bytes. This public verifier intentionally
    reads a new snapshot because callers supply an independently parsed object.
    """

    return _verify_manifest_signature_snapshot(
        manifest_path,
        manifest,
        _read_manifest_snapshot(manifest_path),
    )


def sign_manifest(manifest_path: Path) -> str:
    """Compute a signature string suitable for embedding in a manifest.

    This is the CLI-facing entry point for ``molt sign-manifest``.  It
    returns a string in the format ``sha256:<hex_digest>`` that should be
    written into the manifest's ``[signature]`` section (TOML) or
    ``"signature"`` key (JSON/YAML).

    Parameters
    ----------
    manifest_path : Path
        Path to the manifest file.

    Returns
    -------
    str
        The signature string, e.g. ``"sha256:ab12cd..."``.
    """
    p = Path(manifest_path)
    if not p.exists():
        raise FileNotFoundError(f"manifest file not found: {p}")
    digest = _manifest_hash_from_snapshot(_read_manifest_snapshot(p))
    return f"sha256:{digest}"


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def load_manifest(
    path: Union[str, Path], *, require_signed: bool = False
) -> CapabilityManifest:
    """Load a capability manifest from a TOML, JSON, or YAML file.

    The file format is determined by extension:
      - ``.toml`` -- v2.0 TOML manifest
      - ``.json`` -- v1.0 JSON manifest (backward compatibility)
      - ``.yaml`` / ``.yml`` -- v2.0 YAML manifest

    After loading, the manifest is validated. Warnings are attached but do not
    raise; structural errors raise :class:`ManifestError`.

    Parameters
    ----------
    path : str or Path
        Filesystem path to the manifest file.
    require_signed : bool, optional
        If True, raise :class:`ManifestError` when the manifest has no
        embedded signature.  Defaults to False.

    Returns
    -------
    CapabilityManifest
        The parsed and validated manifest.

    Raises
    ------
    ManifestError
        If the manifest has structural errors, a signature mismatch is
        detected, or *require_signed* is True and the manifest is unsigned.
    FileNotFoundError
        If the file does not exist.
    """
    p = Path(path)
    if not p.exists():
        raise FileNotFoundError(f"manifest file not found: {p}")

    suffix = p.suffix.casefold()
    data = _read_manifest_snapshot(p)
    if suffix == ".toml" or suffix in (".yaml", ".yml"):
        manifest = _parse_v2_dict(data)
    elif suffix == ".json":
        manifest = _parse_json_manifest(data)
    else:
        raise ManifestError(
            f"unsupported manifest format {suffix!r}; expected .toml, .json, .yaml, or .yml"
        )

    # Validate -- raises ManifestError on fatal issues, returns warnings.
    validation_warnings = validate_manifest(manifest)
    for w in validation_warnings:
        warnings.warn(w, stacklevel=2)

    # Verify manifest integrity if a signature is present.
    sig_status = _verify_manifest_signature_snapshot(p, manifest, data)
    if require_signed and sig_status.is_unsigned:
        raise ManifestError(
            f"manifest {p} is unsigned but --require-signed-manifest was specified"
        )

    return manifest


# ---------------------------------------------------------------------------
# Unit tests (run with: python -m molt.capability_manifest)
# ---------------------------------------------------------------------------


def _run_tests() -> None:
    """Self-contained unit tests."""
    import tempfile
    import os

    passed = 0
    failed = 0

    def _assert(condition: bool, msg: str) -> None:
        nonlocal passed, failed
        if condition:
            passed += 1
        else:
            failed += 1
            print(f"  FAIL: {msg}")

    def _assert_raises(exc_type: type[BaseException], fn: Any, msg: str) -> None:
        nonlocal passed, failed
        try:
            fn()
            failed += 1
            print(f"  FAIL (no exception): {msg}")
        except exc_type:
            passed += 1
        except Exception as e:
            failed += 1
            print(f"  FAIL (wrong exception {type(e).__name__}): {msg}")

    # -- parse_size --
    print("Testing parse_size...")
    _assert(parse_size("64MB") == 64 * 1024 * 1024, "64MB")
    _assert(parse_size("10KB") == 10 * 1024, "10KB")
    _assert(parse_size("1GB") == 1024 * 1024 * 1024, "1GB")
    _assert(parse_size("512B") == 512, "512B")
    _assert(parse_size("2TB") == 2 * 1024 * 1024 * 1024 * 1024, "2TB")
    _assert(parse_size("0B") == 0, "0B")
    _assert(parse_size(42) == 42, "passthrough int")
    _assert_raises(ValueError, lambda: parse_size("abc"), "invalid size")
    _assert_raises(ValueError, lambda: parse_size("10"), "missing unit")
    _assert_raises(ValueError, lambda: parse_size(""), "empty string")

    # -- parse_duration --
    print("Testing parse_duration...")
    _assert(parse_duration("30s") == 30.0, "30s")
    _assert(parse_duration("500ms") == 0.5, "500ms")
    _assert(parse_duration("2m") == 120.0, "2m")
    _assert(parse_duration("1h") == 3600.0, "1h")
    _assert(parse_duration("1.5s") == 1.5, "1.5s")
    _assert(parse_duration(42.0) == 42.0, "passthrough float")
    _assert_raises(ValueError, lambda: parse_duration("abc"), "invalid duration")
    _assert_raises(ValueError, lambda: parse_duration("10"), "missing unit")

    # -- TOML loading --
    print("Testing TOML manifest loading...")
    toml_content = b"""\
[manifest]
version = "2.0"
description = "test manifest"

[capabilities]
allow = ["net", "fs.read"]
deny = ["fs.write"]
effects = ["nondet"]

[capabilities.packages.mypkg]
allow = ["net"]
effects = []

[resources]
max_memory = "32MB"
max_duration = "10s"
max_allocations = 500_000
max_recursion_depth = 200

[resources.operation_limits]
max_pow_result = "5MB"
max_repeat_result = "5MB"

[io]
mode = "virtual"

[io.virtual_mounts]
"/tmp" = { type = "memory", max_size = "8MB" }
"/data" = { type = "readonly", source = "/bundle/data" }

[audit]
enabled = true
sink = "jsonl"
output = "logs/molt.jsonl"

"""
    with tempfile.NamedTemporaryFile(suffix=".toml", delete=False) as f:
        f.write(toml_content)
        toml_path = f.name

    try:
        m = load_manifest(toml_path)
        _assert(m.version == "2.0", "version")
        _assert(m.description == "test manifest", "description")
        _assert(m.allow == ["net", "fs.read"], "allow")
        _assert(m.deny == ["fs.write"], "deny")
        _assert(m.effects == ["nondet"], "effects")
        _assert("mypkg" in m.packages, "package exists")
        _assert(m.packages["mypkg"].allow == ["net"], "package allow")
        _assert(m.resources.max_memory == 32 * 1024 * 1024, "max_memory")
        _assert(m.resources.max_duration == 10.0, "max_duration")
        _assert(m.resources.max_allocations == 500_000, "max_allocations")
        _assert(m.resources.max_recursion_depth == 200, "max_recursion_depth")
        _assert(m.resources.max_pow_result == 5 * 1024 * 1024, "max_pow_result")
        _assert(m.resources.max_repeat_result == 5 * 1024 * 1024, "max_repeat_result")
        _assert(m.resources.max_shift_result is None, "max_shift_result unset")
        _assert(m.io.mode == "virtual", "io mode")
        _assert(len(m.io.virtual_mounts) == 2, "virtual mounts count")
        _assert(m.io.virtual_mounts[0].path == "/tmp", "mount path")
        _assert(m.io.virtual_mounts[0].type == "memory", "mount type")
        _assert(m.io.virtual_mounts[0].max_size == 8 * 1024 * 1024, "mount max_size")
        _assert(m.io.virtual_mounts[1].source == "/bundle/data", "mount source")
        _assert(m.audit.enabled is True, "audit enabled")
        _assert(m.audit.sink == "jsonl", "audit sink")
        _assert(m.audit.output == "logs/molt.jsonl", "audit output")
    finally:
        os.unlink(toml_path)

    # -- JSON loading (backward compat) --
    print("Testing JSON manifest loading...")
    json_content = json.dumps(
        {
            "allow": ["net", "time"],
            "deny": ["fs.write"],
            "effects": ["nondet"],
            "fs": {"read": ["/tmp/data"], "write": []},
            "packages": {"mypkg": {"allow": ["net"], "effects": ["nondet"]}},
        }
    )
    with tempfile.NamedTemporaryFile(
        suffix=".json", delete=False, mode="w", encoding="utf-8"
    ) as f:
        f.write(json_content)
        json_path = f.name

    try:
        m = load_manifest(json_path)
        _assert(m.version == "1.0", "json version")
        assert m.allow is not None
        _assert("net" in m.allow, "json allow net")
        _assert("fs.read" in m.allow, "json fs.read inferred from fs section")
        _assert("mypkg" in m.packages, "json package")
    finally:
        os.unlink(json_path)

    # -- expanded_allow / effective_capabilities --
    print("Testing capability expansion...")
    m = CapabilityManifest(allow=["net", "fs.read"], deny=["websocket.listen"])
    expanded = m.expanded_allow()
    _assert("net.connect" in expanded, "net profile expands to connect")
    _assert("net.listen" in expanded, "net profile expands to listen")
    _assert("net.poll" in expanded, "net profile expands to polling")
    _assert("websocket.connect" in expanded, "net profile expands to websocket.connect")
    _assert("websocket.listen" in expanded, "net profile expands to websocket.listen")
    _assert("fs.read" in expanded, "fs.read passthrough")
    effective = m.effective_capabilities()
    _assert("websocket.listen" not in effective, "websocket.listen denied")
    _assert("net.connect" in effective, "exact connect grant remains effective")

    # -- validate_manifest warnings --
    print("Testing validation warnings...")
    m = CapabilityManifest(allow=["net"], deny=["fs.write"])
    warnings = validate_manifest(m)
    _assert(
        any("fs.write" in w and "no effect" in w for w in warnings),
        "deny without allow warns",
    )

    # -- validate_manifest errors --
    print("Testing validation errors...")
    m = CapabilityManifest(
        allow=["net"],
        packages={"bad": PackageCapabilities(name="bad", allow=["fs.read"])},
    )
    _assert_raises(
        ManifestError, lambda: validate_manifest(m), "package exceeds global allow"
    )

    m = CapabilityManifest(resources=ResourceLimits(max_memory=-1))
    _assert_raises(ManifestError, lambda: validate_manifest(m), "negative max_memory")

    m = CapabilityManifest(io=IoConfig(mode=cast(Any, "invalid")))
    _assert_raises(ManifestError, lambda: validate_manifest(m), "invalid io mode")

    # -- File not found --
    print("Testing error cases...")
    _assert_raises(
        FileNotFoundError,
        lambda: load_manifest("/nonexistent/path.toml"),
        "file not found",
    )
    with tempfile.NamedTemporaryFile(suffix=".xml", delete=False) as f:
        xml_path = f.name
    try:
        _assert_raises(
            ManifestError,
            lambda: load_manifest(xml_path),
            "unsupported format",
        )
    finally:
        os.unlink(xml_path)

    # -- Minimal TOML (all defaults) --
    print("Testing minimal manifest...")
    with tempfile.NamedTemporaryFile(suffix=".toml", delete=False) as f:
        f.write(b"# empty manifest\n")
        minimal_path = f.name
    try:
        m = load_manifest(minimal_path)
        _assert(m.version == "2.0", "default version")
        _assert(m.allow == [], "default allow empty")
        _assert(m.resources.max_memory is None, "default resources unset")
        _assert(m.io.mode == "real", "default io mode")
        _assert(m.audit.enabled is False, "default audit disabled")
    finally:
        os.unlink(minimal_path)

    # -- Virtual mount validation --
    print("Testing virtual mount validation...")
    bad_mount_toml = b"""\
[io]
mode = "virtual"

[io.virtual_mounts]
"/data" = { type = "readonly" }
"""
    with tempfile.NamedTemporaryFile(suffix=".toml", delete=False) as f:
        f.write(bad_mount_toml)
        bad_mount_path = f.name
    try:
        _assert_raises(
            ManifestError,
            lambda: load_manifest(bad_mount_path),
            "readonly mount without source",
        )
    finally:
        os.unlink(bad_mount_path)

    # -- Summary --
    total = passed + failed
    print(f"\n{passed}/{total} tests passed.")
    if failed:
        print(f"{failed} tests FAILED.")
        sys.exit(1)
    else:
        print("All tests passed.")


if __name__ == "__main__":
    _run_tests()
