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
from collections.abc import Iterable, Sequence
from typing import Any, Mapping

from molt.cli.capability_spec import _split_tokens
from molt.cli.file_hashing import _sha256_file
from molt.cli.models import _ExternalNativeCallableExport
from molt.native_callable_abi import (
    NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1,
    native_callable_abi_choices,
    normalize_native_callable_abi,
)

_ABI_VERSION_RE = re.compile(r"^(\d+)\.(\d+)(?:\.(\d+))?$")
_MOLT_C_API_VERSION_RE = re.compile(r"^\d+(?:\.\d+){0,2}$")
_WHEEL_TOKEN_RE = re.compile(r"[^A-Za-z0-9_.]+")
_WHEEL_VERSION_RE = re.compile(r"[^A-Za-z0-9._]+")
_PY_IDENTIFIER_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_PYTHON_DOTTED_NAME_RE = re.compile(
    r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*"
)
_PYTHON_IDENTIFIER_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_NATIVE_SYMBOL_RE = re.compile(r"[A-Za-z_.$][A-Za-z0-9_.$@]*")
_PYMETHODDEF_ENTRY_RE = re.compile(
    r"\{\s*\"(?P<name>(?:\\.|[^\"\\])+)\"\s*,"
    r"(?P<body>[^{};]*?\bMETH_[A-Z_]+[^{};]*?)\}",
    re.DOTALL,
)
_SUPPORTED_PKG_ABI_MAJOR = 0
_SUPPORTED_PKG_ABI_MINOR = 1
_SUPPORTED_PKG_ABI = f"{_SUPPORTED_PKG_ABI_MAJOR}.{_SUPPORTED_PKG_ABI_MINOR}"
_LIBMOLT_SOURCE_RUNTIME_LINKAGES = frozenset({"host_resolved", "static_link"})
_LIBMOLT_SOURCE_ARTIFACT_KINDS = frozenset(
    {"shared_library", "wasm_relocatable_object", "static_archive"}
)
_EXTENSION_SUPPORT_FILE_SUFFIXES = (".molt.wasm", ".o", ".a", ".py")


@dataclass(frozen=True)
class ExtensionSupportFile:
    rel_path: str
    sha256: str
    source_path: Path

    def digest_payload(self) -> dict[str, str]:
        return {"path": self.rel_path, "sha256": self.sha256}


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


def _manifest_dotted_name_tuple(
    manifest: Mapping[str, Any],
    field_name: str,
    *,
    package: str,
    errors: list[str],
) -> tuple[str, ...]:
    value = manifest.get(field_name)
    if value is None:
        return ()
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        errors.append(
            f"extension_manifest.json {field_name!r} must be a list of dotted "
            "Python import names"
        )
        return ()
    out: set[str] = set()
    for item in value:
        stripped = item.strip()
        if not stripped or _PYTHON_DOTTED_NAME_RE.fullmatch(stripped) is None:
            errors.append(
                f"extension_manifest.json {field_name!r} contains invalid "
                f"Python import name {item!r}"
            )
            continue
        if stripped != package and not stripped.startswith(package + "."):
            errors.append(
                f"extension_manifest.json {field_name!r} entry {stripped!r} "
                f"escapes admitted package {package!r}"
            )
            continue
        out.add(stripped)
    return tuple(sorted(out))


def _manifest_support_file_payloads(
    value: Any,
    *,
    field_name: str,
    root: Path,
    errors: list[str],
) -> tuple[ExtensionSupportFile, ...]:
    if value is None:
        return ()
    if not isinstance(value, list):
        errors.append(f"{field_name} must be a list of paths or support-file objects")
        return ()
    root = root.resolve()
    out: list[ExtensionSupportFile] = []
    seen: set[str] = set()
    for index, item in enumerate(value):
        label = f"{field_name}[{index}]"
        expected_sha: str | None = None
        raw_source: str | None = None
        if isinstance(item, str):
            raw_path = item
        elif isinstance(item, Mapping):
            raw_path_value = item.get("path")
            if not isinstance(raw_path_value, str):
                errors.append(f"{label}.path must be a non-empty path string")
                continue
            raw_path = raw_path_value
            raw_source_value = item.get("source")
            if raw_source_value is not None:
                if (
                    not isinstance(raw_source_value, str)
                    or not raw_source_value.strip()
                ):
                    errors.append(f"{label}.source must be a non-empty path string")
                    continue
                raw_source = raw_source_value
            raw_sha = item.get("sha256")
            if raw_sha is not None:
                if not isinstance(raw_sha, str) or not re.fullmatch(
                    r"[0-9a-fA-F]{64}",
                    raw_sha.strip(),
                ):
                    errors.append(f"{label}.sha256 must be a SHA-256 hex digest")
                    continue
                expected_sha = raw_sha.strip().lower()
        else:
            errors.append(f"{label} must be a path string or support-file object")
            continue
        if not raw_path.strip():
            errors.append(f"{label} must be a non-empty path")
            continue
        source_path = Path((raw_source or raw_path).strip()).expanduser()
        if not source_path.is_absolute():
            source_path = (root / source_path).resolve()
        else:
            source_path = source_path.resolve()
        if raw_source is not None:
            rel_candidate = Path(raw_path.strip().replace("\\", "/"))
            if rel_candidate.is_absolute() or ".." in rel_candidate.parts:
                errors.append(
                    f"{label}.path must be a relative support-file destination"
                )
                continue
            rel_path = rel_candidate.as_posix()
        else:
            try:
                rel_path = source_path.relative_to(root).as_posix()
            except ValueError:
                errors.append(
                    f"{label} escapes support-file root {root}: {source_path}"
                )
                continue
        if raw_source is None:
            try:
                source_path.relative_to(root)
            except ValueError:
                errors.append(
                    f"{label} escapes support-file root {root}: {source_path}"
                )
                continue
        if not source_path.is_file():
            errors.append(f"{label} does not exist: {source_path}")
            continue
        if not (
            source_path.name.endswith(".molt.wasm")
            or source_path.suffix in _EXTENSION_SUPPORT_FILE_SUFFIXES
        ):
            errors.append(
                f"{label} must name a support artifact (.molt.wasm, .o, or .a) "
                "or checksummed upstream Python source (.py)"
            )
            continue
        if rel_path in seen:
            errors.append(f"{label} duplicates support file {rel_path!r}")
            continue
        actual_sha = _sha256_file(source_path).lower()
        if expected_sha is not None and expected_sha != actual_sha:
            errors.append(
                f"{label}.sha256 mismatch for {rel_path}: expected "
                f"{expected_sha}, got {actual_sha}"
            )
            continue
        seen.add(rel_path)
        out.append(
            ExtensionSupportFile(
                rel_path=rel_path,
                sha256=actual_sha,
                source_path=source_path,
            )
        )
    return tuple(sorted(out, key=lambda entry: entry.rel_path))


def _manifest_callable_exports(
    manifest: Mapping[str, Any],
    *,
    package: str,
    errors: list[str],
) -> tuple[_ExternalNativeCallableExport, ...]:
    value = manifest.get("callable_exports")
    if value is None:
        return ()
    if not isinstance(value, list):
        errors.append("extension_manifest.json 'callable_exports' must be a list")
        return ()

    exports: list[_ExternalNativeCallableExport] = []
    seen: set[str] = set()
    for index, raw_export in enumerate(value):
        label = f"callable_exports[{index}]"
        if not isinstance(raw_export, Mapping):
            errors.append(f"extension_manifest.json {label} must be an object")
            continue

        module = raw_export.get("module")
        name = raw_export.get("name")
        binding = raw_export.get("binding")
        abi = raw_export.get("abi")
        symbol = raw_export.get("symbol")
        provider_module = raw_export.get("provider_module")
        deterministic = raw_export.get("deterministic", False)
        effects_raw = raw_export.get("effects", [])

        if not isinstance(module, str) or not module.strip():
            errors.append(f"extension_manifest.json {label}.module must be non-empty")
            continue
        module = module.strip()
        if _PYTHON_DOTTED_NAME_RE.fullmatch(module) is None:
            errors.append(
                f"extension_manifest.json {label}.module has invalid dotted name "
                f"{module!r}"
            )
            continue
        if module != package and not module.startswith(package + "."):
            errors.append(
                f"extension_manifest.json {label}.module {module!r} escapes "
                f"admitted package {package!r}"
            )
            continue

        if not isinstance(name, str) or _PYTHON_IDENTIFIER_RE.fullmatch(name) is None:
            errors.append(
                f"extension_manifest.json {label}.name must be a Python identifier"
            )
            continue
        if not isinstance(binding, str) or binding not in {
            "module_attr",
            "direct_symbol",
        }:
            errors.append(
                f"extension_manifest.json {label}.binding must be "
                "'module_attr' or 'direct_symbol'"
            )
            continue
        normalized_abi = normalize_native_callable_abi(abi)
        if normalized_abi is None:
            errors.append(
                f"extension_manifest.json {label}.abi must be one of: "
                f"{native_callable_abi_choices()}"
            )
            continue

        normalized_symbol: str | None = None
        if symbol is not None:
            if not isinstance(symbol, str) or not symbol.strip():
                errors.append(
                    f"extension_manifest.json {label}.symbol must be non-empty "
                    "when present"
                )
                continue
            normalized_symbol = symbol.strip()
            if _NATIVE_SYMBOL_RE.fullmatch(normalized_symbol) is None:
                errors.append(
                    f"extension_manifest.json {label}.symbol has invalid native "
                    f"symbol {normalized_symbol!r}"
                )
                continue
        if binding == "direct_symbol" and normalized_symbol is None:
            errors.append(
                f"extension_manifest.json {label} direct_symbol binding requires symbol"
            )
            continue

        normalized_provider_module: str | None = None
        if provider_module is not None:
            if binding != "module_attr":
                errors.append(
                    f"extension_manifest.json {label}.provider_module is only valid "
                    "for module_attr binding"
                )
                continue
            if not isinstance(provider_module, str) or not provider_module.strip():
                errors.append(
                    f"extension_manifest.json {label}.provider_module must be a "
                    "non-empty dotted name when present"
                )
                continue
            normalized_provider_module = provider_module.strip()
            if _PYTHON_DOTTED_NAME_RE.fullmatch(normalized_provider_module) is None:
                errors.append(
                    f"extension_manifest.json {label}.provider_module has invalid "
                    f"dotted name {normalized_provider_module!r}"
                )
                continue
            if (
                normalized_provider_module != package
                and not normalized_provider_module.startswith(package + ".")
            ):
                errors.append(
                    f"extension_manifest.json {label}.provider_module "
                    f"{normalized_provider_module!r} escapes admitted package "
                    f"{package!r}"
                )
                continue

        normalized_effects: list[str] = []
        if isinstance(effects_raw, list):
            normalized_effects = [
                effect.strip()
                for effect in effects_raw
                if isinstance(effect, str) and effect.strip()
            ]
        if not isinstance(effects_raw, list) or len(normalized_effects) != len(
            effects_raw
        ):
            errors.append(
                f"extension_manifest.json {label}.effects must be a list of "
                "non-empty strings"
            )
            continue
        if not isinstance(deterministic, bool):
            errors.append(
                f"extension_manifest.json {label}.deterministic must be boolean"
            )
            continue

        export = _ExternalNativeCallableExport(
            module=module,
            name=name,
            binding=binding,
            symbol=normalized_symbol,
            provider_module=normalized_provider_module,
            abi=normalized_abi,
            effects=tuple(sorted(set(normalized_effects))),
            deterministic=deterministic,
        )
        if export.qualified_name in seen:
            errors.append(
                f"extension_manifest.json {label} duplicates callable export "
                f"{export.qualified_name!r}"
            )
            continue
        seen.add(export.qualified_name)
        exports.append(export)
    return tuple(sorted(exports, key=lambda export: export.qualified_name))


def _decode_c_string_fragment(value: str) -> str:
    try:
        return bytes(value, "utf-8").decode("unicode_escape")
    except UnicodeDecodeError:
        return value


def _py_methoddef_names(source_text: str) -> frozenset[str]:
    return frozenset(
        name
        for match in _PYMETHODDEF_ENTRY_RE.finditer(source_text)
        if (name := _decode_c_string_fragment(match.group("name")))
        and _PYTHON_IDENTIFIER_RE.fullmatch(name) is not None
    )


def _infer_module_attr_callable_export_payloads(
    source_texts: Iterable[str],
    *,
    python_exports: Sequence[str],
    explicit_callable_exports: Sequence[Mapping[str, Any]],
    effects: Sequence[str],
    deterministic: bool,
) -> tuple[dict[str, Any], ...]:
    """Infer module-attribute callable exports from admitted C extension source.

    ``python_exports`` grants package/import visibility.  If the same exported
    attribute is backed by a PyMethodDef entry in the admitted extension source,
    the extension module itself owns executable call dispatch through module
    attribute lookup.  This produces ABI metadata without inventing direct
    symbols or package-specific Python shims.
    """

    method_names: set[str] = set()
    for source_text in source_texts:
        method_names.update(_py_methoddef_names(source_text))
    if not method_names:
        return ()

    explicit_names = {
        f"{module}.{name}"
        for export in explicit_callable_exports
        if isinstance((module := export.get("module")), str)
        and isinstance((name := export.get("name")), str)
    }
    normalized_effects = tuple(
        sorted({effect.strip() for effect in effects if effect.strip()})
    )
    inferred: list[_ExternalNativeCallableExport] = []
    for qualified_name in sorted(set(python_exports)):
        if qualified_name in explicit_names or "." not in qualified_name:
            continue
        module, name = qualified_name.rsplit(".", 1)
        if name not in method_names:
            continue
        inferred.append(
            _ExternalNativeCallableExport(
                module=module,
                name=name,
                binding="module_attr",
                symbol=None,
                abi=NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1,
                effects=normalized_effects,
                deterministic=deterministic,
            )
        )
    return tuple(export.digest_payload() for export in inferred)


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


def _manifest_target_is_wasm(target_triple: Any) -> bool:
    return isinstance(target_triple, str) and target_triple.strip().lower().startswith(
        "wasm32"
    )


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
    allow_missing_wheel: bool = False,
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
            if runtime_linkage not in _LIBMOLT_SOURCE_RUNTIME_LINKAGES:
                errors.append(
                    "runtime_linkage must be one of "
                    f"{sorted(_LIBMOLT_SOURCE_RUNTIME_LINKAGES)} for "
                    "libmolt_source extensions"
                )
            artifact_kind = manifest.get("artifact_kind")
            if (
                artifact_kind is not None
                and artifact_kind not in _LIBMOLT_SOURCE_ARTIFACT_KINDS
            ):
                errors.append(
                    "artifact_kind must be one of "
                    f"{sorted(_LIBMOLT_SOURCE_ARTIFACT_KINDS)} for "
                    "libmolt_source extensions"
                )
            target_is_wasm = _manifest_target_is_wasm(manifest.get("target_triple"))
            if runtime_linkage == "host_resolved":
                if target_is_wasm:
                    errors.append(
                        "runtime_linkage 'host_resolved' is invalid for wasm "
                        "libmolt_source extensions"
                    )
                if artifact_kind not in (None, "shared_library"):
                    errors.append(
                        "runtime_linkage 'host_resolved' requires artifact_kind "
                        "'shared_library'"
                    )
            elif runtime_linkage == "static_link":
                if not target_is_wasm:
                    errors.append(
                        "runtime_linkage 'static_link' requires a wasm32 target_triple"
                    )
                if artifact_kind not in (
                    None,
                    "wasm_relocatable_object",
                    "static_archive",
                ):
                    errors.append(
                        "runtime_linkage 'static_link' requires artifact_kind "
                        "'wasm_relocatable_object' or 'static_archive'"
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
        elif not allow_missing_wheel:
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
        elif not allow_missing_wheel:
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
