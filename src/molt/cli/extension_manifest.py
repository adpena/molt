from __future__ import annotations

import base64
import hashlib
import json
import os
import platform
import re
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping

from molt.cli.capability_spec import _split_tokens
from molt.cli.file_hashing import _sha256_file

_ABI_VERSION_RE = re.compile(r"^(\d+)\.(\d+)(?:\.(\d+))?$")
_MOLT_C_API_VERSION_RE = re.compile(r"^\d+(?:\.\d+){0,2}$")
_WHEEL_TOKEN_RE = re.compile(r"[^A-Za-z0-9_.]+")
_WHEEL_VERSION_RE = re.compile(r"[^A-Za-z0-9._]+")
_PY_IDENTIFIER_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_SUPPORTED_PKG_ABI_MAJOR = 0
_SUPPORTED_PKG_ABI_MINOR = 1
_SUPPORTED_PKG_ABI = f"{_SUPPORTED_PKG_ABI_MAJOR}.{_SUPPORTED_PKG_ABI_MINOR}"


@dataclass(frozen=True)
class ExtensionManifestValidation:
    errors: list[str]
    warnings: list[str]
    wheel_path: Path | None
    abi_version: str
    abi_tag: str | None
    capabilities: list[str]
    wheel_tags: tuple[str, str, str] | None


def _abi_version_error(value: str) -> str | None:
    cleaned = value.strip()
    match = _ABI_VERSION_RE.match(cleaned)
    if match is None:
        return "abi_version must be MAJOR.MINOR[.PATCH] (e.g., 0.1)"
    major = int(match.group(1))
    minor = int(match.group(2))
    if major != _SUPPORTED_PKG_ABI_MAJOR or minor != _SUPPORTED_PKG_ABI_MINOR:
        return f"unsupported abi_version {cleaned} (supported: {_SUPPORTED_PKG_ABI})"
    return None


def _manifest_errors(manifest: dict[str, Any]) -> list[str]:
    required = [
        "name",
        "version",
        "abi_version",
        "target",
        "capabilities",
        "deterministic",
        "effects",
    ]
    errors: list[str] = []
    for key in required:
        if key not in manifest:
            errors.append(f"missing {key}")
    name = manifest.get("name")
    version = manifest.get("version")
    abi_version = manifest.get("abi_version")
    target = manifest.get("target")
    capabilities = manifest.get("capabilities")
    deterministic = manifest.get("deterministic")
    effects = manifest.get("effects")
    exports = manifest.get("exports")
    if name is not None and not isinstance(name, str):
        errors.append("name must be a string")
    if version is not None and not isinstance(version, str):
        errors.append("version must be a string")
    if abi_version is not None and not isinstance(abi_version, str):
        errors.append("abi_version must be a string")
    if isinstance(abi_version, str):
        abi_error = _abi_version_error(abi_version)
        if abi_error:
            errors.append(abi_error)
    if target is not None and not isinstance(target, str):
        errors.append("target must be a string")
    if capabilities is not None:
        if not isinstance(capabilities, list) or not all(
            isinstance(item, str) for item in capabilities
        ):
            errors.append("capabilities must be a list of strings")
    if deterministic is not None and not isinstance(deterministic, bool):
        errors.append("deterministic must be a boolean")
    if effects is not None and not isinstance(effects, (list, str)):
        errors.append("effects must be a list or string")
    if exports is not None:
        if not isinstance(exports, list) or not all(
            isinstance(item, str) for item in exports
        ):
            errors.append("exports must be a list of strings")
    return errors


def _normalize_effects(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        normalized: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    normalized.append(stripped)
        return normalized
    if isinstance(value, str):
        return _split_tokens(value)
    return []


def _load_manifest(path: Path) -> dict[str, Any] | None:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def _write_zip_member(zf: zipfile.ZipFile, name: str, data: bytes) -> None:
    info = zipfile.ZipInfo(name)
    info.date_time = (1980, 1, 1, 0, 0, 0)
    info.compress_type = zipfile.ZIP_DEFLATED
    zf.writestr(info, data)


def _wheel_record_line(path: str, data: bytes) -> str:
    digest = hashlib.sha256(data).digest()
    encoded = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    return f"{path},sha256={encoded},{len(data)}"


def _coerce_str_list(
    value: Any,
    field: str,
    errors: list[str],
    *,
    allow_empty: bool = True,
) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        stripped = value.strip()
        if stripped:
            return [stripped]
        if allow_empty:
            return []
        errors.append(f"{field} must not be empty")
        return []
    if isinstance(value, list):
        items: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    items.append(stripped)
                elif not allow_empty:
                    errors.append(f"{field} must not include empty entries")
            else:
                errors.append(f"{field} entries must be strings")
        return items
    errors.append(f"{field} must be a string or list of strings")
    return []


def _module_parts(module_name: str) -> list[str] | None:
    stripped = module_name.strip()
    if not stripped:
        return None
    parts = stripped.split(".")
    if any(_PY_IDENTIFIER_RE.match(part) is None for part in parts):
        return None
    return parts


def _wheel_token(value: str) -> str:
    cleaned = _WHEEL_TOKEN_RE.sub("_", value.strip())
    cleaned = cleaned.strip("._")
    return cleaned or "unknown"


def _wheel_version_token(value: str) -> str:
    cleaned = _WHEEL_VERSION_RE.sub("_", value.strip())
    cleaned = cleaned.strip("._")
    return cleaned or "0"


def _cpu_baseline(target_triple: str | None) -> str:
    """Return the CPU baseline label for the given target triple.

    When no target-cpu=native is set, Cranelift uses the architecture's
    generic baseline.  This helper returns a human-readable label for
    build metadata.
    """
    triple = (target_triple or _host_target_triple()).lower()
    if triple.startswith("x86_64") or triple.startswith("x86-64"):
        return "x86-64"
    if triple.startswith("aarch64") or triple.startswith("arm64"):
        return "aarch64"
    if triple.startswith("wasm32"):
        return "wasm32"
    return "generic"


def _extension_binary_suffix(target_triple: str | None = None) -> str:
    target = (target_triple or "").strip().lower()
    if "windows" in target:
        return ".pyd"
    if os.name == "nt" and not target:
        return ".pyd"
    return ".so"


def _host_target_triple() -> str:
    system = platform.system().lower()
    arch = platform.machine().lower() or "unknown"
    arch_aliases = {
        "amd64": "x86_64",
        "x86-64": "x86_64",
        "arm64": "aarch64",
    }
    arch = arch_aliases.get(arch, arch)
    if system == "darwin":
        return f"{arch}-apple-darwin"
    if system == "linux":
        return f"{arch}-unknown-linux-gnu"
    if system == "windows":
        return f"{arch}-pc-windows-msvc"
    return f"{arch}-{system}"


def _default_molt_c_api_version(molt_root: Path) -> str:
    header = molt_root / "include" / "molt" / "molt.h"
    try:
        text = header.read_text()
    except OSError:
        return "1"
    match = re.search(
        r"^\s*#\s*define\s+MOLT_C_API_VERSION\s+([0-9]+)u?\s*$",
        text,
        flags=re.MULTILINE,
    )
    if match is None:
        return "1"
    return match.group(1)


def _wheel_filename_tags(path: Path) -> tuple[str, str, str] | None:
    if path.suffix != ".whl":
        return None
    parts = path.stem.split("-")
    if len(parts) < 5:
        return None
    python_tag = parts[-3]
    abi_tag = parts[-2]
    platform_tag = parts[-1]
    if not python_tag or not abi_tag or not platform_tag:
        return None
    return python_tag, abi_tag, platform_tag


def _is_extension_manifest(manifest: Mapping[str, Any]) -> bool:
    extension_keys = {
        "molt_c_api_version",
        "abi_tag",
        "target_triple",
        "platform_tag",
        "module",
        "wheel",
        "extension",
    }
    matched = sum(1 for key in extension_keys if key in manifest)
    return matched >= 2


def _validate_extension_manifest(
    manifest: Mapping[str, Any],
    *,
    manifest_dir: Path,
    wheel_path: Path | None,
    require_capabilities: bool,
    required_abi: str | None,
    require_checksum: bool = False,
    warn_missing_checksum: bool = False,
) -> ExtensionManifestValidation:
    errors: list[str] = []
    warnings: list[str] = []
    required_fields = (
        "molt_c_api_version",
        "capabilities",
        "abi_tag",
        "target_triple",
        "platform_tag",
        "module",
    )
    for field_name in required_fields:
        if field_name not in manifest:
            errors.append(f"Missing manifest field: {field_name}")

    manifest_abi = manifest.get("molt_c_api_version")
    if not isinstance(manifest_abi, str):
        errors.append("molt_c_api_version must be a string")
        manifest_abi = ""
    elif _MOLT_C_API_VERSION_RE.match(manifest_abi.strip()) is None:
        errors.append(
            f"molt_c_api_version must be MAJOR[.MINOR[.PATCH]] (got {manifest_abi!r})"
        )
        manifest_abi = ""
    else:
        manifest_abi = manifest_abi.strip()

    capabilities_value = manifest.get("capabilities")
    manifest_capabilities: list[str] = []
    if not isinstance(capabilities_value, list) or not all(
        isinstance(item, str) for item in capabilities_value
    ):
        errors.append("capabilities must be a list of strings")
    else:
        manifest_capabilities = [
            item.strip() for item in capabilities_value if item.strip()
        ]
    if require_capabilities and not manifest_capabilities:
        errors.append(
            "Capabilities are required but manifest capability list is empty."
        )

    if required_abi is not None and manifest_abi and manifest_abi != required_abi:
        errors.append(
            f"ABI mismatch: required {required_abi}, manifest has {manifest_abi}"
        )

    module_value = manifest.get("module")
    manifest_module = module_value.strip() if isinstance(module_value, str) else ""
    loader_kind = manifest.get("loader_kind")
    if loader_kind is not None:
        if not isinstance(loader_kind, str) or not loader_kind.strip():
            errors.append("loader_kind must be a non-empty string when present")
        elif loader_kind.strip() != "libmolt_source":
            errors.append(f"unsupported loader_kind: {loader_kind!r}")
        else:
            init_symbol = manifest.get("init_symbol")
            module_leaf = manifest_module.rsplit(".", 1)[-1] if manifest_module else ""
            expected_init = f"PyInit_{module_leaf}" if module_leaf else ""
            if init_symbol != expected_init:
                errors.append(
                    f"init_symbol mismatch: expected {expected_init!r}, "
                    f"found {init_symbol!r}"
                )
            runtime_linkage = manifest.get("runtime_linkage")
            if runtime_linkage != "host_resolved":
                errors.append(
                    "runtime_linkage must be 'host_resolved' for "
                    "libmolt_source extensions"
                )

    manifest_abi_tag: str | None = None
    abi_tag_value = manifest.get("abi_tag")
    if isinstance(abi_tag_value, str):
        manifest_abi_tag = abi_tag_value
    if manifest_abi_tag is not None and manifest_abi:
        expected_abi_tag = f"molt_abi{manifest_abi.split('.', 1)[0]}"
        if manifest_abi_tag != expected_abi_tag:
            errors.append(
                f"ABI tag mismatch: expected {expected_abi_tag}, found {manifest_abi_tag}"
            )

    resolved_wheel = wheel_path
    if resolved_wheel is not None:
        resolved_wheel = resolved_wheel.expanduser()
        if not resolved_wheel.is_absolute():
            resolved_wheel = (manifest_dir / resolved_wheel).absolute()

    wheel_field = manifest.get("wheel")
    if resolved_wheel is None and isinstance(wheel_field, str) and wheel_field.strip():
        candidate = Path(wheel_field).expanduser()
        if not candidate.is_absolute():
            candidate = (manifest_dir / candidate).absolute()
        if candidate.exists():
            resolved_wheel = candidate
        else:
            warnings.append(f"Wheel path referenced by manifest not found: {candidate}")

    wheel_tags: tuple[str, str, str] | None = None
    if resolved_wheel is not None and resolved_wheel.exists():
        wheel_tags = _wheel_filename_tags(resolved_wheel)
        if wheel_tags is None:
            errors.append(f"Invalid wheel filename format: {resolved_wheel.name}")
        else:
            _python_tag, wheel_abi_tag, wheel_platform_tag = wheel_tags
            if manifest_abi_tag is not None and wheel_abi_tag != manifest_abi_tag:
                errors.append(
                    f"Wheel ABI tag mismatch: wheel has {wheel_abi_tag}, "
                    f"manifest has {manifest_abi_tag}"
                )
            manifest_platform = manifest.get("platform_tag")
            if (
                isinstance(manifest_platform, str)
                and wheel_platform_tag != manifest_platform
            ):
                errors.append(
                    f"Wheel platform tag mismatch: wheel has {wheel_platform_tag}, "
                    f"manifest has {manifest_platform}"
                )

        expected_wheel_sha = manifest.get("wheel_sha256")
        if isinstance(expected_wheel_sha, str) and expected_wheel_sha.strip():
            actual_wheel_sha = _sha256_file(resolved_wheel)
            if actual_wheel_sha != expected_wheel_sha.strip():
                errors.append("wheel_sha256 does not match wheel contents")
        elif require_checksum:
            errors.append("wheel_sha256 missing")
        elif warn_missing_checksum:
            warnings.append("wheel_sha256 missing")

        extension_entry = manifest.get("extension")
        expected_extension_sha = manifest.get("extension_sha256")
        if isinstance(extension_entry, str) and extension_entry.strip():
            try:
                with zipfile.ZipFile(resolved_wheel) as zf:
                    ext_bytes = zf.read(extension_entry)
            except KeyError:
                errors.append(f"Wheel is missing extension entry: {extension_entry}")
            except (OSError, zipfile.BadZipFile) as exc:
                errors.append(f"Failed to read wheel extension payload: {exc}")
            else:
                if (
                    isinstance(expected_extension_sha, str)
                    and expected_extension_sha.strip()
                ):
                    actual_extension_sha = hashlib.sha256(ext_bytes).hexdigest()
                    if actual_extension_sha != expected_extension_sha.strip():
                        errors.append("extension_sha256 does not match wheel entry")
                elif require_checksum:
                    errors.append("extension_sha256 missing")
                elif warn_missing_checksum:
                    warnings.append("extension_sha256 missing")
        elif require_checksum:
            errors.append("extension path missing")
    else:
        if require_checksum:
            errors.append(
                "wheel artifact required for checksum verification is missing"
            )
        else:
            warnings.append(
                "Wheel artifact not found; wheel tag and checksum checks skipped."
            )

    return ExtensionManifestValidation(
        errors=errors,
        warnings=warnings,
        wheel_path=resolved_wheel,
        abi_version=manifest_abi,
        abi_tag=manifest_abi_tag,
        capabilities=manifest_capabilities,
        wheel_tags=wheel_tags,
    )
