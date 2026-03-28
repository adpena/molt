"""Molt Capability Manifest v2.0 -- TOML-based unified manifest parser.

Parses and validates capability manifests that unify:
  - Capability grants (allow/deny/effects, per-package scoping)
  - Resource limits (memory, duration, allocations, operation guards)
  - IO mode (real, virtual with VFS mounts, callback)
  - Audit configuration (sink type, output destination)
  - Monty interoperability (tiered execution, shared stubs)

Usage::

    from molt.capability_manifest import load_manifest
    manifest = load_manifest("molt.capabilities.toml")
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
import warnings
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, Optional, Union

# tomllib is stdlib in 3.11+; fall back to tomli for 3.10.
if sys.version_info >= (3, 11):
    import tomllib
else:
    try:
        import tomli as tomllib  # type: ignore[no-redef]
    except ImportError as exc:
        raise ImportError(
            "Python < 3.11 requires the 'tomli' package: pip install tomli"
        ) from exc


# ---------------------------------------------------------------------------
# Known capability tokens -- canonical registry
# ---------------------------------------------------------------------------

KNOWN_CAPABILITIES: frozenset[str] = frozenset(
    {
        "net",
        "websocket.connect",
        "websocket.listen",
        "fs.read",
        "fs.write",
        "env.read",
        "env.write",
        "db.read",
        "db.write",
        "time.wall",
        "time",
        "random",
    }
)

# Built-in profiles that expand to multiple capabilities.
CAPABILITY_PROFILES: dict[str, list[str]] = {
    "core": [],
    "fs": ["fs.read", "fs.write"],
    "env": ["env.read", "env.write"],
    "net": ["net", "websocket.connect", "websocket.listen"],
    "db": ["db.read", "db.write"],
    "time": ["time"],
    "random": ["random"],
}

KNOWN_EFFECTS: frozenset[str] = frozenset({"nondet", "io", "network", "fs"})

VALID_IO_MODES: frozenset[str] = frozenset({"real", "virtual", "callback"})
VALID_AUDIT_SINKS: frozenset[str] = frozenset({"null", "stderr", "jsonl", "buffered"})
VALID_EXECUTION_TIERS: frozenset[str] = frozenset({"auto", "interpret", "compile"})
VALID_MOUNT_TYPES: frozenset[str] = frozenset({"memory", "readonly", "readwrite"})


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


def parse_size(s: str) -> int:
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


def parse_duration(s: str) -> float:
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
    sink: Literal["null", "stderr", "jsonl", "buffered"] = "null"
    output: str = "stderr"


@dataclass
class IoConfig:
    """IO mode and virtual filesystem mounts."""

    mode: Literal["real", "virtual", "callback"] = "real"
    virtual_mounts: list[VirtualMount] = field(default_factory=list)


@dataclass
class PackageCapabilities:
    """Per-package capability scoping."""

    name: str
    allow: list[str] = field(default_factory=list)
    deny: list[str] = field(default_factory=list)
    effects: list[str] = field(default_factory=list)


@dataclass
class MontyConfig:
    """Monty interoperability settings for tiered execution."""

    compatible: bool = False
    shared_stubs: Optional[str] = None
    execution_tier: Literal["auto", "interpret", "compile"] = "auto"
    tier_up_threshold: int = 100


@dataclass
class CapabilityManifest:
    """Complete parsed and validated capability manifest."""

    version: str = "2.0"
    description: str = ""
    # Capabilities
    allow: list[str] = field(default_factory=list)
    deny: list[str] = field(default_factory=list)
    effects: list[str] = field(default_factory=list)
    packages: dict[str, PackageCapabilities] = field(default_factory=dict)
    # Sub-configs
    resources: ResourceLimits = field(default_factory=ResourceLimits)
    audit: AuditConfig = field(default_factory=AuditConfig)
    io: IoConfig = field(default_factory=IoConfig)
    monty: MontyConfig = field(default_factory=MontyConfig)
    signature: Optional[str] = None

    def expanded_allow(self) -> set[str]:
        """Return the full set of allowed capabilities after profile expansion."""
        result: set[str] = set()
        for cap in self.allow:
            if cap in CAPABILITY_PROFILES:
                result.update(CAPABILITY_PROFILES[cap])
            else:
                result.add(cap)
        return result

    def effective_capabilities(self) -> set[str]:
        """Return allowed minus denied capabilities."""
        return self.expanded_allow() - set(self.deny)


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------


class ManifestError(Exception):
    """Raised when a manifest is structurally invalid."""


def validate_manifest(manifest: CapabilityManifest) -> list[str]:
    """Validate a parsed manifest, returning a list of warnings.

    Raises :class:`ManifestError` for fatal structural issues.
    Returns a (possibly empty) list of non-fatal warnings.
    """
    warnings: list[str] = []

    # Version check
    if manifest.version not in ("1.0", "2.0"):
        warnings.append(
            f"unrecognized manifest version {manifest.version!r}; "
            f"this parser supports versions 1.0 and 2.0"
        )

    # Validate capability tokens
    expanded = manifest.expanded_allow()
    for cap in expanded:
        if cap not in KNOWN_CAPABILITIES:
            warnings.append(f"unknown capability {cap!r} in allow list")

    for cap in manifest.deny:
        if cap not in KNOWN_CAPABILITIES:
            warnings.append(f"unknown capability {cap!r} in deny list")

    # Deny items that are not in allow are not actionable
    denied_not_allowed = set(manifest.deny) - expanded
    for cap in sorted(denied_not_allowed):
        warnings.append(
            f"capability {cap!r} is in deny but not in allow -- has no effect"
        )

    # Effect annotations
    for eff in manifest.effects:
        if eff not in KNOWN_EFFECTS:
            warnings.append(f"unknown effect annotation {eff!r}")

    # Per-package: allow must be subset of global allow
    for pkg_name, pkg in manifest.packages.items():
        pkg_expanded: set[str] = set()
        for cap in pkg.allow:
            if cap in CAPABILITY_PROFILES:
                pkg_expanded.update(CAPABILITY_PROFILES[cap])
            else:
                pkg_expanded.add(cap)
        extra = pkg_expanded - expanded
        if extra:
            raise ManifestError(
                f"package {pkg_name!r} requests capabilities not in global allow: "
                f"{', '.join(sorted(extra))}"
            )
        for eff in pkg.effects:
            if eff not in KNOWN_EFFECTS:
                warnings.append(
                    f"unknown effect annotation {eff!r} in package {pkg_name!r}"
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
    VALID_AUDIT_OUTPUTS = {"stderr", "stdout", "null"}
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

    # Monty
    if manifest.monty.execution_tier not in VALID_EXECUTION_TIERS:
        raise ManifestError(
            f"invalid monty.execution_tier {manifest.monty.execution_tier!r}; "
            f"valid tiers: {', '.join(sorted(VALID_EXECUTION_TIERS))}"
        )
    if manifest.monty.tier_up_threshold <= 0:
        raise ManifestError(
            f"monty.tier_up_threshold must be positive, got "
            f"{manifest.monty.tier_up_threshold}"
        )
    if (
        manifest.monty.execution_tier != "auto"
        and manifest.monty.tier_up_threshold != 100
    ):
        warnings.append(
            "monty.tier_up_threshold is set but execution_tier is not 'auto' "
            "-- threshold will be ignored"
        )

    return warnings


# ---------------------------------------------------------------------------
# TOML loading
# ---------------------------------------------------------------------------


def _parse_resources(data: dict[str, Any]) -> ResourceLimits:
    """Parse the [resources] table into a ResourceLimits dataclass."""
    rl = ResourceLimits()

    if "max_memory" in data:
        rl.max_memory = parse_size(data["max_memory"])
    if "max_duration" in data:
        rl.max_duration = parse_duration(data["max_duration"])
    if "max_allocations" in data:
        v = data["max_allocations"]
        if not isinstance(v, int):
            raise ManifestError(f"max_allocations must be an integer, got {type(v).__name__}")
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
    for op_field in ("max_pow_result", "max_repeat_result", "max_shift_result", "max_string_result"):
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
        mount_type = mount_cfg.get("type")
        if mount_type is None:
            raise ManifestError(f"virtual mount {mount_path!r} is missing required 'type' field")
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
            is_vfs_ref = any(raw_source == p or raw_source.startswith(p + "/")
                            for p in _VFS_PREFIXES)
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
    io = IoConfig()
    if "mode" in data:
        io.mode = data["mode"]
    if "virtual_mounts" in data:
        io.virtual_mounts = _parse_virtual_mounts(data["virtual_mounts"])
    return io


def _parse_audit(data: dict[str, Any]) -> AuditConfig:
    """Parse the [audit] table into an AuditConfig dataclass."""
    return AuditConfig(
        enabled=data.get("enabled", False),
        sink=data.get("sink", "null"),
        output=data.get("output", "stderr"),
    )


def _parse_monty(data: dict[str, Any]) -> MontyConfig:
    """Parse the [monty] table into a MontyConfig dataclass."""
    mc = MontyConfig()
    if "compatible" in data:
        mc.compatible = data["compatible"]
    if "shared_stubs" in data:
        mc.shared_stubs = data["shared_stubs"]
    if "execution_tier" in data:
        mc.execution_tier = data["execution_tier"]
    if "tier_up_threshold" in data:
        v = data["tier_up_threshold"]
        if not isinstance(v, int):
            raise ManifestError(
                f"monty.tier_up_threshold must be an integer, got {type(v).__name__}"
            )
        mc.tier_up_threshold = v
    return mc


def _parse_capabilities(data: dict[str, Any]) -> tuple[
    list[str], list[str], list[str], dict[str, PackageCapabilities]
]:
    """Parse the [capabilities] table.

    Returns (allow, deny, effects, packages).
    """
    allow = list(data.get("allow", []))
    deny = list(data.get("deny", []))
    effects = list(data.get("effects", []))

    packages: dict[str, PackageCapabilities] = {}
    for pkg_name, pkg_data in data.get("packages", {}).items():
        if not isinstance(pkg_data, dict):
            raise ManifestError(
                f"capabilities.packages.{pkg_name} must be a table"
            )
        packages[pkg_name] = PackageCapabilities(
            name=pkg_name,
            allow=list(pkg_data.get("allow", [])),
            deny=list(pkg_data.get("deny", [])),
            effects=list(pkg_data.get("effects", [])),
        )

    return allow, deny, effects, packages


def _parse_v2_dict(data: dict) -> CapabilityManifest:
    """Parse a v2.0 manifest from an already-loaded dict (TOML or YAML)."""
    manifest_meta = data.get("manifest", {})
    version = manifest_meta.get("version", "2.0")
    description = manifest_meta.get("description", "")

    caps_data = data.get("capabilities", {})
    allow, deny, effects, packages = _parse_capabilities(caps_data)

    resources = _parse_resources(data.get("resources", {}))
    io = _parse_io(data.get("io", {}))
    audit = _parse_audit(data.get("audit", {}))
    monty = _parse_monty(data.get("monty", {}))

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
        monty=monty,
    )


def _load_toml(path: Path) -> CapabilityManifest:
    """Load a v2.0 TOML capability manifest."""
    with open(path, "rb") as f:
        data = tomllib.load(f)
    return _parse_v2_dict(data)


# ---------------------------------------------------------------------------
# JSON loading (backward compatibility with v1.0 JSON manifests)
# ---------------------------------------------------------------------------


def _load_json(path: Path) -> CapabilityManifest:
    """Load a v1.0 JSON capability manifest (backward compat)."""
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)

    if not isinstance(data, dict):
        raise ManifestError(f"JSON manifest must be an object, got {type(data).__name__}")

    allow = list(data.get("allow", []))
    deny = list(data.get("deny", []))
    effects = list(data.get("effects", []))

    # JSON format uses "fs.read" / "fs.write" path lists to imply capabilities.
    fs_section = data.get("fs", {})
    if fs_section.get("read"):
        if "fs.read" not in allow:
            allow.append("fs.read")
    if fs_section.get("write"):
        if "fs.write" not in allow:
            allow.append("fs.write")

    packages: dict[str, PackageCapabilities] = {}
    for pkg_name, pkg_data in data.get("packages", {}).items():
        if isinstance(pkg_data, dict):
            packages[pkg_name] = PackageCapabilities(
                name=pkg_name,
                allow=list(pkg_data.get("allow", [])),
                deny=list(pkg_data.get("deny", [])),
                effects=list(pkg_data.get("effects", [])),
            )

    return CapabilityManifest(
        version="1.0",
        allow=allow,
        deny=deny,
        effects=effects,
        packages=packages,
    )


def _load_yaml(path: Path) -> CapabilityManifest:
    """Load a v2.0 YAML capability manifest.

    Requires PyYAML (``pip install pyyaml``). The YAML structure mirrors
    the TOML format exactly — same keys, same nesting.
    """
    try:
        import yaml  # type: ignore[import-untyped]
    except ImportError as exc:
        raise ImportError(
            "YAML manifests require the 'pyyaml' package: pip install pyyaml"
        ) from exc

    with open(path, "r", encoding="utf-8") as f:
        data = yaml.safe_load(f)

    if not isinstance(data, dict):
        raise ManifestError(f"YAML manifest must be a mapping, got {type(data).__name__}")

    # YAML uses the same structure as TOML — reuse the TOML parser internals.
    return _parse_v2_dict(data)


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def load_manifest(path: Union[str, Path]) -> CapabilityManifest:
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

    Returns
    -------
    CapabilityManifest
        The parsed and validated manifest.

    Raises
    ------
    ManifestError
        If the manifest has structural errors.
    FileNotFoundError
        If the file does not exist.
    """
    p = Path(path)
    if not p.exists():
        raise FileNotFoundError(f"manifest file not found: {p}")

    suffix = p.suffix.lower()
    if suffix == ".toml":
        manifest = _load_toml(p)
    elif suffix == ".json":
        manifest = _load_json(p)
    elif suffix in (".yaml", ".yml"):
        manifest = _load_yaml(p)
    else:
        raise ManifestError(
            f"unsupported manifest format {suffix!r}; expected .toml, .json, .yaml, or .yml"
        )

    # Validate -- raises ManifestError on fatal issues, returns warnings.
    _warnings = validate_manifest(manifest)
    # Warnings are informational; callers who want them can call
    # validate_manifest() directly.

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

    def _assert_raises(exc_type: type, fn: Any, msg: str) -> None:
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

[monty]
compatible = true
shared_stubs = "stubs/"
execution_tier = "auto"
tier_up_threshold = 50
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
        _assert(m.monty.compatible is True, "monty compatible")
        _assert(m.monty.shared_stubs == "stubs/", "monty shared_stubs")
        _assert(m.monty.execution_tier == "auto", "monty execution_tier")
        _assert(m.monty.tier_up_threshold == 50, "monty tier_up_threshold")
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
            "packages": {
                "mypkg": {"allow": ["net"], "effects": ["nondet"]}
            },
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
        _assert("net" in m.allow, "json allow net")
        _assert("fs.read" in m.allow, "json fs.read inferred from fs section")
        _assert("mypkg" in m.packages, "json package")
    finally:
        os.unlink(json_path)

    # -- expanded_allow / effective_capabilities --
    print("Testing capability expansion...")
    m = CapabilityManifest(allow=["net", "fs.read"], deny=["websocket.listen"])
    expanded = m.expanded_allow()
    _assert("net" in expanded, "net profile expands to net")
    _assert("websocket.connect" in expanded, "net profile expands to websocket.connect")
    _assert("websocket.listen" in expanded, "net profile expands to websocket.listen")
    _assert("fs.read" in expanded, "fs.read passthrough")
    effective = m.effective_capabilities()
    _assert("websocket.listen" not in effective, "websocket.listen denied")
    _assert("net" in effective, "net still effective")

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
    _assert_raises(ManifestError, lambda: validate_manifest(m), "package exceeds global allow")

    m = CapabilityManifest(resources=ResourceLimits(max_memory=-1))
    _assert_raises(ManifestError, lambda: validate_manifest(m), "negative max_memory")

    m = CapabilityManifest(io=IoConfig(mode="invalid"))  # type: ignore[arg-type]
    _assert_raises(ManifestError, lambda: validate_manifest(m), "invalid io mode")

    m = CapabilityManifest(monty=MontyConfig(tier_up_threshold=-5))
    _assert_raises(ManifestError, lambda: validate_manifest(m), "negative tier_up_threshold")

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
        _assert(m.monty.compatible is False, "default monty disabled")
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
